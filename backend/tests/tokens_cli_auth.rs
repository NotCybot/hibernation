//! `CliAuth` extraction (`auth/cli_auth.rs`) on non-nix routes (`GET /cache/test` echoes the user id) and
//! `GET /auth/test_token` (`auth/test_token.rs`).
mod auth_common;
mod common;

use auth_common::{jwt_cookie, user_with_perms};
use backend::db::DBTrait;
use base64::{Engine, prelude::BASE64_STANDARD};
use chrono::{Duration, Utc};
use common::{JWT_COOKIE_NAME, TestServer};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde_json::{Value, json};

async fn whoami(req: RequestBuilder) -> (StatusCode, String) {
  let resp = req.send().await.unwrap();
  (resp.status(), resp.text().await.unwrap())
}

fn cache_test(server: &TestServer) -> RequestBuilder {
  server.raw(Method::GET, "/cache/test")
}

// ---- token sources ----

#[tokio::test]
async fn cli_token_accepted_from_every_source() {
  let (server, admin) = TestServer::start_with_admin().await;
  let (token, _) = server.create_cli_token("ci").await;
  let admin = admin.to_string();

  let reqs = [
    cache_test(&server).bearer_auth(&token),
    cache_test(&server).header("Cookie", format!("{JWT_COOKIE_NAME}={token}")),
    server.raw(Method::GET, &format!("/cache/test?token={token}")),
    cache_test(&server).header(
      "Authorization",
      format!("Basic {}", BASE64_STANDARD.encode(format!("netrc:{token}"))),
    ),
  ];
  for req in reqs {
    assert_eq!(whoami(req).await, (StatusCode::OK, admin.clone()));
  }
}

#[tokio::test]
async fn session_jwt_is_accepted() {
  let (server, admin) = TestServer::start_with_admin().await;
  let jwt = server.cookie(JWT_COOKIE_NAME).unwrap();
  assert_ne!(jwt.len(), 32);

  let res = whoami(cache_test(&server).bearer_auth(&jwt)).await;
  assert_eq!(res, (StatusCode::OK, admin.to_string()));
}

