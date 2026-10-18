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
use centaurus::{bail, error::Result, eyre::Context};
use harmonia_protocol::NarHash;
use harmonia_store_core::store_path::{StoreDir, StorePath};
use harmonia_store_remote::{DaemonClient, DaemonStore};
use indicatif::{BinaryBytes, ProgressState, ProgressStyle};
use sha2::{Digest, Sha256};
use shared::{
  api::push::{CHUNK_SIZE, UploadFinishRequest, UploadPathRequest},
  hash::to_nix_base32,
  sig::KeyPair,
};
use tokio::{
  io::{AsyncRead, AsyncReadExt, BufReader},
  net::unix::{OwnedReadHalf, OwnedWriteHalf},
  time::{interval, sleep},
};
use tokio_util::io::InspectReader;
use tracing::{Span, error, info, info_span, instrument, warn};
use tracing_indicatif::span_ext::IndicatifSpanExt;
use uuid::Uuid;

use crate::api::{ApiClient, PushInfoResult};

const CHUNK_RETRIES: u32 = 3;
const PROGRESS_TICK: Duration = Duration::from_millis(100);
const PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(5);

/// Overall push progress, rendered by the header bar on a tty and logged otherwise
struct Progress {
  header: Span,
  uploaded: AtomicU64,
  done: AtomicUsize,
}

