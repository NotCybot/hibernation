//! Quota enforcement and the three eviction policies, on push and on quota edit.
mod common;
mod push_common;

use common::{CacheOptions, TestNar, TestServer};
use entity::nar_info;
use push_common::*;
use reqwest::StatusCode;
use sea_orm::{ActiveValue::Set, IntoActiveModel, prelude::*};
use uuid::Uuid;

struct Ctx {
  server: TestServer,
  cache: Uuid,
  token: String,
  /// Fixed reference time so equal ages give equal timestamps
  now: chrono::NaiveDateTime,
}

async fn ctx(policy: &'static str) -> Ctx {
  let (server, _) = TestServer::start_with_admin().await;
  let cache = server
    .create_cache(
      "main",
      &CacheOptions {
        eviction_policy: policy,
        ..Default::default()
      },
    )
    .await;
  let (token, _) = server.create_cli_token("ci").await;
  Ctx {
    server,
    cache,
    token,
    now: ago(0),
  }
}

impl Ctx {
  async fn set_quota(&self, quota: i64, policy: &'static str) {
    let opts = CacheOptions {
      quota,
      eviction_policy: policy,
      ..Default::default()
    };
    let resp = self
      .server
      .post(
        &format!("/cache/management/{}", self.cache),
        opts.edit_body("main"),
      )
      .await;
    assert_eq!(resp.status(), StatusCode::OK);
  }

  /// Overwrite the eviction relevant columns of `nar`'s row: ages in seconds before now.
  async fn set_stats(&self, nar: &TestNar, created_ago: i64, accessed_ago: i64, accessed: i64) {
    let db = self.server.db().await;
    let mut row = nar_info_of(&db, self.cache, nar).await.into_active_model();
    row.created_at = Set(self.now - chrono::Duration::seconds(created_ago));
    row.last_accessed_at = Set(self.now - chrono::Duration::seconds(accessed_ago));
    row.accessed = Set(accessed);
    row.update(&*db).await.unwrap();
  }

  async fn present(&self, nar: &TestNar) -> bool {
    match narinfo_status(&self.server, "main", nar).await {
      StatusCode::OK => true,
      StatusCode::NOT_FOUND => false,
      s => panic!("unexpected {s}"),
    }
  }

  async fn push(&self, nar: &TestNar) {
    self.server.push_path(&self.token, "main", nar).await;
  }
}

fn nar(name: &str, byte: u8) -> TestNar {
  TestNar::new(name, &[byte; 4096])
}

fn total(nars: &[&TestNar]) -> i64 {
  nars.iter().map(|n| n.file_size() as i64).sum()
}

/// Push a, b, c with `stats`, then push d into a quota that is exactly `overflow` bytes too small for all four.
async fn evict_on_push(
  policy: &'static str,
  stats: [(i64, i64, i64); 3],
  overflow: impl Fn(&[TestNar; 3]) -> i64,
) -> (Ctx, [TestNar; 3], TestNar) {
  let c = ctx(policy).await;
  let existing = [nar("a-1.0", 1), nar("b-1.0", 2), nar("c-1.0", 3)];
  let d = nar("d-1.0", 4);
  for (n, (created, accessed_at, accessed)) in existing.iter().zip(stats) {
    c.push(n).await;
    c.set_stats(n, created, accessed_at, accessed).await;
  }
  let all = [&existing[0], &existing[1], &existing[2], &d];
  c.set_quota(total(&all) - overflow(&existing), policy).await;
  for n in &existing {
    assert!(c.present(n).await, "quota edit must not evict yet");
  }
  c.push(&d).await;
  assert!(c.present(&d).await);
  (c, existing, d)
}

#[tokio::test]
async fn lru_evicts_least_recently_accessed() {
  let (c, [a, b, cc], _) = evict_on_push(
    "LeastRecentlyUsed",
    [(100, 10, 0), (50, 1000, 0), (10, 100, 0)],
    |_| 1,
  )
  .await;
  assert!(c.present(&a).await);
  assert!(!c.present(&b).await);
  assert!(c.present(&cc).await);
}

#[tokio::test]
async fn lru_tie_breaks_on_created_at() {
  let (c, [a, b, cc], _) = evict_on_push(
    "LeastRecentlyUsed",
    [(50, 500, 0), (100, 500, 0), (10, 500, 0)],
    |_| 1,
  )
  .await;
  assert!(c.present(&a).await);
  assert!(!c.present(&b).await);
  assert!(c.present(&cc).await);
}

