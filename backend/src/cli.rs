use aide::axum::routing::{post_with, put_with};
use std::{
  sync::Arc,
  time::{Duration, Instant},
};

use aide::{OperationIo, axum::ApiRouter};
use axum::{
  Extension, Json,
  extract::{FromRequestParts, Query},
};
use centaurus::{
  backend::{
    auth::{jwt_auth::JwtAuth, jwt_state::JwtState, pw_state::PasswordState},
    middleware::rate_limiter::RateLimiter,
  },
  bail,
  db::init::Connection,
  error::Result,
};
use chrono::Utc;
use dashmap::DashMap;
use rand::{RngExt, distr::Alphanumeric};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::{spawn, time::sleep};
use uuid::Uuid;

use crate::{
  auth::cli_auth::CLI_TOKEN_LEN,
  db::DBTrait,
  utils::{UpdateMessage, Updater},
};

pub fn router(rate_limiter: &mut RateLimiter) -> ApiRouter {
  ApiRouter::new()
    .api_route("/", put_with(get_token, |op| op.id("getToken")))
    .layer(rate_limiter.create_limiter())
    .api_route("/", post_with(new_code, |op| op.id("newCode")))
}

pub fn state(router: ApiRouter) -> ApiRouter {
  let cli_state = CliState {
    codes: Arc::new(DashMap::new()),
  };

  spawn({
    let codes = cli_state.codes.clone();

    async move {
      loop {
        codes.retain(|_, (time, _)| time.elapsed() < CODE_TTL);
        sleep(Duration::from_secs(600)).await;
      }
    }
  });

  router.layer(Extension(cli_state))
}

const CODE_TTL: Duration = Duration::from_secs(300);

/// Consume a login code, returning the user it was issued for. Removal is atomic so a code can only ever be
/// exchanged once, even by concurrent requests.
fn take_code(codes: &DashMap<Uuid, (Instant, Uuid)>, code: &Uuid) -> Result<Uuid> {
  let Some((_, (issued, user))) = codes.remove(code) else {
    bail!("Invalid code");
  };
  if issued.elapsed() > CODE_TTL {
    bail!("Invalid user or code");
  }
  Ok(user)
}

#[derive(FromRequestParts, Clone, OperationIo)]
#[from_request(via(Extension))]
struct CliState {
  codes: Arc<DashMap<Uuid, (Instant, Uuid)>>,
}

#[derive(Serialize, JsonSchema)]
struct CodeResponse {
  code: String,
}

async fn new_code(auth: JwtAuth, state: CliState) -> Json<CodeResponse> {
  let code = Uuid::new_v4();
  state.codes.insert(code, (Instant::now(), auth.user_id));
  Json(CodeResponse {
    code: code.to_string(),
  })
}

#[derive(Clone, Deserialize, JsonSchema)]
struct TokenReq {
  code: Uuid,
}

async fn get_token(
  Query(token_req): Query<TokenReq>,
  state: CliState,
  db: Connection,
  jwt: JwtState,
  pw: PasswordState,
  updater: Updater,
) -> Result<String> {
  let user = take_code(&state.codes, &token_req.code)?;

  let Some(exp) = Utc::now().checked_add_signed(chrono::Duration::seconds(jwt.exp)) else {
    bail!(INTERNAL_SERVER_ERROR, "Failed to create token");
  };

  let token: String = gen_token();
  let hash = pw.pw_hash_raw("", &token)?;

  let token_model = db
    .token()
    .insert(
      user,
      format!("Cli-{}", Utc::now().to_rfc3339()),
      hash,
      exp.naive_utc(),
    )
    .await?;
  updater
    .send_to(
      user,
      UpdateMessage::Token {
        uuid: token_model.id,
      },
    )
    .await;

  Ok(token)
}

pub fn gen_token() -> String {
  let mut rng = rand::rng();
  (0..CLI_TOKEN_LEN)
    .map(|_| rng.sample(Alphanumeric) as char)
    .collect()
}

#[cfg(test)]
mod test {
  use std::time::{Duration, Instant};

  use dashmap::DashMap;
  use http::StatusCode;
  use uuid::Uuid;

  use super::{CODE_TTL, gen_token, take_code};
  use crate::auth::cli_auth::CLI_TOKEN_LEN;

  #[test]
  fn take_code_is_single_use() {
    let codes = DashMap::new();
    let (code, user) = (Uuid::new_v4(), Uuid::new_v4());
    codes.insert(code, (Instant::now(), user));

    assert_eq!(take_code(&codes, &code).unwrap(), user);
    assert!(codes.is_empty());
    let err = take_code(&codes, &code).unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST);
  }

  #[test]
  fn take_code_unknown_is_bad_request() {
    let codes = DashMap::new();
    let err = take_code(&codes, &Uuid::new_v4()).unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST);
  }

  #[test]
  fn take_code_expired_is_rejected_and_removed() {
    let codes = DashMap::new();
    let code = Uuid::new_v4();
    let issued = Instant::now() - CODE_TTL - Duration::from_secs(1);
    codes.insert(code, (issued, Uuid::new_v4()));

    let err = take_code(&codes, &code).unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert!(codes.is_empty());
  }

  #[test]
  fn gen_token_is_random_alphanumeric_of_cli_len() {
    let a = gen_token();
    assert_eq!(a.len(), CLI_TOKEN_LEN);
    assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
    assert_ne!(a, gen_token());
  }
}
