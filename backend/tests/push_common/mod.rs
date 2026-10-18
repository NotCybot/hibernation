//! Helpers shared by the push / cache management / eviction / cleanup / dedupe tests.
#![allow(dead_code)]

use std::{path::Path, time::SystemTime};

use chrono::{NaiveDateTime, Utc};
use entity::{
  cache_access, group_user, nar, nar_info, nar_upload, sea_orm_active_enums::AccessType,
};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use sea_orm::{ActiveValue::Set, prelude::*};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::{JWT_COOKIE_NAME, TestNar, TestServer, unique};

/// A non-admin user (no groups, no permissions) with its session cookie.
pub struct User {
  pub id: Uuid,
  pub cookie: String,
}

impl User {
  /// Attach this user's session to a raw request.
  pub fn auth(&self, req: RequestBuilder) -> RequestBuilder {
    req.header("Cookie", format!("{JWT_COOKIE_NAME}={}", self.cookie))
  }
}

/// Create a user as the logged-in admin and log it in without touching the server's cookie jar.
pub async fn create_user(server: &TestServer) -> User {
  let name = unique("user");
  let email = format!("{name}@example.com");
  let password = server.encrypt_password("userpass123").await;
  let resp = server
    .post(
      "/user/management",
      json!({ "name": name, "email": email, "password": password }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "create user");
  let id: Value = resp.json().await.unwrap();
  let id = Uuid::parse_str(id["uuid"].as_str().unwrap()).unwrap();

  let password = server.encrypt_password("userpass123").await;
  let resp = server
    .raw(Method::POST, "/auth/password")
    .json(&json!({ "email": email, "password": password }))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK, "user login");
  let cookie = resp
    .headers()
    .get_all("set-cookie")
    .iter()
    .filter_map(|v| v.to_str().ok())
    .find_map(|v| v.strip_prefix(&format!("{JWT_COOKIE_NAME}=")))
    .and_then(|v| v.split(';').next())
    .expect("jwt cookie")
    .to_string();
  User { id, cookie }
}

/// A CLI token owned by `user`.
pub async fn user_token(server: &TestServer, user: &User) -> String {
  let exp = Utc::now() + chrono::Duration::days(1);
  let resp = user
    .auth(server.raw(Method::POST, "/token"))
    .json(&json!({ "name": unique("token"), "exp": exp }))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK, "create user token");
  let body: Value = resp.json().await.unwrap();
  body["token"].as_str().unwrap().to_string()
}

pub async fn grant_user(db: &DatabaseConnection, cache: Uuid, user: Uuid, access: AccessType) {
  cache_access::ActiveModel {
    cache_id: Set(cache),
    user_id: Set(Some(user)),
    group_id: Set(None),
    access_type: Set(access),
    ..Default::default()
  }
  .insert(db)
  .await
  .unwrap();
}

