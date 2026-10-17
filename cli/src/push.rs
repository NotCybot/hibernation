use std::{
  collections::HashSet,
  fmt::Write,
  io::IsTerminal,
  sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
  },
  time::Duration,
};

use async_compression::{Level, tokio::bufread::ZstdEncoder};
use bytes::{Bytes, BytesMut};
use centaurus::{error::Result, eyre::Context};
use harmonia_protocol::NarHash;
use harmonia_store_core::store_path::{StoreDir, StorePath};
use harmonia_store_remote::{DaemonClient, DaemonStore};
use indicatif::{ProgressState, ProgressStyle};
use sha2::{Digest, Sha256};
use shared::{
  api::push::{CHUNK_SIZE, UploadFinishRequest, UploadPathRequest},
  hash::to_nix_base32,
  sig::KeyPair,
};
use tokio::{
  io::{AsyncReadExt, BufReader},
  net::unix::{OwnedReadHalf, OwnedWriteHalf},
  time::sleep,
};
use tokio_util::io::InspectReader;
use tracing::{Span, error, info, info_span, instrument, warn};
use tracing_indicatif::span_ext::IndicatifSpanExt;
use uuid::Uuid;

use crate::api::{ApiClient, PushInfoResult};

const CHUNK_RETRIES: u32 = 3;

#[derive(Debug, Clone)]
struct PathInfo {
  store_path: StorePath,
  nar_hash: NarHash,
  nar_size: u64,
  deriver: Option<StorePath>,
  references: Vec<StorePath>,
}

pub async fn push_paths(
  api: ApiClient,
  cache: String,
  paths: &[String],
  no_deps: bool,
  force: bool,
  signing_key: KeyPair,
  threads: usize,
) {
  info!("Collecting paths for push");
  let store_dir = StoreDir::default();
  let mut conn = DaemonClient::builder().connect_daemon().await.unwrap();

  let mut queue = paths
    .iter()
    .map(|s| s.trim())
    .filter(|s| !s.is_empty())
    .flat_map(|s| {
      store_dir.parse(s).ok().or_else(|| {
        error!("Invalid store path: {}, skipping.", s);
        None
      })
    })
    .collect::<Vec<StorePath>>();

  let mut visited = HashSet::new();
  let mut path_infos = Vec::new();

  while let Some(path) = queue.pop() {
    if visited.insert(path.clone()) {
      let Some(info) = conn.query_path_info(&path).await.unwrap() else {
        error!("Path {} not found in local store, skipping.", path);
        continue;
      };

      path_infos.push(PathInfo {
        store_path: path,
        deriver: info.deriver,
        nar_hash: info.nar_hash,
        nar_size: info.nar_size,
        references: info.references.clone().into_iter().collect(),
      });

      if !no_deps {
        for dep in info.references {
          queue.push(dep);
        }
      }
    }
  }

  let paths = visited.into_iter().map(|p| p.to_string()).collect();

  info!("Collected {} paths for push", path_infos.len());
  info!("Checking which paths are missing on the server");
  let res = api.push_info(cache, paths, force).await.unwrap();

  let res = match res {
    PushInfoResult::Success(res) => res,
    PushInfoResult::AllPathsExist => {
      warn!("All paths already exist on the server, nothing to push.");
      std::process::exit(0);
    }
    PushInfoResult::CacheNotFound => {
      error!("Cache not found on the server.");
      std::process::exit(1);
    }
    PushInfoResult::ForcePushNotAllowed => {
      warn!("Force push not allowed by the cache.");
      std::process::exit(1);
    }
  };

  let path_infos = path_infos
    .into_iter()
    .filter(|info| res.paths.contains(&info.store_path.to_string()))
    .collect::<Vec<_>>();

  let to_push = path_infos.len();
  info!("Pushing {} paths to the server", to_push);
  let total_size = path_infos.iter().map(|info| info.nar_size).sum();
  let jobs = Arc::new(tokio::sync::Mutex::new(path_infos));
  let error = Arc::new(AtomicBool::new(false));
  let done = Arc::new(AtomicUsize::new(0));
  let mut handles = Vec::new();

  let tty = std::io::stdin().is_terminal();
  let span = info_span!("header");
  if tty {
    span.pb_set_length(total_size);
    span.pb_set_message(&format!("0/{to_push} paths"));
    span.pb_set_style(
      &ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {binary_bytes}/{binary_total_bytes} {msg} ({eta})",
      )
      .unwrap()
      .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
        write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
      })
      .progress_chars("#>-"),
    );
  }
  let enter = span.enter();

  for _ in 0..(threads).min(to_push) {
    let jobs = jobs.clone();
    let error = error.clone();
    let done = done.clone();
    let api = api.clone();
    let signing_key = signing_key.clone();
    let cache = res.cache;
    let span = span.clone();

    let handle = tokio::spawn(async move {
      upload_worker(
        jobs,
        error,
        done,
        to_push,
        api,
        cache,
        force,
        signing_key,
        span,
      )
      .await;
    });
    handles.push(handle);
  }

  for handle in handles {
    handle.await.unwrap();
  }

  std::mem::drop(enter);
  std::mem::drop(span);

  if error.load(Ordering::SeqCst) {
    error!("Failed to upload some paths.");
    std::process::exit(1);
  } else {
    info!("Successfully uploaded all paths.");
  }
}

