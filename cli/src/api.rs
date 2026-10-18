use std::{
  io::Cursor,
  path::PathBuf,
  sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
  },
};

use bytes::Bytes;
use centaurus::{VERSION_HEADER_NAME, bail, error::Result, eyre::Context};
use reqwest::{
  Body, Client, Method, RequestBuilder,
  header::{CONTENT_LENGTH, HeaderMap},
};
use serde::Deserialize;
use shared::api::push::{
  UploadFinishRequest, UploadInfoResponse, UploadPathRequest, UploadPathResponse,
};
use tokio_util::io::{InspectReader, ReaderStream};
use tracing::warn;
use url::Url;
use uuid::Uuid;

use crate::config::Config;

#[derive(Clone)]
pub struct ApiClient {
  client: Client,
  token: String,
  url: Url,
}

pub enum PushInfoResult {
  CacheNotFound,
  ForcePushNotAllowed,
  AllPathsExist,
  Success(UploadInfoResponse),
}

#[derive(Deserialize)]
struct ValidationResponse {
  valid: bool,
}

impl ApiClient {
  pub fn new(token: String, url: Url) -> Self {
    Self {
      token,
      url,
      client: client(),
    }
  }

  pub async fn build(
    config: Option<Config>,
    url: Option<Url>,
    config_path: Option<PathBuf>,
    tty: bool,
  ) -> Result<Self> {
    if let Some(config) = &config
      && let Some(token) = &config.token
    {
      let client = ApiClient::new(token.clone(), url.clone().unwrap_or(config.app_url.clone()));

      match client.test_token().await {
        Ok(true) => return Ok(client),
        Ok(false) => warn!("Token is invalid."),
        Err(e) => warn!("Failed to validate token: {e:#}"),
      }
    }

    if !tty {
      if config.as_ref().is_some_and(|c| c.token.is_some()) {
        bail!("Token validation failed. Re-run \"hibernation auth\" if the token is invalid");
      } else if url.is_some() || config.is_some() {
        bail!("Token not set. Please use \"hibernation auth\" first");
      } else {
        bail!("Cli not configured. Please login with \"hibernation auth --url <url>\" first");
      }
    }

    let mut config = if let Some(config) = config {
      config
    } else if let Some(url) = url {
      Config::new(url, None)
    } else {
      let url = crate::auth::get_tty_url().await;
      Config::new(url, None)
    };

    let token = crate::auth::get_tty_token(&mut config, config_path).await;
    Ok(ApiClient::new(token, config.app_url))
  }

  pub async fn request_token(url: Url, code: &str) -> Result<String> {
    let mut url = url.join("/api/cli")?;
    url.query_pairs_mut().append_pair("code", code);

    let client = client();
    let req = client.put(url).build()?;
    let res = client.execute(req).await?.error_for_status()?;

    check_server_version(res.headers());

    Ok(res.text().await?)
  }

  pub async fn test(&self) -> Result<()> {
    self
      .req("/api/cache/test", Method::GET)?
      .send()
      .await?
      .error_for_status()?;
    Ok(())
  }

  pub async fn test_token(&self) -> Result<bool> {
    let res = self
      .req("/api/auth/test_token", Method::GET)?
      .send()
      .await?
      .error_for_status()?;

    check_server_version(res.headers());

    let valid = res
      .json::<ValidationResponse>()
      .await
      .context("Failed to parse token validation response")?;

    Ok(valid.valid)
  }

  pub async fn push_info(
    &self,
    cache: String,
    paths: Vec<String>,
    force: bool,
  ) -> Result<PushInfoResult> {
    let body = serde_json::json!({
      "paths": paths,
      "force": force,
      "cache": cache,
    });

    let res = self
      .req("/api/cache/push/info", Method::POST)?
      .json(&body)
      .send()
      .await?;

    match res.status() {
      reqwest::StatusCode::NOT_FOUND => Ok(PushInfoResult::CacheNotFound),
      reqwest::StatusCode::NOT_ACCEPTABLE => Ok(PushInfoResult::ForcePushNotAllowed),
      reqwest::StatusCode::NO_CONTENT => Ok(PushInfoResult::AllPathsExist),
      _ => {
        let res: UploadInfoResponse = res
          .error_for_status()?
          .json()
          .await
          .context("Failed to parse push info response")?;
        Ok(PushInfoResult::Success(res))
      }
    }
  }

