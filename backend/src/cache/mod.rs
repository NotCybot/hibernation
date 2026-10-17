use std::time::Duration;

use aide::axum::ApiRouter;
use aide::axum::routing::get_with;
use centaurus::{
  db::init::Connection,
  error::Result,
  storage::{ClientOptions, FileStorage},
};

use crate::{
  auth::cli_auth::CliAuth,
  cache::{management::CacheRegex, push::PushState, state::CacheEvictionState},
  config::Config,
};

mod cleanup;
mod dedupe;
mod management;
mod push;
mod state;

pub fn router() -> ApiRouter {
  ApiRouter::new()
    .nest("/management", management::router())
    .nest("/push", push::router())
    .api_route("/test", get_with(test, |op| op.id("test")))
}

pub async fn state(router: ApiRouter, db: Connection, config: &Config) -> ApiRouter {
  // NAR streams can outlive the default 30s request timeout (e.g. slow downloads), which
  // object_store only retries for 3 minutes before cutting the stream. The read timeout
  // still catches stalled S3 connections as it resets on every read.
  let client_options = ClientOptions::new()
    .with_timeout_disabled()
    .with_read_timeout(Duration::from_secs(60));
  let storage = FileStorage::init_with_client_options(&config.storage, client_options)
    .await
    .expect("Failed to init FileStorage");
  let push_state = PushState::new(config.max_concurrent_uploads);

  let local_path = (!config.storage.use_s3()).then(|| config.storage.storage_path.clone().into());
  cleanup::start(db.clone(), storage.clone(), local_path);
  dedupe::start(db);

  router
    .layer(axum::Extension(storage))
    .layer(axum::Extension(push_state))
    .layer(axum::Extension(CacheEvictionState::new()))
    .layer(axum::Extension(CacheRegex::new()))
}

async fn test(auth: CliAuth) -> Result<String> {
  Ok(auth.user_id.to_string())
}