impl Progress {
  fn inc(&self, bytes: u64) {
    self.header.pb_inc(bytes);
    self.uploaded.fetch_add(bytes, Ordering::Relaxed);
  }
}

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
) -> Result<()> {
  info!("Collecting paths for push");
  let store_dir = StoreDir::default();
  let mut conn = DaemonClient::builder()
    .connect_daemon()
    .await
    .context("Failed to connect to the nix daemon")?;

  let roots = paths
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

  let path_infos = collect_paths(roots, no_deps, async |path| {
    let info = conn
      .query_path_info(path)
      .await
      .context("Failed to query path info")?;
    Ok(info.map(|info| PathInfo {
      store_path: path.clone(),
      deriver: info.deriver,
      nar_hash: info.nar_hash,
      nar_size: info.nar_size,
      references: info.references.into_iter().collect(),
    }))
  })
  .await?;

  // Only paths present locally can be uploaded
  let paths = path_infos
    .iter()
    .map(|info| info.store_path.to_string())
    .collect();

  info!("Collected {} paths for push", path_infos.len());
  info!("Checking which paths are missing on the server");
  let res = api
    .push_info(cache, paths, force)
    .await
    .context("Failed to query missing paths")?;

  let res = match res {
    PushInfoResult::Success(res) => res,
    PushInfoResult::AllPathsExist => {
      warn!("All paths already exist on the server, nothing to push.");
      return Ok(());
    }
    PushInfoResult::CacheNotFound => {
      bail!("Cache not found on the server.");
    }
    PushInfoResult::ForcePushNotAllowed => {
      bail!("Force push not allowed by the cache.");
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
  let progress = Arc::new(Progress {
    header: span.clone(),
    uploaded: AtomicU64::new(0),
    done: AtomicUsize::new(0),
  });

  let logger = (!tty).then(|| {
    let progress = progress.clone();
    tokio::spawn(async move {
      let mut tick = interval(PROGRESS_LOG_INTERVAL);
      loop {
        tick.tick().await;
        let uploaded = progress.uploaded.load(Ordering::Relaxed);
        let percent = uploaded as f64 * 100.0 / total_size.max(1) as f64;
        info!(
          "Progress: {}/{} ({percent:.1}%), {}/{to_push} paths",
          BinaryBytes(uploaded),
          BinaryBytes(total_size),
          progress.done.load(Ordering::Relaxed),
        );
      }
    })
  });

  for _ in 0..(threads).min(to_push) {
    let jobs = jobs.clone();
    let error = error.clone();
    let progress = progress.clone();
    let api = api.clone();
    let signing_key = signing_key.clone();
    let cache = res.cache;

    let handle = tokio::spawn(async move {
      upload_worker(
        jobs,
        error,
        to_push,
        api,
        cache,
        force,
        signing_key,
        progress,
      )
      .await;
    });
    handles.push(handle);
  }

  for handle in handles {
    handle.await.unwrap();
  }
  if let Some(logger) = logger {
    logger.abort();
  }

  std::mem::drop(enter);
  std::mem::drop(span);

  if error.load(Ordering::SeqCst) {
    bail!("Failed to upload some paths.");
  }
  info!("Successfully uploaded all paths.");
  Ok(())
}

/// Walk `roots` and (unless `no_deps`) their references, querying each path once.
/// Paths `query` does not find are skipped.
async fn collect_paths(
  roots: Vec<StorePath>,
  no_deps: bool,
  mut query: impl AsyncFnMut(&StorePath) -> Result<Option<PathInfo>>,
) -> Result<Vec<PathInfo>> {
  let mut queue = roots;
  let mut visited = HashSet::new();
  let mut path_infos = Vec::new();

  while let Some(path) = queue.pop() {
    if !visited.insert(path.clone()) {
      continue;
    }
    let Some(info) = query(&path).await? else {
      error!("Path {} not found in local store, skipping.", path);
      continue;
    };
    if !no_deps {
      queue.extend(info.references.iter().cloned());
    }
    path_infos.push(info);
  }

  Ok(path_infos)
}

#[allow(clippy::too_many_arguments)]
async fn upload_worker(
  jobs: Arc<tokio::sync::Mutex<Vec<PathInfo>>>,
  error: Arc<AtomicBool>,
  to_push: usize,
  api: ApiClient,
  cache: Uuid,
  force: bool,
  signing_key: KeyPair,
  progress: Arc<Progress>,
) {
  let mut conn = match DaemonClient::builder().connect_daemon().await {
    Ok(conn) => conn,
    Err(e) => {
      error!("Failed to connect to the nix daemon: {e:?}");
      error.store(true, Ordering::SeqCst);
      return;
    }
  };

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
      &progress,
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
    progress.inc(info.nar_size.saturating_sub(reported));
    let done = progress.done.fetch_add(1, Ordering::SeqCst) + 1;
    progress
      .header
      .pb_set_message(&format!("{done}/{to_push} paths"));
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
  progress: &Progress,
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
      let chunk = read_chunk(&mut encoder, CHUNK_SIZE).await?;
      // Always send at least one (possibly empty) part
      if chunk.is_empty() && part > 0 {
        break;
      }
      hasher.update(&chunk);
      file_size += chunk.len() as u64;
      let last = chunk.len() < CHUNK_SIZE;

      // Attribute the uncompressed bytes of this chunk proportionally to the compressed bytes sent
      let chunk_len = chunk.len().max(1) as u64;
      let start = *reported;
      let chunk_read = read.load(Ordering::Relaxed).min(info.nar_size) - start;
      let mut advance = |to: u64| {
        let delta = to.saturating_sub(*reported);
        span.pb_inc(delta);
        progress.inc(delta);
        *reported += delta;
      };

      let sent = Arc::new(AtomicU64::new(0));
      let upload = upload_chunk(api, upload_id, part, chunk.freeze(), &sent);
      tokio::pin!(upload);
      let mut tick = interval(PROGRESS_TICK);
      loop {
        tokio::select! {
          res = &mut upload => break res?,
          _ = tick.tick() => {
            let sent = sent.load(Ordering::Relaxed).min(chunk_len);
            advance(start + chunk_read * sent / chunk_len);
          }
        }
      }
      advance(start + chunk_read);

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

/// Read up to `size` bytes, less only at the end of the stream
async fn read_chunk(reader: &mut (impl AsyncRead + Unpin), size: usize) -> Result<BytesMut> {
  let mut chunk = BytesMut::with_capacity(size);
  while chunk.len() < size && reader.read_buf(&mut chunk).await? > 0 {}
  Ok(chunk)
}

async fn upload_chunk(
  api: &ApiClient,
  upload_id: Uuid,
  part: u32,
  chunk: Bytes,
  sent: &Arc<AtomicU64>,
) -> Result<()> {
  let mut attempt = 0;
  loop {
    sent.store(0, Ordering::Relaxed);
    match api
      .upload_chunk(upload_id, part, chunk.clone(), sent.clone())
      .await
    {
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

#[cfg(test)]
mod test {
  use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicU64},
  };

  use bytes::Bytes;
  use harmonia_protocol::NarHash;
  use harmonia_store_core::store_path::{StoreDir, StorePath};
  use sha2::{Digest, Sha256};
  use shared::hash::to_nix_base32;
  use url::Url;
  use uuid::Uuid;
  use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

  use super::{PathInfo, collect_paths, read_chunk, upload_chunk};
  use crate::api::ApiClient;

  // ---- dependency walk ----

  fn sp(name: &str) -> StorePath {
    let hash = to_nix_base32(&Sha256::digest(name));
    let hash = &hash[..32];
    StoreDir::default()
      .parse(&format!("/nix/store/{hash}-{name}"))
      .unwrap()
  }

  fn info(name: &str, refs: &[&str]) -> PathInfo {
    PathInfo {
      store_path: sp(name),
      nar_hash: NarHash::digest(name),
      nar_size: 1,
      deriver: None,
      references: refs.iter().map(|r| sp(r)).collect(),
    }
  }

  /// a -> b, c; b -> c, a (cycle); c -> c (self ref), missing
  fn store() -> HashMap<StorePath, PathInfo> {
    [
      info("a", &["b", "cc"]),
      info("b", &["cc", "a"]),
      info("cc", &["cc", "missing"]),
      info("unrelated", &[]),
    ]
    .into_iter()
    .map(|i| (i.store_path.clone(), i))
    .collect()
  }

  async fn walk(roots: &[&str], no_deps: bool) -> (Vec<StorePath>, Vec<StorePath>) {
    let store = store();
    let mut queried = Vec::new();
    let infos = collect_paths(roots.iter().map(|r| sp(r)).collect(), no_deps, async |p| {
      queried.push(p.clone());
      Ok(store.get(p).cloned())
    })
    .await
    .unwrap();
    let mut found = infos.into_iter().map(|i| i.store_path).collect::<Vec<_>>();
    found.sort();
    queried.sort();
    (found, queried)
  }

  #[tokio::test]
  async fn collect_follows_references_once() {
    let (found, queried) = walk(&["a"], false).await;
    let mut expected = vec![sp("a"), sp("b"), sp("cc")];
    expected.sort();
    assert_eq!(found, expected);
    // every path is queried exactly once, including the missing one
    let mut expected_queried = vec![sp("a"), sp("b"), sp("cc"), sp("missing")];
    expected_queried.sort();
    assert_eq!(queried, expected_queried);
  }

  #[tokio::test]
  async fn collect_no_deps_only_roots() {
    let (found, queried) = walk(&["a", "cc", "a"], true).await;
    let mut expected = vec![sp("a"), sp("cc")];
    expected.sort();
    assert_eq!(found, expected);
    assert_eq!(queried, expected);
  }

  #[tokio::test]
  async fn collect_skips_missing_and_empty() {
    assert_eq!(walk(&["missing"], false).await.0, Vec::<StorePath>::new());
    assert_eq!(walk(&[], false).await.0, Vec::<StorePath>::new());
  }

  #[tokio::test]
  async fn collect_propagates_query_errors() {
    let res = collect_paths(vec![sp("a")], false, async |_| {
      Err(centaurus::anyhow!("daemon down"))
    })
    .await;
    assert!(res.is_err());
  }

  // ---- chunking ----

  #[tokio::test]
  async fn read_chunk_fills_to_size() {
    // A reader yielding many small reads
    let data = (0..=255u8).collect::<Vec<_>>();
    let mut reader = tokio::io::BufReader::with_capacity(7, &data[..]);
    let mut chunks = Vec::new();
    loop {
      let chunk = read_chunk(&mut reader, 100).await.unwrap();
      let len = chunk.len();
      chunks.push(chunk);
      if len < 100 {
        break;
      }
    }
    assert_eq!(
      chunks.iter().map(|c| c.len()).collect::<Vec<_>>(),
      [100, 100, 56]
    );
    assert_eq!(chunks.concat(), data);

    // Exact multiple: last chunk is empty
    let mut reader = &data[..200];
    assert_eq!(read_chunk(&mut reader, 100).await.unwrap().len(), 100);
    assert_eq!(read_chunk(&mut reader, 100).await.unwrap().len(), 100);
    assert_eq!(read_chunk(&mut reader, 100).await.unwrap().len(), 0);
  }

  // ---- retries ----

  async fn chunk_server(failures: u64) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
      .respond_with(ResponseTemplate::new(500))
      .up_to_n_times(failures)
      .with_priority(1)
      .mount(&server)
      .await;
    Mock::given(method("POST"))
      .respond_with(ResponseTemplate::new(200))
      .mount(&server)
      .await;
    server
  }

  fn api(server: &MockServer) -> ApiClient {
    ApiClient::new("tok".into(), Url::parse(&server.uri()).unwrap())
  }

  #[tokio::test(start_paused = true)]
  async fn upload_chunk_retries_then_succeeds() {
    let server = chunk_server(2).await;
    let sent = Arc::new(AtomicU64::new(0));
    let start = tokio::time::Instant::now();
    upload_chunk(
      &api(&server),
      Uuid::new_v4(),
      0,
      Bytes::from_static(b"abc"),
      &sent,
    )
    .await
    .unwrap();
    // 1s + 2s backoff
    assert!(start.elapsed() >= std::time::Duration::from_secs(3));
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    // `sent` is reset per attempt
    assert_eq!(sent.load(std::sync::atomic::Ordering::Relaxed), 3);
  }

  #[tokio::test(start_paused = true)]
  async fn upload_chunk_gives_up_after_retries() {
    let server = chunk_server(4).await;
    let sent = Arc::new(AtomicU64::new(0));
    let start = tokio::time::Instant::now();
    let res = upload_chunk(
      &api(&server),
      Uuid::new_v4(),
      0,
      Bytes::from_static(b"abc"),
      &sent,
    )
    .await;
    assert!(res.is_err());
    assert!(start.elapsed() >= std::time::Duration::from_secs(7));
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
  }
}