  pub async fn upload_path(&self, info: &UploadPathRequest) -> Result<UploadPathResponse> {
    let res = self
      .req("/api/cache/push", Method::POST)?
      .json(info)
      .send()
      .await?
      .error_for_status()?;

    let res: UploadPathResponse = res
      .json()
      .await
      .context("Failed to parse upload path response")?;
    Ok(res)
  }

  /// Uploads a chunk, adding the number of body bytes handed to the connection to `sent`
  pub async fn upload_chunk(
    &self,
    upload_id: Uuid,
    part: u32,
    chunk: Bytes,
    sent: Arc<AtomicU64>,
  ) -> Result<()> {
    let len = chunk.len();
    let reader = InspectReader::new(Cursor::new(chunk), move |bytes| {
      sent.fetch_add(bytes.len() as u64, Ordering::Relaxed);
    });
    self
      .req(&format!("/api/cache/push/{upload_id}/{part}"), Method::POST)?
      .header(CONTENT_LENGTH, len)
      .body(Body::wrap_stream(ReaderStream::with_capacity(
        reader,
        64 * 1024,
      )))
      .send()
      .await?
      .error_for_status()?;

    Ok(())
  }

  pub async fn upload_finish(&self, upload_id: Uuid, body: UploadFinishRequest) -> Result<()> {
    self
      .req(&format!("/api/cache/push/{}", upload_id), Method::PUT)?
      .json(&body)
      .send()
      .await?
      .error_for_status()?;

    Ok(())
  }

  fn req(&self, path: &str, method: Method) -> Result<RequestBuilder> {
    let url = self.url.join(path)?;
    Ok(self.client.request(method, url).bearer_auth(&self.token))
  }
}

fn client() -> Client {
  Client::builder()
    .user_agent(format!("Hibernation CLI v{}", env!("CARGO_PKG_VERSION")))
    .build()
    .expect("Failed to build HTTP client")
}

/// Warns when the server version is missing or differs from ours, returns whether it matched
fn check_server_version(headers: &HeaderMap) -> bool {
  if let Some(version) = headers
    .get(VERSION_HEADER_NAME)
    .and_then(|v| v.to_str().ok())
  {
    if version != env!("CARGO_PKG_VERSION") {
      warn!(
        "Warning: Server version ({version}) does not match client version ({}). Consider updating your CLI.",
        env!("CARGO_PKG_VERSION")
      );
      false
    } else {
      true
    }
  } else {
    warn!("Warning: Server did not provide version information. Consider updating your CLI.");
    false
  }
}

#[cfg(test)]
mod test {
  use std::sync::{Arc, atomic::AtomicU64};