#[allow(clippy::too_many_arguments)]
async fn upload_worker(
  jobs: Arc<tokio::sync::Mutex<Vec<PathInfo>>>,
  error: Arc<AtomicBool>,
  done: Arc<AtomicUsize>,
  to_push: usize,
  api: ApiClient,
  cache: Uuid,
  force: bool,
  signing_key: KeyPair,
  span: Span,
) {
  let mut conn = DaemonClient::builder().connect_daemon().await.unwrap();

  loop {
    let mut lock = jobs.lock().await;
    let Some(info) = lock.pop() else {
      return;
    };
    drop(lock);

    let mut reported = 0;
    if let Err(result) = upload_path(
      &mut conn,
      &api,
      &info,
      cache,
      force,
      &signing_key,
      &span,
      &mut reported,
    )
    .await
    {
      error!("Failed to upload {}: {:?}", info.store_path, result);
      error.store(true, Ordering::SeqCst);
    } else {
      info!("Successfully uploaded {}", info.store_path);
    }
    // Account for the rest of the path, e.g. when it already existed or failed
    span.pb_inc(info.nar_size.saturating_sub(reported));
    let done = done.fetch_add(1, Ordering::SeqCst) + 1;
    span.pb_set_message(&format!("{done}/{to_push} paths"));
  }
}

#[instrument(skip_all)]
#[allow(clippy::too_many_arguments)]
async fn upload_path(
  daemon: &mut DaemonClient<OwnedReadHalf, OwnedWriteHalf>,
  api: &ApiClient,
  info: &PathInfo,
  cache: Uuid,
  force: bool,
  signing_key: &KeyPair,
  header: &Span,
  reported: &mut u64,
) -> Result<()> {
  let span = Span::current();
  if std::io::stdin().is_terminal() {
    span.pb_set_length(info.nar_size);
    span.pb_set_message(&info.store_path.to_string());
    span.pb_set_style(
      &ProgressStyle::with_template(
        "↳ Uploading {msg} [{bar:20.cyan/blue}] {binary_bytes}/{binary_total_bytes}",
      )
      .unwrap()
      .progress_chars("#>-"),
    );
  }

  let nar_hash = to_nix_base32(info.nar_hash.as_ref());
  let signature = signing_key.sign(&info.store_path, &nar_hash, info.nar_size, &info.references);

  let upload = api
    .upload_path(&UploadPathRequest {
      cache,
      force,
      store_path: info.store_path.to_string(),
      deriver: info.deriver.as_ref().map(|d| d.to_string()),
      nar_size: info.nar_size,
      nar_hash,
      references: info.references.iter().map(|r| r.to_string()).collect(),
      signature,
    })
    .await?;
  let upload_id = upload.uuid;

  let mut hasher = Sha256::new();
  let mut file_size = 0;
  if !upload.nar_found {
    let nar = daemon
      .nar_from_path(&info.store_path)
      .await
      .context("Failed to get nar")?;
    // Uncompressed bytes read, compressed sizes are not known upfront
    let read = AtomicU64::new(0);
    let nar = BufReader::new(InspectReader::new(nar, |bytes| {
      read.fetch_add(bytes.len() as u64, Ordering::Relaxed);
    }));
    let mut encoder = ZstdEncoder::with_quality(nar, Level::Default);

    for part in 0.. {
      let mut chunk = BytesMut::with_capacity(CHUNK_SIZE);
      while chunk.len() < CHUNK_SIZE && encoder.read_buf(&mut chunk).await? > 0 {}
      // Always send at least one (possibly empty) part
      if chunk.is_empty() && part > 0 {
        break;
      }
      hasher.update(&chunk);
      file_size += chunk.len() as u64;
      let last = chunk.len() < CHUNK_SIZE;
      upload_chunk(api, upload_id, part, chunk.freeze()).await?;

      let progress = read.load(Ordering::Relaxed).min(info.nar_size) - *reported;
      span.pb_inc(progress);
      header.pb_inc(progress);
      *reported += progress;
      if last {
        break;
      }
    }
  }
  let file_hash = to_nix_base32(&hasher.finalize());

  api
    .upload_finish(
      upload_id,
      UploadFinishRequest {
        file_hash,
        file_size,
      },
    )
    .await?;

  Ok(())
}

async fn upload_chunk(api: &ApiClient, upload_id: Uuid, part: u32, chunk: Bytes) -> Result<()> {
  let mut attempt = 0;
  loop {
    match api.upload_chunk(upload_id, part, chunk.clone()).await {
      Ok(()) => return Ok(()),
      Err(e) if attempt < CHUNK_RETRIES => {
        let delay = Duration::from_secs(1 << attempt);
        warn!("Failed to upload chunk {part}, retrying in {delay:?}: {e:#}");
        sleep(delay).await;
        attempt += 1;
      }
      Err(e) => return Err(e),
    }
  }
}
