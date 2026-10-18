use aide::axum::ApiRouter;
use aide::axum::routing::{delete_with, get_with, post_with, put_with};
use axum::{Json, extract::Path};
use centaurus::backend::auth::jwt_auth::JwtAuth;
use centaurus::backend::auth::pw_state::PasswordState;
use centaurus::{bail, db::init::Connection, error::Result};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::cli_auth::CliTokenCache;
use crate::utils::{UpdateMessage, Updater};
use crate::{cli, db::DBTrait};

pub fn router() -> ApiRouter {
  ApiRouter::new()
    .api_route("/", get_with(list_tokens, |op| op.id("listTokens")))
    .api_route("/", post_with(create_token, |op| op.id("createToken")))
    .api_route("/", delete_with(delete_token, |op| op.id("deleteToken")))
    .api_route("/", put_with(edit_token, |op| op.id("editToken")))
    .api_route(
      "/expired",
      delete_with(delete_expired_tokens, |op| op.id("deleteExpiredTokens")),
    )
    .api_route("/{uuid}", get_with(token_info, |op| op.id("tokenInfo")))
    .api_route(
      "/{uuid}",
      post_with(token_regenerate, |op| op.id("tokenRegenerate")),
    )
}

#[derive(Serialize, JsonSchema)]
struct TokenInfo {
  uuid: Uuid,
  name: String,
  exp: DateTime<Utc>,
  last_used: Option<DateTime<Utc>>,
}

impl From<entity::token::Model> for TokenInfo {
  fn from(value: entity::token::Model) -> Self {
    Self {
      uuid: value.id,
      name: value.name,
      exp: DateTime::from_naive_utc_and_offset(value.exp, Utc),
      last_used: value
        .last_used
        .map(|d| DateTime::from_naive_utc_and_offset(d, Utc)),
    }
  }
}

async fn list_tokens(auth: JwtAuth, db: Connection) -> Result<Json<Vec<TokenInfo>>> {
  let tokens = db.token().get_by_user(auth.user_id).await?;
  let token_info = tokens.into_iter().map(|t| t.into()).collect();
  Ok(Json(token_info))
}

#[derive(Deserialize, JsonSchema)]
struct TokenViewPath {
  uuid: Uuid,
}

async fn token_info(
  auth: JwtAuth,
  db: Connection,
  Path(path): Path<TokenViewPath>,
) -> Result<Json<TokenInfo>> {
  let info = db.token().by_id_user(path.uuid, auth.user_id).await?;
  let Some(info) = info else {
    bail!(NOT_FOUND, "Token not found");
  };
  Ok(Json(info.into()))
}

#[derive(Deserialize, JsonSchema)]
struct CreateTokenRequest {
  name: String,
  exp: DateTime<Utc>,
}

#[derive(Serialize, JsonSchema)]
struct CreateTokenResponse {
  token: String,
  uuid: Uuid,
}

async fn create_token(
  auth: JwtAuth,
  db: Connection,
  pw: PasswordState,
  updater: Updater,
  Json(req): Json<CreateTokenRequest>,
) -> Result<Json<CreateTokenResponse>> {
  if req.name.trim().is_empty() {
    bail!(BAD_REQUEST, "Token name cannot be empty");
  }

  if db
    .token()
    .get_by_name(auth.user_id, &req.name)
    .await
    .is_ok()
  {
    bail!(CONFLICT, "A token with this name already exists");
  }

  let token = cli::gen_token();
  let hash = pw.pw_hash_raw("", &token)?;

  let uuid = db
    .token()
    .insert(auth.user_id, req.name, hash, req.exp.naive_utc())
    .await?
    .id;
  updater
    .send_to(auth.user_id, UpdateMessage::Token { uuid })
    .await;

  Ok(Json(CreateTokenResponse { token, uuid }))
}

#[derive(Deserialize, JsonSchema)]
struct DeleteTokenRequest {
  uuid: Uuid,
}

async fn delete_token(
  auth: JwtAuth,
  db: Connection,
  updater: Updater,
  token_cache: CliTokenCache,
  Json(req): Json<DeleteTokenRequest>,
) -> Result<()> {
  db.token().invalidate(auth.user_id, req.uuid).await?;
  token_cache.invalidate(req.uuid);
  updater
    .send_to(auth.user_id, UpdateMessage::Token { uuid: req.uuid })
    .await;
  Ok(())
}

#[derive(Serialize, JsonSchema)]
struct DeleteExpiredTokensResponse {
  deleted: u64,
}

async fn delete_expired_tokens(
  auth: JwtAuth,
  db: Connection,
  updater: Updater,
) -> Result<Json<DeleteExpiredTokensResponse>> {
  let deleted = db.token().invalidate_expired(auth.user_id).await?;
  if deleted > 0 {
    updater
      .send_to(auth.user_id, UpdateMessage::Token { uuid: Uuid::nil() })
      .await;
  }
  Ok(Json(DeleteExpiredTokensResponse { deleted }))
}

#[derive(Deserialize, JsonSchema)]
struct EditTokenRequest {
  uuid: Uuid,
  name: String,
  exp: DateTime<Utc>,
}

async fn edit_token(
  auth: JwtAuth,
  db: Connection,
  updater: Updater,
  token_cache: CliTokenCache,
  Json(req): Json<EditTokenRequest>,
) -> Result<()> {
  if req.name.trim().is_empty() {
    bail!(BAD_REQUEST, "Token name cannot be empty");
  }

  if let Ok(token) = db.token().get_by_name(auth.user_id, &req.name).await
    && token.id != req.uuid
  {
    bail!(CONFLICT, "A token with this name already exists");
  }

  if !db
    .token()
    .update(auth.user_id, req.uuid, req.name, req.exp.naive_utc())
    .await?
  {
    bail!(NOT_FOUND, "Token not found");
  }
  token_cache.invalidate(req.uuid);
  updater
    .send_to(auth.user_id, UpdateMessage::Token { uuid: req.uuid })
    .await;
  Ok(())
}

#[derive(Serialize, JsonSchema)]
struct TokenRegenerateResponse {
  token: String,
}

async fn token_regenerate(
  auth: JwtAuth,
  db: Connection,
  pw: PasswordState,
  token_cache: CliTokenCache,
  updater: Updater,
  Path(path): Path<TokenViewPath>,
) -> Result<Json<TokenRegenerateResponse>> {
  let token = cli::gen_token();
  let hash = pw.pw_hash_raw("", &token)?;

  if !db.token().replace(auth.user_id, path.uuid, hash).await? {
    bail!(NOT_FOUND, "Token not found");
  }
  token_cache.invalidate(path.uuid);
  updater
    .send_to(auth.user_id, UpdateMessage::Token { uuid: path.uuid })
    .await;

  Ok(Json(TokenRegenerateResponse { token }))
}
