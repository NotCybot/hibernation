//! Health, version header, auth config and the CLI `GET /cache/test` probe.
mod common;

use common::TestServer;
use reqwest::{Method, StatusCode};
use serde_json::Value;

#[tokio::test]
async fn health_check_returns_success() {
  let server = TestServer::start().await;
  assert!(server.get("/health").await.status().is_success());
}

#[tokio::test]
async fn every_response_carries_the_api_version() {
  let server = TestServer::start().await;
  for path in ["/health", "/setup", "/user/info", "/does-not-exist"] {
    let resp = server.get(path).await;
    assert_eq!(
      resp
        .headers()
        .get("x-api-version")
        .and_then(|v| v.to_str().ok()),
      Some(env!("CARGO_PKG_VERSION")),
      "{path}"
    );
  }
}

#[tokio::test]
async fn auth_config_without_oidc_or_mail() {
  let server = TestServer::start().await;
  let resp = server.get("/auth/config").await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["mail_enabled"], false);
  assert_eq!(body["instant_redirect"], false);
}

#[tokio::test]
async fn cache_test_echoes_cli_user() {
  let (server, admin) = TestServer::start_with_admin().await;
  let (token, _) = server.create_cli_token("ci").await;
  let resp = server
    .raw(Method::GET, "/cache/test")
    .bearer_auth(token)
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(resp.text().await.unwrap(), admin.to_string());
}
