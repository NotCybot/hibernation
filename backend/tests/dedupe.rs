//! The dedupe job merges referenced nars sharing a nar hash, the orphan cleanup then removes the extra files.
mod common;
mod push_common;

use backend::{
  cache::{cleanup::run_cleanup, dedupe::run_dedupe},
  db::DBTrait,
};
use common::{CacheOptions, TestNar, TestServer};
use entity::{nar, nar_info};
use push_common::*;
use reqwest::StatusCode;
use sea_orm::{ActiveValue::Set, prelude::*};
use uuid::Uuid;

/// A second nar row (and file) for `of`'s content, referenced by a new narinfo for `as_path` in `cache`.
/// What two concurrent first uploads of the same NAR leave behind.
async fn duplicate(
  server: &TestServer,
  db: &DatabaseConnection,
  cache: Uuid,
  of: &TestNar,
  as_path: &TestNar,
  referenced: bool,
) -> Uuid {
  let id = Uuid::now_v7();
  nar::ActiveModel {
    id: Set(id),
    hash: Set(of.file_hash.clone()),
    size: Set(of.file_size() as i64),
    nar_hash: Set(of.nar_hash.clone()),
    nar_size: Set(of.nar_size() as i64),
    created_at: Set(ago(0)),
  }
  .insert(db)
  .await
  .unwrap();
  write_file(server, &format!("{id}.nar"), &of.file, 0);
  if referenced {
    nar_info::ActiveModel {
      id: Set(Uuid::now_v7()),
      cache_id: Set(cache),
      nar_id: Set(id),
      store_path: Set(as_path.store_path.clone()),
      store_path_hash: Set(as_path.hash.clone()),
      compression: Set("zst".into()),
      deriver: Set(None),
      signature: Set(as_path.signature.clone()),
      last_accessed_at: Set(ago(0)),
      created_at: Set(ago(0)),
      accessed: Set(0),
    }
    .insert(db)
    .await
    .unwrap();
  }
  id
}

#[tokio::test]
async fn merges_duplicates_and_cleanup_removes_extra_files() {
  let (server, _) = TestServer::start_with_admin().await;
  server.create_cache("main", &CacheOptions::default()).await;
  let other = server.create_cache("other", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  let db = server.db().await;

  let a = TestNar::new("a-1.0", b"same");
  let b = TestNar::new("b-1.0", b"same");
  server.push_path(&token, "main", &a).await;
  let kept = nars(&db).await[0].id;
  let dup = duplicate(&server, &db, other, &a, &b, true).await;
  assert_eq!(
    db.nar().find_duplicate_nars().await.unwrap(),
    vec![a.nar_hash.clone()]
  );
  assert_eq!(
    db.nar().find_nars_by_nar_hash(&a.nar_hash).await.unwrap(),
    vec![kept, dup]
  );

  run_dedupe(&db).await;
  assert!(db.nar().find_duplicate_nars().await.unwrap().is_empty());
  assert_eq!(
    nar_info_of(&db, other, &b).await.nar_id,
    kept,
    "the oldest nar is kept"
  );

  age_nars(&db).await;
  run_cleanup(&db, &server.file_storage().await).await;
  assert_eq!(
    nars(&db).await.iter().map(|n| n.id).collect::<Vec<_>>(),
    vec![kept]
  );
  assert_eq!(nar_files(&server), vec![format!("{kept}.nar")]);
  assert_eq!(
    nar_bytes(&server, "main", &a).await,
    (StatusCode::OK, a.file.clone())
  );
  assert_eq!(
    nar_bytes(&server, "other", &b).await,
    (StatusCode::OK, a.file.clone())
  );
}

#[tokio::test]
async fn ignores_unreferenced_duplicates() {
  let (server, _) = TestServer::start_with_admin().await;
  let cache = server.create_cache("main", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  let db = server.db().await;

  let a = TestNar::new("a-1.0", b"same");
  server.push_path(&token, "main", &a).await;
  duplicate(&server, &db, cache, &a, &a, false).await;
  assert!(db.nar().find_duplicate_nars().await.unwrap().is_empty());

  // No duplicates: a no-op
  run_dedupe(&db).await;
  assert_eq!(nars(&db).await.len(), 2);
  assert_eq!(
    nar_bytes(&server, "main", &a).await,
    (StatusCode::OK, a.file.clone())
  );
}