  use bytes::Bytes;
  use centaurus::VERSION_HEADER_NAME;
  use reqwest::header::{HeaderMap, HeaderValue};
  use serde_json::json;
  use shared::api::push::{UploadFinishRequest, UploadPathRequest};
  use url::Url;
  use uuid::Uuid;
  use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_string, header, method, path, query_param},
  };

  use super::{ApiClient, PushInfoResult, check_server_version};
  use crate::config::Config;

  fn client(server: &MockServer) -> ApiClient {
    ApiClient::new("tok".into(), Url::parse(&server.uri()).unwrap())
  }

  fn versioned(status: u16) -> ResponseTemplate {
    ResponseTemplate::new(status).insert_header(VERSION_HEADER_NAME, env!("CARGO_PKG_VERSION"))
  }

  async fn mock_token(server: &MockServer, valid: bool) {
    Mock::given(method("GET"))
      .and(path("/api/auth/test_token"))
      .and(header("authorization", "Bearer tok"))
      .respond_with(versioned(200).set_body_json(json!({ "valid": valid })))
      .mount(server)
      .await;
  }

  fn err_string<T>(res: centaurus::error::Result<T>) -> String {
    match res {
      Ok(_) => panic!("expected error"),
      Err(e) => format!("{e:?}"),
    }
  }

  // ---- version header ----

  #[test]
  fn server_version_check() {
    let mut headers = HeaderMap::new();
    assert!(!check_server_version(&headers), "missing");
    headers.insert(VERSION_HEADER_NAME, HeaderValue::from_static("0.0.0-other"));
    assert!(!check_server_version(&headers), "mismatch");
    headers.insert(
      VERSION_HEADER_NAME,
      HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
    );
    assert!(check_server_version(&headers), "match");
    headers.insert(
      VERSION_HEADER_NAME,
      HeaderValue::from_bytes(b"\xff").unwrap(),
    );
    assert!(!check_server_version(&headers), "non utf8");
  }

  // ---- token ----

  #[tokio::test]
  async fn test_token_valid_invalid_and_errors() {
    let server = MockServer::start().await;
    mock_token(&server, true).await;
    assert!(client(&server).test_token().await.unwrap());

    let server = MockServer::start().await;
    mock_token(&server, false).await;
    assert!(!client(&server).test_token().await.unwrap());

    let server = MockServer::start().await;
    Mock::given(path("/api/auth/test_token"))
      .respond_with(ResponseTemplate::new(200).set_body_string("nope"))
      .mount(&server)
      .await;
    assert!(err_string(client(&server).test_token().await).contains("Failed to parse"));

    // 404 by default
    let server = MockServer::start().await;
    assert!(client(&server).test_token().await.is_err());
  }

  #[tokio::test]
  async fn request_token_encodes_code() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
      .and(path("/api/cli"))
      .and(query_param("code", "a&b c"))
      .respond_with(versioned(200).set_body_string("the-token"))
      .expect(1)
      .mount(&server)
      .await;
    let url = Url::parse(&server.uri()).unwrap();
    assert_eq!(
      ApiClient::request_token(url.clone(), "a&b c")
        .await
        .unwrap(),
      "the-token"
    );
    // Unknown code: 404
    assert!(ApiClient::request_token(url, "other").await.is_err());
  }

  // ---- build ----

  #[tokio::test]
  async fn build_uses_valid_token() {
    let server = MockServer::start().await;
    mock_token(&server, true).await;
    let config = Config::new(Url::parse(&server.uri()).unwrap(), Some("tok".into()));
    let client = ApiClient::build(Some(config), None, None, false)
      .await
      .unwrap();
    assert_eq!(client.token, "tok");
  }

  #[tokio::test]
  async fn build_prefers_url_override() {
    let server = MockServer::start().await;
    mock_token(&server, true).await;
    let config = Config::new(
      Url::parse("http://127.0.0.1:1/").unwrap(),
      Some("tok".into()),
    );
    let url = Url::parse(&server.uri()).unwrap();
    let client = ApiClient::build(Some(config), Some(url.clone()), None, false)
      .await
      .unwrap();
    assert_eq!(client.url, url);
  }

  #[tokio::test]
  async fn build_non_interactive_errors() {
    let server = MockServer::start().await;
    mock_token(&server, false).await;
    let url = Url::parse(&server.uri()).unwrap();

    // Invalid token
    let config = Config::new(url.clone(), Some("tok".into()));
    let err = err_string(ApiClient::build(Some(config), None, None, false).await);
    assert!(err.contains("Token validation failed"), "{err}");

    // Validation request fails (connection refused)
    let config = Config::new(
      Url::parse("http://127.0.0.1:1/").unwrap(),
      Some("tok".into()),
    );
    let err = err_string(ApiClient::build(Some(config), None, None, false).await);
    assert!(err.contains("Token validation failed"), "{err}");

    // No token
    let config = Config::new(url.clone(), None);
    let err = err_string(ApiClient::build(Some(config), None, None, false).await);
    assert!(err.contains("Token not set"), "{err}");
    let err = err_string(ApiClient::build(None, Some(url), None, false).await);
    assert!(err.contains("Token not set"), "{err}");

    let err = err_string(ApiClient::build(None, None, None, false).await);
    assert!(err.contains("Cli not configured"), "{err}");
  }

  // ---- push ----

  #[tokio::test]
  async fn push_info_status_mapping() {
    let cache = Uuid::new_v4();
    for (status, body, expected) in [
      (404, None, "not found"),
      (406, None, "force"),
      (204, None, "exist"),
      (
        200,
        Some(json!({ "paths": ["a"], "cache": cache })),
        "success",
      ),
      (200, Some(json!({ "wrong": true })), "error"),
      (500, None, "error"),
      (403, None, "error"),
    ] {
      let server = MockServer::start().await;
      let mut res = ResponseTemplate::new(status);
      if let Some(body) = body {
        res = res.set_body_json(body);
      }
      Mock::given(method("POST"))
        .and(path("/api/cache/push/info"))
        .and(header("authorization", "Bearer tok"))
        .and(body_json(
          json!({ "cache": "main", "paths": ["a", "b"], "force": true }),
        ))
        .respond_with(res)
        .expect(1)
        .mount(&server)
        .await;

      let res = client(&server)
        .push_info("main".into(), vec!["a".into(), "b".into()], true)
        .await;
      match (expected, res) {
        ("not found", Ok(PushInfoResult::CacheNotFound))
        | ("force", Ok(PushInfoResult::ForcePushNotAllowed))
        | ("exist", Ok(PushInfoResult::AllPathsExist))
        | ("error", Err(_)) => {}
        ("success", Ok(PushInfoResult::Success(res))) => {
          assert_eq!(res.paths, ["a"]);
          assert_eq!(res.cache, cache);
        }
        (expected, _) => panic!("{status}: expected {expected}"),
      }
    }
  }

  #[tokio::test]
  async fn upload_requests() {
    let server = MockServer::start().await;
    let id = Uuid::new_v4();
    Mock::given(method("POST"))
      .and(path("/api/cache/push"))
      .and(header("authorization", "Bearer tok"))
      .respond_with(
        ResponseTemplate::new(200).set_body_json(json!({ "uuid": id, "nar_found": false })),
      )
      .expect(1)
      .mount(&server)
      .await;
    Mock::given(method("POST"))
      .and(path(format!("/api/cache/push/{id}/3")))
      .and(header("content-length", "5"))
      .and(body_string("hello"))
      .respond_with(ResponseTemplate::new(200))
      .expect(1)
      .mount(&server)
      .await;
    Mock::given(method("PUT"))
      .and(path(format!("/api/cache/push/{id}")))
      .and(body_json(json!({ "file_hash": "h", "file_size": 5 })))
      .respond_with(ResponseTemplate::new(200))
      .expect(1)
      .mount(&server)
      .await;

    let api = client(&server);
    let res = api
      .upload_path(&UploadPathRequest {
        cache: Uuid::new_v4(),
        force: false,
        store_path: "p".into(),
        nar_hash: "h".into(),
        nar_size: 1,
        deriver: None,
        references: vec![],
        signature: "s".into(),
      })
      .await
      .unwrap();
    assert_eq!(res.uuid, id);
    assert!(!res.nar_found);

    let sent = Arc::new(AtomicU64::new(0));
    api
      .upload_chunk(id, 3, Bytes::from_static(b"hello"), sent.clone())
      .await
      .unwrap();
    assert_eq!(sent.load(std::sync::atomic::Ordering::Relaxed), 5);

    api
      .upload_finish(
        id,
        UploadFinishRequest {
          file_hash: "h".into(),
          file_size: 5,
        },
      )
      .await
      .unwrap();

    // Errors map to Err
    let other = Uuid::new_v4();
    assert!(
      api
        .upload_finish(
          other,
          UploadFinishRequest {
            file_hash: "h".into(),
            file_size: 5
          }
        )
        .await
        .is_err()
    );
    assert!(
      api
        .upload_chunk(other, 0, Bytes::new(), sent)
        .await
        .is_err()
    );
    assert!(api.test().await.is_err());
  }

  #[tokio::test]
  async fn upload_path_bad_body_errors() {
    let server = MockServer::start().await;
    Mock::given(path("/api/cache/push"))
      .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
      .mount(&server)
      .await;
    Mock::given(path("/api/cache/test"))
      .respond_with(ResponseTemplate::new(200))
      .mount(&server)
      .await;
    let api = client(&server);
    let req = UploadPathRequest {
      cache: Uuid::new_v4(),
      force: false,
      store_path: "p".into(),
      nar_hash: "h".into(),
      nar_size: 1,
      deriver: None,
      references: vec![],
      signature: "s".into(),
    };
    assert!(
      err_string(api.upload_path(&req).await).contains("Failed to parse upload path response")
    );
    api.test().await.unwrap();
  }
}
