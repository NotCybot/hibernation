use std::path::PathBuf;

use centaurus::{
  bail,
  error::{ErrorReportExt, Result},
  eyre::ContextCompat,
};
use clap::{Parser, Subcommand};
use shared::sig::KeyPair;
use tracing::info;
use url::Url;

use crate::{api::ApiClient, config::Config, push};

const SIGING_KEY_ENV_VAR: &str = "HIBERNATION_SIGNING_KEY";

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
  /// The URL of the Hibernation server to connect to.
  #[arg(long, short, global = true)]
  pub url: Option<Url>,

  /// The config file to use
  #[arg(long, short, global = true)]
  pub config: Option<PathBuf>,

  #[command(subcommand)]
  pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
  /// Set the URL of the Hibernation server to connect to.
  SetUrl {
    /// The URL to use for future connections to the Hibernation server.
    // Own id, otherwise clap merges it with the global `--url` arg
    #[arg(id = "new_url", value_name = "URL")]
    url: Url,
  },
  /// Authenticate with a Hibernation server.
  Auth {
    /// The token to use for authentication
    token: Option<String>,
  },
  #[command(flatten)]
  AuthenticatedCommand(AuthenticatedCommand),
}

#[derive(Subcommand)]
pub enum AuthenticatedCommand {
  /// A test command to check if the authentication works.
  Test,
  /// Push the given paths and all their dependencies to the given cache on the Hibernation server.
  Push {
    /// The cache to push to.
    cache: String,
    /// The paths to push.
    paths: Vec<String>,
    /// Only upload the given paths and not its dependencies.
    #[arg(long)]
    no_deps: bool,
    /// Push paths even if they already exist in a downstream cache. The cache needs to be configured to allow this.
    #[arg(long)]
    force: bool,
    /// Path of the key to use for signing.
    #[arg(long)]
    key: Option<PathBuf>,
    /// Number of upload threads to use. Defaults to 5.
    #[arg(long, default_value_t = 5)]
    threads: usize,
  },
}

impl Cli {
  pub async fn run(self, config: Option<Config>, url: Option<Url>, tty: bool) -> Result<()> {
    match self.command {
      Commands::SetUrl { url } => {
        let mut config = config.unwrap_or_else(|| Config::new(url.clone(), None));
        config.app_url = url;
        config
          .save(self.config)
          .await
          .context("Failed to save config")?;
      }
      Commands::Auth { token } => {
        let Some(url) = url else {
          bail!(
            "No URL specified. Please provide a URL using --url or set it using the set-url command."
          );
        };

        // `url` is `--url` or the configured one, so assigning it keeps the config unless overridden
        let mut config = config.unwrap_or_else(|| Config::new(url.clone(), None));
        config.app_url = url;

        if let Some(token) = token {
          config.token = Some(token);
          config
            .save(self.config)
            .await
            .context("Failed to save config")?;
          info!("Token saved successfully.");
        } else if tty {
          crate::auth::get_tty_token(&mut config, self.config).await;
        } else {
          bail!("No token given. Pass it as an argument or run this in a terminal to log in.");
        }
      }
      Commands::AuthenticatedCommand(cmd) => cmd.run(config, url, self.config, tty).await?,
    }
    Ok(())
  }
}

impl AuthenticatedCommand {
  async fn run(
    self,
    config: Option<Config>,
    url: Option<Url>,
    config_path: Option<PathBuf>,
    tty: bool,
  ) -> Result<()> {
    let client = ApiClient::build(config, url, config_path, tty).await?;

    match self {
      AuthenticatedCommand::Test => {
        client.test().await.context("Test request failed")?;
        info!("Test request succeeded.");
      }
      AuthenticatedCommand::Push {
        cache,
        paths,
        no_deps,
        force,
        key,
        threads,
      } => {
        let signing_key = load_signing_key(key)
          .await
          .context("Failed to load signing key")?;
        push::push_paths(client, cache, &paths, no_deps, force, signing_key, threads).await?;
      }
    }
    Ok(())
  }
}

async fn load_signing_key(path: Option<PathBuf>) -> Result<KeyPair> {
  if let Some(path) = path {
    let key_data = tokio::fs::read_to_string(path).await?;
    Ok(KeyPair::from_string(key_data.trim()).context("Invalid signing key format in file")?)
  } else {
    let key_data = std::env::var(SIGING_KEY_ENV_VAR).ok().context(format!(
      "Signing key not found in environment variable {SIGING_KEY_ENV_VAR}"
    ))?;
    Ok(KeyPair::from_string(key_data.trim()).context("Invalid signing key format from env")?)
  }
}

#[cfg(test)]
mod test {
  use std::path::PathBuf;

  use clap::{CommandFactory, Parser, error::ErrorKind};
  use tempfile::TempDir;
  use url::Url;

