//! Background jobs called directly: stale upload cleanup, orphan nar cleanup and the storage sweeps.
mod common;
mod push_common;

use backend::{
  cache::cleanup::{run_cleanup, run_sweep, sweep_staging_files},
  db::DBTrait,
};
use common::{CacheOptions, TestNar, TestServer};
use entity::nar;
use push_common::*;
use reqwest::StatusCode;
use sea_orm::{ActiveValue::Set, prelude::*};
use uuid::Uuid;

async fn setup() -> (TestServer, Uuid, String) {
  let (server, _) = TestServer::start_with_admin().await;
  let cache = server.create_cache("main", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  (server, cache, token)
}

async fn cleanup(server: &TestServer) {
  run_cleanup(&server.db().await, &server.file_storage().await).await;
}

#[tokio::test]
async fn stale_uploads_are_removed() {
  let (server, cache, token) = setup().await;
  let db = server.db().await;
  let a = TestNar::new("a-1.0", b"a");
  let (stale, _) = start_upload(&server, &token, cache, &a).await;
  assert_eq!(
    chunk(&server, &token, stale, 0, a.file.clone())
      .await
      .status(),
    StatusCode::OK
  );
  age_uploads(&db).await;

  // A fresh upload is left alone
  let b = TestNar::new("b-1.0", b"b");
  let (fresh, _) = start_upload(&server, &token, cache, &b).await;
  assert_eq!(
    chunk(&server, &token, fresh, 0, b.file.clone())
      .await
      .status(),
    StatusCode::OK
  );

  cleanup(&server).await;
  let rows = uploads(&db).await;
  assert_eq!(rows.len(), 1);
  assert_eq!(
    storage_files(&server).len(),
    1,
    "only the fresh part is left"
  );

  // Each chunk keeps its upload alive
  assert_eq!(
    chunk(&server, &token, fresh, 0, b.file.clone())
      .await
      .status(),
    StatusCode::OK
  );
  assert!(uploads(&db).await[0].updated_at >= rows[0].updated_at);
}

#[tokio::test]
async fn stale_upload_with_nar_row_keeps_file() {
  // A crash between creating the nar entry and deleting the upload entry: the file belongs to the nar
  let (server, _, _) = setup().await;
  let db = server.db().await;
  let id = Uuid::now_v7();
  nar::ActiveModel {
    id: Set(id),
    hash: Set("h".into()),
    size: Set(1),
    nar_hash: Set("n".into()),
    nar_size: Set(1),
    created_at: Set(ago(0)),
  }
  .insert(&*db)
  .await
  .unwrap();
  db.nar().create_upload(id, None).await.unwrap();
  write_file(&server, &format!("{id}.nar"), b"x", 0);
  write_file(&server, &format!("parts/{id}/0"), b"x", 0);
  age_uploads(&db).await;

  cleanup(&server).await;
  assert!(uploads(&db).await.is_empty());
  assert_eq!(storage_files(&server), vec![format!("{id}.nar")]);
  assert_eq!(nars(&db).await.len(), 1, "not an orphan yet");
}

#[tokio::test]
async fn orphan_nars_are_removed_after_grace_period() {
  let (server, cache, token) = setup().await;
  let db = server.db().await;
  let a = TestNar::new("a-1.0", b"a");
  let b = TestNar::new("b-1.0", b"b");
  server.push_path(&token, "main", &a).await;
  server.push_path(&token, "main", &b).await;
  db.nar().delete_path(cache, &a.store_path).await.unwrap();

  cleanup(&server).await;
  assert_eq!(nars(&db).await.len(), 2, "young orphans are kept");

  age_nars(&db).await;
  // Gone from the db already, the file delete of a missing file only warns
  cleanup(&server).await;
  assert_eq!(nars(&db).await.len(), 1);
  assert_eq!(nar_files(&server).len(), 1);
  assert_eq!(
    nar_bytes(&server, "main", &b).await,
    (StatusCode::OK, b.file.clone())
  );

  // Already deleted orphans are a no-op
  assert!(!db.nar().delete_orphan_nar(Uuid::new_v4()).await.unwrap());
  cleanup(&server).await;
  assert_eq!(nars(&db).await.len(), 1);
}

#[tokio::test]
async fn orphan_file_missing_is_not_fatal() {
  let (server, cache, token) = setup().await;
  let db = server.db().await;
  let a = TestNar::new("a-1.0", b"a");
  server.push_path(&token, "main", &a).await;
  db.cache().clear_cache(cache).await.unwrap();
  for f in nar_files(&server) {
    std::fs::remove_file(server.storage_path().join(f)).unwrap();
  }
  age_nars(&db).await;
  cleanup(&server).await;
  assert!(nars(&db).await.is_empty());
}

#[tokio::test]
async fn sweep_removes_only_old_unknown_files() {
  let (server, cache, token) = setup().await;
  let db = server.db().await;
  let a = TestNar::new("a-1.0", b"a");
  server.push_path(&token, "main", &a).await;
  let known_nar = nar_files(&server).remove(0);
  let b = TestNar::new("b-1.0", b"b");
  let (_, _) = start_upload(&server, &token, cache, &b).await;
  let upload = uploads(&db).await[0].id;

  let old = 2 * 3600;
  let unknown = Uuid::now_v7();
  // Make the existing nar file old too
  write_file(&server, &known_nar, &a.file, old);
  for (path, age) in [
    (format!("{unknown}.nar"), old),
    (format!("parts/{unknown}/0"), old),
    (format!("parts/{upload}/0"), old),
    // The final file of an upload that is being finished
    (format!("{upload}.nar"), old),
    ("other.txt".to_string(), old),
    ("parts/nope/0".to_string(), old),
    (format!("{}.nar", Uuid::now_v7()), 0),
  ] {
    write_file(&server, &path, b"x", age);
  }
  let before = storage_files(&server);

  run_sweep(&db, &server.file_storage().await).await.unwrap();
  let after = storage_files(&server);
  let removed = before
    .iter()
    .filter(|f| !after.contains(f))
    .cloned()
    .collect::<Vec<_>>();
  let mut expected = vec![format!("{unknown}.nar"), format!("parts/{unknown}/0")];
  expected.sort();
  assert_eq!(removed, expected);
  assert_eq!(
    nar_bytes(&server, "main", &a).await,
    (StatusCode::OK, a.file.clone())
  );
}

#[tokio::test]
async fn staging_sweep_removes_old_staging_files() {
  let (server, _, _) = setup().await;
  write_file(&server, "a.nar#1", b"x", 2 * 3600);
  write_file(&server, "parts/x/0#2", b"x", 2 * 3600);
  write_file(&server, "b.nar#3", b"x", 0);
  assert_eq!(sweep_staging_files(&server.storage_path()).unwrap(), 2);
  assert_eq!(storage_files(&server), vec!["b.nar#3".to_string()]);
}
