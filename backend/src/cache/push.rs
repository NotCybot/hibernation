use aide::axum::routing::{post_with, put_with};
use std::{
  io::Write,
  sync::Arc,
  time::{Duration, Instant},
};

use aide::{OperationIo, axum::ApiRouter};
use axum::{
  Extension, Json,
  body::to_bytes,
  extract::{FromRequestParts, Path, Request},
};
use centaurus::{
  bail,
  db::init::Connection,
  error::{ErrorReportStatusExt, Result},
  eyre::Context,
  storage::{FileStorage, MultipartId, PartId, StoragePath},
};
use dashmap::DashMap;
use entity::sea_orm_active_enums::AccessType;
use futures_util::{StreamExt, TryStreamExt, stream};
use harmonia_store_core::store_path::StorePath;
use http::StatusCode;
use schemars::JsonSchema;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use shared::{
  api::push::{
    CHUNK_SIZE, UploadFinishRequest, UploadInfoRequest, UploadInfoResponse, UploadPathRequest,
    UploadPathResponse,
  },
  hash::to_nix_base32,
  pool::FuturePool,
  sig::PublicKey,
};
use tokio::{
  io::{self, AsyncRead, AsyncReadExt},
  sync::Semaphore,
  time::sleep,
};
use tokio_util::io::{InspectReader, StreamReader};
use tracing::warn;
use url::Url;
use uuid::Uuid;

use crate::{
  auth::cli_auth::CliAuth, cache::state::CacheEvictionState, db::DBTrait, utils::client,
};

/// S3 allows at most 10000 parts per multipart upload
const MAX_PARTS: usize = 10000;

pub fn router() -> ApiRouter {
  ApiRouter::new()
    .api_route("/info", post_with(upload_info, |op| op.id("uploadInfo")))
    .api_route("/", post_with(upload_path, |op| op.id("uploadPath")))
    .api_route(
      "/{uuid}/{part}",
      post_with(upload_chunk, |op| op.id("uploadChunk")),
    )
    .api_route(
      "/{uuid}",
      put_with(upload_finish, |op| op.id("uploadFinish")),
    )
}

struct PendingUpload {
  req: UploadPathRequest,
  quota: u64,
  nar_id: Uuid,
  nar_found: bool,
  /// Only set when the storage supports multipart uploads (S3), otherwise parts are stored as separate files
  multipart: Option<MultipartId>,
  parts: Vec<Option<(PartId, u64)>>,
  touched: Instant,
}

struct NarHasher {
  hasher: Sha256,
  size: u64,
  max_size: u64,
}

impl Write for NarHasher {
  fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
    self.size += buf.len() as u64;
    if self.size > self.max_size {
      return Err(std::io::Error::other(
        "Decompressed NAR exceeds declared size",
      ));
    }
    self.hasher.update(buf);
    Ok(buf.len())
  }

  fn flush(&mut self) -> std::io::Result<()> {
    Ok(())
  }
}

#[derive(FromRequestParts, Clone, OperationIo)]
#[from_request(via(Extension))]
pub struct PushState {
  pending_uploads: Arc<DashMap<Uuid, PendingUpload>>,
  upload_limit: Arc<Semaphore>,
}

impl PushState {
  pub fn new(max_concurrent_uploads: usize) -> Self {
    let pending_uploads: Arc<DashMap<Uuid, PendingUpload>> = Arc::new(DashMap::new());

    // Only drops the in-memory state, stored files are tracked in the db and removed by the cleanup job
    tokio::spawn({
      let pending_uploads = pending_uploads.clone();
      async move {
        loop {
          sleep(Duration::from_secs(60)).await;
          let now = Instant::now();
          pending_uploads.retain(|_, u| now.duration_since(u.touched) < Duration::from_secs(300));
        }
      }
    });

    Self {
      pending_uploads,
      upload_limit: Arc::new(Semaphore::new(max_concurrent_uploads)),
    }
  }
}

fn nar_name(nar_id: Uuid) -> StoragePath {
  format!("{nar_id}.nar").into()
}

fn parts_dir(nar_id: Uuid) -> StoragePath {
  format!("parts/{nar_id}").into()
}

fn part_name(nar_id: Uuid, part: usize) -> StoragePath {
  format!("parts/{nar_id}/{part}").into()
}

