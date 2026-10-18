use aide::axum::ApiRouter;
use axum::{Extension, Router};
use centaurus::{
  backend::{
    endpoints::{mail, setup, websocket},
    init::{listener_setup, run_app_connect_info},
    middleware::rate_limiter::RateLimiter,
    rewrite::virtual_host::HostRouter,
    router::build_router,
  },
  db::init::init_db,
  logging::init_logging,
  version_header,
};
use tokio::net::TcpListener;
use tracing::info;

use crate::{auth::cli_auth::CliTokenCache, config::Config, nix::NixCache, utils::UpdateMessage};

pub mod auth;
pub mod cache;
pub mod cli;
pub mod config;
pub mod db;
pub mod group;
pub mod nix;
pub mod settings;
pub mod token;
pub mod user;
pub mod utils;

pub async fn serve() {
  let config = Config::parse();
  init_logging(config.base.log_level);

  App::from_config(config).await.run().await;
}

pub struct App {
  app: Router,
  listener: TcpListener,
  host_routing: Option<url::Url>,
}

impl App {
  pub async fn new() -> App {
    App::from_config(Config::parse()).await
  }

  pub async fn from_config(config: Config) -> App {
    let listener = listener_setup(config.base.port).await;
    let host_routing = config
      .virtual_host_routing
      .then(|| config.site.site_url.clone());
    let mut app = build_router(api_router, state, config).await;
    version_header!(app);

    App {
      app,
      listener,
      host_routing,
    }
  }

  pub fn port(&self) -> u16 {
    self
      .listener
      .local_addr()
      .expect("Failed to read listener address")
      .port()
  }

  pub async fn run(self) {
    if let Some(site_url) = self.host_routing {
      let host_router = HostRouter::new(self.app, site_url, "/api/nix/{subdomain}{path}".into());
      run_app_connect_info(self.listener, host_router).await;
    } else {
      info!("Starting application");
      run_app_connect_info(self.listener, self.app).await;
    }
  }
}

fn api_router(rate_limiter: &mut RateLimiter) -> ApiRouter {
  ApiRouter::new()
    .nest("/ws", websocket::router::<UpdateMessage>())
    .nest("/setup", setup::router())
    .nest("/auth", auth::router(rate_limiter))
    .nest("/user", user::router(rate_limiter))
    .nest("/settings", settings::router())
    .nest("/mail", mail::router(rate_limiter))
    .nest("/group", group::router())
    .nest("/cli", cli::router(rate_limiter))
    .nest("/cache", cache::router())
    .nest("/token", token::router())
    .nest("/nix", nix::router())
}

async fn state(mut router: ApiRouter, config: Config) -> ApiRouter {
  let db = init_db::<migration::Migrator>(&config.db, &config.db_url).await;
  centaurus::backend::endpoints::setup::create_admin_group(
    &db,
    utils::permissions(),
    Some(config.admin_group.clone()),
  )
  .await
  .expect("Failed to create admin group");

  router = auth::state(router, &config, &db).await;
  router = mail::state(router, &db, &config).await;
  router = cli::state(router);
  router = cache::state(router, db.clone(), &config).await;
  router = websocket::state::<UpdateMessage>(router).await;

  router
    .layer(Extension(CliTokenCache::default()))
    .layer(Extension(NixCache::default()))
    .layer(Extension(db))
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn api_router_builds_all_module_routers() {
    let mut rate_limiter = RateLimiter::default();
    let _ = api_router(&mut rate_limiter);
  }
}