/// Create a group (as admin) containing `user` with `access` to `cache`.
pub async fn grant_group(
  server: &TestServer,
  db: &DatabaseConnection,
  cache: Uuid,
  user: Uuid,
  access: AccessType,
) {
  let resp = server
    .post("/group", json!({ "name": unique("group") }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "create group");
  let body: Value = resp.json().await.unwrap();
  let group = Uuid::parse_str(body["uuid"].as_str().unwrap()).unwrap();
  group_user::ActiveModel {
    group_id: Set(group),
    user_id: Set(user),
  }
  .insert(db)
  .await
  .unwrap();
  cache_access::ActiveModel {
    cache_id: Set(cache),
    user_id: Set(None),
    group_id: Set(Some(group)),
    access_type: Set(access),
    ..Default::default()
  }
  .insert(db)
  .await
  .unwrap();
}

// ---- push requests ----

pub async fn info(
  server: &TestServer,
  token: &str,
  cache: &str,
  paths: &[&str],
  force: bool,
) -> Response {
  server
    .raw(Method::POST, "/cache/push/info")
    .bearer_auth(token)
    .json(&json!({ "cache": cache, "paths": paths, "force": force }))
    .send()
    .await
    .unwrap()
}

pub async fn upload(server: &TestServer, token: &str, body: &Value) -> Response {
  server
    .raw(Method::POST, "/cache/push")
    .bearer_auth(token)
    .json(body)
    .send()
    .await
    .unwrap()
}

/// `POST /cache/push` for `nar`, asserting success. Returns `(upload id, nar_found)`.
pub async fn start_upload(
  server: &TestServer,
  token: &str,
  cache: Uuid,
  nar: &TestNar,
) -> (Uuid, bool) {
  let resp = upload(server, token, &nar.upload_request(cache, false)).await;
  assert_eq!(resp.status(), StatusCode::OK, "start upload");
  let body: Value = resp.json().await.unwrap();
  (
    Uuid::parse_str(body["uuid"].as_str().unwrap()).unwrap(),
    body["nar_found"].as_bool().unwrap(),
  )
}

pub async fn chunk(
  server: &TestServer,
  token: &str,
  upload: Uuid,
  part: usize,
  body: Vec<u8>,
) -> Response {
  server
    .raw(Method::POST, &format!("/cache/push/{upload}/{part}"))
    .bearer_auth(token)
    .body(body)
    .send()
    .await
    .unwrap()
}

pub async fn finish(server: &TestServer, token: &str, upload: Uuid, body: &Value) -> Response {
  server
    .raw(Method::PUT, &format!("/cache/push/{upload}"))
    .bearer_auth(token)
    .json(body)
    .send()
    .await
    .unwrap()
}

pub async fn narinfo_status(server: &TestServer, cache: &str, nar: &TestNar) -> StatusCode {
  server
    .raw(Method::GET, &format!("/nix/{cache}/{}.narinfo", nar.hash))
    .send()
    .await
    .unwrap()
    .status()
}

pub async fn nar_bytes(server: &TestServer, cache: &str, nar: &TestNar) -> (StatusCode, Vec<u8>) {
  let resp = server
    .raw(Method::GET, &format!("/nix/{cache}/{}", nar.nar_url()))
    .send()
    .await
    .unwrap();
  (resp.status(), resp.bytes().await.unwrap().to_vec())
}

// ---- db ----

pub async fn nar_infos(db: &DatabaseConnection, cache: Uuid) -> Vec<nar_info::Model> {
  nar_info::Entity::find()
    .filter(nar_info::Column::CacheId.eq(cache))
    .all(db)
    .await
    .unwrap()
}

pub async fn nar_info_of(db: &DatabaseConnection, cache: Uuid, nar: &TestNar) -> nar_info::Model {
  nar_info::Entity::find()
    .filter(nar_info::Column::CacheId.eq(cache))
    .filter(nar_info::Column::StorePath.eq(nar.store_path.clone()))
    .one(db)
    .await
    .unwrap()
    .expect("nar_info row")
}

pub async fn nars(db: &DatabaseConnection) -> Vec<nar::Model> {
  nar::Entity::find().all(db).await.unwrap()
}

pub async fn uploads(db: &DatabaseConnection) -> Vec<nar_upload::Model> {
  nar_upload::Entity::find().all(db).await.unwrap()
}

pub fn ago(secs: i64) -> NaiveDateTime {
  (Utc::now() - chrono::Duration::seconds(secs)).naive_utc()
}

/// Make every nar look older than the orphan cleanup cut off (10 min).
pub async fn age_nars(db: &DatabaseConnection) {
  nar::Entity::update_many()
    .col_expr(nar::Column::CreatedAt, Expr::value(ago(3600)))
    .exec(db)
    .await
    .unwrap();
}

/// Make every upload look stale (> 1 h without activity).
pub async fn age_uploads(db: &DatabaseConnection) {
  nar_upload::Entity::update_many()
    .col_expr(nar_upload::Column::UpdatedAt, Expr::value(ago(7200)))
    .exec(db)
    .await
    .unwrap();
}

// ---- storage ----

/// Every file below the local storage dir as a `/`-separated relative path, sorted (local fs only, see
/// `TestServer::storage_files` for an S3-aware async version).
pub fn storage_files(server: &TestServer) -> Vec<String> {
  fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
      return;
    };
    for entry in entries.flatten() {
      let path = entry.path();
      if path.is_dir() {
        walk(root, &path, out);
      } else {
        out.push(
          path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/"),
        );
      }
    }
  }
  let root = server.storage_path();
  let mut out = Vec::new();
  walk(&root, &root, &mut out);
  out.sort();
  out
}

/// `{id}.nar` files in the storage.
pub fn nar_files(server: &TestServer) -> Vec<String> {
  storage_files(server)
    .into_iter()
    .filter(|f| f.ends_with(".nar") && !f.contains('/'))
    .collect()
}

/// Write `contents` to `rel` below the storage dir with an mtime `secs` in the past.
pub fn write_file(server: &TestServer, rel: &str, contents: &[u8], secs: u64) {
  let path = server.storage_path().join(rel);
  std::fs::create_dir_all(path.parent().unwrap()).unwrap();
  std::fs::write(&path, contents).unwrap();
  let file = std::fs::File::options().write(true).open(&path).unwrap();
  file
    .set_modified(SystemTime::now() - std::time::Duration::from_secs(secs))
    .unwrap();
}
