//! `GET`/`HEAD /api/nix/{cache}/{hash}.narinfo`
mod common;
mod nix_common;

use backend::db::DBTrait;
use common::{CacheOptions, TestNar, TestServer};
use nix_common::{get_status, header, nix, send};
use reqwest::{Method, StatusCode};
use sea_orm::{ConnectionTrait, EntityTrait};

/// Server with a public cache `main` and a CLI token of the admin
async fn setup() -> (TestServer, String) {
  let (server, _) = TestServer::start_with_admin().await;
  server.create_cache("main", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  (server, token)
}

fn expected_narinfo(nar: &TestNar, references: &[&str]) -> String {
  let mut references = references.to_vec();
  references.sort_unstable();
  format!(
    "StorePath: {}\nURL: {}\nCompression: zstd\nFileHash: sha256:{}\nFileSize: {}\nNarHash: sha256:{}\nNarSize: {}\nReferences: {}\n{}Sig: {}\n",
    nar.full_path(),
    nar.nar_url(),
    nar.file_hash,
    nar.file_size(),
    nar.nar_hash,
    nar.nar_size(),
    references.join(" "),
    nar
      .deriver
      .as_ref()
      .map(|d| format!("Deriver: {d}\n"))
      .unwrap_or_default(),
    nar.signature,
  )
}

async fn narinfo(server: &TestServer, cache: &str, hash: &str) -> reqwest::Response {
  send(nix(server, Method::GET, cache, &format!("{hash}.narinfo"))).await
}

#[tokio::test]
async fn narinfo_round_trips_pushed_path() {
  let (server, token) = setup().await;
  let dep_a = TestNar::new("dep-a-1.0", b"a");
  let dep_b = TestNar::new("dep-b-1.0", b"b");
  let drv = TestNar::new("hello-1.0.drv", b"drv");
  // references in non sorted order, including a self reference
  let mut refs = vec![dep_b.store_path.as_str(), dep_a.store_path.as_str()];
  let base = TestNar::new("hello-1.0", b"hello world");
  refs.push(&base.store_path);
  let nar = base.clone().references(&refs).deriver(&drv.store_path);
  server.push_path(&token, "main", &nar).await;

  let resp = narinfo(&server, "main", &nar.hash).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(header(&resp, "content-type"), Some("text/x-nix-narinfo"));
  let body = resp.text().await.unwrap();
  assert_eq!(body, expected_narinfo(&nar, &refs));
  // Sig is `<key name>:<base64>` of the test key
  assert!(body.contains("\nSig: testing:"));
}

#[tokio::test]
async fn narinfo_without_references_or_deriver() {
  let (server, token) = setup().await;
  let nar = TestNar::new("plain-1.0", b"plain");
  server.push_path(&token, "main", &nar).await;

  let body = narinfo(&server, "main", &nar.hash)
    .await
    .text()
    .await
    .unwrap();
  assert_eq!(body, expected_narinfo(&nar, &[]));
  assert!(body.contains("\nReferences: \n"));
  assert!(!body.contains("Deriver"));
}

#[tokio::test]
async fn narinfo_empty_file_contents() {
  let (server, token) = setup().await;
  let nar = TestNar::new("empty-1.0", b"");
  server.push_path(&token, "main", &nar).await;

  let body = narinfo(&server, "main", &nar.hash)
    .await
    .text()
    .await
    .unwrap();
  assert_eq!(body, expected_narinfo(&nar, &[]));
}

#[tokio::test]
async fn narinfo_head_found_and_missing() {
  let (server, token) = setup().await;
  let nar = TestNar::new("hello-1.0", b"hello");
  server.push_path(&token, "main", &nar).await;
  let path = format!("{}.narinfo", nar.hash);

  let resp = send(nix(&server, Method::HEAD, "main", &path)).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(header(&resp, "content-type"), Some("text/x-nix-narinfo"));
  assert!(resp.bytes().await.unwrap().is_empty());

  let missing = TestNar::new("missing-1.0", b"");
  for (cache, path) in [
    ("main", format!("{}.narinfo", missing.hash)),
    ("main", nar.hash.clone()),
    ("nope", path.clone()),
  ] {
    assert_eq!(
      get_status(nix(&server, Method::HEAD, cache, &path)).await,
      StatusCode::NOT_FOUND,
      "{cache}/{path}"
    );
  }
}

#[tokio::test]
async fn narinfo_missing_and_malformed_paths_are_not_found() {
  let (server, token) = setup().await;
  let nar = TestNar::new("hello-1.0", b"hello");
  server.push_path(&token, "main", &nar).await;
  let missing = TestNar::new("missing-1.0", b"");

  for path in [
    format!("{}.narinfo", missing.hash),
    // wrong or no suffix
    nar.hash.clone(),
    format!("{}.nar", nar.hash),
    format!("{}.NARINFO", nar.hash),
    format!("{}.narinfo.narinfo", nar.hash),
    format!("{}.narinfox", nar.hash),
    // hash must match exactly
    format!("{}.narinfo", nar.hash.to_uppercase()),
    format!("{}.narinfo", &nar.hash[..31]),
    format!("{}x.narinfo", nar.hash),
    format!("{}.narinfo", nar.store_path),
    ".narinfo".to_string(),
    "garbage!@$.narinfo".to_string(),
    "%00.narinfo".to_string(),
  ] {
    let resp = send(nix(&server, Method::GET, "main", &path)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
  }
}

#[tokio::test]
async fn narinfo_cache_name_is_case_insensitive() {
  let (server, token) = setup().await;
  let nar = TestNar::new("hello-1.0", b"hello");
  server.push_path(&token, "main", &nar).await;

  for cache in ["main", "MAIN", "Main"] {
    assert_eq!(
      narinfo(&server, cache, &nar.hash).await.status(),
      StatusCode::OK
    );
  }
  assert_eq!(
    narinfo(&server, "unknown", &nar.hash).await.status(),
    StatusCode::NOT_FOUND
  );
}

#[tokio::test]
async fn narinfo_of_other_cache_is_not_visible() {
  let (server, token) = setup().await;
  server.create_cache("other", &CacheOptions::default()).await;
  let in_main = TestNar::new("main-only-1.0", b"m");
  let in_other = TestNar::new("other-only-1.0", b"o");
  server.push_path(&token, "main", &in_main).await;
  server.push_path(&token, "other", &in_other).await;

  assert_eq!(
    narinfo(&server, "main", &in_main.hash).await.status(),
    StatusCode::OK
  );
  assert_eq!(
    narinfo(&server, "other", &in_other.hash).await.status(),
    StatusCode::OK
  );
  assert_eq!(
    narinfo(&server, "main", &in_other.hash).await.status(),
    StatusCode::NOT_FOUND
  );
  assert_eq!(
    narinfo(&server, "other", &in_main.hash).await.status(),
    StatusCode::NOT_FOUND
  );
}

#[tokio::test]
async fn narinfo_same_path_in_two_caches_is_independent() {
  let (server, token) = setup().await;
  server.create_cache("other", &CacheOptions::default()).await;
  let nar = TestNar::new("shared-1.0", b"shared");
  server.push_path(&token, "main", &nar).await;
  // same NAR, deduped on the server
  assert!(server.push_path(&token, "other", &nar).await.nar_found);

  for cache in ["main", "other"] {
    let body = narinfo(&server, cache, &nar.hash)
      .await
      .text()
      .await
      .unwrap();
    assert_eq!(body, expected_narinfo(&nar, &[]), "{cache}");
  }
}

#[tokio::test]
async fn narinfo_gone_after_cache_cleared() {
  let (server, token) = setup().await;
  let nar = TestNar::new("hello-1.0", b"hello");
  let cache = server.push_path(&token, "main", &nar).await.cache.unwrap();
  assert_eq!(
    narinfo(&server, "main", &nar.hash).await.status(),
    StatusCode::OK
  );

  let resp = server
    .delete(&format!("/cache/management/{cache}"), serde_json::json!({}))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(
    narinfo(&server, "main", &nar.hash).await.status(),
    StatusCode::NOT_FOUND
  );
}

#[tokio::test]
async fn narinfo_with_unsupported_compression_is_not_found() {
  let (server, token) = setup().await;
  let nar = TestNar::new("hello-1.0", b"hello");
  server.push_path(&token, "main", &nar).await;

  let db = server.db().await;
  db.execute_unprepared("UPDATE nar_info SET compression = 'xz'")
    .await
    .unwrap();
  assert_eq!(
    narinfo(&server, "main", &nar.hash).await.status(),
    StatusCode::NOT_FOUND
  );
  // HEAD only checks presence
  assert_eq!(
    get_status(nix(
      &server,
      Method::HEAD,
      "main",
      &format!("{}.narinfo", nar.hash)
    ))
    .await,
    StatusCode::OK
  );
}

#[tokio::test]
async fn narinfo_does_not_count_as_access() {
  let (server, token) = setup().await;
  let nar = TestNar::new("hello-1.0", b"hello");
  let cache = server.push_path(&token, "main", &nar).await.cache.unwrap();
  narinfo(&server, "main", &nar.hash).await;
  send(nix(
    &server,
    Method::HEAD,
    "main",
    &format!("{}.narinfo", nar.hash),
  ))
  .await;

  let db = server.db().await;
  let data = db
    .nar()
    .nar_info_data(cache, &nar.hash)
    .await
    .unwrap()
    .unwrap();
  let row = entity::nar_info::Entity::find_by_id(data.id)
    .one(&*db)
    .await
    .unwrap()
    .unwrap();
  assert_eq!(row.accessed, 0);
}

#[tokio::test]
async fn duplicate_store_path_rows_are_rejected_by_the_db() {
  let (server, token) = setup().await;
  let nar = TestNar::new("hello-1.0", b"hello");
  server.push_path(&token, "main", &nar).await;

  // Unique (cache_id, store_path_hash): a racing second insert can never create an ambiguous narinfo
  let db = server.db().await;
  let err = db
    .execute_unprepared(
      "INSERT INTO nar_info (id, cache_id, nar_id, store_path, store_path_hash, compression, signature, created_at, last_accessed_at, accessed) \
       SELECT 'dup', cache_id, nar_id, store_path, store_path_hash, compression, signature, created_at, last_accessed_at, accessed FROM nar_info",
    )
    .await
    .unwrap_err();
  assert!(
    matches!(
      err.sql_err(),
      Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
    ),
    "{err:?}"
  );
}

#[tokio::test]
async fn concurrent_pushes_of_same_path_never_duplicate() {
  let (server, token) = setup().await;
  let nar = TestNar::new("racy-1.0", b"race");

  // Start both uploads before either finishes, so both pass the "already in cache" check
  let cache = {
    let resp = server
      .raw(Method::POST, "/cache/push/info")
      .bearer_auth(&token)
      .json(&serde_json::json!({ "cache": "main", "paths": [nar.store_path], "force": false }))
      .send()
      .await
      .unwrap();
    let info: serde_json::Value = resp.json().await.unwrap();
    uuid::Uuid::parse_str(info["cache"].as_str().unwrap()).unwrap()
  };
  let mut uploads = Vec::new();
  for _ in 0..2 {
    let resp = server
      .raw(Method::POST, "/cache/push")
      .bearer_auth(&token)
      .json(&nar.upload_request(cache, false))
      .send()
      .await
      .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    let id = body["uuid"].as_str().unwrap().to_string();
    if !body["nar_found"].as_bool().unwrap() {
      for (part, chunk) in nar
        .chunks(shared::api::push::CHUNK_SIZE)
        .into_iter()
        .enumerate()
      {
        let resp = server
          .raw(Method::POST, &format!("/cache/push/{id}/{part}"))
          .bearer_auth(&token)
          .body(chunk)
          .send()
          .await
          .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
      }
    }
    uploads.push(id);
  }

  let mut statuses = Vec::new();
  for id in uploads {
    let resp = server
      .raw(Method::PUT, &format!("/cache/push/{id}"))
      .bearer_auth(&token)
      .json(&nar.finish_request())
      .send()
      .await
      .unwrap();
    statuses.push(resp.status());
  }
  assert_eq!(statuses, [StatusCode::OK, StatusCode::BAD_REQUEST]);

  let db = server.db().await;
  let rows = entity::nar_info::Entity::find().all(&*db).await.unwrap();
  assert_eq!(rows.len(), 1);
  assert_eq!(
    narinfo(&server, "main", &nar.hash)
      .await
      .text()
      .await
      .unwrap(),
    expected_narinfo(&nar, &[])
  );
}