#[tokio::test]
async fn lfu_evicts_least_frequently_accessed() {
  let (c, [a, b, cc], _) = evict_on_push(
    "LeastFrequentlyUsed",
    [(100, 1000, 5), (50, 10, 1), (10, 10, 3)],
    |_| 1,
  )
  .await;
  assert!(c.present(&a).await);
  assert!(!c.present(&b).await);
  assert!(c.present(&cc).await);
}

#[tokio::test]
async fn lfu_tie_breaks_on_created_at() {
  let (c, [a, b, cc], _) = evict_on_push(
    "LeastFrequentlyUsed",
    [(50, 10, 2), (10, 10, 2), (100, 10, 2)],
    |_| 1,
  )
  .await;
  assert!(c.present(&a).await);
  assert!(c.present(&b).await);
  assert!(!c.present(&cc).await);
}

#[tokio::test]
async fn oldest_first_evicts_oldest_created() {
  let (c, [a, b, cc], _) = evict_on_push(
    "OldestFirst",
    [(50, 10_000, 0), (10, 10_000, 0), (100, 1, 100)],
    |_| 1,
  )
  .await;
  assert!(c.present(&a).await);
  assert!(c.present(&b).await);
  assert!(!c.present(&cc).await);
}

#[tokio::test]
async fn evicts_as_many_as_needed() {
  // One byte more than the first victim frees: the second victim goes too, the third stays
  let (c, [a, b, cc], _) = evict_on_push(
    "OldestFirst",
    [(300, 0, 0), (200, 0, 0), (100, 0, 0)],
    |n| n[0].file_size() as i64 + 1,
  )
  .await;
  assert!(!c.present(&a).await);
  assert!(!c.present(&b).await);
  assert!(c.present(&cc).await);
}

#[tokio::test]
async fn exactly_full_quota_evicts_nothing() {
  let (c, existing, _) =
    evict_on_push("OldestFirst", [(3, 0, 0), (2, 0, 0), (1, 0, 0)], |_| 0).await;
  for n in &existing {
    assert!(c.present(n).await);
  }
}

#[tokio::test]
async fn quota_edit_evicts_with_new_policy() {
  // Regression: the edit evicted with the previously stored policy
  let c = ctx("LeastRecentlyUsed").await;
  let a = nar("a-1.0", 1);
  let b = nar("b-1.0", 2);
  c.push(&a).await;
  c.push(&b).await;
  // a: oldest but recently used, b: newer but least recently used
  c.set_stats(&a, 1000, 1, 0).await;
  c.set_stats(&b, 10, 1000, 0).await;

  c.set_quota(total(&[&a, &b]) - 1, "OldestFirst").await;
  assert!(!c.present(&a).await);
  assert!(c.present(&b).await);

  c.set_quota(total(&[&b]), "OldestFirst").await;
  assert!(c.present(&b).await, "exactly full keeps everything");
  c.set_quota(0, "LeastFrequentlyUsed").await;
  assert!(!c.present(&b).await);
  assert!(nar_infos(&*c.server.db().await, c.cache).await.is_empty());
}

#[tokio::test]
async fn file_larger_than_quota_is_rejected() {
  let c = ctx("LeastRecentlyUsed").await;
  let a = nar("a-1.0", 1);
  c.set_quota(a.file_size() as i64 - 1, "LeastRecentlyUsed")
    .await;
  let (upload, _) = start_upload(&c.server, &c.token, c.cache, &a).await;
  let resp = chunk(&c.server, &c.token, upload, 0, a.file.clone()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn evicting_shared_nar_keeps_it_for_other_caches() {
  let c = ctx("LeastRecentlyUsed").await;
  let other = c
    .server
    .create_cache("other", &CacheOptions::default())
    .await;
  let a = nar("a-1.0", 1);
  c.push(&a).await;
  let pushed = c.server.push_path(&c.token, "other", &a).await;
  assert!(pushed.nar_found);

  c.set_quota(0, "LeastRecentlyUsed").await;
  assert!(!c.present(&a).await);

  let db = c.server.db().await;
  let storage = c.server.file_storage().await;
  age_nars(&db).await;
  backend::cache::cleanup::run_cleanup(&db, &storage).await;
  assert_eq!(
    nar_bytes(&c.server, "other", &a).await,
    (StatusCode::OK, a.file.clone())
  );
  assert_eq!(nar_files(&c.server).len(), 1);

  // Once the last reference is gone the nar and its file go too
  nar_info::Entity::delete_many()
    .filter(nar_info::Column::CacheId.eq(other))
    .exec(&*db)
    .await
    .unwrap();
  age_nars(&db).await;
  backend::cache::cleanup::run_cleanup(&db, &storage).await;
  assert!(nars(&db).await.is_empty());
  assert!(nar_files(&c.server).is_empty());
}
