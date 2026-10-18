//! Full-stack tests on a real Postgres (testcontainers) instead of sqlite, covering every query whose SQL or typing
//! differs between the backends: centaurus `init_db` + all migrations, the `access_type` / `eviction_policy` enums,
//! the `cache_access` CHECK constraint, the unique narinfo index, `UPDATE .. RETURNING` nar reuse, the `VALUES` join
//! of `/push/info`, `BIGINT` casts of `SUM` / `COUNT`, the eviction window function, `lower()` lookups, `LIKE ..
//! ESCAPE` search, `NOT IN` orphan cleanup, the dedupe `GROUP BY .. HAVING`, FK cascades and `timestamp` round trips.
//!
//! Each test boots its own container (~7 s), so tests group related assertions per area.
mod auth_common;
mod common;
mod nix_common;
mod push_common;

use backend::{
  cache::{
    cleanup::{run_cleanup, run_sweep},
    dedupe::run_dedupe,
  },
  db::DBTrait,
};
use chrono::{Duration, Utc};
use common::{
  ADMIN_EMAIL, ADMIN_PASSWORD, CacheOptions, JWT_COOKIE_NAME, TEST_PUBLIC_KEY, TestNar, TestServer,
  mock_downstream,
};
use entity::{
  cache, cache_access, nar, nar_info, nar_info_reference, nar_upload,
  sea_orm_active_enums::{AccessType, EvictionPolicy},
  token,
};
use migration::MigratorTrait;
use nix_common::{create_group, create_user, edit_group, edit_user, header, mapping, nix, send};
use push_common::{
  age_nars, age_uploads, ago, chunk, finish, info, nar_bytes, nar_files, nar_info_of, nar_infos,
  narinfo_status, nars, start_upload, storage_files, uploads, write_file,
};
use reqwest::{Method, RequestBuilder, StatusCode};
use sea_orm::{ActiveValue::Set, ConnectionTrait, IntoActiveModel, QueryOrder, prelude::*};
use serde_json::{Value, json};
use uuid::Uuid;

/// A Postgres server with the admin logged in, a public `main` cache and an admin CLI token.
async fn pg_main() -> (TestServer, Uuid, String) {
  let (server, _) = TestServer::with_admin(TestServer::start_pg().await).await;
  assert!(server.is_pg());
  let cache = server.create_cache("main", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  (server, cache, token)
}

async fn details(server: &TestServer, cache: Uuid) -> Value {
  let resp = server.get(&format!("/cache/management/{cache}")).await;
  assert_eq!(resp.status(), StatusCode::OK);
  resp.json().await.unwrap()
}

async fn edit_cache(server: &TestServer, cache: Uuid, name: &str, opts: &CacheOptions) {
  let resp = server
    .post(&format!("/cache/management/{cache}"), opts.edit_body(name))
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "edit cache {name}");
}