  use super::{AuthenticatedCommand, Cli, Commands, SIGING_KEY_ENV_VAR, load_signing_key};
  use crate::config::Config;

  const SECRET: &str = "testing:iDTfQV9yVOwU/VFPVW6Zh/l9Ibdh1W6KurGI6KAVF/hKRlL9bp2evH0Bj/+LceiF4gbzl7sSgS8Fcj8fTiIeJA==";
  const PUBLIC: &str = "testing:SkZS/W6dnrx9AY//i3HoheIG85e7EoEvBXI/H04iHiQ=";

  fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(std::iter::once("hibernation").chain(args.iter().copied())).unwrap()
  }

  fn parse_err(args: &[&str]) -> ErrorKind {
    match Cli::try_parse_from(std::iter::once("hibernation").chain(args.iter().copied())) {
      Ok(_) => panic!("{args:?} should not parse"),
      Err(e) => e.kind(),
    }
  }

  fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
  }

  // ---- parsing ----

  #[test]
  fn command_is_valid() {
    Cli::command().debug_assert();
  }

  #[test]
  fn parses_set_url() {
    let cli = parse(&["set-url", "https://cache.example.com"]);
    assert!(cli.url.is_none() && cli.config.is_none());
    assert!(
      matches!(cli.command, Commands::SetUrl { url: u } if u == url("https://cache.example.com"))
    );
    assert_eq!(
      parse_err(&["set-url", "not a url"]),
      ErrorKind::ValueValidation
    );
    // Positional and global --url are distinct
    let cli = parse(&["set-url", "--url", "http://old/", "http://new/"]);
    assert_eq!(cli.url, Some(url("http://old/")));
    assert!(matches!(cli.command, Commands::SetUrl { url: u } if u == url("http://new/")));
    assert_eq!(parse_err(&["set-url"]), ErrorKind::MissingRequiredArgument);
  }

  #[test]
  fn parses_auth() {
    assert!(matches!(
      parse(&["auth"]).command,
      Commands::Auth { token: None }
    ));
    assert!(
      matches!(parse(&["auth", "secret"]).command, Commands::Auth { token: Some(t) } if t == "secret")
    );
  }

  #[test]
  fn parses_test() {
    assert!(matches!(
      parse(&["test"]).command,
      Commands::AuthenticatedCommand(AuthenticatedCommand::Test)
    ));
  }

  #[test]
  fn parses_push_defaults() {
    let Commands::AuthenticatedCommand(AuthenticatedCommand::Push {
      cache,
      paths,
      no_deps,
      force,
      key,
      threads,
    }) = parse(&["push", "main"]).command
    else {
      panic!("expected push");
    };
    assert_eq!(cache, "main");
    assert!(paths.is_empty());
    assert!(!no_deps && !force);
    assert!(key.is_none());
    assert_eq!(threads, 5);
  }

  #[test]
  fn parses_push_flags() {
    let Commands::AuthenticatedCommand(AuthenticatedCommand::Push {
      cache,
      paths,
      no_deps,
      force,
      key,
      threads,
    }) = parse(&[
      "push",
      "main",
      "/nix/store/a",
      "/nix/store/b",
      "--no-deps",
      "--force",
      "--key",
      "key.sec",
      "--threads",
      "2",
    ])
    .command
    else {
      panic!("expected push");
    };
    assert_eq!(cache, "main");
    assert_eq!(paths, ["/nix/store/a", "/nix/store/b"]);
    assert!(no_deps && force);
    assert_eq!(key, Some(PathBuf::from("key.sec")));
    assert_eq!(threads, 2);

    assert_eq!(parse_err(&["push"]), ErrorKind::MissingRequiredArgument);
    assert_eq!(
      parse_err(&["push", "main", "--threads", "x"]),
      ErrorKind::ValueValidation
    );
    assert_eq!(
      parse_err(&["push", "main", "--threads", "-1"]),
      ErrorKind::UnknownArgument
    );
  }

  #[test]
  fn global_flags_anywhere() {
    for args in [
      &["--url", "http://a/", "-c", "/tmp/c.json", "test"][..],
      &["test", "-u", "http://a/", "--config", "/tmp/c.json"][..],
    ] {
      let cli = parse(args);
      assert_eq!(cli.url, Some(url("http://a/")));
      assert_eq!(cli.config, Some(PathBuf::from("/tmp/c.json")));
    }
    assert_eq!(
      parse_err(&["--url", "nope", "test"]),
      ErrorKind::ValueValidation
    );
  }

  #[test]
  fn rejects_missing_and_unknown_subcommands() {
    assert_eq!(
      parse_err(&[]),
      ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
    assert_eq!(parse_err(&["frobnicate"]), ErrorKind::InvalidSubcommand);
    assert_eq!(parse_err(&["test", "--bogus"]), ErrorKind::UnknownArgument);
    assert_eq!(parse_err(&["--version"]), ErrorKind::DisplayVersion);
    assert_eq!(parse_err(&["--help"]), ErrorKind::DisplayHelp);
  }

  // ---- run ----

  fn cli(args: &[&str], dir: &TempDir) -> (Cli, PathBuf) {
    let path = dir.path().join("config.json");
    let mut full = vec!["--config", path.to_str().unwrap()];
    full.extend_from_slice(args);
    (parse(&full), path)
  }

  #[tokio::test]
  async fn set_url_creates_and_updates_config() {
    let dir = TempDir::new().unwrap();
    let (c, path) = cli(&["set-url", "http://a/"], &dir);
    c.run(None, None, false).await.unwrap();
    let config = Config::load(Some(path.clone())).await.unwrap().unwrap();
    assert_eq!(config.app_url, url("http://a/"));
    assert_eq!(config.token, None);

    // Keeps the token
    let (c, _) = cli(&["set-url", "http://b/"], &dir);
    let existing = Config::new(url("http://a/"), Some("tok".into()));
    c.run(Some(existing), None, false).await.unwrap();
    let config = Config::load(Some(path)).await.unwrap().unwrap();
    assert_eq!(config.app_url, url("http://b/"));
    assert_eq!(config.token.as_deref(), Some("tok"));
  }

  #[tokio::test]
  async fn auth_saves_token() {
    let dir = TempDir::new().unwrap();
    let (c, path) = cli(&["auth", "tok"], &dir);
    c.run(None, Some(url("http://a/")), false).await.unwrap();
    let config = Config::load(Some(path.clone())).await.unwrap().unwrap();
    assert_eq!(config.app_url, url("http://a/"));
    assert_eq!(config.token.as_deref(), Some("tok"));

    // --url overrides the configured url
    let (c, _) = cli(&["auth", "tok2"], &dir);
    c.run(Some(config), Some(url("http://b/")), false)
      .await
      .unwrap();
    let config = Config::load(Some(path)).await.unwrap().unwrap();
    assert_eq!(config.app_url, url("http://b/"));
    assert_eq!(config.token.as_deref(), Some("tok2"));
  }

  #[tokio::test]
  async fn auth_without_url_or_token_fails() {
    let dir = TempDir::new().unwrap();
    let (c, path) = cli(&["auth", "tok"], &dir);
    let err = c.run(None, None, false).await.unwrap_err();
    assert!(format!("{err:?}").contains("No URL specified"));
    assert!(!path.exists());

    let (c, path) = cli(&["auth"], &dir);
    let err = c
      .run(None, Some(url("http://a/")), false)
      .await
      .unwrap_err();
    assert!(format!("{err:?}").contains("No token given"));
    assert!(!path.exists());
  }

  #[tokio::test]
  async fn authenticated_command_without_config_fails_non_interactive() {
    let dir = TempDir::new().unwrap();
    let (c, _) = cli(&["test"], &dir);
    let err = c.run(None, None, false).await.unwrap_err();
    assert!(format!("{err:?}").contains("Cli not configured"));
  }

  // ---- signing key ----

  #[tokio::test]
  async fn load_signing_key_from_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("key.sec");

    // `echo key > file` adds a newline
    std::fs::write(&path, format!("{SECRET}\n")).unwrap();
    let key = load_signing_key(Some(path.clone())).await.unwrap();
    assert_eq!(key.export_public_key(), PUBLIC);

    std::fs::write(&path, "garbage").unwrap();
    let err = load_signing_key(Some(path)).await.err().unwrap();
    assert!(format!("{err:?}").contains("Invalid signing key format in file"));

    assert!(
      load_signing_key(Some(dir.path().join("missing")))
        .await
        .is_err()
    );
  }

  /// One test for all env cases, `cargo test` runs tests of a binary in parallel threads
  #[tokio::test]
  async fn load_signing_key_from_env() {
    unsafe { std::env::remove_var(SIGING_KEY_ENV_VAR) };
    let err = load_signing_key(None).await.err().unwrap();
    assert!(format!("{err:?}").contains("Signing key not found in environment variable"));

    unsafe { std::env::set_var(SIGING_KEY_ENV_VAR, "garbage") };
    let err = load_signing_key(None).await.err().unwrap();
    assert!(format!("{err:?}").contains("Invalid signing key format from env"));

    unsafe { std::env::set_var(SIGING_KEY_ENV_VAR, SECRET) };
    let key = load_signing_key(None).await.unwrap();
    assert_eq!(key.export_public_key(), PUBLIC);

    // --key wins over the env var
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("key.sec");
    std::fs::write(&path, "garbage").unwrap();
    assert!(load_signing_key(Some(path)).await.is_err());

    unsafe { std::env::remove_var(SIGING_KEY_ENV_VAR) };
  }
}
