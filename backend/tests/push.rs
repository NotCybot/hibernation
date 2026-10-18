//! Push API: `/cache/push/info`, `/cache/push`, `/cache/push/{id}/{part}`, `/cache/push/{id}` and `/cache/test`.
mod common;
mod push_common;

use std::time::Duration;

use backend::db::DBTrait;
use common::{CacheOptions, TestNar, TestServer, downstream_has, mock_downstream, random_key};
use entity::{cache, nar_info_reference, sea_orm_active_enums::AccessType};
use push_common::*;
use reqwest::{Method, StatusCode};
use sea_orm::{ActiveValue::Set, IntoActiveModel, prelude::*};
use serde_json::{Value, json};
use shared::api::push::CHUNK_SIZE;
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::TcpStream,
};
use uuid::Uuid;
use wiremock::{Mock, ResponseTemplate, matchers};

async fn setup(opts: CacheOptions) -> (TestServer, Uuid, String) {
  let (server, _) = TestServer::start_with_admin().await;
  let cache = server.create_cache("main", &opts).await;
  let (token, _) = server.create_cli_token("ci").await;
  (server, cache, token)
}

async fn default_setup() -> (TestServer, Uuid, String) {
  setup(CacheOptions::default()).await
}

fn force_opts() -> CacheOptions {
  CacheOptions {
    allow_force_push: true,
    ..Default::default()
  }
}

/// Upload every chunk of `nar` for `upload` and assert success.
async fn send_chunks(server: &TestServer, token: &str, upload: Uuid, nar: &TestNar) {
  for (part, data) in nar.chunks(CHUNK_SIZE).into_iter().enumerate() {
    assert_eq!(
      chunk(server, token, upload, part, data).await.status(),
      StatusCode::OK
    );
  }
}

// ---- /cache/test ----

#[tokio::test]
async fn test_endpoint_returns_cli_user_id() {
  let (server, admin) = TestServer::start_with_admin().await;
  let (token, _) = server.create_cli_token("ci").await;
  let resp = server
    .raw(Method::GET, "/cache/test")
    .bearer_auth(&token)
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(resp.text().await.unwrap(), admin.to_string());

  let resp = server.raw(Method::GET, "/cache/test").send().await.unwrap();
  assert!(!resp.status().is_success());
}

// ---- /cache/push/info ----

