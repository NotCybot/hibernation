use std::{
  io::{Cursor, IsTerminal},
  path::PathBuf,
  sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
  },
};

use bytes::Bytes;
use centaurus::{VERSION_HEADER_NAME, error::Result, eyre::Context};
use reqwest::{Body, Client, Method, RequestBuilder, Response, header::CONTENT_LENGTH};
use serde::Deserialize;
use shared::api::push::{
  UploadFinishRequest, UploadInfoResponse, UploadPathRequest, UploadPathResponse,
};
use tokio_util::io::{InspectReader, ReaderStream};
use tracing::{error, warn};
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
  fn new(token: String, url: Url) -> Self {
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
  ) -> Self {
    if let Some(config) = &config
      && let Some(token) = &config.token
    {
      let client = ApiClient::new(token.clone(), url.clone().unwrap_or(config.app_url.clone()));

      match client.test_token().await {
        Ok(true) => return client,
        Ok(false) => warn!("Token is invalid."),
        Err(e) => warn!("Failed to validate token: {e:#}"),
      }
    }

    let tty = std::io::stdin().is_terminal();
    if !tty {
      if config.as_ref().is_some_and(|c| c.token.is_some()) {
        error!("Token validation failed. Re-run \"hibernation auth\" if the token is invalid");
      } else if url.is_some() {
        error!("Token not set. Please use \"hibernation auth\" first");
      } else {
        error!("Cli not configured. Please login with \"hibernation auth --url <url>\" first");
      }
      std::process::exit(1);
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
    ApiClient::new(token, config.app_url)
  }

  pub async fn request_token(url: Url, code: &str) -> Result<String> {
    let url = url.join(&format!("/api/cli?code={}", code))?;

    let client = client();
    let req = client.put(url).build()?;
    let res = client.execute(req).await?.error_for_status()?;

    check_server_version(&res);

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

    check_server_version(&res);

    let valid = res.json::<ValidationResponse>().await.unwrap();

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

fn check_server_version(res: &Response) {
  if let Some(version) = res
    .headers()
    .get(VERSION_HEADER_NAME)
    .and_then(|v| v.to_str().ok())
  {
    if version != env!("CARGO_PKG_VERSION") {
      warn!(
        "Warning: Server version ({version}) does not match client version ({}). Consider updating your CLI.",
        env!("CARGO_PKG_VERSION")
      );
    }
  } else {
    warn!("Warning: Server did not provide version information. Consider updating your CLI.");
  }
}
