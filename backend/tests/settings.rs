//! `GET /settings/general` (`settings.rs`) and the centaurus `/settings/{user,mail}` endpoints.
mod auth_common;
mod common;

use auth_common::user_with_perms;
use common::TestServer;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

#[tokio::test]
async fn general_settings_returns_site_url_and_routing() {
  let (server, _) = TestServer::start_with_admin().await;
  let resp = server.get("/settings/general").await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  // common::prepare_env sets SITE_URL=http://localhost/
  assert_eq!(
    body,
    json!({ "site_url": "http://localhost/", "virtual_host_routing": false })
  );
}

#[tokio::test]
async fn general_settings_reflects_virtual_host_routing() {
  let server = TestServer::start_with(|| unsafe {
    std::env::set_var("VIRTUAL_HOST_ROUTING", "true");
    std::env::set_var("SITE_URL", "http://cache.localhost/");
  })
  .await;
  server
    .setup_admin(
      common::ADMIN_NAME,
      common::ADMIN_EMAIL,
      common::ADMIN_PASSWORD,
    )
    .await;
  let body: Value = server.get("/settings/general").await.json().await.unwrap();
  assert_eq!(
    body,
    json!({ "site_url": "http://cache.localhost/", "virtual_host_routing": true })
  );
}

#[tokio::test]
async fn general_settings_needs_session_but_no_permission() {
  let (server, _) = TestServer::start_with_admin().await;
  let nobody = user_with_perms(&server, &[]).await;
  assert_eq!(
    nobody.get(&server, "/settings/general").await.status(),
    StatusCode::OK
  );

  let resp = server
    .raw(Method::GET, "/settings/general")
    .send()
    .await
    .unwrap();
  assert!(resp.status().is_client_error());
  let (token, _) = server.create_cli_token("ci").await;
  let resp = server
    .raw(Method::GET, "/settings/general")
    .bearer_auth(token)
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn user_settings_round_trip() {
  let (server, _) = TestServer::start_with_admin().await;
  let resp = server.get("/settings/user").await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["from_env"], json!([]));

  let resp = server
    .post("/settings/user", body["settings"].clone())
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let after: Value = server.get("/settings/user").await.json().await.unwrap();
  assert_eq!(after, body);
}

#[tokio::test]
async fn mail_settings_readable_by_admin() {
  let (server, _) = TestServer::start_with_admin().await;
  let resp = server.get("/settings/mail").await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert!(body.get("settings").is_some());
}

#[tokio::test]
async fn settings_routes_are_permission_gated() {
  let (server, _) = TestServer::start_with_admin().await;
  let user_settings: Value = server.get("/settings/user").await.json().await.unwrap();
  let nobody = user_with_perms(&server, &[]).await;
  let viewer = user_with_perms(&server, &["settings:view"]).await;

  for path in ["/settings/user", "/settings/mail"] {
    assert_eq!(
      nobody.get(&server, path).await.status(),
      StatusCode::FORBIDDEN,
      "{path}"
    );
    assert_eq!(
      viewer.get(&server, path).await.status(),
      StatusCode::OK,
      "{path}"
    );
    let resp = server.raw(Method::GET, path).send().await.unwrap();
    assert!(resp.status().is_client_error(), "{path}");
  }

  let resp = viewer
    .send(
      &server,
      Method::POST,
      "/settings/user",
      user_settings["settings"].clone(),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