/// Removes everything an unfinished or failed upload left in the storage, including its db entry.
/// Keeps the db entry on failure so the cleanup job can retry.
pub async fn cleanup_upload(
  storage: &FileStorage,
  db: &Connection,
  nar_id: Uuid,
  multipart: Option<&MultipartId>,
) -> Result<()> {
  let name = nar_name(nar_id);
  if let (Some(id), Some(store)) = (multipart, storage.multipart())
    && let Err(e) = store.abort_multipart(&name, id).await
    // Already aborted by an earlier attempt
    && !matches!(e, object_store::Error::NotFound { .. })
    // Aborting fails if the upload was already completed
    && !storage.exists(&name).await?
  {
    return Err(e).status(StatusCode::INTERNAL_SERVER_ERROR);
  }

  let parts = storage
    .list(Some(&parts_dir(nar_id)))
    .map_ok(|meta| meta.location)
    .try_collect::<Vec<_>>()
    .await
    .status(StatusCode::INTERNAL_SERVER_ERROR)?;
  for part in parts {
    storage.delete_file(&part).await?;
  }

  // Once the nar entry exists the file belongs to it and is removed by the orphan cleanup
  if !db.nar().nar_exists(nar_id).await? {
    storage.delete_file(&name).await?;
  }
  db.nar().delete_upload(nar_id).await?;

  Ok(())
}

async fn upload_info(
  auth: CliAuth,
  db: Connection,
  Json(req): Json<UploadInfoRequest>,
) -> Result<Json<UploadInfoResponse>> {
  let paths = req
    .paths
    .iter()
    .filter_map(|p| StorePath::from_base_path(p).ok())
    .collect::<Vec<_>>();
  if paths.len() != req.paths.len() {
    bail!(UNPROCESSABLE_ENTITY, "One or more invalid store paths");
  }

  let Some(cache) = db
    .cache()
    .by_name_filtered(req.cache, auth.user_id, AccessType::Edit)
    .await?
  else {
    bail!(NOT_FOUND, "Cache not found or access denied");
  };

  if !cache.allow_force_push && req.force {
    bail!(NOT_ACCEPTABLE, "Force push is not allowed for this cache");
  }

  let mut missing_paths = db.nar().missing_paths(cache.id, paths).await?;
  if missing_paths.is_empty() {
    bail!(NO_CONTENT, "All paths are already present in the cache");
  }

  if req.force {
    return Ok(Json(UploadInfoResponse {
      paths: missing_paths.into_iter().map(|p| p.to_string()).collect(),
      cache: cache.id,
    }));
  }

  let mut downstream_caches = db.cache().downstream_caches(cache.id).await?;
  let client = client();
  while let Some(downstream) = downstream_caches.pop() {
    let mut futures = Vec::new();

    for path in missing_paths {
      let url = Url::parse(&downstream.url)
        .unwrap()
        .join(&format!("{}.narinfo", path.hash()))?;
      let req = client.head(url).build()?;
      let res_future = client.execute(req);

      futures.push(async move {
        let Ok(res) = res_future.await else {
          return (false, path);
        };
        let Ok(res) = res.error_for_status() else {
          return (false, path);
        };

        (res.status() == reqwest::StatusCode::OK, path)
      });
    }

    let results = FuturePool::new(futures).run().await;
    let mut remaining_missing = Vec::new();

    for result in results.into_iter() {
      if let Ok((exists, path)) = result
        && !exists
      {
        remaining_missing.push(path);
      }
    }

    if remaining_missing.is_empty() {
      bail!(
        NO_CONTENT,
        "All paths are already present in the downstream cache"
      );
    }
    missing_paths = remaining_missing;
  }

  Ok(Json(UploadInfoResponse {
    paths: missing_paths.into_iter().map(|p| p.to_string()).collect(),
    cache: cache.id,
  }))
}