#[tokio::test]
async fn missing_credentials_are_rejected() {
  let (server, _) = TestServer::start_with_admin().await;
  let (status, _) = whoami(cache_test(&server)).await;
  assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_cli_token_is_unauthorized() {
  let (server, _) = TestServer::start_with_admin().await;
  let fake = "a".repeat(32);
  let (status, _) = whoami(cache_test(&server).bearer_auth(&fake)).await;
  assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn garbage_jwt_is_unauthorized() {
  let (server, _) = TestServer::start_with_admin().await;
  for token in ["short", &"a".repeat(31), &"a".repeat(33), "a.b.c"] {
    let (status, _) = whoami(cache_test(&server).bearer_auth(token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{token}");
  }
}

#[tokio::test]
async fn logged_out_jwt_is_unauthorized() {
  let (server, _) = TestServer::start_with_admin().await;
  let jwt = server.cookie(JWT_COOKIE_NAME).unwrap();
  assert_eq!(
    server.post("/auth/logout", json!(null)).await.status(),
    StatusCode::OK
  );

  let (status, _) = whoami(cache_test(&server).bearer_auth(&jwt)).await;
  assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn deleted_users_jwt_is_forbidden() {
  let (server, _) = TestServer::start_with_admin().await;
  let user = user_with_perms(&server, &[]).await;
  let resp = server
    .delete("/user/management", json!({ "uuid": user.id }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);

  let (status, _) = whoami(cache_test(&server).bearer_auth(&user.jwt)).await;
  assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn deleted_users_cli_token_is_unauthorized() {
  let (server, _) = TestServer::start_with_admin().await;
  let user = user_with_perms(&server, &[]).await;
  let (token, _) =
    auth_common::token_for(&server, &user, "ci", Utc::now() + Duration::days(1)).await;
  let resp = server
    .delete("/user/management", json!({ "uuid": user.id }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);

  // never authenticated before, so nothing is cached: the token row was cascaded away
  let (status, _) = whoami(cache_test(&server).bearer_auth(&token)).await;
  assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---- expiry + cache ----

#[tokio::test]
async fn expired_cli_token_is_bad_request() {
  let (server, _) = TestServer::start_with_admin().await;
  let (token, _) = server
    .create_cli_token_exp("old", Utc::now() - Duration::minutes(1))
    .await;
  let (status, body) = whoami(cache_test(&server).bearer_auth(&token)).await;
  assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn cached_token_is_rechecked_once_it_expires() {
  let (server, admin) = TestServer::start_with_admin().await;
  // generous window: under load, setup can eat seconds before the first (caching) use
  let exp = Utc::now() + Duration::seconds(5);
  let (token, _) = server.create_cli_token_exp("short", exp).await;
  let res = whoami(cache_test(&server).bearer_auth(&token)).await;
  assert_eq!(res, (StatusCode::OK, admin.to_string()));

  // sleep relative to the expiry, not a fixed amount
  let left = (exp - Utc::now()).to_std().unwrap_or_default();
  tokio::time::sleep(left + std::time::Duration::from_millis(500)).await;
  let (status, _) = whoami(cache_test(&server).bearer_auth(&token)).await;
  assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn verified_tokens_are_cached_without_db_lookups() {
  let (server, admin) = TestServer::start_with_admin().await;
  let (token, id) = server.create_cli_token("ci").await;
  assert_eq!(
    whoami(cache_test(&server).bearer_auth(&token)).await.0,
    StatusCode::OK
  );

  let db = server.db().await;
  let first_use = db.token().by_id(id).await.unwrap().last_used.unwrap();

  // Expire the token behind the server's back: the cached verification (30 s) still answers and does not
  // touch `last_used`.
  use sea_orm::{ActiveModelTrait, IntoActiveModel, Set};
  let mut model = db.token().by_id(id).await.unwrap().into_active_model();
  model.exp = Set((Utc::now() - Duration::hours(1)).naive_utc());
  model.update(&*db).await.unwrap();

  let res = whoami(cache_test(&server).bearer_auth(&token)).await;
  assert_eq!(res, (StatusCode::OK, admin.to_string()));
  assert_eq!(
    db.token().by_id(id).await.unwrap().last_used.unwrap(),
    first_use
  );
}

// ---- /auth/test_token ----

#[tokio::test]
async fn test_token_reports_validity_and_clears_bad_cookie() {
  let (server, _) = TestServer::start_with_admin().await;
  let (token, _) = server.create_cli_token("ci").await;

  let body: Value = server.get("/auth/test_token").await.json().await.unwrap();
  assert_eq!(body["valid"], true);

  let resp = server
    .raw(Method::GET, "/auth/test_token")
    .bearer_auth(&token)
    .send()
    .await
    .unwrap();
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["valid"], true);

  // invalid credentials: valid=false
  for req in [
    server.raw(Method::GET, "/auth/test_token"),
    server
      .raw(Method::GET, "/auth/test_token")
      .bearer_auth("a".repeat(32)),
  ] {
    let resp = req.send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["valid"], false);
  }

  // an invalid session cookie is removed
  let resp = server
    .raw(Method::GET, "/auth/test_token")
    .header("Cookie", format!("{JWT_COOKIE_NAME}=garbage"))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  let cookies: Vec<_> = resp
    .headers()
    .get_all("set-cookie")
    .iter()
    .map(|v| v.to_str().unwrap().to_string())
    .collect();
  assert!(
    cookies
      .iter()
      .any(|c| c.starts_with(&format!("{JWT_COOKIE_NAME}=;"))
        || c.starts_with(&format!("{JWT_COOKIE_NAME}=\"\";"))),
    "{cookies:?}"
  );
  assert!(jwt_cookie(&resp).is_none());
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["valid"], false);
}

#[tokio::test]
async fn test_token_expired_cli_token_is_invalid() {
  let (server, _) = TestServer::start_with_admin().await;
  let (token, _) = server
    .create_cli_token_exp("old", Utc::now() - Duration::minutes(1))
    .await;
  let resp = server
    .raw(Method::GET, "/auth/test_token")
    .bearer_auth(&token)
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body["valid"], false);
}

#[tokio::test]
async fn deleted_users_cached_cli_token_stops_working_immediately() {
  let (server, _) = TestServer::start_with_admin().await;
  let user = user_with_perms(&server, &[]).await;
  let (token, _) =
    auth_common::token_for(&server, &user, "ci", Utc::now() + Duration::days(1)).await;
  // cache the verification (30 s)
  let res = whoami(cache_test(&server).bearer_auth(&token)).await;
  assert_eq!(res, (StatusCode::OK, user.id.to_string()));

  let resp = server
    .delete("/user/management", json!({ "uuid": user.id }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let (status, _) = whoami(cache_test(&server).bearer_auth(&token)).await;
  assert_eq!(status, StatusCode::UNAUTHORIZED);
}