async fn search(server: &TestServer, cache: Uuid, query: &str) -> Vec<String> {
  let resp = server
    .post(
      &format!("/cache/management/{cache}/search"),
      json!({ "query": query, "sort": "StorePath", "order": "Asc" }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let rows: Vec<Value> = resp.json().await.unwrap();
  rows
    .iter()
    .map(|r| r["store_path"].as_str().unwrap().to_string())
    .collect()
}

/// A copy of `template`'s narinfo row under a different store path (same nar).
async fn clone_row(db: &DatabaseConnection, template: &nar_info::Model, i: usize, name: &str) {
  let mut row = template.clone().into_active_model();
  row.id = Set(Uuid::now_v7());
  row.store_path = Set(format!("{i:032}-{name}"));
  row.store_path_hash = Set(format!("{i:032}"));
  nar_info::Entity::insert(row).exec(db).await.unwrap();
}

fn expected_narinfo(nar: &TestNar) -> String {
  let mut references = nar.references.clone();
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

async fn pg_type_exists(db: &DatabaseConnection, name: &str) -> bool {
  db.query_one_raw(sea_orm::Statement::from_sql_and_values(
    sea_orm::DatabaseBackend::Postgres,
    "SELECT 1 FROM pg_type WHERE typname = $1",
    [name.into()],
  ))
  .await
  .unwrap()
  .is_some()
}

// ---- boot / setup / login ----

#[tokio::test]
async fn pg_boot_runs_migrations_setup_and_login() {
  let server = TestServer::start_pg().await;
  let body: Value = server.get("/setup").await.json().await.unwrap();
  assert_eq!(body["is_setup"], false);
  assert_eq!(body["db_backend"], "PostgreSQL");

  // centaurus init_db ran every migration, including the postgres enum types
  let db = server.db().await;
  let applied = migration::Migrator::get_applied_migrations(&*db)
    .await
    .unwrap();
  assert_eq!(applied.len(), migration::Migrator::migrations().len());
  for ty in ["access_type", "eviction_policy"] {
    assert!(pg_type_exists(&db, ty).await, "{ty}");
  }

  let (server, admin) = TestServer::with_admin(server).await;
  let body: Value = server.get("/setup").await.json().await.unwrap();
  assert_eq!(body["is_setup"], true);
  server.clear_cookies();
  let resp = server.login(ADMIN_EMAIL, ADMIN_PASSWORD).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert!(server.has_cookie(JWT_COOKIE_NAME));
  let info: Value = server.get("/user/info").await.json().await.unwrap();
  assert_eq!(info["uuid"], admin.to_string());
  let mut perms: Vec<String> = serde_json::from_value(info["permissions"].clone()).unwrap();
  perms.sort();
  let mut expected: Vec<String> = backend::utils::permissions()
    .iter()
    .map(|p| String::from(*p))
    .collect();
  expected.sort();
  assert_eq!(perms, expected);
  let groups: Value = server.get("/group").await.json().await.unwrap();
  assert_eq!(groups["groups"][0]["name"], "Admin");
  // wrong password stays rejected on postgres
  server.clear_cookies();
  assert_eq!(
    server.login(ADMIN_EMAIL, "wrongpass").await.status(),
    StatusCode::UNAUTHORIZED
  );
}

#[tokio::test]
async fn pg_admin_group_name_is_configurable() {
  let server = TestServer::start_backends(true, false, || unsafe {
    std::env::set_var("ADMIN_GROUP", "Wizards")
  })
  .await;
  let (server, _) = TestServer::with_admin(server).await;
  let groups: Value = server.get("/group").await.json().await.unwrap();
  assert_eq!(groups["groups"].as_array().unwrap().len(), 1);
  assert_eq!(groups["groups"][0]["name"], "Wizards");
  let info: Value = server.get("/user/info").await.json().await.unwrap();
  assert_eq!(
    info["permissions"].as_array().unwrap().len(),
    backend::utils::permissions().len()
  );
}

// ---- caches, enums, constraints ----

#[tokio::test]
async fn pg_cache_settings_and_access_enums_round_trip() {
  let (server, main, _) = pg_main().await;
  let db = server.db().await;
  let downstream = mock_downstream().await;

  // every eviction policy survives the enum round trip, together with the other settings
  for (policy, variant, public) in [
    ("OldestFirst", EvictionPolicy::OldestFirst, false),
    (
      "LeastFrequentlyUsed",
      EvictionPolicy::LeastFrequentlyUsed,
      true,
    ),
    (
      "LeastRecentlyUsed",
      EvictionPolicy::LeastRecentlyUsed,
      false,
    ),
  ] {
    let opts = CacheOptions {
      eviction_policy: policy,
      public,
      priority: 3,
      quota: 4242,
      allow_force_push: !public,
      downstream_caches: vec![downstream.uri(), "https://b.example".into()],
      ..Default::default()
    };
    edit_cache(&server, main, "main", &opts).await;
    let d = details(&server, main).await;
    assert_eq!(d["eviction_policy"], policy);
    assert_eq!(d["public"], public);
    assert_eq!(d["allow_force_push"], !public);
    assert_eq!(d["priority"], 3);
    assert_eq!(d["quota"], 4242);
    let mut expected = vec![
      format!("{}/", downstream.uri()),
      "https://b.example/".to_string(),
    ];
    expected.sort();
    assert_eq!(d["downstream_caches"], json!(expected));
    let row = cache::Entity::find_by_id(main)
      .one(&*db)
      .await
      .unwrap()
      .unwrap();
    assert_eq!(row.eviction_policy, variant);
  }

  // create defaults (enum literal inserted in a transaction with its access row and downstream)
  let resp = server
    .post(
      "/cache/management",
      json!({ "name": "fresh", "public": true, "quota": 2, "sig_key": TEST_PUBLIC_KEY }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let fresh = Uuid::parse_str(
    resp.json::<Value>().await.unwrap()["uuid"]
      .as_str()
      .unwrap(),
  )
  .unwrap();
  let d = details(&server, fresh).await;
  assert_eq!(d["eviction_policy"], "LeastRecentlyUsed");
  assert_eq!(d["quota"], 2 * 1024 * 1024);
  assert_eq!(d["downstream_caches"], json!(["https://cache.nixos.org/"]));
  assert_eq!(d["has_write_access"], true);

  // invalid key, name clashes (case-insensitive via lower())
  let bad_key = json!({ "name": "badkey", "public": true, "quota": 1, "sig_key": "not-a-key" });
  assert_eq!(
    server.post("/cache/management", bad_key).await.status(),
    StatusCode::NOT_ACCEPTABLE
  );
  let mut bad_edit = CacheOptions::default().edit_body("main");
  bad_edit["sig_key"] = json!("x");
  assert_eq!(
    server
      .post(&format!("/cache/management/{main}"), bad_edit)
      .await
      .status(),
    StatusCode::NOT_ACCEPTABLE
  );
  let dup = json!({ "name": "MAIN", "public": true, "quota": 1, "sig_key": TEST_PUBLIC_KEY });
  assert_eq!(
    server.post("/cache/management", dup).await.status(),
    StatusCode::CONFLICT
  );
  let mut rename = CacheOptions::default().edit_body("Fresh");
  rename["name"] = json!("Main");
  assert_eq!(
    server
      .post(&format!("/cache/management/{fresh}"), rename)
      .await
      .status(),
    StatusCode::CONFLICT
  );

  // cache_access with every access type, for a user and for a group
  let private = CacheOptions {
    public: false,
    ..Default::default()
  };
  let secret = server.create_cache("secret", &private).await;
  let user = create_user(&server).await;
  edit_user(
    &server,
    &user,
    &[],
    vec![
      mapping(main, "main", "Edit"),
      mapping(secret, "secret", "View"),
    ],
  )
  .await;
  let info: Value = server
    .get(&format!("/user/management/{}", user.id))
    .await
    .json()
    .await
    .unwrap();
  let mut caches = info["caches"].as_array().unwrap().clone();
  caches.sort_by_key(|c| c["name"].as_str().unwrap().to_string());
  assert_eq!(
    json!(caches),
    json!([
      mapping(main, "main", "Edit"),
      mapping(secret, "secret", "View")
    ])
  );
  let mut rows = cache_access::Entity::find()
    .filter(cache_access::Column::UserId.eq(user.id))
    .order_by_asc(cache_access::Column::CacheId)
    .all(&*db)
    .await
    .unwrap()
    .into_iter()
    .map(|r| (r.cache_id, r.access_type))
    .collect::<Vec<_>>();
  rows.sort_by_key(|r| r.0);
  let mut expected = vec![(main, AccessType::Edit), (secret, AccessType::View)];
  expected.sort_by_key(|r| r.0);
  assert_eq!(rows, expected);

  let as_user = |cache: Uuid| {
    server
      .raw(Method::GET, &format!("/cache/management/{cache}"))
      .bearer_auth(&user.jwt)
  };
  let d: Value = send(as_user(main)).await.json().await.unwrap();
  assert_eq!(d["has_write_access"], true);
  let d: Value = send(as_user(secret)).await.json().await.unwrap();
  assert_eq!(d["has_write_access"], false);
  let hidden = server.create_cache("hidden", &private).await;
  assert_eq!(send(as_user(hidden)).await.status(), StatusCode::NOT_FOUND);

  let member = create_user(&server).await;
  let group = create_group(&server).await;
  edit_group(
    &server,
    group,
    &[],
    &[member.id],
    vec![
      mapping(hidden, "hidden", "Edit"),
      mapping(secret, "secret", "View"),
    ],
  )
  .await;
  let g: Value = server
    .get(&format!("/group/{group}"))
    .await
    .json()
    .await
    .unwrap();
  let mut caches = g["group"]["caches"].as_array().unwrap().clone();
  caches.sort_by_key(|c| c["name"].as_str().unwrap().to_string());
  assert_eq!(
    json!(caches),
    json!([
      mapping(hidden, "hidden", "Edit"),
      mapping(secret, "secret", "View")
    ])
  );
  let resp = send(
    server
      .raw(Method::GET, &format!("/cache/management/{hidden}"))
      .bearer_auth(&member.jwt),
  )
  .await;
  let d: Value = resp.json().await.unwrap();
  assert_eq!(d["has_write_access"], true);
  // group edit access allows pushing (by_name_filtered with the Edit enum filter)
  let resp = info_req(&server, &member.token, "HIDDEN", &[]).await;
  assert_eq!(resp, StatusCode::NO_CONTENT);
  let resp = info_req(&server, &member.token, "secret", &[]).await;
  assert_eq!(resp, StatusCode::NOT_FOUND, "view access can not push");

  // CHECK constraint: exactly one of user/group, and the enum rejects unknown labels
  let insert = |user: &str, group: &str, access: &str| {
    format!(
      "INSERT INTO cache_access (cache_id, user_id, group_id, access_type) VALUES ('{main}', {user}, {group}, '{access}')"
    )
  };
  let (u, g) = (format!("'{}'", user.id), format!("'{group}'"));
  for (sql, what) in [
    (insert("NULL", "NULL", "view"), "neither"),
    (insert(&u, &g, "view"), "both"),
  ] {
    let err = db.execute_unprepared(&sql).await.unwrap_err().to_string();
    assert!(err.contains("check constraint"), "{what}: {err}");
  }
  let err = db
    .execute_unprepared(&insert(&u, "NULL", "View"))
    .await
    .unwrap_err()
    .to_string();
  assert!(err.contains("invalid input value for enum"), "{err}");
  db.execute_unprepared(&insert(&u, "NULL", "view"))
    .await
    .unwrap();
  db.execute_unprepared(&insert("NULL", &g, "edit"))
    .await
    .unwrap();
}

async fn info_req(server: &TestServer, token: &str, cache: &str, paths: &[&str]) -> StatusCode {
  info(server, token, cache, paths, false).await.status()
}

#[tokio::test]
async fn pg_unique_store_path_and_concurrent_push() {
  let (server, cache, token) = pg_main().await;
  let db = server.db().await;
  let nar = TestNar::new("hello-1.0", b"hello");
  server.push_path(&token, "main", &nar).await;

  let err = db
    .execute_unprepared(
      "INSERT INTO nar_info (id, cache_id, nar_id, store_path, store_path_hash, compression, signature, created_at, last_accessed_at, accessed) \
       SELECT gen_random_uuid(), cache_id, nar_id, store_path, store_path_hash, compression, signature, created_at, last_accessed_at, accessed FROM nar_info",
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

  // Both uploads pass the "already in cache" check, then finish truly concurrently
  let racy = TestNar::new("racy-1.0", b"race");
  let mut ids = Vec::new();
  for _ in 0..2 {
    let (id, found) = start_upload(&server, &token, cache, &racy).await;
    assert!(!found);
    assert_eq!(
      chunk(&server, &token, id, 0, racy.file.clone())
        .await
        .status(),
      StatusCode::OK
    );
    ids.push(id);
  }
  let body = racy.finish_request();
  let (a, b) = tokio::join!(
    finish(&server, &token, ids[0], &body),
    finish(&server, &token, ids[1], &body)
  );
  let mut statuses = [a.status(), b.status()];
  statuses.sort();
  assert_eq!(statuses, [StatusCode::OK, StatusCode::BAD_REQUEST]);
  let rows = nar_info::Entity::find()
    .filter(nar_info::Column::StorePath.eq(racy.store_path.clone()))
    .all(&*db)
    .await
    .unwrap();
  assert_eq!(rows.len(), 1);
  assert_eq!(
    send(nix(
      &server,
      Method::GET,
      "main",
      &format!("{}.narinfo", racy.hash)
    ))
    .await
    .text()
    .await
    .unwrap(),
    expected_narinfo(&racy)
  );
  assert_eq!(
    nar_bytes(&server, "main", &racy).await,
    (StatusCode::OK, racy.file.clone())
  );
  assert!(uploads(&db).await.is_empty());
}

// ---- push ----

#[tokio::test]
async fn pg_push_info_nar_reuse_references_and_multipart() {
  let (server, main, token) = pg_main().await;
  let other = server.create_cache("other", &CacheOptions::default()).await;
  let db = server.db().await;
  let a = TestNar::new("a-1.0", b"shared contents");
  let b = TestNar::new("b-1.0", b"b");
  let c = TestNar::new("c-1.0", b"c");

  // the push path looks the cache up case-insensitively
  let pushed = server.push_path(&token, "MAIN", &a).await;
  assert_eq!(pushed.cache, Some(main));

  // VALUES join: only the missing paths of this cache
  let resp = info(
    &server,
    &token,
    "main",
    &[&b.store_path, &a.store_path, &c.store_path],
    false,
  )
  .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["cache"], main.to_string());
  let mut missing: Vec<String> = serde_json::from_value(body["paths"].clone()).unwrap();
  missing.sort();
  let mut expected = vec![b.store_path.clone(), c.store_path.clone()];
  expected.sort();
  assert_eq!(missing, expected);
  assert_eq!(
    info_req(&server, &token, "main", &[&a.store_path]).await,
    StatusCode::NO_CONTENT
  );
  assert_eq!(
    info_req(&server, &token, "main", &[]).await,
    StatusCode::NO_CONTENT
  );
  // present in main only: still missing in other
  let resp = info(&server, &token, "other", &[&a.store_path], false).await;
  assert_eq!(resp.status(), StatusCode::OK);

  // finish reuses the nar via UPDATE .. RETURNING, refreshing its created_at
  age_nars(&db).await;
  let (upload, found) = start_upload(&server, &token, other, &a).await;
  assert!(found);
  assert!(nars(&db).await[0].created_at < ago(60));
  assert!(uploads(&db).await.is_empty());
  assert_eq!(
    finish(&server, &token, upload, &a.finish_request())
      .await
      .status(),
    StatusCode::OK
  );
  let refreshed = nars(&db).await;
  assert_eq!(refreshed.len(), 1);
  assert!(
    refreshed[0].created_at > ago(60),
    "created_at refreshed by finish"
  );
  assert_eq!(
    nar_info_of(&db, main, &a).await.nar_id,
    nar_info_of(&db, other, &a).await.nar_id
  );
  assert_eq!(
    nar_bytes(&server, "other", &a).await,
    (StatusCode::OK, a.file.clone())
  );

  // create_path transaction: references + deriver rows
  let drv = TestNar::new("d-1.0.drv", b"drv");
  let d = TestNar::new("d-1.0", b"d")
    .references(&[&c.store_path, &b.store_path, &a.store_path])
    .deriver(&drv.store_path);
  server.push_path(&token, "main", &d).await;
  let row = nar_info_of(&db, main, &d).await;
  assert_eq!(row.deriver.as_deref(), Some(drv.store_path.as_str()));
  assert_eq!(row.signature, d.signature);
  assert_eq!(row.store_path_hash, d.hash);
  assert_eq!(row.compression, "zst");
  assert_eq!(row.accessed, 0);
  let mut refs = nar_info_reference::Entity::find()
    .filter(nar_info_reference::Column::NarInfoId.eq(row.id))
    .all(&*db)
    .await
    .unwrap()
    .into_iter()
    .map(|r| r.store_path)
    .collect::<Vec<_>>();
  refs.sort();
  let mut expected = d.references.clone();
  expected.sort();
  assert_eq!(refs, expected);
  let nar_row = nar::Entity::find_by_id(row.nar_id)
    .one(&*db)
    .await
    .unwrap()
    .unwrap();
  assert_eq!(
    (
      nar_row.hash,
      nar_row.size,
      nar_row.nar_hash,
      nar_row.nar_size
    ),
    (
      d.file_hash.clone(),
      d.file_size() as i64,
      d.nar_hash.clone(),
      d.nar_size() as i64
    )
  );

  // multi-part upload, out of order with a re-uploaded part
  let big = TestNar::new(
    "big-1.0",
    &(0..20_000u32)
      .flat_map(|i| i.wrapping_mul(2654435761).to_le_bytes())
      .collect::<Vec<_>>(),
  );
  let (upload, found) = start_upload(&server, &token, main, &big).await;
  assert!(!found);
  let parts = big.chunks(big.file.len() / 3 + 1);
  assert_eq!(parts.len(), 3);
  assert_eq!(
    chunk(&server, &token, upload, 2, vec![0xff; 17])
      .await
      .status(),
    StatusCode::OK
  );
  for (i, part) in parts.iter().enumerate().rev() {
    let resp = chunk(&server, &token, upload, i, part.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
  }
  // the chunks keep the upload row alive
  assert_eq!(uploads(&db).await.len(), 1);
  assert_eq!(
    finish(&server, &token, upload, &big.finish_request())
      .await
      .status(),
    StatusCode::OK
  );
  assert_eq!(
    nar_bytes(&server, "main", &big).await,
    (StatusCode::OK, big.file.clone())
  );
  assert!(uploads(&db).await.is_empty());
  assert_eq!(storage_files(&server), nar_files(&server), "parts removed");
  assert_eq!(nar_files(&server).len(), 3);
}

// ---- nix download ----

#[tokio::test]
async fn pg_nix_download_narinfo_nar_ranges_and_access() {
  let (server, cache, token) = pg_main().await;
  let dep_a = TestNar::new("dep-a-1.0", b"a");
  let dep_b = TestNar::new("dep-b-1.0", b"b");
  let drv = TestNar::new("hello-1.0.drv", b"drv");
  let base = TestNar::new(
    "hello-1.0",
    &(0..5_000u32)
      .flat_map(|i| i.wrapping_mul(2654435761).to_le_bytes())
      .collect::<Vec<_>>(),
  );
  let self_ref = base.store_path.clone();
  let nar = base
    .references(&[&dep_b.store_path, &self_ref, &dep_a.store_path])
    .deriver(&drv.store_path);
  server.push_path(&token, "main", &nar).await;

  // every read path resolves the cache name with lower()
  for name in ["main", "MAIN", "Main"] {
    let resp = send(nix(&server, Method::GET, name, "nix-cache-info")).await;
    assert_eq!(resp.status(), StatusCode::OK, "{name}");
    assert_eq!(
      resp.text().await.unwrap(),
      "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 40\n"
    );
    let resp = send(nix(
      &server,
      Method::GET,
      name,
      &format!("{}.narinfo", nar.hash),
    ))
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{name}");
    assert_eq!(header(&resp, "content-type"), Some("text/x-nix-narinfo"));
    assert_eq!(resp.text().await.unwrap(), expected_narinfo(&nar));
  }
  assert_eq!(
    narinfo_status(&server, "main", &dep_a).await,
    StatusCode::NOT_FOUND
  );
  assert_eq!(
    narinfo_status(&server, "nope", &nar).await,
    StatusCode::NOT_FOUND
  );

  let db = server.db().await;
  let row = || async { nar_info_of(&db, cache, &nar).await };
  let created = row().await;
  assert_eq!(created.accessed, 0);

  // HEAD: headers only, not counted
  let resp = send(nix(&server, Method::HEAD, "MAIN", &nar.nar_url())).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(
    header(&resp, "content-length"),
    Some(nar.file_size().to_string().as_str())
  );
  assert_eq!(header(&resp, "accept-ranges"), Some("bytes"));
  assert!(resp.bytes().await.unwrap().is_empty());
  assert_eq!(row().await.accessed, 0);

  tokio::time::sleep(std::time::Duration::from_millis(20)).await;
  let resp = send(nix(&server, Method::GET, "MAIN", &nar.nar_url())).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(header(&resp, "content-type"), Some("application/x-nix-nar"));
  assert_eq!(resp.bytes().await.unwrap(), nar.file);
  let first = row().await;
  assert_eq!(first.accessed, 1);
  assert!(first.last_accessed_at > created.last_accessed_at);
  assert_eq!(first.created_at, created.created_at);

  let size = nar.file_size();
  tokio::time::sleep(std::time::Duration::from_millis(20)).await;
  let resp =
    send(nix(&server, Method::GET, "main", &nar.nar_url()).header("Range", "bytes=10-19")).await;
  assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
  assert_eq!(
    header(&resp, "content-range"),
    Some(format!("bytes 10-19/{size}").as_str())
  );
  assert_eq!(resp.bytes().await.unwrap(), nar.file[10..20]);
  let resp =
    send(nix(&server, Method::GET, "main", &nar.nar_url()).header("Range", "bytes=-5")).await;
  assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
  assert_eq!(resp.bytes().await.unwrap(), nar.file[nar.file.len() - 5..]);
  let second = row().await;
  assert_eq!(second.accessed, 3, "range requests count too");
  assert!(second.last_accessed_at > first.last_accessed_at);

  let resp = send(
    nix(&server, Method::GET, "main", &nar.nar_url()).header("Range", format!("bytes={size}-")),
  )
  .await;
  assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
  assert_eq!(
    header(&resp, "content-range"),
    Some(format!("bytes */{size}").as_str())
  );
  assert_eq!(row().await.accessed, 3, "416 is not counted");
  assert_eq!(
    send(nix(&server, Method::GET, "main", &dep_a.nar_url()))
      .await
      .status(),
    StatusCode::NOT_FOUND
  );
}

#[tokio::test]
async fn pg_private_cache_auth() {
  let (server, _) = TestServer::with_admin(TestServer::start_pg().await).await;
  let cache = server
    .create_cache(
      "private",
      &CacheOptions {
        public: false,
        ..Default::default()
      },
    )
    .await;
  let (token, _) = server.create_cli_token("ci").await;
  let nar = TestNar::new("secret-1.0", b"secret");
  server.push_path(&token, "private", &nar).await;
  let admin_jwt = server.cookie(JWT_COOKIE_NAME).unwrap();

  let paths = [
    "nix-cache-info".to_string(),
    format!("{}.narinfo", nar.hash),
    nar.nar_url(),
  ];
  let check =
    async |auth: &dyn Fn(RequestBuilder) -> RequestBuilder, expected: StatusCode, what: &str| {
      for path in &paths {
        // mixed case name: the private lookup path uses lower() too
        let resp = send(auth(nix(&server, Method::GET, "Private", path))).await;
        assert_eq!(resp.status(), expected, "{what}: {path}");
      }
    };

  check(&|r| r, StatusCode::UNAUTHORIZED, "anonymous").await;
  check(
    &|r| r.bearer_auth(&token),
    StatusCode::OK,
    "admin cli token",
  )
  .await;
  check(&|r| r.bearer_auth(&admin_jwt), StatusCode::OK, "admin jwt").await;
  check(
    &|r| r.header("Cookie", format!("{JWT_COOKIE_NAME}={admin_jwt}")),
    StatusCode::OK,
    "admin jwt cookie",
  )
  .await;

  let nobody = create_user(&server).await;
  let direct = create_user(&server).await;
  let member = create_user(&server).await;
  let global = create_user(&server).await;
  edit_user(
    &server,
    &direct,
    &[],
    vec![mapping(cache, "private", "View")],
  )
  .await;
  let group = create_group(&server).await;
  edit_group(
    &server,
    group,
    &[],
    &[member.id],
    vec![mapping(cache, "private", "Edit")],
  )
  .await;
  let perm_group = create_group(&server).await;
  edit_group(&server, perm_group, &["cache:view"], &[global.id], vec![]).await;

  for (user, expected, what) in [
    (&nobody, StatusCode::FORBIDDEN, "no access"),
    (&direct, StatusCode::OK, "user view"),
    (&member, StatusCode::OK, "group edit"),
    (&global, StatusCode::OK, "cache:view"),
  ] {
    check(&|r| r.bearer_auth(&user.token), expected, what).await;
    check(&|r| r.bearer_auth(&user.jwt), expected, what).await;
  }
}

// ---- search ----

#[tokio::test]
async fn pg_search_is_case_insensitive_and_escapes_like() {
  let (server, cache, token) = pg_main().await;
  let alpha = TestNar::new("Alpha-Upper-1.0", b"a");
  server.push_path(&token, "main", &alpha).await;
  let db = server.db().await;
  let template = nar_info_of(&db, cache, &alpha).await;
  let names = [
    "a_b",
    "axb",
    "50%",
    "500",
    "back\\slash",
    "backxslash",
    "MiXeD-Case",
  ];
  for (i, name) in names.iter().enumerate() {
    clone_row(&db, &template, i, name).await;
  }
  let found = |paths: Vec<String>| {
    paths
      .into_iter()
      .map(|p| p[33..].to_string())
      .collect::<Vec<_>>()
  };

  // Postgres LIKE is case-sensitive, lower() on both sides fixes that
  for query in ["alpha", "ALPHA", "  aLpHa-upper "] {
    assert_eq!(
      found(search(&server, cache, query).await),
      ["Alpha-Upper-1.0"],
      "{query}"
    );
  }
  assert_eq!(
    found(search(&server, cache, "mixed-case").await),
    ["MiXeD-Case"]
  );
  for (query, expected) in [("_", "a_b"), ("%", "50%"), ("\\", "back\\slash")] {
    assert_eq!(
      found(search(&server, cache, query).await),
      [expected],
      "{query}"
    );
  }
  assert_eq!(search(&server, cache, "").await.len(), names.len() + 1);
  assert!(search(&server, cache, "nothing").await.is_empty());

  // the other sort columns work on postgres too
  for sort in ["Created", "Accessed", "Size", "AccessCount"] {
    for order in ["Asc", "Desc"] {
      let resp = server
        .post(
          &format!("/cache/management/{cache}/search"),
          json!({ "query": "", "sort": sort, "order": order }),
        )
        .await;
      assert_eq!(resp.status(), StatusCode::OK, "{sort} {order}");
      let rows: Vec<Value> = resp.json().await.unwrap();
      assert_eq!(rows.len(), names.len() + 1);
      assert_eq!(rows[0]["size"], alpha.file_size());
    }
  }
}

// ---- eviction ----

/// Quota-limited cache `name` with `policy` holding a, b, c with `(created ago, accessed ago, accessed)` stats.
/// Pushing d (bigger than any of them) then has to free `overflow(sizes of a, b, c)` bytes.
async fn evict_scenario(
  server: &TestServer,
  token: &str,
  name: &str,
  policy: &'static str,
  stats: [(i64, i64, i64); 3],
  overflow: impl Fn([i64; 3]) -> i64,
) -> [bool; 3] {
  let db = server.db().await;
  let opts = CacheOptions {
    eviction_policy: policy,
    ..Default::default()
  };
  let cache = server.create_cache(name, &opts).await;
  let now = ago(0);
  let nars: Vec<TestNar> = ["a", "b", "c", "d"]
    .iter()
    .enumerate()
    .map(|(i, n)| {
      let contents: Vec<u8> = if i == 3 {
        (0..4096u32)
          .map(|j| (j.wrapping_mul(2654435761) >> 13) as u8)
          .collect()
      } else {
        vec![i as u8; 4096]
      };
      TestNar::new(&format!("{name}-{n}-1.0"), &contents)
    })
    .collect();
  for (n, (created, accessed_at, accessed)) in nars.iter().zip(stats) {
    server.push_path(token, name, n).await;
    let mut row = nar_info_of(&db, cache, n).await.into_active_model();
    row.created_at = Set(now - Duration::seconds(created));
    row.last_accessed_at = Set(now - Duration::seconds(accessed_at));
    row.accessed = Set(accessed);
    row.update(&*db).await.unwrap();
  }
  let total: i64 = nars.iter().map(|n| n.file_size() as i64).sum();
  edit_cache(
    server,
    cache,
    name,
    &CacheOptions {
      quota: total - overflow(std::array::from_fn(|i| nars[i].file_size() as i64)),
      ..opts
    },
  )
  .await;
  assert_eq!(
    nar_infos(&db, cache).await.len(),
    3,
    "{name}: edit must not evict"
  );
  server.push_path(token, name, &nars[3]).await;
  assert_eq!(narinfo_status(server, name, &nars[3]).await, StatusCode::OK);
  let mut present = [false; 3];
  for (i, n) in nars[..3].iter().enumerate() {
    present[i] = narinfo_status(server, name, n).await == StatusCode::OK;
  }
  present
}

#[tokio::test]
async fn pg_eviction_on_push_picks_exact_victims() {
  let (server, _, token) = pg_main().await;
  let s = &server;
  let t = &token;

  // LRU: b least recently accessed
  assert_eq!(
    evict_scenario(
      s,
      t,
      "lru",
      "LeastRecentlyUsed",
      [(100, 10, 0), (50, 1000, 0), (10, 100, 0)],
      |_| 1
    )
    .await,
    [true, false, true]
  );
  // LRU tie on last access: oldest created goes
  assert_eq!(
    evict_scenario(
      s,
      t,
      "lrutie",
      "LeastRecentlyUsed",
      [(50, 500, 0), (100, 500, 0), (10, 500, 0)],
      |_| 1
    )
    .await,
    [true, false, true]
  );
  // LFU: b least frequently accessed
  assert_eq!(
    evict_scenario(
      s,
      t,
      "lfu",
      "LeastFrequentlyUsed",
      [(100, 1000, 5), (50, 10, 1), (10, 10, 3)],
      |_| 1
    )
    .await,
    [true, false, true]
  );
  // OldestFirst: c oldest created even though most used
  assert_eq!(
    evict_scenario(
      s,
      t,
      "oldest",
      "OldestFirst",
      [(50, 10_000, 0), (10, 10_000, 0), (100, 1, 100)],
      |_| 1
    )
    .await,
    [true, true, false]
  );
  // one byte more than the first victim frees: two victims
  assert_eq!(
    evict_scenario(
      s,
      t,
      "multi",
      "OldestFirst",
      [(300, 0, 0), (200, 0, 0), (100, 0, 0)],
      |s| s[0] + 1
    )
    .await,
    [false, false, true]
  );
  // exactly full: nothing goes
  assert_eq!(
    evict_scenario(
      s,
      t,
      "full",
      "OldestFirst",
      [(3, 0, 0), (2, 0, 0), (1, 0, 0)],
      |_| 0
    )
    .await,
    [true, true, true]
  );

  // a file larger than the whole quota is rejected (SUM(..)::BIGINT cache size check)
  let tiny = s
    .create_cache(
      "tiny",
      &CacheOptions {
        quota: 10,
        ..Default::default()
      },
    )
    .await;
  let big = TestNar::new("too-big-1.0", &[9; 4096]);
  let (upload, _) = start_upload(s, t, tiny, &big).await;
  assert_eq!(
    chunk(s, t, upload, 0, big.file.clone()).await.status(),
    StatusCode::BAD_REQUEST
  );
}

#[tokio::test]
async fn pg_quota_edit_evicts_with_new_policy_and_keeps_shared_nar() {
  let (server, cache, token) = pg_main().await;
  let other = server.create_cache("other", &CacheOptions::default()).await;
  let db = server.db().await;
  let a = TestNar::new("a-1.0", &[1; 4096]);
  let b = TestNar::new("b-1.0", &[2; 4096]);
  server.push_path(&token, "main", &a).await;
  server.push_path(&token, "main", &b).await;
  assert!(server.push_path(&token, "other", &a).await.nar_found);
  let now = ago(0);
  for (n, created, accessed_at) in [(&a, 1000, 1), (&b, 10, 1000)] {
    let mut row = nar_info_of(&db, cache, n).await.into_active_model();
    row.created_at = Set(now - Duration::seconds(created));
    row.last_accessed_at = Set(now - Duration::seconds(accessed_at));
    row.update(&*db).await.unwrap();
  }
  let quota = |q: i64, policy: &'static str| CacheOptions {
    quota: q,
    eviction_policy: policy,
    ..Default::default()
  };

  // stored policy is LRU (would evict b), the edit switches to OldestFirst and evicts a
  let total = (a.file_size() + b.file_size()) as i64;
  edit_cache(&server, cache, "main", &quota(total - 1, "OldestFirst")).await;
  assert_eq!(
    details(&server, cache).await["eviction_policy"],
    "OldestFirst"
  );
  assert_eq!(
    narinfo_status(&server, "main", &a).await,
    StatusCode::NOT_FOUND
  );
  assert_eq!(narinfo_status(&server, "main", &b).await, StatusCode::OK);
  edit_cache(&server, cache, "main", &quota(0, "LeastFrequentlyUsed")).await;
  assert!(nar_infos(&db, cache).await.is_empty());
  assert_eq!(details(&server, cache).await["size"], 0);

  // a's nar is still referenced by other and survives the orphan cleanup, b's goes
  age_nars(&db).await;
  run_cleanup(&db, &server.file_storage().await).await;
  assert_eq!(nars(&db).await.len(), 1);
  assert_eq!(nar_files(&server).len(), 1);
  assert_eq!(
    nar_bytes(&server, "other", &a).await,
    (StatusCode::OK, a.file.clone())
  );
  assert_eq!(details(&server, other).await["size"], a.file_size());
}

// ---- list / details sizes ----

#[tokio::test]
async fn pg_sizes_and_counts_are_not_multiplied_by_access_rows() {
  let (server, cache, token) = pg_main().await;
  let db = server.db().await;
  let empty = server.create_cache("empty", &CacheOptions::default()).await;
  let a = TestNar::new("a-1.0", b"a");
  let b = TestNar::new("b-1.0", b"bb");
  server.push_path(&token, "main", &a).await;
  server.push_path(&token, "main", &b).await;
  for _ in 0..3 {
    let user = create_user(&server).await;
    edit_user(&server, &user, &[], vec![mapping(cache, "main", "View")]).await;
  }
  let viewer = create_user(&server).await;
  let group = create_group(&server).await;
  edit_group(
    &server,
    group,
    &[],
    &[viewer.id],
    vec![
      mapping(cache, "main", "Edit"),
      mapping(empty, "empty", "View"),
    ],
  )
  .await;
  let size = a.file_size() + b.file_size();

  let check = |list: Value| {
    let list = list.as_array().unwrap().clone();
    assert_eq!(list.len(), 2, "{list:?}");
    let main = list
      .iter()
      .find(|c| c["uuid"] == cache.to_string())
      .unwrap();
    assert_eq!(main["size"], size);
    let e = list
      .iter()
      .find(|c| c["uuid"] == empty.to_string())
      .unwrap();
    assert_eq!(e["size"], 0, "NULL sum is reported as 0");
  };
  check(server.get("/cache/management").await.json().await.unwrap());
  // a user only seeing caches through access rows
  check(
    send(
      server
        .raw(Method::GET, "/cache/management")
        .bearer_auth(&viewer.jwt),
    )
    .await
    .json()
    .await
    .unwrap(),
  );

  let d = details(&server, cache).await;
  assert_eq!(d["size"], size);
  assert_eq!(d["nar_count"], 2);
  let d = details(&server, empty).await;
  assert_eq!(
    (d["size"].clone(), d["nar_count"].clone()),
    (json!(0), json!(0))
  );
  let d: Value = send(
    server
      .raw(Method::GET, &format!("/cache/management/{cache}"))
      .bearer_auth(&viewer.jwt),
  )
  .await
  .json()
  .await
  .unwrap();
  assert_eq!(d["size"], size);
  assert_eq!(d["nar_count"], 2);
  assert_eq!(d["has_write_access"], true);
  assert_eq!(
    db.cache().cache_size(cache).await.unwrap(),
    Some(size as i64)
  );
  assert_eq!(db.cache().cache_size(empty).await.unwrap(), Some(0));

  // Regression: the simple lists joined cache_access without DISTINCT and repeated a cache per access row
  for path in ["/user/management/caches", "/group/caches"] {
    let simple: Value = server.get(path).await.json().await.unwrap();
    let mut names: Vec<&str> = simple
      .as_array()
      .unwrap()
      .iter()
      .map(|c| c["name"].as_str().unwrap())
      .collect();
    names.sort();
    assert_eq!(names, ["empty", "main"], "{path}");
  }
}

// ---- background jobs ----

#[tokio::test]
async fn pg_cleanup_and_dedupe_jobs() {
  let (server, cache, token) = pg_main().await;
  let other = server.create_cache("other", &CacheOptions::default()).await;
  let db = server.db().await;
  let storage = server.file_storage().await;

  // stale vs fresh uploads
  let stale_nar = TestNar::new("stale-1.0", b"stale");
  let (stale, _) = start_upload(&server, &token, cache, &stale_nar).await;
  chunk(&server, &token, stale, 0, stale_nar.file.clone()).await;
  // upload rows (and part dirs) are keyed by the future nar id, not the session id
  let stale_row = uploads(&db).await[0].id;
  age_uploads(&db).await;
  let fresh_nar = TestNar::new("fresh-1.0", b"fresh");
  let (fresh, _) = start_upload(&server, &token, cache, &fresh_nar).await;
  chunk(&server, &token, fresh, 0, fresh_nar.file.clone()).await;
  let fresh_row = uploads(&db)
    .await
    .into_iter()
    .find(|u| u.id != stale_row)
    .unwrap()
    .id;
  assert_eq!(
    db.nar()
      .stale_uploads()
      .await
      .unwrap()
      .into_iter()
      .map(|u| u.id)
      .collect::<Vec<_>>(),
    [stale_row]
  );

  // orphans: a deleted path's nar goes after the grace period, a referenced one stays
  let a = TestNar::new("a-1.0", b"same");
  let gone = TestNar::new("gone-1.0", b"gone");
  server.push_path(&token, "main", &a).await;
  server.push_path(&token, "main", &gone).await;
  let gone_nar = nar_info_of(&db, cache, &gone).await.nar_id;
  db.nar().delete_path(cache, &gone.store_path).await.unwrap();
  run_cleanup(&db, &storage).await;
  assert_eq!(
    uploads(&db)
      .await
      .into_iter()
      .map(|u| u.id)
      .collect::<Vec<_>>(),
    [fresh_row]
  );
  // the young orphan survives the grace period
  assert!(nars(&db).await.iter().any(|n| n.id == gone_nar));
  let parts: Vec<String> = storage_files(&server)
    .into_iter()
    .filter(|f| f.starts_with("parts/"))
    .collect();
  assert_eq!(parts, [format!("parts/{fresh_row}/0")]);

  age_nars(&db).await;
  assert!(
    db.nar()
      .orphan_nars()
      .await
      .unwrap()
      .iter()
      .any(|n| n.id == gone_nar)
  );
  // NOT IN (narinfo nar ids): only the unreferenced nar goes
  run_cleanup(&db, &storage).await;
  let left: Vec<Uuid> = nars(&db).await.into_iter().map(|n| n.id).collect();
  assert!(!left.contains(&gone_nar));
  let kept = nar_info_of(&db, cache, &a).await.nar_id;
  assert!(left.contains(&kept));
  assert!(
    !db.nar().delete_orphan_nar(kept).await.unwrap(),
    "referenced"
  );
  assert_eq!(
    nar_bytes(&server, "main", &a).await,
    (StatusCode::OK, a.file.clone())
  );

  // dedupe: a second nar row with a's content referenced from other
  let b = TestNar::new("b-1.0", b"same");
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
  write_file(&server, &format!("{dup}.nar"), &a.file, 0);
  let mut row = nar_info_of(&db, cache, &a).await.into_active_model();
  row.id = Set(Uuid::now_v7());
  row.cache_id = Set(other);
  row.nar_id = Set(dup);
  row.store_path = Set(b.store_path.clone());
  row.store_path_hash = Set(b.hash.clone());
  row.signature = Set(b.signature.clone());
  nar_info::Entity::insert(row).exec(&*db).await.unwrap();
  // an unreferenced duplicate is ignored
  let unreferenced = Uuid::now_v7();
  nar::ActiveModel {
    id: Set(unreferenced),
    hash: Set(a.file_hash.clone()),
    size: Set(a.file_size() as i64),
    nar_hash: Set(a.nar_hash.clone()),
    nar_size: Set(a.nar_size() as i64),
    created_at: Set(ago(0)),
  }
  .insert(&*db)
  .await
  .unwrap();

  assert_eq!(
    db.nar().find_duplicate_nars().await.unwrap(),
    std::slice::from_ref(&a.nar_hash)
  );
  assert_eq!(
    db.nar().find_nars_by_nar_hash(&a.nar_hash).await.unwrap(),
    [kept, dup],
    "oldest (v7) first"
  );
  run_dedupe(&db).await;
  assert!(db.nar().find_duplicate_nars().await.unwrap().is_empty());
  assert_eq!(nar_info_of(&db, other, &b).await.nar_id, kept);

  age_nars(&db).await;
  run_cleanup(&db, &storage).await;
  let left: Vec<Uuid> = nars(&db).await.into_iter().map(|n| n.id).collect();
  assert!(left.contains(&kept));
  assert!(!left.contains(&dup) && !left.contains(&unreferenced));
  assert!(!nar_files(&server).contains(&format!("{dup}.nar")));
  assert_eq!(
    nar_bytes(&server, "other", &b).await,
    (StatusCode::OK, a.file.clone())
  );

  // the storage sweep only removes old files no row knows
  let (nar_ids, upload_ids) = db.nar().known_ids().await.unwrap();
  assert!(nar_ids.contains(&kept) && upload_ids.contains(&fresh_row));
  let stray = Uuid::now_v7();
  write_file(&server, &format!("{stray}.nar"), b"x", 7200);
  run_sweep(&db, &storage).await.unwrap();
  assert!(!storage_files(&server).contains(&format!("{stray}.nar")));
  assert!(nar_files(&server).contains(&format!("{kept}.nar")));
}

// ---- tokens / users / groups ----

#[tokio::test]
async fn pg_tokens_and_cli_login() {
  let (server, admin) = TestServer::with_admin(TestServer::start_pg().await).await;
  let cli_test = async |token: &str| {
    send(server.raw(Method::GET, "/cache/test").bearer_auth(token))
      .await
      .status()
  };

  let exp = Utc::now() + Duration::days(3);
  let (token, id) = server.create_cli_token_exp("ci", exp).await;
  assert_eq!(cli_test(&token).await, StatusCode::OK);
  let info: Value = server
    .get(&format!("/token/{id}"))
    .await
    .json()
    .await
    .unwrap();
  assert_eq!(info["name"], "ci");
  assert!(
    info["last_used"].is_string(),
    "token_used updated last_used"
  );
  assert_eq!(
    server
      .post("/token", json!({ "name": "ci", "exp": exp }))
      .await
      .status(),
    StatusCode::CONFLICT
  );

  // edit (UPDATE of name + timestamp)
  let new_exp = Utc::now() + Duration::days(10);
  let resp = server
    .put(
      "/token",
      json!({ "uuid": id, "name": "deploy", "exp": new_exp }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let info: Value = server
    .get(&format!("/token/{id}"))
    .await
    .json()
    .await
    .unwrap();
  assert_eq!(info["name"], "deploy");
  let listed: chrono::DateTime<Utc> = serde_json::from_value(info["exp"].clone()).unwrap();
  assert!((listed - new_exp).num_milliseconds().abs() < 1);

  // regenerate
  let resp = server.post(&format!("/token/{id}"), json!({})).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let new = resp.json::<Value>().await.unwrap()["token"]
    .as_str()
    .unwrap()
    .to_string();
  assert_ne!(new, token);
  assert_eq!(cli_test(&token).await, StatusCode::UNAUTHORIZED);
  assert_eq!(cli_test(&new).await, StatusCode::OK);

  // expired cleanup only deletes own expired tokens (exp < now on timestamp columns)
  let past = Utc::now() - Duration::hours(1);
  let (expired, _) = server.create_cli_token_exp("old1", past).await;
  server.create_cli_token_exp("old2", past).await;
  assert_eq!(cli_test(&expired).await, StatusCode::BAD_REQUEST);
  let other = auth_common::user_with_perms(&server, &[]).await;
  let (_, other_old) = auth_common::token_for(&server, &other, "old", past).await;
  let body: Value = server
    .delete("/token/expired", json!({}))
    .await
    .json()
    .await
    .unwrap();
  assert_eq!(body["deleted"], 2);
  let tokens: Vec<Value> = server.get("/token").await.json().await.unwrap();
  assert_eq!(tokens.len(), 1);
  assert_eq!(tokens[0]["uuid"], id.to_string());
  assert_eq!(
    other
      .get(&server, &format!("/token/{other_old}"))
      .await
      .status(),
    StatusCode::OK
  );

  // delete
  assert_eq!(
    server
      .delete("/token", json!({ "uuid": id }))
      .await
      .status(),
    StatusCode::OK
  );
  assert_eq!(cli_test(&new).await, StatusCode::UNAUTHORIZED);

  // CLI login code flow issues a token valid for the session lifetime
  let before = Utc::now();
  let resp = server.post("/cli", json!(null)).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let code = resp.json::<Value>().await.unwrap()["code"]
    .as_str()
    .unwrap()
    .to_string();
  let exchange = async || send(server.raw(Method::PUT, &format!("/cli?code={code}"))).await;
  let resp = exchange().await;
  assert_eq!(resp.status(), StatusCode::OK);
  let cli = resp.text().await.unwrap();
  assert_eq!(
    send(server.raw(Method::GET, "/cache/test").bearer_auth(&cli))
      .await
      .text()
      .await
      .unwrap(),
    admin.to_string()
  );
  assert_eq!(
    exchange().await.status(),
    StatusCode::BAD_REQUEST,
    "single use"
  );
  let tokens: Vec<Value> = server.get("/token").await.json().await.unwrap();
  assert_eq!(tokens.len(), 1);
  assert!(tokens[0]["name"].as_str().unwrap().starts_with("Cli-"));
  let exp: chrono::DateTime<Utc> = serde_json::from_value(tokens[0]["exp"].clone()).unwrap();
  assert!(exp > before + Duration::minutes(1));
}

#[tokio::test]
async fn pg_user_and_group_delete_cascade_and_revoke() {
  let (server, _) = TestServer::with_admin(TestServer::start_pg().await).await;
  let db = server.db().await;
  let private = CacheOptions {
    public: false,
    ..Default::default()
  };
  let cache = server.create_cache("private", &private).await;
  let (admin_token, _) = server.create_cli_token("ci").await;
  let nar = TestNar::new("secret-1.0", b"secret");
  server.push_path(&admin_token, "private", &nar).await;
  let narinfo = format!("{}.narinfo", nar.hash);
  let status = async |token: &str| {
    send(nix(&server, Method::GET, "private", &narinfo).bearer_auth(token))
      .await
      .status()
  };

  // user delete: cache_access + token rows cascade, the cached token stops working at once
  let user = create_user(&server).await;
  edit_user(&server, &user, &[], vec![mapping(cache, "private", "Edit")]).await;
  assert_eq!(status(&user.token).await, StatusCode::OK);
  let whoami = send(
    server
      .raw(Method::GET, "/cache/test")
      .bearer_auth(&user.token),
  )
  .await;
  assert_eq!(whoami.text().await.unwrap(), user.id.to_string());
  let count_access = async |col: cache_access::Column, id: Uuid| {
    cache_access::Entity::find()
      .filter(col.eq(id))
      .count(&*db)
      .await
      .unwrap()
  };
  assert_eq!(count_access(cache_access::Column::UserId, user.id).await, 1);
  assert_eq!(
    server
      .delete("/user/management", json!({ "uuid": user.id }))
      .await
      .status(),
    StatusCode::OK
  );
  assert_eq!(count_access(cache_access::Column::UserId, user.id).await, 0);
  assert_eq!(
    token::Entity::find()
      .filter(token::Column::UserId.eq(user.id))
      .count(&*db)
      .await
      .unwrap(),
    0
  );
  assert_eq!(
    send(
      server
        .raw(Method::GET, "/cache/test")
        .bearer_auth(&user.token)
    )
    .await
    .status(),
    StatusCode::UNAUTHORIZED
  );
  assert_eq!(status(&user.token).await, StatusCode::UNAUTHORIZED);
  assert_eq!(
    send(
      server
        .raw(Method::GET, "/cache/test")
        .bearer_auth(&user.jwt)
    )
    .await
    .status(),
    StatusCode::FORBIDDEN
  );

  // group delete: memberships + cache_access cascade, nix access is revoked at once
  let member = create_user(&server).await;
  let group = create_group(&server).await;
  edit_group(
    &server,
    group,
    &["cache:view"],
    &[member.id],
    vec![mapping(cache, "private", "View")],
  )
  .await;
  assert_eq!(status(&member.token).await, StatusCode::OK);
  assert_eq!(count_access(cache_access::Column::GroupId, group).await, 1);
  assert_eq!(
    server
      .delete("/group", json!({ "uuid": group }))
      .await
      .status(),
    StatusCode::OK
  );
  assert_eq!(count_access(cache_access::Column::GroupId, group).await, 0);
  assert_eq!(status(&member.token).await, StatusCode::FORBIDDEN);
  let info: Value = server
    .get(&format!("/user/management/{}", member.id))
    .await
    .json()
    .await
    .unwrap();
  assert_eq!(info["groups"], json!([]));
  assert_eq!(info["permissions"], json!([]));

  // deleting the cache cascades its access rows and narinfos
  assert_eq!(
    server
      .delete("/cache/management", json!({ "uuid": cache }))
      .await
      .status(),
    StatusCode::OK
  );
  assert_eq!(count_access(cache_access::Column::CacheId, cache).await, 0);
  assert!(nar_infos(&db, cache).await.is_empty());
}

// ---- timestamps ----

#[tokio::test]
async fn pg_timestamps_round_trip_without_shift() {
  let (server, cache, token) = pg_main().await;
  let db = server.db().await;

  // whole microseconds survive exactly (postgres timestamp precision), nothing is shifted by a time zone
  let exp =
    chrono::DateTime::from_timestamp_micros((Utc::now() + Duration::days(2)).timestamp_micros())
      .unwrap();
  let (_, id) = server.create_cli_token_exp("precise", exp).await;
  let info: Value = server
    .get(&format!("/token/{id}"))
    .await
    .json()
    .await
    .unwrap();
  let listed: chrono::DateTime<Utc> = serde_json::from_value(info["exp"].clone()).unwrap();
  assert_eq!(listed, exp);
  let row = db.token().by_id(id).await.unwrap();
  assert_eq!(row.exp, exp.naive_utc());

  // created_at is utc "now" and ordered by push order
  let before = ago(1);
  let a = TestNar::new("a-1.0", b"a");
  let b = TestNar::new("b-1.0", b"b");
  server.push_path(&token, "main", &a).await;
  tokio::time::sleep(std::time::Duration::from_millis(5)).await;
  server.push_path(&token, "main", &b).await;
  let after = ago(-1);
  let (ra, rb) = (
    nar_info_of(&db, cache, &a).await,
    nar_info_of(&db, cache, &b).await,
  );
  for r in [&ra, &rb] {
    assert!(r.created_at > before && r.created_at < after, "{r:?}");
    assert_eq!(r.created_at, r.last_accessed_at);
  }
  assert!(ra.created_at < rb.created_at);
  let resp = server
    .post(
      &format!("/cache/management/{cache}/search"),
      json!({ "query": "", "sort": "Created", "order": "Desc" }),
    )
    .await;
  let rows: Vec<Value> = resp.json().await.unwrap();
  assert_eq!(rows[0]["store_path"], b.store_path);
  let created: chrono::DateTime<Utc> =
    serde_json::from_value(rows[0]["created_at"].clone()).unwrap();
  assert_eq!(created.naive_utc(), rb.created_at);

  // the stale upload cut off (1 h) is applied in utc: 59 min is fresh, 61 min stale
  let upload_nar = TestNar::new("up-1.0", b"up");
  start_upload(&server, &token, cache, &upload_nar).await;
  let upload = uploads(&db).await[0].id;
  let set_updated = async |secs: i64| {
    nar_upload::Entity::update_many()
      .col_expr(nar_upload::Column::UpdatedAt, Expr::value(ago(secs)))
      .exec(&*db)
      .await
      .unwrap();
  };
  set_updated(59 * 60).await;
  assert!(db.nar().stale_uploads().await.unwrap().is_empty());
  set_updated(61 * 60).await;
  assert_eq!(db.nar().stale_uploads().await.unwrap()[0].id, upload);

  // the orphan grace period (10 min) likewise
  let orphan = Uuid::now_v7();
  nar::ActiveModel {
    id: Set(orphan),
    hash: Set("h".into()),
    size: Set(1),
    nar_hash: Set("n".into()),
    nar_size: Set(1),
    created_at: Set(ago(9 * 60)),
  }
  .insert(&*db)
  .await
  .unwrap();
  assert!(!db.nar().delete_orphan_nar(orphan).await.unwrap());
  nar::Entity::update_many()
    .col_expr(nar::Column::CreatedAt, Expr::value(ago(11 * 60)))
    .filter(nar::Column::Id.eq(orphan))
    .exec(&*db)
    .await
    .unwrap();
  assert!(db.nar().delete_orphan_nar(orphan).await.unwrap());
}