async fn upload_path(
  auth: CliAuth,
  db: Connection,
  state: PushState,
  storage: FileStorage,
  Json(req): Json<UploadPathRequest>,
) -> Result<Json<UploadPathResponse>> {
  let Ok(store_path) = StorePath::from_base_path(&req.store_path) else {
    bail!(UNPROCESSABLE_ENTITY, "Invalid store path format");
  };
  let references = req
    .references
    .iter()
    .flat_map(|r| StorePath::from_base_path(r).ok())
    .collect::<Vec<_>>();
  if references.len() != req.references.len() {
    bail!(
      UNPROCESSABLE_ENTITY,
      "One or more invalid reference store paths"
    );
  }

  let Some(cache) = db
    .cache()
    .by_id_filtered(req.cache, auth.user_id, AccessType::Edit)
    .await?
  else {
    bail!("Cache not found or access denied");
  };

  if !cache.allow_force_push && req.force {
    bail!("Force push is not allowed for this cache");
  }

  let pk =
    PublicKey::from_string(&cache.public_signing_key).status(StatusCode::INTERNAL_SERVER_ERROR)?;
  if !pk.verify(
    &req.signature,
    &store_path,
    &req.nar_hash,
    req.nar_size,
    &references,
  ) {
    bail!("Invalid signature");
  }

  if db
    .nar()
    .is_store_path_in_cache(cache.id, &req.store_path.to_string())
    .await?
  {
    bail!("Store path is already in the cache");
  }

  if !req.force {
    let mut downstream_caches = db.cache().downstream_caches(cache.id).await?;
    let client = client();
    while let Some(downstream) = downstream_caches.pop() {
      let url = Url::parse(&downstream.url)
        .unwrap()
        .join(&format!("{}.narinfo", store_path.hash()))?;
      let req = client.head(url).build()?;
      if let Ok(res) = client.execute(req).await
        && let Ok(res) = res.error_for_status()
        && res.status() == reqwest::StatusCode::OK
      {
        bail!("Store path is already in the downstream cache");
      }
    }
  }

  let nar_id = Uuid::now_v7();
  let nar_found = db
    .nar()
    .find_nar(&req.nar_hash, req.nar_size)
    .await?
    .is_some();
  let multipart = match storage.multipart() {
    Some(multipart) if !nar_found => Some(
      multipart
        .create_multipart(&nar_name(nar_id))
        .await
        .status(StatusCode::INTERNAL_SERVER_ERROR)?,
    ),
    _ => None,
  };
  if !nar_found {
    // a crash between creating the multipart upload and this insert leaks it, add an S3 lifecycle rule
    // for incomplete multipart uploads if that matters
    if let Err(e) = db.nar().create_upload(nar_id, multipart.clone()).await {
      if let (Some(id), Some(store)) = (&multipart, storage.multipart())
        && let Err(e) = store.abort_multipart(&nar_name(nar_id), id).await
      {
        warn!("Failed to abort multipart upload of {nar_id}: {e}");
      }
      return Err(e.into());
    }
  }

  let upload_id = Uuid::new_v4();
  state.pending_uploads.insert(
    upload_id,
    PendingUpload {
      req,
      quota: cache.quota as u64,
      nar_id,
      nar_found,
      multipart,
      parts: Vec::new(),
      touched: Instant::now(),
    },
  );

  Ok(Json(UploadPathResponse {
    uuid: upload_id,
    nar_found,
  }))
}

#[derive(Deserialize, Clone, Copy, JsonSchema)]
struct UploadChunkPath {
  uuid: Uuid,
  part: usize,
}

async fn upload_chunk(
  _auth: CliAuth,
  db: Connection,
  state: PushState,
  storage: FileStorage,
  Path(path): Path<UploadChunkPath>,
  body: Request,
) -> Result<()> {
  if path.part >= MAX_PARTS {
    bail!(BAD_REQUEST, "Too many upload parts");
  }
  let _permit = state
    .upload_limit
    .acquire()
    .await
    .context("Upload limiter closed")?;

  let chunk = to_bytes(body.into_body(), CHUNK_SIZE)
    .await
    .status(StatusCode::PAYLOAD_TOO_LARGE)?;

  let Some((nar_id, multipart)) = state.pending_uploads.get(&path.uuid).and_then(|upload| {
    let others = upload
      .parts
      .iter()
      .enumerate()
      .filter(|(i, _)| *i != path.part)
      .filter_map(|(_, p)| p.as_ref().map(|(_, size)| *size))
      .sum::<u64>();
    (!upload.nar_found && others + chunk.len() as u64 <= upload.quota)
      .then(|| (upload.nar_id, upload.multipart.clone()))
  }) else {
    bail!("Invalid upload session or file size exceeds cache quota");
  };

  let size = chunk.len() as u64;
  // Re-uploading a part overwrites the previous attempt, which makes retries safe
  let part_id = match (&multipart, storage.multipart()) {
    (Some(id), Some(store)) => store
      .put_part(&nar_name(nar_id), id, path.part, chunk.into())
      .await
      .status(StatusCode::INTERNAL_SERVER_ERROR)?,
    _ => {
      storage
        .save_file(&mut &chunk[..], part_name(nar_id, path.part))
        .await?;
      PartId {
        content_id: String::new(),
      }
    }
  };

  let Some(mut upload) = state.pending_uploads.get_mut(&path.uuid) else {
    bail!("Upload session expired");
  };
  if upload.parts.len() <= path.part {
    upload.parts.resize(path.part + 1, None);
  }
  upload.parts[path.part] = Some((part_id, size));
  upload.touched = Instant::now();
  drop(upload);
  db.nar().touch_upload(nar_id).await?;

  Ok(())
}

