//! Full-stack tests with S3 file storage (Adobe S3Mock via testcontainers): multipart uploads, their abort on
//! every failure path, serving NARs from S3 and the cleanup / sweep / dedupe jobs against a bucket.
//!
//! S3Mock caveats: object `LastModified` can't be backdated, so the sweep (which only touches objects older than
//! an hour) can only be checked to keep fresh objects here, its deletions are covered on local storage in
//! `cleanup.rs`.
mod common;
mod push_common;

use backend::{
  cache::{
    cleanup::{run_cleanup, run_sweep},
    dedupe::run_dedupe,
    push::cleanup_upload,
  },
  db::DBTrait,
};
use centaurus::storage::{FileStorage, StoragePath};
use common::{CacheOptions, TestNar, TestServer};
use entity::{nar, nar_info};
use push_common::*;
use reqwest::{Method, StatusCode, header};
use sea_orm::{ActiveValue::Set, ConnectionTrait, prelude::*};
use serde_json::json;
use uuid::Uuid;

const MIB: usize = 1024 * 1024;

async fn setup() -> (TestServer, Uuid, String) {
  let (server, _) = TestServer::with_admin(TestServer::start_s3().await).await;
  let cache = server.create_cache("main", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  (server, cache, token)
}

/// Incompressible contents so the zstd file is about `len` bytes.
fn random_bytes(len: usize, seed: u64) -> Vec<u8> {
  let mut x = seed | 1;
  (0..len)
    .map(|_| {
      x ^= x << 13;
      x ^= x >> 7;
      x ^= x << 17;
      x as u8
    })
    .collect()
}

fn nar_key(id: Uuid) -> String {
  format!("{id}.nar")
}

async fn put_object(storage: &FileStorage, key: &str, body: &[u8]) {
  storage
    .save_file(&mut &body[..], StoragePath::from(key))
    .await
    .unwrap();
}

/// The single in-progress upload: its nar id and multipart id (checked against S3).
async fn only_upload(server: &TestServer) -> (Uuid, String) {
  let rows = uploads(&*server.db().await).await;
  assert_eq!(rows.len(), 1);
  let id = rows[0].multipart_id.clone().expect("multipart id stored");
  assert_eq!(
    server.s3_multipart_uploads().await,
    vec![(nar_key(rows[0].id), id.clone())]
  );
  (rows[0].id, id)
}

/// Nothing of a failed upload is left: no multipart upload, object, upload or nar row.
async fn assert_clean(server: &TestServer) {
  let db = server.db().await;
  assert!(server.s3_multipart_uploads().await.is_empty());
  assert!(server.storage_files().await.is_empty());
  assert!(uploads(&db).await.is_empty());
  assert!(nars(&db).await.is_empty());
}

#[tokio::test]
#[should_panic(expected = "Failed to init FileStorage")]
async fn boot_fails_without_bucket() {
  // FileStorage::init lists the bucket
  TestServer::start_backends(false, true, || unsafe {
    std::env::set_var("S3_BUCKET", "missing")
  })
  .await;
}

#[tokio::test]
async fn multipart_push_out_of_order_and_serve() {
  let (server, cache, token) = setup().await;
  let db = server.db().await;
  assert!(server.is_s3() && server.file_storage().await.multipart().is_some());

  // Parts other than the last must be >= 5 MiB on real S3
  let nar = TestNar::new("big-1.0", &random_bytes(11 * MIB, 1));
  let chunks = nar.chunks(5 * MIB);
  assert_eq!(chunks.len(), 3);
  let (upload, nar_found) = start_upload(&server, &token, cache, &nar).await;
  assert!(!nar_found);
  // The multipart upload is created by POST /cache/push and tracked in the db
  let (nar_id, _) = only_upload(&server).await;

  // Out of order, part 1 first with wrong data, then retried
  for (part, body) in [
    (2, chunks[2].clone()),
    (1, vec![0; 5 * MIB]),
    (0, chunks[0].clone()),
    (1, chunks[1].clone()),
  ] {
    let resp = chunk(&server, &token, upload, part, body).await;
    assert_eq!(resp.status(), StatusCode::OK, "part {part}");
  }
  // Parts go to the multipart upload, never to `parts/` objects
  assert!(server.storage_files().await.is_empty());

  let resp = finish(&server, &token, upload, &nar.finish_request()).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert!(server.s3_multipart_uploads().await.is_empty());
  assert!(uploads(&db).await.is_empty());
  assert_eq!(server.storage_files().await, vec![nar_key(nar_id)]);
  assert_eq!(nars(&db).await[0].id, nar_id);
  assert_eq!(
    nar_bytes(&server, "main", &nar).await,
    (StatusCode::OK, nar.file.clone())
  );

  let url = format!("/nix/main/{}", nar.nar_url());
  let resp = server
    .raw(Method::GET, &url)
    .header(header::RANGE, "bytes=5242870-5242889")
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
  assert_eq!(
    resp.headers()[header::CONTENT_RANGE],
    format!("bytes 5242870-5242889/{}", nar.file_size()).as_str()
  );
  assert_eq!(resp.bytes().await.unwrap(), nar.file[5242870..5242890]);

  let resp = server
    .raw(Method::GET, &url)
    .header(header::RANGE, format!("bytes={}-", nar.file_size()))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);

  let resp = server.raw(Method::HEAD, &url).send().await.unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(
    resp.headers()[header::CONTENT_LENGTH],
    nar.file_size().to_string().as_str()
  );

  // A row without its object: 404 instead of an S3 error
  server
    .file_storage()
    .await
    .delete_file(&StoragePath::from(nar_key(nar_id)))
    .await
    .unwrap();
  let resp = server.raw(Method::HEAD, &url).send().await.unwrap();
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
  assert_eq!(
    nar_bytes(&server, "main", &nar).await.0,
    StatusCode::NOT_FOUND
  );
}

