//! CLI token management (`/token`, `token.rs`, `db/token.rs`).
mod auth_common;
mod common;

use auth_common::{token_for, user_with_perms, uuid};
use chrono::{Duration, Utc};
use common::TestServer;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

async fn cli_test(server: &TestServer, token: &str) -> StatusCode {
  server
    .raw(Method::GET, "/cache/test")
    .bearer_auth(token)
    .send()
    .await
    .unwrap()
    .status()
}

async fn list(server: &TestServer) -> Vec<Value> {
  let resp = server.get("/token").await;
  assert_eq!(resp.status(), StatusCode::OK);
  resp.json().await.unwrap()
}

// ---- create / list / get ----

#[tokio::test]
async fn create_lists_and_gets_token() {
  let (server, _) = TestServer::start_with_admin().await;
  assert!(list(&server).await.is_empty());

  let exp = Utc::now() + Duration::days(3);
  let (token, id) = server.create_cli_token_exp("ci", exp).await;
  assert!(token.chars().all(|c| c.is_ascii_alphanumeric()));

  let tokens = list(&server).await;
  assert_eq!(tokens.len(), 1);
  assert_eq!(uuid(&tokens[0]["uuid"]), id);
  assert_eq!(tokens[0]["name"], "ci");
  assert!(tokens[0]["last_used"].is_null());
  let listed_exp: chrono::DateTime<Utc> = serde_json::from_value(tokens[0]["exp"].clone()).unwrap();
  assert!((listed_exp - exp).num_milliseconds().abs() < 1000);
  // the secret is never returned again
  assert!(tokens[0].get("token").is_none());

  let resp = server.get(&format!("/token/{id}")).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let info: Value = resp.json().await.unwrap();
  assert_eq!(info, tokens[0]);
}

#[tokio::test]
async fn create_rejects_empty_and_duplicate_names() {
  let (server, _) = TestServer::start_with_admin().await;
  let exp = Utc::now() + Duration::days(1);

  for name in ["", "   "] {
    let resp = server
      .post("/token", json!({ "name": name, "exp": exp }))
      .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{name:?}");
  }

  server.create_cli_token("ci").await;
  let resp = server
    .post("/token", json!({ "name": "ci", "exp": exp }))
    .await;
  assert_eq!(resp.status(), StatusCode::CONFLICT);
  assert_eq!(list(&server).await.len(), 1);

  // names are only unique per user
  let other = user_with_perms(&server, &[]).await;
  token_for(&server, &other, "ci", exp).await;
}

