//! `GET`/`HEAD /api/nix/{cache}/nar/{file hash}.nar.{compression}`
mod common;
mod nix_common;

use backend::db::DBTrait;
use common::{CacheOptions, TestNar, TestServer};
use nix_common::{get_status, header, nix, send};
use reqwest::{Method, Response, StatusCode};
use sea_orm::{ConnectionTrait, EntityTrait};
use uuid::Uuid;

async fn setup() -> (TestServer, String) {
  let (server, _) = TestServer::start_with_admin().await;
  server.create_cache("main", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  (server, token)
}

/// A NAR big enough for interesting ranges (incompressible-ish contents)
fn big_nar() -> TestNar {
  let contents: Vec<u8> = (0..20_000u32)
    .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
    .collect();
  TestNar::new("big-1.0", &contents)
}

async fn get_nar(server: &TestServer, cache: &str, nar: &TestNar, range: Option<&str>) -> Response {
  let mut req = nix(server, Method::GET, cache, &nar.nar_url());
  if let Some(range) = range {
    req = req.header("Range", range);
  }
  send(req).await
}

/// `(accessed, last_accessed_at)` of the nar_info row of `nar` in `cache`
async fn access(server: &TestServer, cache: Uuid, nar: &TestNar) -> (i64, chrono::NaiveDateTime) {
  let db = server.db().await;
  let id = db
    .nar()
    .nar_info_data(cache, &nar.hash)
    .await
    .unwrap()
    .unwrap()
    .id;
  let row = entity::nar_info::Entity::find_by_id(id)
    .one(&*db)
    .await
    .unwrap()
    .unwrap();
  (row.accessed, row.last_accessed_at)
}

async fn nar_id(server: &TestServer, cache: Uuid, nar: &TestNar) -> Uuid {
  let db = server.db().await;
  db.nar()
    .get_nar(cache, &nar.file_hash, "zst")
    .await
    .unwrap()
    .unwrap()
    .0
}

// ---- full download ----

#[tokio::test]
async fn nar_full_body_is_byte_exact() {
  let (server, token) = setup().await;
  let nar = big_nar();
  server.push_path(&token, "main", &nar).await;

  let resp = get_nar(&server, "main", &nar, None).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(header(&resp, "content-type"), Some("application/x-nix-nar"));
  assert_eq!(header(&resp, "accept-ranges"), Some("bytes"));
  assert_eq!(
    header(&resp, "content-length"),
    Some(nar.file_size().to_string().as_str())
  );
  assert!(header(&resp, "content-range").is_none());
  let body = resp.bytes().await.unwrap();
  assert_eq!(body, nar.file);
  // and it decompresses to the NAR the narinfo NarHash describes
  assert_eq!(zstd::decode_all(&body[..]).unwrap(), nar.nar);
}

#[tokio::test]
async fn nar_url_from_narinfo_resolves() {
  let (server, token) = setup().await;
  let nar = TestNar::new("hello-1.0", b"hello world");
  server.push_path(&token, "main", &nar).await;

  let narinfo = send(nix(
    &server,
    Method::GET,
    "main",
    &format!("{}.narinfo", nar.hash),
  ))
  .await
  .text()
  .await
  .unwrap();
  let url = narinfo
    .lines()
    .find_map(|l| l.strip_prefix("URL: "))
    .unwrap();
  // relative to the cache root, like nix resolves it
  let resp = send(nix(&server, Method::GET, "main", url)).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(resp.bytes().await.unwrap(), nar.file);
}

#[tokio::test]
async fn nar_cache_name_is_case_insensitive() {
  let (server, token) = setup().await;
  let nar = TestNar::new("hello-1.0", b"hello");
  server.push_path(&token, "main", &nar).await;
  assert_eq!(
    get_nar(&server, "MAIN", &nar, None).await.status(),
    StatusCode::OK
  );
}

#[tokio::test]
async fn nar_missing_and_malformed_paths_are_not_found() {
  let (server, token) = setup().await;
  let nar = TestNar::new("hello-1.0", b"hello");
  server.push_path(&token, "main", &nar).await;
  let other = TestNar::new("other-1.0", b"other");

  for path in [
    other.nar_url(),
    format!("nar/{}.nar", nar.file_hash),
    format!("nar/{}", nar.file_hash),
    format!("nar/{}.nar.xz", nar.file_hash),
    format!("nar/{}.nar.zstd", nar.file_hash),
    format!("nar/{}.nar.zst.nar.zst", nar.file_hash),
    format!("nar/{}.nar.zst", nar.file_hash.to_uppercase()),
    // the store path hash or nar hash is not the file hash
    format!("nar/{}.nar.zst", nar.hash),
    format!("nar/{}.nar.zst", nar.nar_hash),
    "nar/.nar.zst".to_string(),
    "nar/.nar.".to_string(),
  ] {
    assert_eq!(
      get_status(nix(&server, Method::GET, "main", &path)).await,
      StatusCode::NOT_FOUND,
      "{path}"
    );
  }
  assert_eq!(
    get_nar(&server, "nope", &nar, None).await.status(),
    StatusCode::NOT_FOUND
  );
}

#[tokio::test]
async fn nar_of_other_cache_is_not_visible() {
  let (server, token) = setup().await;
  server.create_cache("other", &CacheOptions::default()).await;
  let nar = TestNar::new("hello-1.0", b"hello");
  server.push_path(&token, "main", &nar).await;

  assert_eq!(
    get_nar(&server, "other", &nar, None).await.status(),
    StatusCode::NOT_FOUND
  );
}

#[tokio::test]
async fn deduped_nar_is_served_from_both_caches() {
  let (server, token) = setup().await;
  server.create_cache("other", &CacheOptions::default()).await;
  let nar = big_nar();
  let main = server.push_path(&token, "main", &nar).await.cache.unwrap();
  let other = server.push_path(&token, "other", &nar).await;
  assert!(other.nar_found);
  let other = other.cache.unwrap();
  assert_eq!(
    nar_id(&server, main, &nar).await,
    nar_id(&server, other, &nar).await
  );

  for cache in ["main", "other"] {
    let resp = get_nar(&server, cache, &nar, None).await;
    assert_eq!(resp.status(), StatusCode::OK, "{cache}");
    assert_eq!(resp.bytes().await.unwrap(), nar.file, "{cache}");
  }
  // each cache counts its own access
  assert_eq!(access(&server, main, &nar).await.0, 1);
  assert_eq!(access(&server, other, &nar).await.0, 1);
}

#[tokio::test]
async fn missing_file_on_disk_is_not_found() {
  let (server, token) = setup().await;
  let nar = TestNar::new("hello-1.0", b"hello");
  let cache = server.push_path(&token, "main", &nar).await.cache.unwrap();
  let id = nar_id(&server, cache, &nar).await;
  std::fs::remove_file(server.storage_path().join(format!("{id}.nar"))).unwrap();

  assert_eq!(
    get_nar(&server, "main", &nar, None).await.status(),
    StatusCode::NOT_FOUND
  );
  assert_eq!(
    get_nar(&server, "main", &nar, Some("bytes=0-1"))
      .await
      .status(),
    StatusCode::NOT_FOUND
  );
  assert_eq!(
    get_status(nix(&server, Method::HEAD, "main", &nar.nar_url())).await,
    StatusCode::NOT_FOUND
  );
  // failed downloads are not counted
  assert_eq!(access(&server, cache, &nar).await.0, 0);
}

// ---- access tracking ----

#[tokio::test]
async fn download_counts_access_and_updates_timestamp() {
  let (server, token) = setup().await;
  let nar = TestNar::new("hello-1.0", b"hello");
  let cache = server.push_path(&token, "main", &nar).await.cache.unwrap();
  let (count, created) = access(&server, cache, &nar).await;
  assert_eq!(count, 0);

  tokio::time::sleep(std::time::Duration::from_millis(20)).await;
  get_nar(&server, "main", &nar, None)
    .await
    .bytes()
    .await
    .unwrap();
  let (count, first) = access(&server, cache, &nar).await;
  assert_eq!(count, 1);
  assert!(first > created);

  tokio::time::sleep(std::time::Duration::from_millis(20)).await;
  get_nar(&server, "main", &nar, Some("bytes=0-0"))
    .await
    .bytes()
    .await
    .unwrap();
  let (count, second) = access(&server, cache, &nar).await;
  assert_eq!(count, 2, "range requests count too");
  assert!(second > first);

  // Unsatisfiable range: nothing served, nothing counted
  get_nar(&server, "main", &nar, Some("bytes=100000-")).await;
  assert_eq!(access(&server, cache, &nar).await.0, 2);
}

// ---- HEAD ----

#[tokio::test]
async fn head_nar_has_headers_without_body_or_access() {
  let (server, token) = setup().await;
  let nar = big_nar();
  let cache = server.push_path(&token, "main", &nar).await.cache.unwrap();

  let resp = send(nix(&server, Method::HEAD, "main", &nar.nar_url())).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(header(&resp, "content-type"), Some("application/x-nix-nar"));
  assert_eq!(header(&resp, "accept-ranges"), Some("bytes"));
  assert_eq!(
    header(&resp, "content-length"),
    Some(nar.file_size().to_string().as_str())
  );
  assert!(resp.bytes().await.unwrap().is_empty());
  assert_eq!(access(&server, cache, &nar).await.0, 0);

  let other = TestNar::new("other-1.0", b"other");
  assert_eq!(
    get_status(nix(&server, Method::HEAD, "main", &other.nar_url())).await,
    StatusCode::NOT_FOUND
  );
  assert_eq!(
    get_status(nix(&server, Method::HEAD, "main", "nar/abc")).await,
    StatusCode::NOT_FOUND
  );
}

// ---- ranges ----

#[tokio::test]
async fn range_requests_return_exact_bytes() {
  let (server, token) = setup().await;
  let nar = big_nar();
  server.push_path(&token, "main", &nar).await;
  let size = nar.file_size();
  let n = size as usize;

  let cases: Vec<(String, usize, usize)> = vec![
    ("bytes=0-0".into(), 0, 0),
    ("bytes=0-99".into(), 0, 99),
    ("bytes=100-199".into(), 100, 199),
    (format!("bytes={}-", n - 1), n - 1, n - 1),
    ("bytes=10-".into(), 10, n - 1),
    (format!("bytes=5-{}", n * 2), 5, n - 1),
    ("bytes=-1".into(), n - 1, n - 1),
    ("bytes=-500".into(), n - 500, n - 1),
    (format!("bytes=-{}", n + 10), 0, n - 1),
    ("BYTES=1-2".into(), 1, 2),
    ("bytes=0-99999999999999999999999".into(), 0, n - 1),
  ];
  for (range, start, end) in cases {
    let resp = get_nar(&server, "main", &nar, Some(&range)).await;
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT, "{range}");
    assert_eq!(header(&resp, "accept-ranges"), Some("bytes"));
    assert_eq!(header(&resp, "content-type"), Some("application/x-nix-nar"));
    assert_eq!(
      header(&resp, "content-range"),
      Some(format!("bytes {start}-{end}/{size}").as_str()),
      "{range}"
    );
    assert_eq!(
      header(&resp, "content-length"),
      Some((end - start + 1).to_string().as_str()),
      "{range}"
    );
    assert_eq!(
      resp.bytes().await.unwrap(),
      nar.file[start..=end],
      "{range}"
    );
  }
}

