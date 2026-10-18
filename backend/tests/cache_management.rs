//! `/cache/management`: list, details, create, edit, delete, clear, search and delete path.
mod common;
mod push_common;

use common::{CacheOptions, TEST_PUBLIC_KEY, TestNar, TestServer, mock_downstream};
use entity::{cache_access, nar_info, sea_orm_active_enums::AccessType};
use push_common::*;
use reqwest::{Method, StatusCode};
use sea_orm::{ActiveValue::Set, IntoActiveModel, prelude::*};
use serde_json::{Value, json};
use uuid::Uuid;

async fn setup() -> (TestServer, Uuid, String) {
  let (server, _) = TestServer::start_with_admin().await;
  let cache = server.create_cache("main", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  (server, cache, token)
}

async fn create(server: &TestServer, body: Value) -> StatusCode {
  server.post("/cache/management", body).await.status()
}

fn create_body(name: &str) -> Value {
  json!({ "name": name, "public": true, "quota": 1, "sig_key": TEST_PUBLIC_KEY })
}

async fn details(server: &TestServer, cache: Uuid) -> Value {
  let resp = server.get(&format!("/cache/management/{cache}")).await;
  assert_eq!(resp.status(), StatusCode::OK);
  resp.json().await.unwrap()
}

async fn edit(server: &TestServer, cache: Uuid, body: Value) -> StatusCode {
  server
    .post(&format!("/cache/management/{cache}"), body)
    .await
    .status()
}

async fn nix_cache_info(server: &TestServer, name: &str) -> (StatusCode, String) {
  let resp = server
    .raw(Method::GET, &format!("/nix/{name}/nix-cache-info"))
    .send()
    .await
    .unwrap();
  (resp.status(), resp.text().await.unwrap())
}

async fn search(
  server: &TestServer,
  cache: Uuid,
  query: &str,
  sort: &str,
  order: &str,
) -> Vec<String> {
  let resp = server
    .post(
      &format!("/cache/management/{cache}/search"),
      json!({ "query": query, "sort": sort, "order": order }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Vec<Value> = resp.json().await.unwrap();
  body
    .iter()
    .map(|r| r["store_path"].as_str().unwrap().to_string())
    .collect()
}

// ---- list / details ----

#[tokio::test]
async fn list_and_details_shape() {
  let (server, cache, token) = setup().await;
  let resp = server.get("/cache/management").await;
  assert_eq!(resp.status(), StatusCode::OK);
  let list: Value = resp.json().await.unwrap();
  assert_eq!(
    list,
    json!([{ "uuid": cache, "name": "main", "size": 0, "quota": 1024 * 1024 * 1024, "public": true }])
  );

  let a = TestNar::new("a-1.0", b"a");
  server.push_path(&token, "main", &a).await;
  let list: Value = server.get("/cache/management").await.json().await.unwrap();
  assert_eq!(list[0]["size"], a.file_size());

  assert_eq!(
    details(&server, cache).await,
    json!({
      "uuid": cache,
      "name": "main",
      "size": a.file_size(),
      "quota": 1024 * 1024 * 1024,
      "public": true,
      "sig_key": TEST_PUBLIC_KEY,
      "priority": 40,
      "nar_count": 1,
      "allow_force_push": false,
      "eviction_policy": "LeastRecentlyUsed",
      "has_write_access": true,
      "downstream_caches": [],
    })
  );
}

#[tokio::test]
async fn size_is_not_multiplied_by_access_rows() {
  // Regression: joining cache_access for the permission filter multiplied the summed size and nar count
  let (server, cache, token) = setup().await;
  let db = server.db().await;
  let a = TestNar::new("a-1.0", b"a");
  server.push_path(&token, "main", &a).await;
  for _ in 0..2 {
    let user = create_user(&server).await;
    grant_user(&db, cache, user.id, AccessType::View).await;
  }

  let list: Value = server.get("/cache/management").await.json().await.unwrap();
  assert_eq!(list.as_array().unwrap().len(), 1);
  assert_eq!(list[0]["size"], a.file_size());
  let details = details(&server, cache).await;
  assert_eq!(details["size"], a.file_size());
  assert_eq!(details["nar_count"], 1);
}

#[tokio::test]
async fn visibility_depends_on_access() {
  let (server, public, _) = setup().await;
  let private = server
    .create_cache(
      "private",
      &CacheOptions {
        public: false,
        ..Default::default()
      },
    )
    .await;
  let granted = server
    .create_cache(
      "granted",
      &CacheOptions {
        public: false,
        ..Default::default()
      },
    )
    .await;
  let db = server.db().await;
  let user = create_user(&server).await;
  grant_user(&db, granted, user.id, AccessType::View).await;

  let resp = user
    .auth(server.raw(Method::GET, "/cache/management"))
    .send()
    .await
    .unwrap();
  let list: Vec<Value> = resp.json().await.unwrap();
  let mut names = list
    .iter()
    .map(|c| c["name"].as_str().unwrap())
    .collect::<Vec<_>>();
  names.sort();
  assert_eq!(names, ["granted", "main"]);

  let get = |cache: Uuid| {
    let req = user.auth(server.raw(Method::GET, &format!("/cache/management/{cache}")));
    async move { req.send().await.unwrap() }
  };
  let resp = get(public).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["has_write_access"], false);
  assert_eq!(get(granted).await.status(), StatusCode::OK);
  assert_eq!(get(private).await.status(), StatusCode::NOT_FOUND);
  assert_eq!(get(Uuid::new_v4()).await.status(), StatusCode::NOT_FOUND);

  // Edit access through a group
  grant_group(&server, &db, private, user.id, AccessType::Edit).await;
  let body: Value = get(private).await.json().await.unwrap();
  assert_eq!(body["has_write_access"], true);
}

#[tokio::test]
async fn management_requires_session() {
  let (server, cache, _) = setup().await;
  server.clear_cookies();
  assert!(!server.get("/cache/management").await.status().is_success());
  assert!(
    !server
      .get(&format!("/cache/management/{cache}"))
      .await
      .status()
      .is_success()
  );
}

// ---- create ----

#[tokio::test]
async fn create_defaults_and_quota_in_mib() {
  let (server, _) = TestServer::start_with_admin().await;
  let resp = server
    .post(
      "/cache/management",
      json!({ "name": "main", "public": false, "quota": 3, "sig_key": TEST_PUBLIC_KEY }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  let cache = Uuid::parse_str(body["uuid"].as_str().unwrap()).unwrap();

  let details = details(&server, cache).await;
  assert_eq!(details["quota"], 3 * 1024 * 1024);
  assert_eq!(details["public"], false);
  assert_eq!(details["priority"], 50);
  assert_eq!(details["eviction_policy"], "LeastRecentlyUsed");
  assert_eq!(details["allow_force_push"], false);
  assert_eq!(
    details["downstream_caches"],
    json!(["https://cache.nixos.org/"])
  );

  // Creator gets edit access
  let db = server.db().await;
  let access = cache_access::Entity::find()
    .filter(cache_access::Column::CacheId.eq(cache))
    .all(&*db)
    .await
    .unwrap();
  assert_eq!(access.len(), 1);
  assert_eq!(access[0].access_type, AccessType::Edit);
}

#[tokio::test]
async fn create_clamps_quota() {
  let (server, _) = TestServer::start_with_admin().await;
  let mut body = create_body("negative");
  body["quota"] = json!(-5);
  let resp = server.post("/cache/management", body).await;
  let id: Value = resp.json().await.unwrap();
  let id = Uuid::parse_str(id["uuid"].as_str().unwrap()).unwrap();
  assert_eq!(details(&server, id).await["quota"], 0);

  // Regression: MiB to bytes overflowed i64 and panicked the handler
  let mut body = create_body("huge");
  body["quota"] = json!(i64::MAX);
  let resp = server.post("/cache/management", body).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let id: Value = resp.json().await.unwrap();
  let id = Uuid::parse_str(id["uuid"].as_str().unwrap()).unwrap();
  assert_eq!(details(&server, id).await["quota"], i64::MAX);
}

#[tokio::test]
async fn create_validates_name_and_key() {
  let (server, _) = TestServer::start_with_admin().await;
  let long = "a".repeat(64);
  for name in ["", "-a", "a-", "a_b", "a.b", "123", long.as_str()] {
    assert_eq!(
      create(&server, create_body(name)).await,
      StatusCode::BAD_REQUEST,
      "{name}"
    );
  }
  assert_eq!(
    create(&server, create_body(&"a".repeat(63))).await,
    StatusCode::OK
  );
  assert_eq!(create(&server, create_body("1a")).await, StatusCode::OK);

  let mut body = create_body("badkey");
  body["sig_key"] = json!("not-a-key");
  assert_eq!(create(&server, body).await, StatusCode::NOT_ACCEPTABLE);

  assert_eq!(create(&server, create_body("main")).await, StatusCode::OK);
  assert_eq!(
    create(&server, create_body("main")).await,
    StatusCode::CONFLICT
  );
  assert_eq!(
    create(&server, create_body("MAIN")).await,
    StatusCode::CONFLICT
  );
}

#[tokio::test]
async fn create_requires_permission() {
  let (server, _) = TestServer::start_with_admin().await;
  let user = create_user(&server).await;
  let resp = user
    .auth(server.raw(Method::POST, "/cache/management"))
    .json(&create_body("main"))
    .send()
    .await
    .unwrap();
  assert!(!resp.status().is_success());
}

// ---- edit ----

#[tokio::test]
async fn edit_validation() {
  let (server, cache, _) = setup().await;
  server.create_cache("other", &CacheOptions::default()).await;
  let ok = CacheOptions::default();
  let body = |f: &dyn Fn(&mut Value)| {
    let mut b = ok.edit_body("main");
    f(&mut b);
    b
  };

  assert_eq!(
    edit(&server, cache, body(&|b| b["priority"] = json!(-1))).await,
    StatusCode::BAD_REQUEST
  );
  assert_eq!(
    edit(&server, cache, body(&|b| b["name"] = json!("-x"))).await,
    StatusCode::BAD_REQUEST
  );
  assert_eq!(
    edit(&server, cache, body(&|b| b["name"] = json!("42"))).await,
    StatusCode::BAD_REQUEST
  );
  assert_eq!(
    edit(&server, cache, body(&|b| b["sig_key"] = json!("x"))).await,
    StatusCode::NOT_ACCEPTABLE
  );
  assert_eq!(
    edit(&server, cache, body(&|b| b["quota"] = json!(-1))).await,
    StatusCode::BAD_REQUEST
  );
  assert_eq!(
    edit(&server, cache, body(&|b| b["name"] = json!("Other"))).await,
    StatusCode::CONFLICT
  );
  // Renaming to itself (in another case) is fine
  assert_eq!(
    edit(&server, cache, body(&|b| b["name"] = json!("Main"))).await,
    StatusCode::OK
  );
  assert_eq!(details(&server, cache).await["name"], "Main");
  // An unknown cache with the global permission
  assert_eq!(
    edit(&server, Uuid::new_v4(), ok.edit_body("new")).await,
    StatusCode::BAD_REQUEST
  );
}

#[tokio::test]
async fn edit_requires_edit_access() {
  let (server, cache, _) = setup().await;
  let user = create_user(&server).await;
  grant_user(&*server.db().await, cache, user.id, AccessType::View).await;
  let resp = user
    .auth(server.raw(Method::POST, &format!("/cache/management/{cache}")))
    .json(&CacheOptions::default().edit_body("main"))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);

  let editor = create_user(&server).await;
  grant_user(&*server.db().await, cache, editor.id, AccessType::Edit).await;
  let resp = editor
    .auth(server.raw(Method::POST, &format!("/cache/management/{cache}")))
    .json(&CacheOptions::default().edit_body("main"))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn edit_applies_settings_and_invalidates_nix_cache() {
  let (server, cache, _) = setup().await;
  let (_, info) = nix_cache_info(&server, "main").await;
  assert!(info.contains("Priority: 40"));

  let downstream = mock_downstream().await;
  let opts = CacheOptions {
    priority: 7,
    public: false,
    quota: 1234,
    allow_force_push: true,
    eviction_policy: "OldestFirst",
    // Sorted and deduplicated
    downstream_caches: vec![
      "https://b.example/".into(),
      downstream.uri(),
      "https://b.example".into(),
    ],
    ..Default::default()
  };
  assert_eq!(
    edit(&server, cache, opts.edit_body("main")).await,
    StatusCode::OK
  );
  let d = details(&server, cache).await;
  assert_eq!(d["priority"], 7);
  assert_eq!(d["public"], false);
  assert_eq!(d["quota"], 1234);
  assert_eq!(d["allow_force_push"], true);
  assert_eq!(d["eviction_policy"], "OldestFirst");
  let mut expected = vec![
    format!("{}/", downstream.uri()),
    "https://b.example/".to_string(),
  ];
  expected.sort();
  assert_eq!(d["downstream_caches"], json!(expected));

  // Not served from the 30 s lookup cache: now private
  assert_eq!(
    nix_cache_info(&server, "main").await.0,
    StatusCode::UNAUTHORIZED
  );
  let public = CacheOptions {
    priority: 9,
    ..Default::default()
  };
  assert_eq!(
    edit(&server, cache, public.edit_body("main")).await,
    StatusCode::OK
  );
  assert!(
    nix_cache_info(&server, "main")
      .await
      .1
      .contains("Priority: 9")
  );
  assert_eq!(
    details(&server, cache).await["downstream_caches"],
    json!([])
  );
}

// ---- delete / clear ----

#[tokio::test]
async fn delete_cache_removes_everything() {
  let (server, cache, token) = setup().await;
  let a = TestNar::new("a-1.0", b"a");
  server.push_path(&token, "main", &a).await;
  assert_eq!(nix_cache_info(&server, "main").await.0, StatusCode::OK);

  let viewer = create_user(&server).await;
  let db = server.db().await;
  grant_user(&db, cache, viewer.id, AccessType::View).await;
  let resp = viewer
    .auth(server.raw(Method::DELETE, "/cache/management"))
    .json(&json!({ "uuid": cache }))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);

  let resp = server
    .delete("/cache/management", json!({ "uuid": cache }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(
    nix_cache_info(&server, "main").await.0,
    StatusCode::NOT_FOUND
  );
  assert_eq!(
    server
      .get(&format!("/cache/management/{cache}"))
      .await
      .status(),
    StatusCode::NOT_FOUND
  );
  assert!(nar_infos(&db, cache).await.is_empty());
  assert!(
    cache_access::Entity::find()
      .all(&*db)
      .await
      .unwrap()
      .is_empty()
  );

  age_nars(&db).await;
  backend::cache::cleanup::run_cleanup(&db, &server.file_storage().await).await;
  assert!(nars(&db).await.is_empty());
  assert!(storage_files(&server).is_empty());

  // The name is free again
  assert_eq!(create(&server, create_body("main")).await, StatusCode::OK);
}

#[tokio::test]
async fn clear_cache_removes_paths() {
  let (server, cache, token) = setup().await;
  let a = TestNar::new("a-1.0", b"a");
  server.push_path(&token, "main", &a).await;

  let viewer = create_user(&server).await;
  grant_user(&*server.db().await, cache, viewer.id, AccessType::View).await;
  let resp = viewer
    .auth(server.raw(Method::DELETE, &format!("/cache/management/{cache}")))
    .json(&json!({}))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);

  let resp = server
    .delete(&format!("/cache/management/{cache}"), json!({}))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let d = details(&server, cache).await;
  assert_eq!(d["nar_count"], 0);
  assert_eq!(d["size"], 0);
  assert_eq!(
    narinfo_status(&server, "main", &a).await,
    StatusCode::NOT_FOUND
  );
}

// ---- search / delete path ----

#[tokio::test]
async fn search_filters_and_sorts() {
  let (server, cache, token) = setup().await;
  let small = TestNar::new("Alpha-1.0", b"s");
  let big = TestNar::new("beta-1.0", &[7u8; 10_000].map(|_| rand_byte()));
  let other = TestNar::new("gamma-2.0", b"gg");
  for n in [&small, &big, &other] {
    server.push_path(&token, "main", n).await;
  }
  let db = server.db().await;
  for (n, accessed, secs) in [(&small, 3, 30), (&big, 1, 10), (&other, 2, 20)] {
    let mut row = nar_info_of(&db, cache, n).await.into_active_model();
    row.accessed = Set(accessed);
    row.last_accessed_at = Set(ago(secs));
    row.created_at = Set(ago(secs * 2));
    row.update(&*db).await.unwrap();
  }
  let p = |n: &TestNar| n.store_path.clone();

  // Case-insensitive, trimmed substring match
  assert_eq!(
    search(&server, cache, "  ALPHA ", "StorePath", "Asc").await,
    vec![p(&small)]
  );
  assert_eq!(
    search(&server, cache, "-1.0", "Size", "Desc").await,
    vec![p(&big), p(&small)]
  );
  assert_eq!(
    search(&server, cache, "", "AccessCount", "Asc").await,
    vec![p(&big), p(&other), p(&small)]
  );
  assert_eq!(
    search(&server, cache, "", "Accessed", "Desc").await,
    vec![p(&big), p(&other), p(&small)]
  );
  assert_eq!(
    search(&server, cache, "", "Created", "Asc").await,
    vec![p(&small), p(&other), p(&big)]
  );
  let mut sorted = vec![p(&small), p(&big), p(&other)];
  sorted.sort();
  assert_eq!(search(&server, cache, "", "StorePath", "Asc").await, sorted);
  assert!(
    search(&server, cache, "nothing", "StorePath", "Asc")
      .await
      .is_empty()
  );

  let resp = server
    .post(
      &format!("/cache/management/{cache}/search"),
      json!({ "query": "", "sort": "StorePath", "order": "Asc" }),
    )
    .await;
  let rows: Vec<Value> = resp.json().await.unwrap();
  let row = rows.iter().find(|r| r["store_path"] == p(&big)).unwrap();
  assert_eq!(row["size"], big.file_size());
  assert_eq!(row["accessed"], 1);
  assert!(row["created_at"].is_string() && row["last_accessed_at"].is_string());
}

fn rand_byte() -> u8 {
  Uuid::new_v4().as_bytes()[0]
}

#[tokio::test]
async fn search_is_limited_to_100() {
  let (server, cache, token) = setup().await;
  let a = TestNar::new("a-1.0", b"a");
  server.push_path(&token, "main", &a).await;
  let db = server.db().await;
  let template = nar_info_of(&db, cache, &a).await;
  for i in 0..120 {
    let mut row = template.clone().into_active_model();
    row.id = Set(Uuid::now_v7());
    row.store_path = Set(format!("{i:032}-x"));
    row.store_path_hash = Set(format!("{i:032}"));
    nar_info::Entity::insert(row).exec(&*db).await.unwrap();
  }
  assert_eq!(
    search(&server, cache, "", "StorePath", "Asc").await.len(),
    100
  );
}

#[tokio::test]
async fn search_and_delete_path_require_edit_access() {
  let (server, cache, _) = setup().await;
  let user = create_user(&server).await;
  grant_user(&*server.db().await, cache, user.id, AccessType::View).await;
  let resp = user
    .auth(server.raw(Method::POST, &format!("/cache/management/{cache}/search")))
    .json(&json!({ "query": "", "sort": "StorePath", "order": "Asc" }))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);
  let resp = user
    .auth(server.raw(Method::DELETE, &format!("/cache/management/{cache}/path")))
    .json(&json!({ "store_path": "x" }))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_path_removes_only_that_path() {
  let (server, cache, token) = setup().await;
  let a = TestNar::new("a-1.0", b"a");
  let b = TestNar::new("b-1.0", b"b");
  server.push_path(&token, "main", &a).await;
  server.push_path(&token, "main", &b).await;

  let path = format!("/cache/management/{cache}/path");
  let resp = server
    .delete(&path, json!({ "store_path": a.store_path }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(
    narinfo_status(&server, "main", &a).await,
    StatusCode::NOT_FOUND
  );
  assert_eq!(narinfo_status(&server, "main", &b).await, StatusCode::OK);

  // Unknown paths are a no-op
  let resp = server
    .delete(&path, json!({ "store_path": "missing" }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  // The path can be pushed again
  assert!(!server.push_path(&token, "main", &a).await.skipped);
}

#[tokio::test]
async fn search_escapes_like_wildcards() {
  // Regression: `%`, `_` and `\` in the query acted as LIKE wildcards / escapes
  let (server, cache, token) = setup().await;
  let a = TestNar::new("a-1.0", b"a");
  server.push_path(&token, "main", &a).await;
  let db = server.db().await;
  let template = nar_info_of(&db, cache, &a).await;
  for (i, name) in ["a_b", "axb", "50%", "500", "back\\slash", "backxslash"]
    .iter()
    .enumerate()
  {
    let mut row = template.clone().into_active_model();
    row.id = Set(Uuid::now_v7());
    row.store_path = Set(format!("{i:032}-{name}"));
    row.store_path_hash = Set(format!("{i:032}"));
    nar_info::Entity::insert(row).exec(&*db).await.unwrap();
  }
  let names = |paths: Vec<String>| {
    paths
      .into_iter()
      .map(|p| p[33..].to_string())
      .collect::<Vec<_>>()
  };
  for (query, expected) in [("_", "a_b"), ("%", "50%"), ("\\", "back\\slash")] {
    assert_eq!(
      names(search(&server, cache, query, "StorePath", "Asc").await),
      [expected],
      "{query}"
    );
  }
}
