//! Browser-assisted CLI login (`cli.rs`): `POST /cli` issues a one-time code for the logged-in user,
//! `PUT /cli?code=` exchanges it for a CLI token.
mod common;

use backend::db::DBTrait;
use chrono::Utc;
use common::TestServer;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

async fn new_code(server: &TestServer) -> Uuid {
  let resp = server.post("/cli", json!(null)).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  Uuid::parse_str(body["code"].as_str().unwrap()).unwrap()
}

async fn exchange(server: &TestServer, code: &str) -> reqwest::Response {
  server
    .raw(Method::PUT, &format!("/cli?code={code}"))
    .send()
    .await
    .unwrap()
}

#[tokio::test]
async fn code_exchange_issues_working_token() {
  let (server, admin) = TestServer::start_with_admin().await;
  let code = new_code(&server).await;
  let before = Utc::now();

  // the exchange itself needs no auth, the code is the credential
  let resp = exchange(&server, &code.to_string()).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let token = resp.text().await.unwrap();
  assert_eq!(token.len(), 32);

  let resp = server
    .raw(Method::GET, "/cache/test")
    .bearer_auth(&token)
    .send()
    .await
    .unwrap();
  assert_eq!(resp.text().await.unwrap(), admin.to_string());

  let tokens: Vec<Value> = server.get("/token").await.json().await.unwrap();
  assert_eq!(tokens.len(), 1);
  assert!(tokens[0]["name"].as_str().unwrap().starts_with("Cli-"));
  // valid for the session lifetime (JWT exp), i.e. in the future
  let exp: chrono::DateTime<Utc> = serde_json::from_value(tokens[0]["exp"].clone()).unwrap();
  assert!(exp > before + chrono::Duration::minutes(1));
  let db = server.db().await;
  assert_eq!(db.token().get_by_user(admin).await.unwrap().len(), 1);
}

#[tokio::test]
async fn code_is_single_use() {
  let (server, _) = TestServer::start_with_admin().await;
  let code = new_code(&server).await.to_string();
  assert_eq!(exchange(&server, &code).await.status(), StatusCode::OK);
  assert_eq!(
    exchange(&server, &code).await.status(),
    StatusCode::BAD_REQUEST
  );
}

#[tokio::test]
async fn codes_are_independent() {
  let (server, _) = TestServer::start_with_admin().await;
  let a = new_code(&server).await;
  let b = new_code(&server).await;
  assert_ne!(a, b);
  let ta = exchange(&server, &a.to_string())
    .await
    .text()
    .await
    .unwrap();
  let tb = exchange(&server, &b.to_string())
    .await
    .text()
    .await
    .unwrap();
  assert_ne!(ta, tb);
}

#[tokio::test]
async fn unknown_missing_or_malformed_code_is_bad_request() {
  let (server, _) = TestServer::start_with_admin().await;
  let resp = exchange(&server, &Uuid::new_v4().to_string()).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  assert_eq!(
    exchange(&server, "not-a-uuid").await.status(),
    StatusCode::BAD_REQUEST
  );
  let resp = server.raw(Method::PUT, "/cli").send().await.unwrap();
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  assert!(
    server
      .get("/token")
      .await
      .json::<Vec<Value>>()
      .await
      .unwrap()
      .is_empty()
  );
}

#[tokio::test]
async fn new_code_requires_session() {
  let (server, _) = TestServer::start_with_admin().await;
  let (token, _) = server.create_cli_token("ci").await;

  let resp = server.raw(Method::POST, "/cli").send().await.unwrap();
  assert!(resp.status().is_client_error());
  // a CLI token cannot mint new CLI tokens
  let resp = server
    .raw(Method::POST, "/cli")
    .bearer_auth(&token)
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn exchange_is_rate_limited_but_new_code_is_not() {
  let (server, _) = TestServer::start_with_admin().await;

  let mut limited = false;
  for _ in 0..30 {
    let status = exchange(&server, &Uuid::new_v4().to_string())
      .await
      .status();
    if status == StatusCode::TOO_MANY_REQUESTS {
      limited = true;
      break;
    }
    assert_eq!(status, StatusCode::BAD_REQUEST);
  }
  assert!(limited, "PUT /cli must be rate limited (burst 20)");

  for _ in 0..30 {
    new_code(&server).await;
  }
}
