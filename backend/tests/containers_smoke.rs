//! Smoke tests of the container harness: boot on Postgres, S3 and both, then push and serve a path.
mod common;

use backend::db::DBTrait;
use centaurus::storage::StoragePath;
use common::{ADMIN_EMAIL, ADMIN_PASSWORD, CacheOptions, JWT_COOKIE_NAME, TestNar, TestServer};
use reqwest::{Method, StatusCode};

/// Push `nar` to a fresh `main` cache and check it is served byte-exact.
async fn push_and_download(server: &TestServer) -> TestNar {
  server.create_cache("main", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  let nar = TestNar::new("hello-1.0", b"hello world");
  let pushed = server.push_path(&token, "main", &nar).await;
  assert!(!pushed.skipped && !pushed.nar_found);

  let resp = server
    .raw(Method::GET, &format!("/nix/main/{}", nar.nar_url()))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(resp.bytes().await.unwrap(), nar.file);
  nar
}

#[tokio::test]
async fn pg_setup_login_push() {
  let (server, _) = TestServer::with_admin(TestServer::start_pg().await).await;
  assert!(server.is_pg() && !server.is_s3());
  server.clear_cookies();
  let resp = server.login(ADMIN_EMAIL, ADMIN_PASSWORD).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert!(server.has_cookie(JWT_COOKIE_NAME));

  push_and_download(&server).await;
  // db() connects to the same postgres database
  let (nar_ids, _) = server.db().await.nar().known_ids().await.unwrap();
  assert_eq!(nar_ids.len(), 1);
  assert_eq!(server.storage_files().await.len(), 1);
}

#[tokio::test]
async fn s3_push_download() {
  let (server, _) = TestServer::with_admin(TestServer::start_s3().await).await;
  assert!(server.is_s3() && !server.is_pg());
  push_and_download(&server).await;

  let files = server.storage_files().await;
  assert_eq!(files.len(), 1, "{files:?}");
  assert!(files[0].ends_with(".nar"));
  assert!(server.s3_multipart_uploads().await.is_empty());

  // the multipart helper sees an in-progress upload until it is aborted
  let storage = server.file_storage().await;
  let multipart = storage.multipart().expect("s3 multipart");
  let key = StoragePath::from("probe.nar");
  let id = multipart.create_multipart(&key).await.unwrap();
  assert_eq!(
    server.s3_multipart_uploads().await,
    vec![("probe.nar".to_string(), id.clone())]
  );
  multipart.abort_multipart(&key, &id).await.unwrap();
  assert!(server.s3_multipart_uploads().await.is_empty());
}

#[tokio::test]
async fn pg_s3_push_download() {
  let (server, _) = TestServer::with_admin(TestServer::start_pg_s3().await).await;
  assert!(server.is_pg() && server.is_s3());
  push_and_download(&server).await;
  assert_eq!(server.storage_files().await.len(), 1);
}
