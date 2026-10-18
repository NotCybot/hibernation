//! Runs the real `hibernation` binary against a wiremock server. Needs only loopback networking,
//! so it also runs in the nix build sandbox.

use std::{
  path::{Path, PathBuf},
  process::{Command, Output, Stdio},
};

use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::{
  Mock, MockServer, ResponseTemplate,
  matchers::{header, method, path},
};

struct Env {
  dir: TempDir,
}

impl Env {
  fn new() -> Self {
    Self {
      dir: TempDir::new().unwrap(),
    }
  }

  fn config_path(&self) -> PathBuf {
    self.dir.path().join("config.json")
  }

  fn write_config(&self, config: Value) {
    std::fs::write(self.config_path(), config.to_string()).unwrap();
  }

  fn config(&self) -> Value {
    serde_json::from_str(&std::fs::read_to_string(self.config_path()).unwrap()).unwrap()
  }

  /// Run with `--config` in the temp dir, a non-tty stdin and an isolated data dir
  fn run(&self, args: &[&str]) -> Output {
    run_in(self.dir.path(), &self.config_path(), args)
  }
}

fn run_in(dir: &Path, config: &Path, args: &[&str]) -> Output {
  Command::new(env!("CARGO_BIN_EXE_hibernation"))
    .arg("--config")
    .arg(config)
    .args(args)
    .env("XDG_DATA_HOME", dir)
    .env("HOME", dir)
    .env_remove("HIBERNATION_SIGNING_KEY")
    .env_remove("LOG_LEVEL")
    .env_remove("RUST_LOG")
    .stdin(Stdio::null())
    .output()
    .unwrap()
}

fn stderr(out: &Output) -> String {
  String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Logs (non-tty) go to stdout, errors to stderr
fn output(out: &Output) -> String {
  format!("{}{}", String::from_utf8_lossy(&out.stdout), stderr(out))
}

async fn server(token_valid: bool, test_status: u16) -> MockServer {
  let server = MockServer::start().await;
  Mock::given(method("GET"))
    .and(path("/api/auth/test_token"))
    .and(header("authorization", "Bearer tok"))
    .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "valid": token_valid })))
    .mount(&server)
    .await;
  Mock::given(method("GET"))
    .and(path("/api/cache/test"))
    .and(header("authorization", "Bearer tok"))
    .respond_with(ResponseTemplate::new(test_status))
    .mount(&server)
    .await;
  server
}

#[test]
fn version_and_help() {
  let env = Env::new();
  let out = env.run(&["--version"]);
  assert!(out.status.success());
  assert!(String::from_utf8_lossy(&out.stdout).contains(env!("CARGO_PKG_VERSION")));

  let out = env.run(&[]);
  assert_eq!(out.status.code(), Some(2));
  assert!(output(&out).contains("Usage"));
}

#[test]
fn set_url_writes_config() {
  let env = Env::new();
  let out = env.run(&["set-url", "http://cache.example/"]);
  assert!(out.status.success(), "{}", stderr(&out));
  assert_eq!(
    env.config(),
    json!({ "app_url": "http://cache.example/", "token": null })
  );

  let out = env.run(&["set-url", "nope"]);
  assert_eq!(out.status.code(), Some(2));
}

#[test]
fn set_url_creates_config_dir() {
  let env = Env::new();
  let config = env.dir.path().join("a/b/config.json");
  let out = run_in(env.dir.path(), &config, &["set-url", "http://x/"]);
  assert!(out.status.success(), "{}", stderr(&out));
  assert!(config.exists());
}

#[test]
fn invalid_config_fails() {
  let env = Env::new();
  std::fs::write(env.config_path(), "{").unwrap();
  let out = env.run(&["test"]);
  assert_eq!(out.status.code(), Some(1));
  assert!(output(&out).contains("Failed to load config"));
}

#[test]
fn auth_saves_token() {
  let env = Env::new();
  let out = env.run(&["--url", "http://cache.example/", "auth", "tok"]);
  assert!(out.status.success(), "{}", stderr(&out));
  assert_eq!(
    env.config(),
    json!({ "app_url": "http://cache.example/", "token": "tok" })
  );

  // Uses the configured url
  let out = env.run(&["auth", "tok2"]);
  assert!(out.status.success(), "{}", stderr(&out));
  assert_eq!(env.config()["token"], "tok2");
}

#[test]
fn auth_errors_without_url_or_token() {
  let env = Env::new();
  let out = env.run(&["auth", "tok"]);
  assert_eq!(out.status.code(), Some(1));
  assert!(output(&out).contains("No URL specified"));
  assert!(!env.config_path().exists());

  let out = env.run(&["--url", "http://x/", "auth"]);
  assert_eq!(out.status.code(), Some(1));
  assert!(output(&out).contains("No token given"));
}

#[tokio::test]
async fn test_command_succeeds_with_valid_token() {
  let server = server(true, 200).await;
  let env = Env::new();
  env.write_config(json!({ "app_url": server.uri(), "token": "tok" }));
  let out = env.run(&["test"]);
  assert!(out.status.success(), "{}", stderr(&out));
  assert!(output(&out).contains("Test request succeeded"));
}

#[tokio::test]
async fn test_command_url_flag_overrides_config() {
  let server = server(true, 200).await;
  let env = Env::new();
  env.write_config(json!({ "app_url": "http://127.0.0.1:1/", "token": "tok" }));
  let out = env.run(&["--url", &server.uri(), "test"]);
  assert!(out.status.success(), "{}", stderr(&out));
}

#[tokio::test]
async fn test_command_failure_exits_1() {
  let server = server(true, 500).await;
  let env = Env::new();
  env.write_config(json!({ "app_url": server.uri(), "token": "tok" }));
  let out = env.run(&["test"]);
  assert_eq!(out.status.code(), Some(1));
  assert!(output(&out).contains("Test request failed"));
}

#[tokio::test]
async fn invalid_token_non_interactive_exits_1() {
  let server = server(false, 200).await;
  let env = Env::new();
  env.write_config(json!({ "app_url": server.uri(), "token": "tok" }));
  let out = env.run(&["test"]);
  assert_eq!(out.status.code(), Some(1));
  assert!(output(&out).contains("Token validation failed"));
}

#[test]
fn unconfigured_non_interactive_exits_1() {
  let env = Env::new();
  let out = env.run(&["test"]);
  assert_eq!(out.status.code(), Some(1));
  assert!(output(&out).contains("Cli not configured"));

  env.write_config(json!({ "app_url": "http://127.0.0.1:1/", "token": null }));
  let out = env.run(&["test"]);
  assert_eq!(out.status.code(), Some(1));
  assert!(output(&out).contains("Token not set"));
}

#[tokio::test]
async fn push_without_signing_key_exits_1() {
  let server = server(true, 200).await;
  let env = Env::new();
  env.write_config(json!({ "app_url": server.uri(), "token": "tok" }));
  let out = env.run(&["push", "main", "/nix/store/x"]);
  assert_eq!(out.status.code(), Some(1));
  assert!(output(&out).contains("Failed to load signing key"));

  let key = env.dir.path().join("key.sec");
  std::fs::write(&key, "garbage").unwrap();
  let out = env.run(&["push", "main", "--key", key.to_str().unwrap()]);
  assert_eq!(out.status.code(), Some(1));
  assert!(output(&out).contains("Invalid signing key format in file"));
}