#[tokio::test]
async fn get_unknown_token_is_not_found() {
  let (server, _) = TestServer::start_with_admin().await;
  let resp = server.get(&format!("/token/{}", Uuid::new_v4())).await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_with_malformed_body_is_rejected() {
  let (server, _) = TestServer::start_with_admin().await;
  let resp = server
    .post("/token", json!({ "name": "x", "exp": "tomorrow" }))
    .await;
  assert!(resp.status().is_client_error());
  let resp = server.post("/token", json!({ "name": "x" })).await;
  assert!(resp.status().is_client_error());
}

// ---- edit ----

#[tokio::test]
async fn edit_renames_and_changes_expiry() {
  let (server, _) = TestServer::start_with_admin().await;
  let (_, id) = server.create_cli_token("ci").await;

  let exp = Utc::now() + Duration::days(10);
  let resp = server
    .put(
      "/token",
      json!({ "uuid": id, "name": "deploy", "exp": exp }),
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
  let new_exp: chrono::DateTime<Utc> = serde_json::from_value(info["exp"].clone()).unwrap();
  assert!((new_exp - exp).num_milliseconds().abs() < 1000);

  // keeping its own name is not a conflict
  let resp = server
    .put(
      "/token",
      json!({ "uuid": id, "name": "deploy", "exp": exp }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn edit_rejects_empty_and_taken_names() {
  let (server, _) = TestServer::start_with_admin().await;
  let (_, a) = server.create_cli_token("a").await;
  server.create_cli_token("b").await;
  let exp = Utc::now() + Duration::days(1);

  let resp = server
    .put("/token", json!({ "uuid": a, "name": " ", "exp": exp }))
    .await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  let resp = server
    .put("/token", json!({ "uuid": a, "name": "b", "exp": exp }))
    .await;
  assert_eq!(resp.status(), StatusCode::CONFLICT);

  let info: Value = server
    .get(&format!("/token/{a}"))
    .await
    .json()
    .await
    .unwrap();
  assert_eq!(info["name"], "a");
}

#[tokio::test]
async fn edit_unknown_token_is_not_found() {
  let (server, _) = TestServer::start_with_admin().await;
  let resp = server
    .put(
      "/token",
      json!({ "uuid": Uuid::new_v4(), "name": "x", "exp": Utc::now() }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn edit_to_past_expiry_invalidates_cached_token() {
  let (server, _) = TestServer::start_with_admin().await;
  let (token, id) = server.create_cli_token("ci").await;
  assert_eq!(cli_test(&server, &token).await, StatusCode::OK);

  let resp = server
    .put(
      "/token",
      json!({ "uuid": id, "name": "ci", "exp": Utc::now() - Duration::hours(1) }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(cli_test(&server, &token).await, StatusCode::BAD_REQUEST);
}

// ---- delete ----

#[tokio::test]
async fn delete_removes_token_and_cached_auth() {
  let (server, _) = TestServer::start_with_admin().await;
  let (token, id) = server.create_cli_token("ci").await;
  let (_, keep) = server.create_cli_token("keep").await;
  // populate the 30 s auth cache
  assert_eq!(cli_test(&server, &token).await, StatusCode::OK);

  let resp = server.delete("/token", json!({ "uuid": id })).await;
  assert_eq!(resp.status(), StatusCode::OK);

  let tokens = list(&server).await;
  assert_eq!(tokens.len(), 1);
  assert_eq!(uuid(&tokens[0]["uuid"]), keep);
  assert_eq!(cli_test(&server, &token).await, StatusCode::UNAUTHORIZED);
  assert_eq!(
    server.get(&format!("/token/{id}")).await.status(),
    StatusCode::NOT_FOUND
  );

  // idempotent
  let resp = server.delete("/token", json!({ "uuid": id })).await;
  assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn delete_expired_only_removes_own_expired_tokens() {
  let (server, _) = TestServer::start_with_admin().await;
  let past = Utc::now() - Duration::hours(1);
  server.create_cli_token_exp("old1", past).await;
  server.create_cli_token_exp("old2", past).await;
  let (_, valid) = server.create_cli_token("valid").await;

  let other = user_with_perms(&server, &[]).await;
  let (_, other_old) = token_for(&server, &other, "old", past).await;

  let resp = server.delete("/token/expired", json!({})).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["deleted"], 2);

  let tokens = list(&server).await;
  assert_eq!(tokens.len(), 1);
  assert_eq!(uuid(&tokens[0]["uuid"]), valid);

  // nothing left to delete
  let body: Value = server
    .delete("/token/expired", json!({}))
    .await
    .json()
    .await
    .unwrap();
  assert_eq!(body["deleted"], 0);

  let resp = other.get(&server, &format!("/token/{other_old}")).await;
  assert_eq!(resp.status(), StatusCode::OK);
}

// ---- regenerate ----

#[tokio::test]
async fn regenerate_replaces_secret() {
  let (server, _) = TestServer::start_with_admin().await;
  let (old, id) = server.create_cli_token("ci").await;
  assert_eq!(cli_test(&server, &old).await, StatusCode::OK);

  let resp = server.post(&format!("/token/{id}"), json!({})).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  let new = body["token"].as_str().unwrap();
  assert_eq!(new.len(), 32);
  assert_ne!(new, old);

  // old secret is dead even though it was cached, the new one works, the token keeps its id
  assert_eq!(cli_test(&server, &old).await, StatusCode::UNAUTHORIZED);
  assert_eq!(cli_test(&server, new).await, StatusCode::OK);
  let tokens = list(&server).await;
  assert_eq!(tokens.len(), 1);
  assert_eq!(uuid(&tokens[0]["uuid"]), id);
}

#[tokio::test]
async fn regenerate_unknown_token_is_not_found() {
  let (server, _) = TestServer::start_with_admin().await;
  let resp = server
    .post(&format!("/token/{}", Uuid::new_v4()), json!({}))
    .await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---- ownership isolation ----

#[tokio::test]
async fn users_cannot_see_or_modify_other_users_tokens() {
  let (server, admin) = TestServer::start_with_admin().await;
  let (admin_token, admin_id) = server.create_cli_token("admin").await;
  let other = user_with_perms(&server, &[]).await;
  let exp = Utc::now() + Duration::days(1);

  // list is per user
  let resp = other.get(&server, "/token").await;
  assert_eq!(resp.status(), StatusCode::OK);
  let tokens: Vec<Value> = resp.json().await.unwrap();
  assert!(tokens.is_empty());

  let path = format!("/token/{admin_id}");
  assert_eq!(
    other.get(&server, &path).await.status(),
    StatusCode::NOT_FOUND
  );
  let resp = other
    .send(
      &server,
      Method::PUT,
      "/token",
      json!({ "uuid": admin_id, "name": "pwned", "exp": exp }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
  let resp = other.send(&server, Method::POST, &path, json!({})).await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
  let resp = other
    .send(
      &server,
      Method::DELETE,
      "/token",
      json!({ "uuid": admin_id }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "delete is a silent no-op");

  // the admin's token is untouched and still authenticates as the admin
  let info: Value = server.get(&path).await.json().await.unwrap();
  assert_eq!(info["name"], "admin");
  let resp = server
    .raw(Method::GET, "/cache/test")
    .bearer_auth(&admin_token)
    .send()
    .await
    .unwrap();
  assert_eq!(resp.text().await.unwrap(), admin.to_string());
}

// ---- auth ----

#[tokio::test]
async fn token_routes_require_session() {
  let (server, _) = TestServer::start_with_admin().await;
  let (token, id) = server.create_cli_token("ci").await;
  let exp = Utc::now() + Duration::days(1);

  let routes = [
    (Method::GET, "/token".to_string(), json!(null)),
    (
      Method::POST,
      "/token".to_string(),
      json!({ "name": "x", "exp": exp }),
    ),
    (
      Method::PUT,
      "/token".to_string(),
      json!({ "uuid": id, "name": "x", "exp": exp }),
    ),
    (Method::DELETE, "/token".to_string(), json!({ "uuid": id })),
    (Method::DELETE, "/token/expired".to_string(), json!(null)),
    (Method::GET, format!("/token/{id}"), json!(null)),
    (Method::POST, format!("/token/{id}"), json!(null)),
  ];
  for (method, path, body) in routes {
    let resp = server
      .raw(method.clone(), &path)
      .json(&body)
      .send()
      .await
      .unwrap();
    assert!(resp.status().is_client_error(), "{method} {path} no auth");

    // a CLI token is not a session
    let resp = server
      .raw(method.clone(), &path)
      .bearer_auth(&token)
      .json(&body)
      .send()
      .await
      .unwrap();
    assert_eq!(
      resp.status(),
      StatusCode::UNAUTHORIZED,
      "{method} {path} cli token"
    );
  }
  assert_eq!(list(&server).await.len(), 1);
}

#[tokio::test]
async fn last_used_is_set_when_token_authenticates() {
  let (server, _) = TestServer::start_with_admin().await;
  let (token, id) = server.create_cli_token("ci").await;
  let before = Utc::now();

  assert_eq!(cli_test(&server, &token).await, StatusCode::OK);

  let info: Value = server
    .get(&format!("/token/{id}"))
    .await
    .json()
    .await
    .unwrap();
  let used: chrono::DateTime<Utc> = serde_json::from_value(info["last_used"].clone()).unwrap();
  assert!(used >= before - Duration::seconds(1));
}
