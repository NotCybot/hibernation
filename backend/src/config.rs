use aide::OperationIo;
use axum::{Extension, extract::FromRequestParts};
use centaurus::{
  Config,
  backend::{
    auth::settings::{AuthConfig, UserSettings},
    config::{BaseConfig, MetricsConfig, SiteConfig},
  },
  db::config::DBConfig,
  mail::MailSettings,
  storage::StorageConfig,
};
use figment::{
  Figment,
  providers::{Env, Serialized},
};
use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};

#[derive(Deserialize, Serialize, Clone, FromRequestParts, OperationIo, Config)]
#[from_request(via(Extension))]
pub struct Config {
  #[base]
  #[serde(flatten)]
  pub base: BaseConfig,
  #[serde(flatten)]
  pub db: DBConfig,
  #[metrics]
  #[serde(flatten)]
  pub metrics: MetricsConfig,
  #[serde(flatten)]
  pub storage: StorageConfig,
  #[site]
  #[serde(flatten)]
  pub site: SiteConfig,
  #[auth]
  #[serde(flatten)]
  pub auth: AuthConfig,
  #[mail]
  #[serde(flatten)]
  pub mail: MailSettings,
  #[oidc]
  #[serde(flatten)]
  pub oidc: UserSettings,

  pub db_url: String,
  pub admin_group: String,
  pub virtual_host_routing: bool,
  pub max_concurrent_uploads: usize,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      base: BaseConfig::default(),
      db: DBConfig::default(),
      site: SiteConfig::default(),
      mail: MailSettings::default(),
      oidc: UserSettings::default(),
      db_url: "".to_string(),
      admin_group: "Admin".to_string(),
      virtual_host_routing: false,
      max_concurrent_uploads: 8,
      metrics: MetricsConfig {
        metrics_name: "hibernation".to_string(),
        ..Default::default()
      },
      storage: StorageConfig::default(),
      auth: AuthConfig {
        auth_pepper: "__HIBERNATION_PEPPER__".to_string(),
        ..Default::default()
      },
    }
  }
}

impl Config {
  #[instrument]
  pub fn parse() -> Self {
    let config = Figment::new()
      .merge(Serialized::defaults(Self::default()))
      .merge(Env::raw().global());

    let mut config: Self = config.extract().expect("Failed to parse configuration");

    if config.db_url.is_empty() {
      panic!("DB_URL is not set");
    }

    if config.db_url.starts_with("sqlite") {
      config.db.validate_sqlite();
    }

    if config.max_concurrent_uploads == 0 {
      panic!("MAX_CONCURRENT_UPLOADS must be greater than 0");
    }

    config.storage.validate();

    config
  }
}

#[cfg(test)]
mod test {
  use super::Config;

  /// nextest runs every test in its own process, so mutating the env is isolated per test.
  fn parse_with(vars: &[(&str, &str)]) -> Config {
    for key in [
      "DB_URL",
      "STORAGE_PATH",
      "DATABASE_MAX_CONNECTIONS",
      "DATABASE_MIN_CONNECTIONS",
      "MAX_CONCURRENT_UPLOADS",
      "VIRTUAL_HOST_ROUTING",
      "ADMIN_GROUP",
      "S3_BUCKET",
      "S3_REGION",
      "S3_HOST",
      "S3_ACCESS_KEY",
      "S3_SECRET_KEY",
    ] {
      unsafe { std::env::remove_var(key) };
    }
    for (key, value) in vars {
      unsafe { std::env::set_var(key, value) };
    }
    Config::parse()
  }

  const PG: (&str, &str) = ("DB_URL", "postgres://u:p@localhost/db");
  const STORAGE: (&str, &str) = ("STORAGE_PATH", "/tmp/hibernation");

  #[test]
  fn defaults() {
    let config = parse_with(&[PG, STORAGE]);
    assert_eq!(config.db_url, PG.1);
    assert_eq!(config.admin_group, "Admin");
    assert!(!config.virtual_host_routing);
    assert_eq!(config.max_concurrent_uploads, 8);
    assert_eq!(config.metrics.metrics_name, "hibernation");
    assert_eq!(config.auth.auth_pepper, "__HIBERNATION_PEPPER__");
    // postgres keeps the configured pool
    assert_eq!(config.db.database_max_connections, 20);
    assert!(!config.storage.use_s3());
  }

  #[test]
  fn overrides_from_env() {
    let config = parse_with(&[
      PG,
      STORAGE,
      ("VIRTUAL_HOST_ROUTING", "true"),
      ("MAX_CONCURRENT_UPLOADS", "3"),
      ("ADMIN_GROUP", "Root"),
      ("DATABASE_MAX_CONNECTIONS", "7"),
    ]);
    assert!(config.virtual_host_routing);
    assert_eq!(config.max_concurrent_uploads, 3);
    assert_eq!(config.admin_group, "Root");
    assert_eq!(config.db.database_max_connections, 7);
  }

  #[test]
  fn sqlite_forces_single_connection() {
    let config = parse_with(&[
      ("DB_URL", "sqlite::memory:"),
      STORAGE,
      ("DATABASE_MAX_CONNECTIONS", "10"),
      ("DATABASE_MIN_CONNECTIONS", "5"),
    ]);
    assert_eq!(config.db.database_max_connections, 1);
    assert_eq!(config.db.database_min_connections, 1);
  }

  #[test]
  #[should_panic(expected = "DB_URL is not set")]
  fn missing_db_url_panics() {
    parse_with(&[STORAGE]);
  }

  #[test]
  #[should_panic(expected = "DB_URL is not set")]
  fn empty_db_url_panics() {
    parse_with(&[("DB_URL", ""), STORAGE]);
  }

  #[test]
  #[should_panic(expected = "MAX_CONCURRENT_UPLOADS must be greater than 0")]
  fn zero_concurrent_uploads_panics() {
    parse_with(&[PG, STORAGE, ("MAX_CONCURRENT_UPLOADS", "0")]);
  }

  #[test]
  #[should_panic(expected = "Failed to parse configuration")]
  fn invalid_value_panics() {
    parse_with(&[PG, STORAGE, ("MAX_CONCURRENT_UPLOADS", "many")]);
  }

  #[test]
  fn full_s3_config_enables_s3_without_storage_path() {
    let config = parse_with(&[
      PG,
      ("S3_BUCKET", "b"),
      ("S3_REGION", "r"),
      ("S3_HOST", "http://localhost:9000"),
      ("S3_ACCESS_KEY", "a"),
      ("S3_SECRET_KEY", "s"),
    ]);
    assert!(config.storage.use_s3());
  }

  #[test]
  fn partial_s3_config_falls_back_to_storage_path() {
    let config = parse_with(&[PG, STORAGE, ("S3_BUCKET", "b")]);
    assert!(!config.storage.use_s3());
  }

  #[test]
  #[should_panic(expected = "STORAGE_PATH is not set")]
  fn partial_s3_without_storage_path_panics() {
    parse_with(&[PG, ("STORAGE_PATH", ""), ("S3_BUCKET", "b")]);
  }
}
