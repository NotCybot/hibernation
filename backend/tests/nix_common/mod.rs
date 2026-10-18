//! Helpers shared by the `nix_*` integration tests (users, groups, sessions, raw nix requests).
#![allow(dead_code)]

use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::{ADMIN_EMAIL, ADMIN_PASSWORD, JWT_COOKIE_NAME, TestServer, unique};

pub const USER_PASSWORD: &str = "userpass123";

/// A non admin user plus its credentials.
pub struct User {
  pub id: Uuid,
  pub name: String,
  pub email: String,
  /// JWT session cookie value
  pub jwt: String,
  /// 32 char CLI token
  pub token: String,
}

/// Create a user (no groups) as the admin, log in as it to grab a JWT and a CLI token, then log back in as admin.
pub async fn create_user(server: &TestServer) -> User {
  let name = unique("user");
  let email = format!("{name}@example.com");
  let password = server.encrypt_password(USER_PASSWORD).await;
  let resp = server
    .post(
      "/user/management",
      json!({ "name": name, "email": email, "password": password }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "create user");
  let id = Uuid::parse_str(
    resp.json::<Value>().await.unwrap()["uuid"]
      .as_str()
      .unwrap(),
  )
  .unwrap();

  server.clear_cookies();
  assert_eq!(
    server.login(&email, USER_PASSWORD).await.status(),
    StatusCode::OK
  );
  let jwt = server.cookie(JWT_COOKIE_NAME).expect("user jwt");
  let (token, _) = server.create_cli_token(&unique("token")).await;
  relogin_admin(server).await;

  User {
    id,
    name,
    email,
    jwt,
    token,
  }
}

pub async fn relogin_admin(server: &TestServer) {
  server.clear_cookies();
  assert_eq!(
    server.login(ADMIN_EMAIL, ADMIN_PASSWORD).await.status(),
    StatusCode::OK
  );
}

pub fn mapping(cache: Uuid, cache_name: &str, access: &str) -> Value {
  json!({ "uuid": cache, "name": cache_name, "access_type": access })
}

/// `PUT /user/management` keeping the user name, setting groups and cache mappings.
pub async fn edit_user(server: &TestServer, user: &User, groups: &[Uuid], caches: Vec<Value>) {
  let resp = server
    .put(
      "/user/management",
      json!({ "uuid": user.id, "name": user.name, "groups": groups, "caches": caches }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "edit user");
}

pub async fn create_group(server: &TestServer) -> Uuid {
  let resp = server
    .post("/group", json!({ "name": unique("group") }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "create group");
  Uuid::parse_str(
    resp.json::<Value>().await.unwrap()["uuid"]
      .as_str()
      .unwrap(),
  )
  .unwrap()
}

/// `PUT /group` replacing permissions, members and cache mappings.
pub async fn edit_group(
  server: &TestServer,
  group: Uuid,
  permissions: &[&str],
  users: &[Uuid],
  caches: Vec<Value>,
) {
  let resp = server
    .put(
      "/group",
      json!({
        "uuid": group,
        "name": format!("g-{group}"),
        "permissions": permissions,
        "users": users,
        "caches": caches,
      }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "edit group");
}

/// A cookie-less request to a nix endpoint.
pub fn nix(server: &TestServer, method: Method, cache: &str, path: &str) -> RequestBuilder {
  server.raw(method, &format!("/nix/{cache}/{path}"))
}

pub async fn get_status(req: RequestBuilder) -> StatusCode {
  req.send().await.unwrap().status()
}

pub async fn send(req: RequestBuilder) -> Response {
  req.send().await.unwrap()
}

pub fn header<'a>(resp: &'a Response, name: &str) -> Option<&'a str> {
  resp.headers().get(name).map(|v| v.to_str().unwrap())
}
