use aide::axum::ApiRouter;
use aide::axum::routing::get_with;
use centaurus::{db::init::Connection, error::Result, storage::FileStorage};

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
  let storage = FileStorage::init(&config.storage)
    .await
    .expect("Failed to init FileStorage");
  let push_state = PushState::new(config.max_concurrent_uploads);

  cleanup::start(db.clone(), storage.clone());
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
