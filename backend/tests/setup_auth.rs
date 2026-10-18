//! First-run setup + password auth via centaurus (`/setup`, `/auth/password`, `/auth/logout`).
mod auth_common;
mod common;

use common::{ADMIN_EMAIL, ADMIN_PASSWORD, JWT_COOKIE_NAME, TestServer};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

#[tokio::test]
async fn is_setup_reports_false_then_true() {
  let server = TestServer::start().await;
  let resp = server.get("/setup").await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["is_setup"], false);
  assert_eq!(body["db_backend"], "SQLite");
  assert!(body["storage_backend"].is_string());

  server
    .setup_admin("admin", ADMIN_EMAIL, ADMIN_PASSWORD)
    .await;
  let body: Value = server.get("/setup").await.json().await.unwrap();
  assert_eq!(body["is_setup"], true);
}

#[tokio::test]
async fn setup_puts_admin_in_admin_group_with_all_permissions() {
  let (server, admin) = TestServer::start_with_admin().await;
  let body: Value = server.get("/user/info").await.json().await.unwrap();
  assert_eq!(body["uuid"], admin.to_string());
  let mut perms: Vec<String> = serde_json::from_value(body["permissions"].clone()).unwrap();
  perms.sort();
  let mut expected: Vec<String> = backend::utils::permissions()
    .iter()
    .map(|p| p.to_string())
    .collect();
  expected.sort();
  assert_eq!(perms, expected);

  let groups: Value = server.get("/group").await.json().await.unwrap();
  assert_eq!(groups["groups"][0]["name"], "Admin");
}

#[tokio::test]
async fn admin_group_name_is_configurable() {
  let server =
    TestServer::start_with(|| unsafe { std::env::set_var("ADMIN_GROUP", "Wizards") }).await;
  server
    .setup_admin("admin", ADMIN_EMAIL, ADMIN_PASSWORD)
    .await;
  let groups: Value = server.get("/group").await.json().await.unwrap();
  assert_eq!(groups["groups"][0]["name"], "Wizards");
}

#[tokio::test]
async fn setup_twice_conflicts() {
  let (server, _) = TestServer::start_with_admin().await;
  let encrypted = server.encrypt_password("anotherpass").await;
  let resp = server
    .post(
      "/setup",
      json!({ "admin_username": "a2", "admin_email": "a2@example.com", "admin_password": encrypted }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn setup_rejects_empty_username_or_email() {
  let server = TestServer::start().await;
  let encrypted = server.encrypt_password(ADMIN_PASSWORD).await;
  for (name, email) in [("  ", ADMIN_EMAIL), ("admin", "  ")] {
    let resp = server
      .post(
        "/setup",
        json!({ "admin_username": name, "admin_email": email, "admin_password": encrypted }),
      )
      .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  }
  let body: Value = server.get("/setup").await.json().await.unwrap();
  assert_eq!(body["is_setup"], false);
}

#[tokio::test]
async fn login_with_correct_password_succeeds() {
  let (server, admin) = TestServer::start_with_admin().await;
  server.clear_cookies();
  let resp = server.login(ADMIN_EMAIL, ADMIN_PASSWORD).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert!(server.has_cookie(JWT_COOKIE_NAME));
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["user"], admin.to_string());
}

#[tokio::test]
async fn login_failures_are_unauthorized() {
  let (server, _) = TestServer::start_with_admin().await;
  server.clear_cookies();
  assert_eq!(
    server.login(ADMIN_EMAIL, "wrongpass").await.status(),
    StatusCode::UNAUTHORIZED
  );
  let resp = server.login("nobody@example.com", ADMIN_PASSWORD).await;
  assert!(resp.status().is_client_error());
  assert!(!server.has_cookie(JWT_COOKIE_NAME));
}

#[tokio::test]
async fn test_token_valid_then_invalid_after_logout() {
  let (server, _) = TestServer::start_with_admin().await;
  let body: Value = server.get("/auth/test_token").await.json().await.unwrap();
  assert_eq!(body["valid"], true);

  assert_eq!(
    server.post("/auth/logout", json!(null)).await.status(),
    StatusCode::OK
  );
  assert!(!server.has_cookie(JWT_COOKIE_NAME));
  let body: Value = server.get("/auth/test_token").await.json().await.unwrap();
  assert_eq!(body["valid"], false);
  assert!(!server.get("/user/info").await.status().is_success());
}

#[tokio::test]
async fn logout_requires_session() {
  let (server, _) = TestServer::start_with_admin().await;
  let resp = server
    .raw(Method::POST, "/auth/logout")
    .send()
    .await
    .unwrap();
  assert!(resp.status().is_client_error());
}
