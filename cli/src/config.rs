use std::{env, path::PathBuf, str::FromStr};

use centaurus::{error::Result, eyre::ContextCompat};
use dirs::data_dir;
use serde::{Deserialize, Serialize};
use tracing::level_filters::LevelFilter;
use url::Url;

pub fn log_level() -> LevelFilter {
  for key in ["LOG_LEVEL", "RUST_LOG"] {
    if let Ok(raw_level) = env::var(key)
      && let Ok(level) = LevelFilter::from_str(&raw_level)
    {
      return level;
    }
  }
  LevelFilter::INFO
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Config {
  pub app_url: Url,
  pub token: Option<String>,
}

impl Config {
  fn path() -> Result<PathBuf> {
    let data_dir = data_dir().context("Could not determine data directory")?;
    Ok(data_dir.join("hibernation").join("config.json"))
  }

  pub fn new(app_url: Url, token: Option<String>) -> Self {
    Self { app_url, token }
  }

  pub async fn load(path: Option<PathBuf>) -> Result<Option<Self>> {
    let path = path.map_or_else(Self::path, Ok)?;
    if !path.exists() {
      return Ok(None);
    }

    let data = tokio::fs::read_to_string(path).await?;
    Ok(serde_json::from_str(&data)?)
  }

  pub async fn save(&self, path: Option<PathBuf>) -> Result<()> {
    let path = path.map_or_else(Self::path, Ok)?;
    if let Some(parent) = path.parent() {
      tokio::fs::create_dir_all(parent).await?;
    }
    let data = serde_json::to_string_pretty(self)?;
    tokio::fs::write(path, data).await?;
    Ok(())
  }
}

#[cfg(test)]
mod test {
  use tempfile::TempDir;
  use tracing::level_filters::LevelFilter;
  use url::Url;

  use super::{Config, log_level};

  #[tokio::test]
  async fn save_load_round_trip_creates_parents() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nested/dir/config.json");
    let config = Config::new(Url::parse("http://a/").unwrap(), Some("tok".into()));
    config.save(Some(path.clone())).await.unwrap();

    let loaded = Config::load(Some(path.clone())).await.unwrap().unwrap();
    assert_eq!(loaded.app_url, config.app_url);
    assert_eq!(loaded.token, config.token);

    let raw: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
      raw,
      serde_json::json!({ "app_url": "http://a/", "token": "tok" })
    );
  }

  #[tokio::test]
  async fn load_missing_is_none() {
    let dir = TempDir::new().unwrap();
    assert!(
      Config::load(Some(dir.path().join("nope.json")))
        .await
        .unwrap()
        .is_none()
    );
  }

  #[tokio::test]
  async fn load_invalid_errors() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.json");
    for bad in ["{", "{}", r#"{"app_url": "not a url"}"#] {
      std::fs::write(&path, bad).unwrap();
      assert!(Config::load(Some(path.clone())).await.is_err(), "{bad}");
    }
    // Token is optional
    std::fs::write(&path, r#"{"app_url": "http://a/"}"#).unwrap();
    assert_eq!(Config::load(Some(path)).await.unwrap().unwrap().token, None);
  }

  #[tokio::test]
  async fn save_to_directory_errors() {
    let dir = TempDir::new().unwrap();
    let config = Config::new(Url::parse("http://a/").unwrap(), None);
    assert!(config.save(Some(dir.path().to_path_buf())).await.is_err());
    assert!(Config::load(Some(dir.path().to_path_buf())).await.is_err());
  }

  /// One test for all env cases, `cargo test` runs tests of a binary in parallel threads
  #[test]
  fn log_level_from_env() {
    let set = |log: Option<&str>, rust: Option<&str>| unsafe {
      match log {
        Some(v) => std::env::set_var("LOG_LEVEL", v),
        None => std::env::remove_var("LOG_LEVEL"),
      }
      match rust {
        Some(v) => std::env::set_var("RUST_LOG", v),
        None => std::env::remove_var("RUST_LOG"),
      }
    };

    set(None, None);
    assert_eq!(log_level(), LevelFilter::INFO);
    set(Some("debug"), Some("error"));
    assert_eq!(log_level(), LevelFilter::DEBUG);
    set(None, Some("WARN"));
    assert_eq!(log_level(), LevelFilter::WARN);
    // Invalid LOG_LEVEL falls through to RUST_LOG
    set(Some("loud"), Some("trace"));
    assert_eq!(log_level(), LevelFilter::TRACE);
    set(Some("loud"), Some("hibernation=debug"));
    assert_eq!(log_level(), LevelFilter::INFO);
    set(Some("off"), None);
    assert_eq!(log_level(), LevelFilter::OFF);
    set(None, None);
  }
}
