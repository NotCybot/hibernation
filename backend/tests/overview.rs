//! `/cache/management/overview`: most accessed and recently pushed paths across caches.
mod common;
mod push_common;

use common::{CacheOptions, TestNar, TestServer};
use entity::sea_orm_active_enums::AccessType;
use push_common::*;
use reqwest::{Method, StatusCode};
use sea_orm::{ActiveValue::Set, IntoActiveModel, prelude::*};
use serde_json::Value;

fn paths(body: &Value, key: &str) -> Vec<String> {
  body[key]
    .as_array()
    .unwrap()
    .iter()
    .map(|r| r["store_path"].as_str().unwrap().to_string())
    .collect()
}

#[tokio::test]
async fn overview_sorts_across_caches() {
  let (server, _) = TestServer::start_with_admin().await;
  let main = server.create_cache("main", &CacheOptions::default()).await;
  let other = server.create_cache("other", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  let a = TestNar::new("a-1.0", b"a");
  let b = TestNar::new("b-1.0", b"bb");
  let unused = TestNar::new("c-1.0", b"c");
  server.push_path(&token, "main", &a).await;
  server.push_path(&token, "main", &unused).await;
  server.push_path(&token, "other", &b).await;

  let db = server.db().await;
  for (cache, n, accessed, secs) in [
    (main, &a, 1, 30),
    (other, &b, 5, 10),
    (main, &unused, 0, 20),
  ] {
    let mut row = nar_info_of(&db, cache, n).await.into_active_model();
    row.accessed = Set(accessed);
    row.created_at = Set(ago(secs));
    row.update(&*db).await.unwrap();
  }

  let resp = server.get("/cache/management/overview").await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();

  // Never pulled paths are left out
  assert_eq!(
    paths(&body, "most_accessed"),
    vec![b.store_path.clone(), a.store_path.clone()]
  );
  assert_eq!(
    paths(&body, "recent_pushes"),
    vec![
      b.store_path.clone(),
      unused.store_path.clone(),
      a.store_path.clone()
    ]
  );
  let row = &body["most_accessed"][0];
  assert_eq!(row["cache_name"], "other");
  assert_eq!(row["cache_uuid"], other.to_string());
  assert_eq!(row["accessed"], 5);
  assert_eq!(row["size"], b.file_size());
  assert!(row["created_at"].is_string() && row["last_accessed_at"].is_string());
}

#[tokio::test]
async fn overview_is_limited_to_10() {
  let (server, _) = TestServer::start_with_admin().await;
  server.create_cache("main", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  for i in 0..12 {
    server
      .push_path(&token, "main", &TestNar::new(&format!("p{i}-1.0"), b"p"))
      .await;
  }

  let body: Value = server
    .get("/cache/management/overview")
    .await
    .json()
    .await
    .unwrap();
  assert_eq!(paths(&body, "recent_pushes").len(), 10);
}

#[tokio::test]
async fn overview_only_lists_paths_of_editable_caches() {
  let (server, _) = TestServer::start_with_admin().await;
  let viewed = server
    .create_cache("viewed", &CacheOptions::default())
    .await;
  let edited = server
    .create_cache("edited", &CacheOptions::default())
    .await;
  server
    .create_cache("hidden", &CacheOptions::default())
    .await;
  let (token, _) = server.create_cli_token("ci").await;
  let v = TestNar::new("v-1.0", b"v");
  let e = TestNar::new("e-1.0", b"e");
  let h = TestNar::new("h-1.0", b"h");
  server.push_path(&token, "viewed", &v).await;
  server.push_path(&token, "edited", &e).await;
  server.push_path(&token, "hidden", &h).await;

  let user = create_user(&server).await;
  let db = server.db().await;
  grant_user(&db, viewed, user.id, AccessType::View).await;
  grant_group(&server, &db, edited, user.id, AccessType::Edit).await;

  let resp = user
    .auth(server.raw(Method::GET, "/cache/management/overview"))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(paths(&body, "recent_pushes"), vec![e.store_path.clone()]);
}