/// Assembles the uploaded parts into the final file and validates its content
async fn store_upload(
  storage: &FileStorage,
  upload: &PendingUpload,
  req: &UploadFinishRequest,
) -> Result<(String, u64)> {
  let Some(parts) = upload.parts.iter().cloned().collect::<Option<Vec<_>>>() else {
    bail!("Missing upload parts");
  };
  if parts.is_empty() {
    bail!("Missing upload parts");
  }

  let name = nar_name(upload.nar_id);
  if let (Some(id), Some(multipart)) = (&upload.multipart, storage.multipart()) {
    multipart
      .complete_multipart(&name, id, parts.into_iter().map(|(p, _)| p).collect())
      .await
      .status(StatusCode::INTERNAL_SERVER_ERROR)?;
  } else {
    let mut streams = Vec::new();
    for part in 0..parts.len() {
      streams.push(
        storage
          .get_file(&part_name(upload.nar_id, part), None)
          .await?
          .into_data_stream(),
      );
    }
    let mut reader = StreamReader::new(stream::iter(streams).flatten().map_err(io::Error::other));
    storage.save_file(&mut reader, name.clone()).await?;
    for part in 0..parts.len() {
      storage.delete_file(&part_name(upload.nar_id, part)).await?;
    }
  }

  let file = storage.get_file(&name, None).await?;
  let reader = StreamReader::new(file.into_data_stream().map_err(io::Error::other));
  let (file_hash, file_size) = verify_nar(
    reader,
    upload.quota,
    &upload.req.nar_hash,
    upload.req.nar_size,
  )
  .await?;

  if file_hash != req.file_hash || file_size != req.file_size {
    bail!("File hash or size mismatch");
  }

  Ok((file_hash, file_size))
}

async fn verify_nar<R: AsyncRead + Unpin + Send>(
  body: R,
  quota: u64,
  nar_hash: &str,
  nar_size: u64,
) -> Result<(String, u64)> {
  let mut file_hasher = Sha256::new();
  let mut file_size = 0;
  let mut nar = zstd::stream::write::Decoder::new(NarHasher {
    hasher: Sha256::new(),
    size: 0,
    max_size: nar_size,
  })?;
  let mut decode_err = None;

  let mut reader = InspectReader::new(body.take(quota.saturating_add(1)), |buf| {
    file_hasher.update(buf);
    file_size += buf.len() as u64;
    if decode_err.is_none()
      && let Err(e) = nar.write_all(buf)
    {
      decode_err = Some(e);
    }
  });
  io::copy(&mut reader, &mut io::sink()).await?;
  drop(reader);

  if file_size > quota {
    bail!("File size exceeds cache quota");
  } else if let Some(e) = decode_err.or_else(|| nar.flush().err()) {
    bail!("Failed to decompress NAR: {e}");
  }
  let decoded = nar.into_inner();
  if to_nix_base32(&decoded.hasher.finalize()) != nar_hash || decoded.size != nar_size {
    bail!("NAR hash or size mismatch");
  }

  Ok((to_nix_base32(&file_hasher.finalize()), file_size))
}