#[tokio::test]
async fn unsatisfiable_ranges_are_416() {
  let (server, token) = setup().await;
  let nar = big_nar();
  server.push_path(&token, "main", &nar).await;
  let size = nar.file_size();

  for range in [
    format!("bytes={size}-"),
    format!("bytes={size}-{}", size + 10),
    format!("bytes={}-", size + 1000),
    "bytes=-0".to_string(),
    "bytes=99999999999999999999999-".to_string(),
  ] {
    let resp = get_nar(&server, "main", &nar, Some(&range)).await;
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE, "{range}");
    assert_eq!(
      header(&resp, "content-range"),
      Some(format!("bytes */{size}").as_str()),
      "{range}"
    );
    assert!(resp.bytes().await.unwrap().is_empty());
  }
}

#[tokio::test]
async fn invalid_or_multi_ranges_serve_full_file() {
  let (server, token) = setup().await;
  let nar = big_nar();
  server.push_path(&token, "main", &nar).await;

  for range in [
    "bytes=5-2",
    "bytes=abc",
    "bytes=",
    "bytes=-",
    "items=0-5",
    "0-5",
    "bytes=0-1,5-6",
    "bytes=1-b",
  ] {
    let resp = get_nar(&server, "main", &nar, Some(range)).await;
    assert_eq!(resp.status(), StatusCode::OK, "{range}");
    assert!(header(&resp, "content-range").is_none(), "{range}");
    assert_eq!(resp.bytes().await.unwrap(), nar.file, "{range}");
  }

  // Non UTF-8 header value
  let resp = send(nix(&server, Method::GET, "main", &nar.nar_url()).header(
    "Range",
    reqwest::header::HeaderValue::from_bytes(b"bytes=\xff-1").unwrap(),
  ))
  .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(resp.bytes().await.unwrap(), nar.file);
}

#[tokio::test]
async fn ranges_on_zero_byte_file() {
  let (server, token) = setup().await;
  let nar = TestNar::new("tiny-1.0", b"");
  let cache = server.push_path(&token, "main", &nar).await.cache.unwrap();
  // Make the stored file empty to hit the zero size edge case (zstd output is never empty)
  let db = server.db().await;
  let id = nar_id(&server, cache, &nar).await;
  db.execute_unprepared(&format!(
    "UPDATE nar SET size = 0 WHERE hash = '{}'",
    nar.file_hash
  ))
  .await
  .unwrap();
  std::fs::write(server.storage_path().join(format!("{id}.nar")), b"").unwrap();

  let resp = get_nar(&server, "main", &nar, None).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(header(&resp, "content-length"), Some("0"));
  assert!(resp.bytes().await.unwrap().is_empty());

  for range in ["bytes=0-", "bytes=0-0", "bytes=-1", "bytes=-0"] {
    let resp = get_nar(&server, "main", &nar, Some(range)).await;
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE, "{range}");
    assert_eq!(header(&resp, "content-range"), Some("bytes */0"), "{range}");
  }
}
