use std::{marker::PhantomData, sync::Arc, time::Instant};

use aide::OperationIo;
use axum::{
  Extension,
  extract::{FromRequestParts, OptionalFromRequestParts},
};
use centaurus::{
  backend::{
    auth::{
      jwt::jwt_from_request,
      jwt_state::{JWT_COOKIE_NAME, JwtState},
      permission::{NoPerm, Permission},
      pw_state::PasswordState,
    },
    request::extract::StateExtractExt,
  },
  bail,
  db::init::Connection,
  error::{ErrorReport, ErrorReportStatusExt},
};
use chrono::{NaiveDateTime, Utc};
use dashmap::DashMap;
use http::{StatusCode, request::Parts};
use sea_orm::DbErr;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
  db::DBTrait,
  utils::{UpdateMessage, Updater, fresh},
};

pub const CLI_TOKEN_LEN: usize = 32;

#[derive(Clone, Copy)]
struct VerifiedToken {
  id: Uuid,
  user_id: Uuid,
  exp: NaiveDateTime,
}

type TokenKey = ([u8; 32], &'static str);

#[derive(Clone, Default, FromRequestParts, OperationIo)]
#[from_request(via(Extension))]
pub struct CliTokenCache {
  tokens: Arc<DashMap<TokenKey, (Instant, VerifiedToken)>>,
}

impl CliTokenCache {
  pub fn invalidate(&self, token_id: Uuid) {
    self.tokens.retain(|_, (_, token)| token.id != token_id);
  }

  pub fn clear(&self) {
    self.tokens.clear();
  }
}

#[derive(Debug, OperationIo)]
pub struct CliAuth<P: Permission = NoPerm> {
  pub user_id: Uuid,
  _perm: PhantomData<P>,
}

impl<S: Sync, P: Permission> FromRequestParts<S> for CliAuth<P> {
  type Rejection = ErrorReport;

  async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
    let token = jwt_from_request(parts, JWT_COOKIE_NAME).await?;

    let db = parts.extract_state::<Connection>().await;
    let user = if token.len() == CLI_TOKEN_LEN {
      let cache = parts.extract_state::<CliTokenCache>().await;
      let key = (Sha256::digest(&token).into(), P::name());

      match fresh(&cache.tokens, &key) {
        Some(token) if token.exp > Utc::now().naive_utc() => token.user_id,
        _ => {
          let token = check_token(&db, parts, token).await?;
          P::check(&db, token.user_id, parts).await?;
          cache.tokens.insert(key, (Instant::now(), token));
          token.user_id
        }
      }
    } else {
      let state = parts.extract_state::<JwtState>().await;

      let Ok(claims) = state.validate_token(&token) else {
        tracing::debug!("invalid token claims");
        bail!(UNAUTHORIZED, "invalid token");
      };
      state.auth.check(&db, parts, &token, &claims).await?;
      P::check(&db, claims.sub, parts).await?;

      claims.sub
    };

    Ok(CliAuth {
      user_id: user,
      _perm: PhantomData,
    })
  }
}

impl<S: Sync, P: Permission> OptionalFromRequestParts<S> for CliAuth<P> {
  type Rejection = ErrorReport;

  async fn from_request_parts(
    parts: &mut Parts,
    state: &S,
  ) -> Result<Option<Self>, Self::Rejection> {
    match <Self as FromRequestParts<S>>::from_request_parts(parts, state).await {
      Ok(auth) => Ok(Some(auth)),
      Err(err) if err.status.is_server_error() => Err(err),
      Err(_) => Ok(None),
    }
  }
}

async fn check_token(
  db: &Connection,
  parts: &mut Parts,
  token: String,
) -> Result<VerifiedToken, ErrorReport> {
  let pw = parts.extract_state::<PasswordState>().await;
  let hash = tokio::task::spawn_blocking(move || pw.pw_hash_raw("", &token))
    .await
    .status(StatusCode::INTERNAL_SERVER_ERROR)??;

  let record = match db.token().get_by_token(&hash).await {
    Ok(record) => record,
    Err(DbErr::RecordNotFound(_)) => {
      bail!(UNAUTHORIZED, "invalid token");
    }
    Err(err) => return Err(err.into()),
  };

  if record.exp < Utc::now().naive_utc() {
    bail!("CLI token expired");
  }

  db.token().token_used(record.id).await?;
  let updater = parts.extract_state::<Updater>().await;
  updater
    .send_to(record.user_id, UpdateMessage::Token { uuid: record.id })
    .await;

  Ok(VerifiedToken {
    id: record.id,
    user_id: record.user_id,
    exp: record.exp,
  })
}
