use std::io::IsTerminal;

use centaurus::error::{ErrorReportExt, Result};
use clap::Parser;
use tracing::level_filters::LevelFilter;
use tracing_error::ErrorLayer;
use tracing_indicatif::IndicatifLayer;
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{cli::Cli, config::Config};

mod api;
mod auth;
mod cli;
mod config;
mod push;

pub fn init_logging(log_level: LevelFilter) {
  color_eyre::install().expect("Failed to install color_eyre");

  let indicatif_layer = IndicatifLayer::new();
  let layer = tracing_subscriber::fmt::layer()
    .with_writer(indicatif_layer.get_stderr_writer())
    .with_ansi(true)
    .with_ansi_sanitization(false)
    .with_filter(log_level);

  tracing_subscriber::registry()
    .with(layer)
    .with(ErrorLayer::default())
    .with(
      indicatif_layer.with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
        !metadata.target().starts_with("h2")
      })),
    )
    .init();
}

#[tokio::main]
async fn main() -> Result<()> {
  let level = config::log_level();

  let tty = std::io::stdin().is_terminal();
  if tty {
    init_logging(level);
  } else {
    centaurus::logging::init_logging(level);
  }

  let cli = Cli::parse();
  let config = Config::load(cli.config.clone())
    .await
    .context("Failed to load config")?;

  let url = cli
    .url
    .clone()
    .or_else(|| config.as_ref().map(|c| c.app_url.clone()));

  if let Err(e) = cli.run(config, url, tty).await {
    eprintln!("Error: {e:?}");
    std::process::exit(1);
  }

  // Tokio does not exit when other tasks are still running (e.g. blocking stdin reads)
  std::process::exit(0);
}