#[tokio::test]
async fn info_invalid_store_path_is_unprocessable() {
  let (server, _, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let resp = info(
    &server,
    &token,
    "main",
    &[&a.store_path, "not-a-path"],
    false,
  )
  .await;
  assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn info_unknown_cache_is_not_found() {
  let (server, _, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let resp = info(&server, &token, "other", &[&a.store_path], false).await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn info_cache_name_is_case_insensitive() {
  // Regression: the nix read path resolved names case-insensitively, the push path did not
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let resp = info(&server, &token, "MaIn", &[&a.store_path], false).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["cache"], json!(cache));
}

#[tokio::test]
async fn info_requires_edit_access() {
  let (server, cache, _) = default_setup().await;
  let db = server.db().await;
  let a = TestNar::new("a-1.0", b"a");

  // No access to a public cache: reading is fine, pushing is not
  let nobody = create_user(&server).await;
  let token = user_token(&server, &nobody).await;
  let resp = info(&server, &token, "main", &[&a.store_path], false).await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);

  let viewer = create_user(&server).await;
  grant_user(&db, cache, viewer.id, AccessType::View).await;
  let token = user_token(&server, &viewer).await;
  let resp = info(&server, &token, "main", &[&a.store_path], false).await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);

  let editor = create_user(&server).await;
  grant_user(&db, cache, editor.id, AccessType::Edit).await;
  let token = user_token(&server, &editor).await;
  let resp = info(&server, &token, "main", &[&a.store_path], false).await;
  assert_eq!(resp.status(), StatusCode::OK);

  let group_editor = create_user(&server).await;
  grant_group(&server, &db, cache, group_editor.id, AccessType::Edit).await;
  let token = user_token(&server, &group_editor).await;
  let resp = info(&server, &token, "main", &[&a.store_path], false).await;
  assert_eq!(resp.status(), StatusCode::OK);

  let group_viewer = create_user(&server).await;
  grant_group(&server, &db, cache, group_viewer.id, AccessType::View).await;
  let token = user_token(&server, &group_viewer).await;
  let resp = info(&server, &token, "main", &[&a.store_path], false).await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn info_force_without_permission_is_not_acceptable() {
  let (server, _, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let resp = info(&server, &token, "main", &[&a.store_path], true).await;
  assert_eq!(resp.status(), StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn info_returns_only_missing_paths() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let b = TestNar::new("b-1.0", b"b");
  server.push_path(&token, "main", &a).await;

  let resp = info(
    &server,
    &token,
    "main",
    &[&a.store_path, &b.store_path],
    false,
  )
  .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body, json!({ "paths": [b.store_path], "cache": cache }));

  let resp = info(&server, &token, "main", &[&a.store_path], false).await;
  assert_eq!(resp.status(), StatusCode::NO_CONTENT);
  let resp = info(&server, &token, "main", &[], false).await;
  assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn info_drops_paths_present_downstream() {
  let first = mock_downstream().await;
  let second = mock_downstream().await;
  let (server, _, token) = setup(CacheOptions {
    downstream_caches: vec![first.uri(), second.uri()],
    ..Default::default()
  })
  .await;
  let a = TestNar::new("a-1.0", b"a");
  let b = TestNar::new("b-1.0", b"b");
  let c = TestNar::new("c-1.0", b"c");
  downstream_has(&first, &a).await;
  downstream_has(&second, &b).await;

  let resp = info(
    &server,
    &token,
    "main",
    &[&a.store_path, &b.store_path, &c.store_path],
    false,
  )
  .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["paths"], json!([c.store_path]));

  let resp = info(
    &server,
    &token,
    "main",
    &[&a.store_path, &b.store_path],
    false,
  )
  .await;
  assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn info_downstream_errors_count_as_missing() {
  let failing = mock_downstream().await;
  Mock::given(matchers::method("HEAD"))
    .respond_with(ResponseTemplate::new(500))
    .mount(&failing)
    .await;
  let redirect = mock_downstream().await;
  Mock::given(matchers::method("HEAD"))
    .respond_with(ResponseTemplate::new(204))
    .mount(&redirect)
    .await;
  // Nothing listens there
  let unreachable = {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", l.local_addr().unwrap())
  };
  let (server, _, token) = setup(CacheOptions {
    downstream_caches: vec![failing.uri(), redirect.uri(), unreachable],
    ..Default::default()
  })
  .await;
  let a = TestNar::new("a-1.0", b"a");

  let resp = info(&server, &token, "main", &[&a.store_path], false).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["paths"], json!([a.store_path]));
}

#[tokio::test]
async fn info_downstream_with_path_prefix() {
  // Regression: `Url::join` dropped the last path segment of downstream caches like `https://host/api/nix/main`
  let downstream = mock_downstream().await;
  let a = TestNar::new("a-1.0", b"a");
  Mock::given(matchers::method("HEAD"))
    .and(matchers::path(format!("/api/nix/up/{}.narinfo", a.hash)))
    .respond_with(ResponseTemplate::new(200))
    .mount(&downstream)
    .await;
  let (server, cache, token) = setup(CacheOptions {
    downstream_caches: vec![format!("{}/api/nix/up", downstream.uri())],
    ..Default::default()
  })
  .await;

  let resp = info(&server, &token, "main", &[&a.store_path], false).await;
  assert_eq!(resp.status(), StatusCode::NO_CONTENT);
  let resp = upload(&server, &token, &a.upload_request(cache, false)).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn info_force_skips_downstream_check() {
  let downstream = mock_downstream().await;
  let (server, _, token) = setup(CacheOptions {
    downstream_caches: vec![downstream.uri()],
    ..force_opts()
  })
  .await;
  let a = TestNar::new("a-1.0", b"a");
  let b = TestNar::new("b-1.0", b"b");
  downstream_has(&downstream, &a).await;
  server.push_path_opts(&token, "main", &b, true).await;

  let resp = info(
    &server,
    &token,
    "main",
    &[&a.store_path, &b.store_path],
    true,
  )
  .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["paths"], json!([a.store_path]));

  // Force does not re-push what the cache itself already has
  let resp = info(&server, &token, "main", &[&b.store_path], true).await;
  assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

// ---- POST /cache/push ----

#[tokio::test]
async fn upload_invalid_paths_are_unprocessable() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");

  let mut body = a.upload_request(cache, false);
  body["store_path"] = json!("garbage");
  let resp = upload(&server, &token, &body).await;
  assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

  let mut body = a.upload_request(cache, false);
  body["references"] = json!([a.store_path, "garbage"]);
  let resp = upload(&server, &token, &body).await;
  assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn upload_requires_edit_access() {
  let (server, cache, _) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let viewer = create_user(&server).await;
  grant_user(&*server.db().await, cache, viewer.id, AccessType::View).await;
  let token = user_token(&server, &viewer).await;

  let resp = upload(&server, &token, &a.upload_request(cache, false)).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  let resp = upload(&server, &token, &a.upload_request(Uuid::new_v4(), false)).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn upload_force_requires_permission() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let resp = upload(&server, &token, &a.upload_request(cache, true)).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn upload_with_broken_stored_key_is_server_error() {
  let (server, cache, token) = default_setup().await;
  let db = server.db().await;
  let mut model = cache::Entity::find_by_id(cache)
    .one(&*db)
    .await
    .unwrap()
    .unwrap()
    .into_active_model();
  model.public_signing_key = Set("broken".into());
  model.update(&*db).await.unwrap();

  let a = TestNar::new("a-1.0", b"a");
  let resp = upload(&server, &token, &a.upload_request(cache, false)).await;
  assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn upload_rejects_invalid_signatures() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let other = TestNar::new("other-1.0", b"other");

  let wrong_key = a.clone().signed_with(random_key("evil"));
  let resp = upload(&server, &token, &wrong_key.upload_request(cache, false)).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "wrong key");

  for (field, value) in [
    ("nar_hash", json!(other.nar_hash)),
    ("nar_size", json!(a.nar_size() + 8)),
    ("references", json!([other.store_path])),
    ("store_path", json!(other.store_path)),
    ("signature", json!("garbage")),
  ] {
    let mut body = a.upload_request(cache, false);
    body[field] = value;
    let resp = upload(&server, &token, &body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "tampered {field}");
  }
  assert!(uploads(&*server.db().await).await.is_empty());
}

#[tokio::test]
async fn upload_already_present_is_bad_request() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  server.push_path(&token, "main", &a).await;
  let resp = upload(&server, &token, &a.upload_request(cache, false)).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn upload_present_downstream_unless_forced() {
  let downstream = mock_downstream().await;
  let (server, cache, token) = setup(CacheOptions {
    downstream_caches: vec![downstream.uri()],
    ..force_opts()
  })
  .await;
  let a = TestNar::new("a-1.0", b"a");
  downstream_has(&downstream, &a).await;

  let resp = upload(&server, &token, &a.upload_request(cache, false)).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  let resp = upload(&server, &token, &a.upload_request(cache, true)).await;
  assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn upload_creates_upload_row() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let (_, nar_found) = start_upload(&server, &token, cache, &a).await;
  assert!(!nar_found);
  let rows = uploads(&*server.db().await).await;
  assert_eq!(rows.len(), 1);
  assert_eq!(rows[0].multipart_id, None);
}

#[tokio::test]
async fn upload_reuses_nar_across_caches() {
  let (server, _, token) = default_setup().await;
  let other = server.create_cache("other", &CacheOptions::default()).await;
  let a = TestNar::new("a-1.0", b"shared contents");
  server.push_path(&token, "main", &a).await;

  let (upload_id, nar_found) = start_upload(&server, &token, other, &a).await;
  assert!(nar_found);
  let db = server.db().await;
  assert!(
    uploads(&db).await.is_empty(),
    "no upload row for a found nar"
  );

  // A found nar needs no chunks
  let resp = chunk(&server, &token, upload_id, 0, a.file.clone()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

  let resp = finish(&server, &token, upload_id, &a.finish_request()).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(
    nar_bytes(&server, "other", &a).await,
    (StatusCode::OK, a.file.clone())
  );
  assert_eq!(nars(&db).await.len(), 1);
  assert_eq!(nar_files(&server).len(), 1);
}

// ---- POST /cache/push/{id}/{part} ----

#[tokio::test]
async fn chunk_part_limit() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let (upload_id, _) = start_upload(&server, &token, cache, &a).await;

  let resp = chunk(&server, &token, upload_id, 10000, vec![1]).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  let resp = chunk(&server, &token, upload_id, 9999, vec![1]).await;
  assert_eq!(resp.status(), StatusCode::OK);

  // Parts 0..9999 are missing
  let resp = finish(&server, &token, upload_id, &a.finish_request()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  assert!(
    storage_files(&server).is_empty(),
    "{:?}",
    storage_files(&server)
  );
  assert!(uploads(&*server.db().await).await.is_empty());
}

#[tokio::test]
async fn chunk_size_limit() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let (upload_id, _) = start_upload(&server, &token, cache, &a).await;

  let resp = chunk(&server, &token, upload_id, 0, vec![0; CHUNK_SIZE + 1]).await;
  assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
  let resp = chunk(&server, &token, upload_id, 0, vec![0; CHUNK_SIZE]).await;
  assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn chunk_unknown_session() {
  let (server, _, token) = default_setup().await;
  let resp = chunk(&server, &token, Uuid::new_v4(), 0, vec![1]).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn chunk_requires_cli_auth() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let (upload_id, _) = start_upload(&server, &token, cache, &a).await;
  let resp = server
    .raw(Method::POST, &format!("/cache/push/{upload_id}/0"))
    .body(a.file.clone())
    .send()
    .await
    .unwrap();
  assert!(!resp.status().is_success());
}

#[tokio::test]
async fn chunk_quota_counts_other_parts() {
  let a = TestNar::new(
    "a-1.0",
    &(0..20_000u32)
      .flat_map(|i| i.to_le_bytes())
      .collect::<Vec<_>>(),
  );
  let half = a.file.len() / 2 + 1;
  let (server, cache, token) = setup(CacheOptions {
    quota: a.file_size() as i64 - 1,
    ..Default::default()
  })
  .await;
  let (upload_id, _) = start_upload(&server, &token, cache, &a).await;
  let parts = a.chunks(half);
  assert_eq!(parts.len(), 2);

  let resp = chunk(&server, &token, upload_id, 0, parts[0].clone()).await;
  assert_eq!(resp.status(), StatusCode::OK);
  // Re-uploading a part replaces its size instead of adding to it
  let resp = chunk(&server, &token, upload_id, 0, parts[0].clone()).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let resp = chunk(&server, &token, upload_id, 1, parts[1].clone()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn chunk_reupload_overwrites_and_out_of_order_assembles() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new(
    "a-1.0",
    &(0..20_000u32)
      .flat_map(|i| i.to_le_bytes())
      .collect::<Vec<_>>(),
  );
  let (upload_id, _) = start_upload(&server, &token, cache, &a).await;
  let parts = a.chunks(a.file.len() / 3 + 1);
  assert_eq!(parts.len(), 3);

  // A garbage first attempt of the last part, then all parts in reverse order
  let resp = chunk(&server, &token, upload_id, 2, vec![0xff; 17]).await;
  assert_eq!(resp.status(), StatusCode::OK);
  for (i, part) in parts.iter().enumerate().rev() {
    let resp = chunk(&server, &token, upload_id, i, part.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
  }
  let resp = finish(&server, &token, upload_id, &a.finish_request()).await;
  assert_eq!(resp.status(), StatusCode::OK);

  assert_eq!(
    nar_bytes(&server, "main", &a).await,
    (StatusCode::OK, a.file.clone())
  );
  assert_eq!(nar_files(&server).len(), 1);
  assert_eq!(storage_files(&server).len(), 1, "parts removed");
  assert!(uploads(&*server.db().await).await.is_empty());
}

#[tokio::test]
async fn chunk_concurrency_is_limited() {
  let (server, _) = {
    let server = TestServer::start_with(|| unsafe {
      std::env::set_var("MAX_CONCURRENT_UPLOADS", "1");
    })
    .await;
    let id = server
      .setup_admin(
        common::ADMIN_NAME,
        common::ADMIN_EMAIL,
        common::ADMIN_PASSWORD,
      )
      .await;
    (server, id)
  };
  let cache = server.create_cache("main", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  let a = TestNar::new("a-1.0", b"a");
  let (upload_id, _) = start_upload(&server, &token, cache, &a).await;

  // Hold the only permit with a chunk request whose body never completes
  let mut held = TcpStream::connect(("127.0.0.1", server.port))
    .await
    .unwrap();
  held
    .write_all(
      format!(
        "POST /api/cache/push/{upload_id}/1 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Length: 4\r\n\r\nab"
      )
      .as_bytes(),
    )
    .await
    .unwrap();
  tokio::time::sleep(Duration::from_millis(300)).await;

  let blocked = server
    .raw(Method::POST, &format!("/cache/push/{upload_id}/0"))
    .bearer_auth(&token)
    .body(a.file.clone())
    .timeout(Duration::from_millis(500))
    .send()
    .await;
  assert!(
    blocked.unwrap_err().is_timeout(),
    "second chunk must wait for the permit"
  );

  held.write_all(b"cd").await.unwrap();
  let mut response = [0u8; 12];
  held.read_exact(&mut response).await.unwrap();
  assert_eq!(&response, b"HTTP/1.1 200");

  let resp = chunk(&server, &token, upload_id, 0, a.file.clone()).await;
  assert_eq!(resp.status(), StatusCode::OK);
}

// ---- PUT /cache/push/{id} ----

#[tokio::test]
async fn finish_unknown_session() {
  let (server, _, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let resp = finish(&server, &token, Uuid::new_v4(), &a.finish_request()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn finish_without_parts_cleans_up() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let (upload_id, _) = start_upload(&server, &token, cache, &a).await;
  let resp = finish(&server, &token, upload_id, &a.finish_request()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  assert!(uploads(&*server.db().await).await.is_empty());
  // The session is consumed
  let resp = finish(&server, &token, upload_id, &a.finish_request()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn finish_rejects_bad_content_and_cleans_up() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let b = TestNar::new("b-1.0", b"b");
  let cases: [(Vec<u8>, Value); 4] = [
    // Wrong declared file hash / size
    (
      a.file.clone(),
      json!({ "file_hash": b.file_hash, "file_size": a.file_size() }),
    ),
    (
      a.file.clone(),
      json!({ "file_hash": a.file_hash, "file_size": a.file_size() + 1 }),
    ),
    // Not zstd
    (a.nar.clone(), a.finish_request()),
    // Valid zstd, but another NAR than signed
    (
      b.file.clone(),
      json!({ "file_hash": b.file_hash, "file_size": b.file_size() }),
    ),
  ];
  for (data, body) in cases {
    let (upload_id, _) = start_upload(&server, &token, cache, &a).await;
    assert_eq!(
      chunk(&server, &token, upload_id, 0, data).await.status(),
      StatusCode::OK
    );
    let resp = finish(&server, &token, upload_id, &body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body}");
    assert!(
      storage_files(&server).is_empty(),
      "{:?}",
      storage_files(&server)
    );
    assert!(uploads(&*server.db().await).await.is_empty());
  }
  assert_eq!(
    narinfo_status(&server, "main", &a).await,
    StatusCode::NOT_FOUND
  );
  assert!(nars(&*server.db().await).await.is_empty());
}

#[tokio::test]
async fn finish_after_concurrent_upload_of_same_nar() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"same");
  let b = TestNar::new("b-1.0", b"same");
  let (first, found_a) = start_upload(&server, &token, cache, &a).await;
  let (second, found_b) = start_upload(&server, &token, cache, &b).await;
  assert!(!found_a && !found_b);
  send_chunks(&server, &token, first, &a).await;
  send_chunks(&server, &token, second, &b).await;

  assert_eq!(
    finish(&server, &token, first, &a.finish_request())
      .await
      .status(),
    StatusCode::OK
  );
  assert_eq!(
    finish(&server, &token, second, &b.finish_request())
      .await
      .status(),
    StatusCode::OK
  );

  let db = server.db().await;
  assert_eq!(nars(&db).await.len(), 1);
  assert_eq!(
    storage_files(&server).len(),
    1,
    "the second copy is deleted"
  );
  assert!(uploads(&db).await.is_empty());
  let infos = nar_infos(&db, cache).await;
  assert_eq!(infos.len(), 2);
  assert_eq!(infos[0].nar_id, infos[1].nar_id);
  assert_eq!(
    nar_bytes(&server, "main", &b).await,
    (StatusCode::OK, b.file.clone())
  );
}

#[tokio::test]
async fn finish_found_nar_deleted_in_between() {
  let (server, _, token) = default_setup().await;
  let other = server.create_cache("other", &CacheOptions::default()).await;
  let a = TestNar::new("a-1.0", b"shared contents");
  server.push_path(&token, "main", &a).await;
  let (upload_id, nar_found) = start_upload(&server, &token, other, &a).await;
  assert!(nar_found);

  // The only reference goes away and the orphan cleanup removes the nar
  let db = server.db().await;
  db.nar()
    .delete_path(server_cache(&server, "main").await, &a.store_path)
    .await
    .unwrap();
  age_nars(&db).await;
  backend::cache::cleanup::run_cleanup(&db, &server.file_storage().await).await;
  assert!(nars(&db).await.is_empty());

  let resp = finish(&server, &token, upload_id, &a.finish_request()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  assert_eq!(
    narinfo_status(&server, "other", &a).await,
    StatusCode::NOT_FOUND
  );
}

async fn server_cache(server: &TestServer, name: &str) -> Uuid {
  server
    .db()
    .await
    .cache()
    .by_name(name.into())
    .await
    .unwrap()
    .unwrap()
    .id
}

#[tokio::test]
async fn finish_over_quota_lowered_during_upload() {
  let (server, cache, token) = default_setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let (upload_id, _) = start_upload(&server, &token, cache, &a).await;
  send_chunks(&server, &token, upload_id, &a).await;

  let lowered = CacheOptions {
    quota: a.file_size() as i64 - 1,
    ..Default::default()
  };
  let resp = server
    .post(
      &format!("/cache/management/{cache}"),
      lowered.edit_body("main"),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);

  let resp = finish(&server, &token, upload_id, &a.finish_request()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  assert_eq!(
    narinfo_status(&server, "main", &a).await,
    StatusCode::NOT_FOUND
  );

  // The file left behind is an orphan the cleanup removes
  let db = server.db().await;
  age_nars(&db).await;
  backend::cache::cleanup::run_cleanup(&db, &server.file_storage().await).await;
  assert!(nars(&db).await.is_empty());
  assert!(storage_files(&server).is_empty());
}

#[tokio::test]
async fn finish_stores_references_and_deriver() {
  let (server, cache, token) = default_setup().await;
  let dep = TestNar::new("dep-1.0", b"dep");
  let drv = TestNar::new("a-1.0.drv", b"drv");
  let a = TestNar::new("a-1.0", b"a")
    .references(&[&dep.store_path, "00000000000000000000000000000000-self"])
    .deriver(&drv.store_path);
  server.push_path(&token, "main", &a).await;

  let db = server.db().await;
  let info = nar_info_of(&db, cache, &a).await;
  assert_eq!(info.deriver.as_deref(), Some(drv.store_path.as_str()));
  assert_eq!(info.signature, a.signature);
  assert_eq!(info.store_path_hash, a.hash);
  assert_eq!(info.compression, "zst");
  let mut refs = nar_info_reference::Entity::find()
    .filter(nar_info_reference::Column::NarInfoId.eq(info.id))
    .all(&*db)
    .await
    .unwrap()
    .into_iter()
    .map(|r| r.store_path)
    .collect::<Vec<_>>();
  refs.sort();
  let mut expected = a.references.clone();
  expected.sort();
  assert_eq!(refs, expected);

  let nar = &nars(&db).await[0];
  assert_eq!(nar.hash, a.file_hash);
  assert_eq!(nar.size, a.file_size() as i64);
  assert_eq!(nar.nar_hash, a.nar_hash);
  assert_eq!(nar.nar_size, a.nar_size() as i64);
}

#[tokio::test]
async fn create_path_unknown_nar_inserts_nothing() {
  let (server, cache, _) = default_setup().await;
  let db = server.db().await;
  let a = TestNar::new("a-1.0", b"a");
  let res = db
    .nar()
    .create_path(
      Uuid::new_v4(),
      cache,
      a.store_path.clone(),
      a.hash.clone(),
      a.nar_hash.clone(),
      a.nar_size(),
      a.file_hash.clone(),
      a.file_size(),
      None,
      a.signature.clone(),
      vec![a.store_path.clone()],
    )
    .await;
  assert_eq!(res.unwrap_err().status, StatusCode::INTERNAL_SERVER_ERROR);
  assert!(nar_infos(&db, cache).await.is_empty());
}

#[tokio::test]
async fn info_slow_downstream_times_out_as_missing() {
  // Regression: downstream checks had no timeout, a hanging downstream stalled the push forever
  let slow = mock_downstream().await;
  Mock::given(matchers::method("HEAD"))
    .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(15)))
    .mount(&slow)
    .await;
  let (server, cache, token) = setup(CacheOptions {
    downstream_caches: vec![slow.uri()],
    ..Default::default()
  })
  .await;
  let a = TestNar::new("a-1.0", b"a");

  let start = std::time::Instant::now();
  let resp = info(&server, &token, "main", &[&a.store_path], false).await;
  assert_eq!(
    resp.status(),
    StatusCode::OK,
    "timed out check counts as missing"
  );
  let resp = upload(&server, &token, &a.upload_request(cache, false)).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert!(start.elapsed() < Duration::from_secs(25));
}
