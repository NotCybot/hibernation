//! Extra helpers for the auth/user/group/token suites: second users with chosen permissions, authenticated by a
//! bearer JWT so the admin session in the [`TestServer`] cookie jar stays untouched.
#![allow(dead_code)]

use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::{JWT_COOKIE_NAME, TestServer, unique};

pub const USER_PASSWORD: &str = "userpass123";

pub struct User {
  pub id: Uuid,
  pub email: String,
  /// Session JWT, send as bearer
  pub jwt: String,
}

impl User {
  pub fn req(&self, server: &TestServer, method: Method, path: &str) -> RequestBuilder {
    server.raw(method, path).bearer_auth(&self.jwt)
  }

  pub async fn get(&self, server: &TestServer, path: &str) -> Response {
    self.req(server, Method::GET, path).send().await.unwrap()
  }

  pub async fn send(
    &self,
    server: &TestServer,
    method: Method,
    path: &str,
    body: Value,
  ) -> Response {
    self
      .req(server, method, path)
      .json(&body)
      .send()
      .await
      .unwrap()
  }
}

/// `Set-Cookie` value of the session cookie, if the response set one.
pub fn jwt_cookie(resp: &Response) -> Option<String> {
  resp.headers().get_all("set-cookie").iter().find_map(|v| {
    let (pair, _) = v
      .to_str()
      .ok()?
      .split_once(';')
      .unwrap_or((v.to_str().ok()?, ""));
    let (name, value) = pair.split_once('=')?;
    (name.trim() == JWT_COOKIE_NAME && !value.is_empty()).then(|| value.to_string())
  })
}

/// Create a user (as the admin in the jar). Returns its id and email.
pub async fn create_user(server: &TestServer, name: &str) -> (Uuid, String) {
  let email = format!("{}@example.com", unique(name));
  let password = server.encrypt_password(USER_PASSWORD).await;
  let resp = server
    .post(
      "/user/management",
      json!({ "name": name, "email": email, "password": password }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "create user {name}");
  let body: Value = resp.json().await.unwrap();
  (
    Uuid::parse_str(body["uuid"].as_str().unwrap()).unwrap(),
    email,
  )
}

/// Create a group with `perms` and `users` (as the admin in the jar). Returns the group id.
pub async fn create_group(server: &TestServer, perms: &[&str], users: &[Uuid]) -> Uuid {
  let name = unique("group");
  let resp = server.post("/group", json!({ "name": name })).await;
  assert_eq!(resp.status(), StatusCode::OK, "create group");
  let body: Value = resp.json().await.unwrap();
  let id = Uuid::parse_str(body["uuid"].as_str().unwrap()).unwrap();
  let resp = server
    .put(
      "/group",
      json!({ "uuid": id, "name": name, "permissions": perms, "users": users, "caches": [] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "edit group");
  id
}

/// Log in without touching the cookie jar, returning the session JWT.
pub async fn login_raw(server: &TestServer, email: &str, password: &str) -> String {
  let encrypted = server.encrypt_password(password).await;
  let resp = server
    .raw(Method::POST, "/auth/password")
    .json(&json!({ "email": email, "password": encrypted }))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK, "login {email}");
  jwt_cookie(&resp).expect("login sets jwt cookie")
}

/// A logged-in user holding exactly `perms` (no group at all when empty).
pub async fn user_with_perms(server: &TestServer, perms: &[&str]) -> User {
  let (id, email) = create_user(server, "user").await;
  if !perms.is_empty() {
    create_group(server, perms, &[id]).await;
  }
  let jwt = login_raw(server, &email, USER_PASSWORD).await;
  User { id, email, jwt }
}

/// Create a CLI token for `user` via its JWT. Returns `(secret, id)`.
pub async fn token_for(
  server: &TestServer,
  user: &User,
  name: &str,
  exp: chrono::DateTime<chrono::Utc>,
) -> (String, Uuid) {
  let resp = user
    .send(
      server,
      Method::POST,
      "/token",
      json!({ "name": name, "exp": exp }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "create token {name}");
  let body: Value = resp.json().await.unwrap();
  (
    body["token"].as_str().unwrap().to_string(),
    Uuid::parse_str(body["uuid"].as_str().unwrap()).unwrap(),
  )
}

pub fn uuid(v: &Value) -> Uuid {
  Uuid::parse_str(v.as_str().expect("uuid string")).unwrap()
}