#[tokio::test]
async fn small_non_final_parts_are_rejected_at_finish() {
  // S3 rejects completing with parts < 5 MiB that aren't the last one. The CLI sends CHUNK_SIZE (32 MiB) parts,
  // another client gets a 400 before completing and the upload is cleaned up
  let (server, cache, token) = setup().await;
  let nar = TestNar::new("small-1.0", &random_bytes(64 * 1024, 2));
  let (upload, _) = start_upload(&server, &token, cache, &nar).await;
  for (part, body) in nar.chunks(16 * 1024).into_iter().enumerate() {
    assert_eq!(
      chunk(&server, &token, upload, part, body).await.status(),
      StatusCode::OK
    );
  }
  let resp = finish(&server, &token, upload, &nar.finish_request()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  assert_clean(&server).await;
}

#[tokio::test]
async fn failed_finish_aborts_multipart() {
  let (server, cache, token) = setup().await;
  let nar = TestNar::new("a-1.0", b"hello s3");
  let mut wrong_hash = nar.finish_request();
  wrong_hash["file_hash"] = json!(TestNar::new("b-1.0", b"other").file_hash);
  let mut wrong_size = nar.finish_request();
  wrong_size["file_size"] = json!(nar.file_size() + 1);

  let cases = [
    // Completed, then fails verification: the object is removed
    (
      "invalid zstd",
      vec![(0, nar.nar.clone())],
      nar.finish_request(),
    ),
    ("hash mismatch", vec![(0, nar.file.clone())], wrong_hash),
    ("size mismatch", vec![(0, nar.file.clone())], wrong_size),
    // Never completed: the multipart upload is aborted
    (
      "missing part",
      vec![(0, nar.file.clone()), (2, nar.file.clone())],
      nar.finish_request(),
    ),
    ("no parts", vec![], nar.finish_request()),
  ];
  for (case, parts, finish_body) in cases {
    let (upload, _) = start_upload(&server, &token, cache, &nar).await;
    only_upload(&server).await;
    for (part, body) in parts {
      assert_eq!(
        chunk(&server, &token, upload, part, body).await.status(),
        StatusCode::OK,
        "{case}"
      );
    }
    let resp = finish(&server, &token, upload, &finish_body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{case}");
    assert_clean(&server).await;
  }

  // Still pushable afterwards
  assert!(!server.push_path(&token, "main", &nar).await.nar_found);
}

#[tokio::test]
async fn chunk_and_start_rejections() {
  let (server, cache, token) = setup().await;
  let db = server.db().await;
  let nar = TestNar::new("a-1.0", b"hello s3");
  // The quota is read when the upload starts
  let resp = server
    .post(
      &format!("/cache/management/{cache}"),
      CacheOptions {
        quota: nar.file_size() as i64 - 1,
        ..Default::default()
      }
      .edit_body("main"),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let (up, _) = start_upload(&server, &token, cache, &nar).await;

  // S3 allows at most 10000 parts (part numbers are 1-based, part 9999 is the last)
  let resp = chunk(&server, &token, up, 10000, nar.file.clone()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

  // Over quota: rejected before anything reaches S3, the multipart upload waits for the cleanup
  let resp = chunk(&server, &token, up, 0, nar.file.clone()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  assert!(server.storage_files().await.is_empty());

  // The upload row can't be stored: the fresh multipart upload is aborted again
  let (nar_id, _) = only_upload(&server).await;
  db.execute_unprepared("ALTER TABLE nar_upload RENAME TO nar_upload_hidden")
    .await
    .unwrap();
  let other = TestNar::new("b-1.0", b"other");
  let resp = upload(&server, &token, &other.upload_request(cache, false)).await;
  assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
  db.execute_unprepared("ALTER TABLE nar_upload_hidden RENAME TO nar_upload")
    .await
    .unwrap();
  let left = server.s3_multipart_uploads().await;
  assert_eq!(left.len(), 1);
  assert_eq!(left[0].0, nar_key(nar_id));
}

#[tokio::test]
async fn nar_reuse_and_concurrent_duplicate() {
  let (server, cache, token) = setup().await;
  let db = server.db().await;
  let a = TestNar::new("a-1.0", b"same");
  let b = TestNar::new("b-1.0", b"same");
  let c = TestNar::new("c-1.0", b"same");

  // Two uploads of the same NAR racing: both get a multipart upload
  let (up_a, _) = start_upload(&server, &token, cache, &a).await;
  let (up_b, _) = start_upload(&server, &token, cache, &b).await;
  assert_eq!(server.s3_multipart_uploads().await.len(), 2);
  for up in [up_a, up_b] {
    assert_eq!(
      chunk(&server, &token, up, 0, a.file.clone()).await.status(),
      StatusCode::OK
    );
  }
  for (up, nar) in [(up_a, &a), (up_b, &b)] {
    let resp = finish(&server, &token, up, &nar.finish_request()).await;
    assert_eq!(resp.status(), StatusCode::OK);
  }
  // The loser's object is deleted, both paths share the winner's nar
  let kept = nars(&db).await;
  assert_eq!(kept.len(), 1);
  assert_eq!(server.storage_files().await, vec![nar_key(kept[0].id)]);
  assert!(server.s3_multipart_uploads().await.is_empty());
  assert!(uploads(&db).await.is_empty());

  // A known NAR: no multipart upload, no upload row
  let (up_c, nar_found) = start_upload(&server, &token, cache, &c).await;
  assert!(nar_found);
  assert!(server.s3_multipart_uploads().await.is_empty());
  assert!(uploads(&db).await.is_empty());
  let resp = finish(&server, &token, up_c, &c.finish_request()).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(server.storage_files().await.len(), 1);
  for nar in [&a, &b, &c] {
    assert_eq!(nar_info_of(&db, cache, nar).await.nar_id, kept[0].id);
    assert_eq!(
      nar_bytes(&server, "main", nar).await,
      (StatusCode::OK, a.file.clone())
    );
  }
}

#[tokio::test]
async fn evicted_and_deleted_paths_lose_their_objects() {
  let (server, main, token) = setup().await;
  let other = server.create_cache("other", &CacheOptions::default()).await;
  let third = server.create_cache("third", &CacheOptions::default()).await;
  let db = server.db().await;
  let shared = TestNar::new("shared-1.0", b"shared");
  let evicted = TestNar::new("evicted-1.0", b"evicted");
  let deleted = TestNar::new("deleted-1.0", b"deleted");
  let dropped = TestNar::new("dropped-1.0", b"dropped");
  server.push_path(&token, "main", &shared).await;
  server.push_path(&token, "main", &evicted).await;
  assert!(server.push_path(&token, "other", &shared).await.nar_found);
  server.push_path(&token, "other", &deleted).await;
  server.push_path(&token, "third", &dropped).await;
  let id_of = |infos: Vec<nar_info::Model>, nar: &TestNar| {
    infos
      .into_iter()
      .find(|i| i.store_path == nar.store_path)
      .unwrap()
      .nar_id
  };
  let shared_id = id_of(nar_infos(&db, main).await, &shared);
  assert_eq!(server.storage_files().await.len(), 4);

  // Evict everything from main
  let resp = server
    .post(
      &format!("/cache/management/{main}"),
      CacheOptions {
        quota: 0,
        ..Default::default()
      }
      .edit_body("main"),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert!(nar_infos(&db, main).await.is_empty());
  let resp = server
    .delete(
      &format!("/cache/management/{other}/path"),
      json!({ "store_path": deleted.store_path }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let resp = server
    .delete("/cache/management", json!({ "uuid": third }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);

  // Rows only, the orphan cleanup removes the objects
  assert_eq!(server.storage_files().await.len(), 4);
  age_nars(&db).await;
  run_cleanup(&db, &server.file_storage().await).await;
  assert_eq!(server.storage_files().await, vec![nar_key(shared_id)]);
  assert_eq!(
    nar_bytes(&server, "other", &shared).await,
    (StatusCode::OK, shared.file.clone())
  );
}

#[tokio::test]
async fn cleanup_aborts_stale_multiparts() {
  let (server, cache, token) = setup().await;
  let db = server.db().await;
  let storage = server.file_storage().await;
  let multipart = storage.multipart().unwrap();

  // A stale upload with an uploaded part
  let a = TestNar::new("a-1.0", b"a");
  let (up_a, _) = start_upload(&server, &token, cache, &a).await;
  assert_eq!(
    chunk(&server, &token, up_a, 0, a.file.clone())
      .await
      .status(),
    StatusCode::OK
  );
  let (stale, stale_mp) = only_upload(&server).await;
  age_uploads(&db).await;

  // A stale upload that already created its nar row and object (crash while finishing): the object stays
  let finished = Uuid::now_v7();
  nar::ActiveModel {
    id: Set(finished),
    hash: Set("h".into()),
    size: Set(1),
    nar_hash: Set("n".into()),
    nar_size: Set(1),
    created_at: Set(ago(0)),
  }
  .insert(&*db)
  .await
  .unwrap();
  let key = StoragePath::from(nar_key(finished));
  let finished_mp = multipart.create_multipart(&key).await.unwrap();
  let part = multipart
    .put_part(&key, &finished_mp, 0, b"x".to_vec().into())
    .await
    .unwrap();
  multipart
    .complete_multipart(&key, &finished_mp, vec![part])
    .await
    .unwrap();
  db.nar()
    .create_upload(finished, Some(finished_mp.clone()))
    .await
    .unwrap();
  age_uploads(&db).await;

  // A fresh upload is left alone
  let b = TestNar::new("b-1.0", b"b");
  start_upload(&server, &token, cache, &b).await;
  let fresh = uploads(&db)
    .await
    .into_iter()
    .find(|u| u.id != stale && u.id != finished)
    .unwrap();

  run_cleanup(&db, &storage).await;
  assert_eq!(
    server.s3_multipart_uploads().await,
    vec![(nar_key(fresh.id), fresh.multipart_id.clone().unwrap())]
  );
  assert_eq!(
    uploads(&db)
      .await
      .into_iter()
      .map(|u| u.id)
      .collect::<Vec<_>>(),
    vec![fresh.id]
  );
  assert_eq!(server.storage_files().await, vec![nar_key(finished)]);

  // Retrying is safe: the stale upload is already aborted
  cleanup_upload(&storage, &db, stale, Some(&stale_mp))
    .await
    .unwrap();
  assert_eq!(server.storage_files().await, vec![nar_key(finished)]);

  // Orphan nar rows lose their object
  age_nars(&db).await;
  run_cleanup(&db, &storage).await;
  assert!(nars(&db).await.is_empty());
  assert!(server.storage_files().await.is_empty());
}

#[tokio::test]
async fn sweep_keeps_fresh_objects() {
  let (server, cache, token) = setup().await;
  let db = server.db().await;
  let storage = server.file_storage().await;
  let a = TestNar::new("a-1.0", b"a");
  server.push_path(&token, "main", &a).await;
  let b = TestNar::new("b-1.0", b"b");
  start_upload(&server, &token, cache, &b).await;
  let unknown = Uuid::now_v7();
  put_object(&storage, &nar_key(unknown), b"x").await;
  put_object(&storage, &format!("parts/{unknown}/0"), b"x").await;
  put_object(&storage, "other.txt", b"x").await;
  let before = server.storage_files().await;
  assert_eq!(before.len(), 4);

  // Objects younger than an hour are never swept (S3Mock can't backdate LastModified)
  run_sweep(&db, &storage).await.unwrap();
  assert_eq!(server.storage_files().await, before);
  assert_eq!(server.s3_multipart_uploads().await.len(), 1);
}

#[tokio::test]
async fn dedupe_then_cleanup_deletes_duplicate_object() {
  let (server, main, token) = setup().await;
  let other = server.create_cache("other", &CacheOptions::default()).await;
  let db = server.db().await;
  let a = TestNar::new("a-1.0", b"same");
  let b = TestNar::new("b-1.0", b"same");
  server.push_path(&token, "main", &a).await;
  let kept = nars(&db).await[0].id;

  // What two concurrent first uploads leave behind when the second finishes before the first creates its row
  let dup = Uuid::now_v7();
  nar::ActiveModel {
    id: Set(dup),
    hash: Set(a.file_hash.clone()),
    size: Set(a.file_size() as i64),
    nar_hash: Set(a.nar_hash.clone()),
    nar_size: Set(a.nar_size() as i64),
    created_at: Set(ago(0)),
  }
  .insert(&*db)
  .await
  .unwrap();
  put_object(&server.file_storage().await, &nar_key(dup), &a.file).await;
  nar_info::ActiveModel {
    id: Set(Uuid::now_v7()),
    cache_id: Set(other),
    nar_id: Set(dup),
    store_path: Set(b.store_path.clone()),
    store_path_hash: Set(b.hash.clone()),
    compression: Set("zst".into()),
    deriver: Set(None),
    signature: Set(b.signature.clone()),
    last_accessed_at: Set(ago(0)),
    created_at: Set(ago(0)),
    accessed: Set(0),
  }
  .insert(&*db)
  .await
  .unwrap();
  let mut both = vec![nar_key(kept), nar_key(dup)];
  both.sort();
  assert_eq!(server.storage_files().await, both);

  run_dedupe(&db).await;
  age_nars(&db).await;
  run_cleanup(&db, &server.file_storage().await).await;
  assert_eq!(server.storage_files().await, vec![nar_key(kept)]);
  assert_eq!(nar_info_of(&db, main, &a).await.nar_id, kept);
  assert_eq!(
    nar_bytes(&server, "other", &b).await,
    (StatusCode::OK, a.file.clone())
  );
}