async fn upload_finish(
  _auth: CliAuth,
  db: Connection,
  state: PushState,
  storage: FileStorage,
  lock: CacheEvictionState,
  Path(uuid): Path<Uuid>,
  Json(req): Json<UploadFinishRequest>,
) -> Result<()> {
  let Some((_, upload)) = state.pending_uploads.remove(&uuid) else {
    bail!("Invalid upload session");
  };
  let _permit = state
    .upload_limit
    .acquire()
    .await
    .context("Upload limiter closed")?;

  let stored = if upload.nar_found {
    None
  } else {
    // Keeps the cleanup job away while the file is assembled and verified
    db.nar().touch_upload(upload.nar_id).await?;
    match store_upload(&storage, &upload, &req).await {
      Ok(stored) => Some(stored),
      Err(e) => {
        if let Err(e) =
          cleanup_upload(&storage, &db, upload.nar_id, upload.multipart.as_ref()).await
        {
          warn!("Failed to clean up upload {}: {e:?}", upload.nar_id);
        }
        return Err(e);
      }
    }
  };

  let uploaded = stored.is_some();
  let info = upload.req;
  let (nar_id, nar_hash, nar_size, file_hash, file_size) = match (
    db.nar()
      .create_nar(upload.nar_id, &info.nar_hash, info.nar_size)
      .await?,
    stored,
  ) {
    (Some(existing), _) => {
      if uploaded {
        // Someone else uploaded the same NAR in the meantime
        storage.delete_file(&nar_name(upload.nar_id)).await?;
      }
      (
        existing.id,
        existing.nar_hash,
        existing.nar_size as u64,
        existing.hash,
        existing.size as u64,
      )
    }
    (None, Some((file_hash, file_size))) => (
      upload.nar_id,
      info.nar_hash,
      info.nar_size,
      file_hash,
      file_size,
    ),
    (None, None) => {
      bail!("NAR no longer exists on the server, retry the upload");
    }
  };
  if uploaded {
    // The file is now owned by the nar entry
    db.nar().delete_upload(upload.nar_id).await?;
  }

  let lock = lock.lock_cache(info.cache).await;
  let Some(cache) = db.cache().by_id(info.cache).await? else {
    bail!("Cache not found");
  };

  let Some(size) = db.cache().cache_size(info.cache).await? else {
    bail!("Cache not found");
  };

  if cache.quota < file_size as i64 {
    bail!("File size exceeds cache quota");
  }

  let diff = (size + file_size as i64) - cache.quota;
  if diff > 0 {
    db.cache()
      .evict(info.cache, diff, cache.eviction_policy)
      .await?;
  }

  let store_path_hash = StorePath::from_base_path(&info.store_path)
    .unwrap()
    .hash()
    .to_string();
  db.nar()
    .create_path(
      nar_id,
      info.cache,
      info.store_path,
      store_path_hash,
      nar_hash,
      nar_size,
      file_hash,
      file_size,
      info.deriver,
      info.signature,
      info.references,
    )
    .await?;

  drop(lock); // Release the cache lock as soon as possible

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn nar() -> Vec<u8> {
    (0..200_000u32)
      .flat_map(|i| (i % 251).to_le_bytes())
      .collect()
  }

  fn hash(data: &[u8]) -> String {
    to_nix_base32(&Sha256::digest(data))
  }

  #[tokio::test]
  async fn verifies_valid_nar() {
    let nar = nar();
    let file = zstd::encode_all(&nar[..], 3).unwrap();

    let (file_hash, file_size) = verify_nar(&file[..], u64::MAX, &hash(&nar), nar.len() as u64)
      .await
      .unwrap();

    assert_eq!(file_hash, hash(&file));
    assert_eq!(file_size, file.len() as u64);
  }

  async fn assert_rejected(file: &[u8], quota: u64, nar_hash: &str, nar_size: u64) {
    assert!(verify_nar(file, quota, nar_hash, nar_size).await.is_err());
  }

  #[tokio::test]
  async fn rejects_over_quota() {
    let nar = nar();
    let file = zstd::encode_all(&nar[..], 3).unwrap();
    assert_rejected(&file, file.len() as u64 - 1, &hash(&nar), nar.len() as u64).await;
  }

  #[tokio::test]
  async fn accepts_exactly_quota() {
    let nar = nar();
    let file = zstd::encode_all(&nar[..], 3).unwrap();
    let res = verify_nar(&file[..], file.len() as u64, &hash(&nar), nar.len() as u64).await;
    assert!(res.is_ok());
  }

  #[tokio::test]
  async fn rejects_invalid_zstd() {
    let nar = nar();
    assert_rejected(&nar, u64::MAX, &hash(&nar), nar.len() as u64).await;
  }

  #[tokio::test]
  async fn rejects_truncated_zstd() {
    let nar = nar();
    let file = zstd::encode_all(&nar[..], 3).unwrap();
    assert_rejected(
      &file[..file.len() / 2],
      u64::MAX,
      &hash(&nar),
      nar.len() as u64,
    )
    .await;
  }

  #[tokio::test]
  async fn rejects_hash_mismatch() {
    let nar = nar();
    let file = zstd::encode_all(&nar[..], 3).unwrap();
    assert_rejected(&file, u64::MAX, &hash(b"other"), nar.len() as u64).await;
  }

  #[tokio::test]
  async fn rejects_larger_than_declared_size() {
    let nar = nar();
    let file = zstd::encode_all(&nar[..], 3).unwrap();
    assert_rejected(&file, u64::MAX, &hash(&nar), nar.len() as u64 - 1).await;
  }

  #[tokio::test]
  async fn rejects_smaller_than_declared_size() {
    let nar = nar();
    let file = zstd::encode_all(&nar[..], 3).unwrap();
    assert_rejected(&file, u64::MAX, &hash(&nar), nar.len() as u64 + 1).await;
  }

  async fn local_storage() -> (FileStorage, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("hibernation-push-{}", Uuid::new_v4()));
    let config = centaurus::storage::StorageConfig {
      storage_path: dir.to_string_lossy().into_owned(),
      ..Default::default()
    };
    (FileStorage::init(&config).await.unwrap(), dir)
  }

  fn pending(nar: &[u8], parts: usize) -> PendingUpload {
    PendingUpload {
      req: UploadPathRequest {
        cache: Uuid::new_v4(),
        force: false,
        store_path: String::new(),
        nar_hash: hash(nar),
        nar_size: nar.len() as u64,
        deriver: None,
        references: Vec::new(),
        signature: String::new(),
      },
      quota: u64::MAX,
      nar_id: Uuid::new_v4(),
      nar_found: false,
      multipart: None,
      parts: vec![
        Some((
          PartId {
            content_id: String::new()
          },
          0
        ));
        parts
      ],
      touched: Instant::now(),
    }
  }

  #[tokio::test]
  async fn assembles_local_parts() {
    let (storage, dir) = local_storage().await;
    let nar = nar();
    let file = zstd::encode_all(&nar[..], 3).unwrap();
    let chunks = file.chunks(file.len() / 3 + 1).collect::<Vec<_>>();
    let upload = pending(&nar, chunks.len());
    // Upload out of order like retried chunks could
    for (i, chunk) in chunks.iter().enumerate().rev() {
      storage
        .save_file(&mut &chunk[..], part_name(upload.nar_id, i))
        .await
        .unwrap();
    }

    let req = UploadFinishRequest {
      file_hash: hash(&file),
      file_size: file.len() as u64,
    };
    store_upload(&storage, &upload, &req).await.unwrap();

    assert_eq!(
      std::fs::read(dir.join(format!("{}.nar", upload.nar_id))).unwrap(),
      file
    );
    assert!(!storage.exists(&part_name(upload.nar_id, 0)).await.unwrap());
    let _ = std::fs::remove_dir_all(dir);
  }

  #[tokio::test]
  async fn rejects_missing_part() {
    let (storage, dir) = local_storage().await;
    let nar = nar();
    let mut upload = pending(&nar, 2);
    upload.parts[0] = None;
    let req = UploadFinishRequest {
      file_hash: String::new(),
      file_size: 0,
    };
    assert!(store_upload(&storage, &upload, &req).await.is_err());
    let _ = std::fs::remove_dir_all(dir);
  }

  #[tokio::test]
  async fn limits_concurrent_uploads() {
    let state = PushState::new(3);
    let permits = (0..3)
      .map(|_| state.upload_limit.clone().try_acquire_owned().unwrap())
      .collect::<Vec<_>>();
    assert!(state.upload_limit.try_acquire().is_err());
    drop(permits);
    assert!(state.upload_limit.try_acquire().is_ok());
  }
}
