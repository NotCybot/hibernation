//! Real push: the backend runs in-process (sqlite + local storage in a temp dir), the `hibernation` binary reads
//! store paths from the local nix daemon and uploads them, then `nix` itself verifies the result.
//!
//! Gated twice: the `e2e` feature (pulls in the backend, which the nix build has no sources for) and a reachable
//! nix daemon + `nix` binary at runtime (skipped otherwise, e.g. in the nix build sandbox).
//! Run with `cargo nextest run -p hibernation --all-features`.
#![cfg(feature = "e2e")]

use std::{
  collections::HashMap,
  path::Path,
  process::{Command, Output, Stdio},
  time::Duration,
};

use backend::App;
use base64::{Engine, prelude::BASE64_STANDARD};
use reqwest::{Client, StatusCode};
use rsa::{Pkcs1v15Encrypt, RsaPublicKey, pkcs1::DecodeRsaPublicKey, rand_core::OsRng};
use serde_json::{Value, json};
use tempfile::TempDir;
use testcontainers_modules::{
  postgres::Postgres,
  testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
  },
};
use uuid::Uuid;

/// Same key as `secret.key` / `public.key` in the repo root.
const SECRET_KEY: &str = "testing:iDTfQV9yVOwU/VFPVW6Zh/l9Ibdh1W6KurGI6KAVF/hKRlL9bp2evH0Bj/+LceiF4gbzl7sSgS8Fcj8fTiIeJA==";
const PUBLIC_KEY: &str = "testing:SkZS/W6dnrx9AY//i3HoheIG85e7EoEvBXI/H04iHiQ=";
const DAEMON_SOCKET: &str = "/nix/var/nix/daemon-socket/socket";

fn nix_available() -> bool {
  let ok = Path::new(DAEMON_SOCKET).exists()
    && Command::new("nix")
      .arg("--version")
      .output()
      .is_ok_and(|o| o.status.success());
  if !ok {
    eprintln!("skipping: no nix daemon at {DAEMON_SOCKET} or no nix binary");
  }
  ok
}

struct Server {
  port: u16,
  client: Client,
  cookie: String,
  token: String,
  _dir: TempDir,
  _containers: Option<(ContainerAsync<Postgres>, ContainerAsync<GenericImage>)>,
}

/// Postgres + S3Mock containers like `backend/tests/common.rs`, started concurrently. Sets the backend's env vars.
async fn start_pg_s3() -> (ContainerAsync<Postgres>, ContainerAsync<GenericImage>) {
  let (pg, s3) = tokio::join!(
    Postgres::default().with_tag("18-alpine").start(),
    GenericImage::new("adobe/s3mock", "5.2.3")
      .with_exposed_port(ContainerPort::Tcp(9090))
      .with_wait_for(WaitFor::message_on_stdout("Started S3MockApplication"))
      .with_env_var(
        "COM_ADOBE_TESTING_S3MOCK_STORE_INITIAL_BUCKETS",
        "hibernation"
      )
      .start(),
  );
  let (pg, s3) = (pg.expect("start postgres"), s3.expect("start S3Mock"));
  let db_url = format!(
    "postgres://postgres:postgres@{}:{}/postgres",
    pg.get_host().await.unwrap(),
    pg.get_host_port_ipv4(5432).await.unwrap()
  );
  let s3_host = format!(
    "http://{}:{}",
    s3.get_host().await.unwrap(),
    s3.get_host_port_ipv4(9090).await.unwrap()
  );
  unsafe {
    std::env::set_var("DB_URL", db_url);
    std::env::set_var("S3_BUCKET", "hibernation");
    std::env::set_var("S3_REGION", "us-east-1");
    std::env::set_var("S3_HOST", s3_host);
    std::env::set_var("S3_ACCESS_KEY", "test");
    std::env::set_var("S3_SECRET_KEY", "test");
    std::env::set_var("S3_FORCE_PATH_STYLE", "true");
  }
  (pg, s3)
}

impl Server {
  fn url(&self, path: &str) -> String {
    format!("http://localhost:{}/api{path}", self.port)
  }

  async fn post(&self, path: &str, body: Value) -> reqwest::Response {
    self
      .client
      .post(self.url(path))
      .header("Cookie", &self.cookie)
      .json(&body)
      .send()
      .await
      .unwrap()
  }

  /// Boot the backend (sqlite + local fs, or Postgres + S3 containers), complete setup, create the public cache
  /// `main` (no downstream caches) and a CLI token
  async fn start(pg_s3: bool) -> Server {
    let dir = TempDir::new().unwrap();
    unsafe {
      std::env::set_var("PORT", "0");
      std::env::set_var(
        "DB_URL",
        format!("sqlite://{}/db.sqlite?mode=rwc", dir.path().display()),
      );
      std::env::set_var("SITE_URL", "http://localhost/");
      std::env::set_var("STORAGE_PATH", dir.path().join("storage"));
      std::env::set_var("LOG_LEVEL", "off");
    }
    let containers = if pg_s3 {
      Some(start_pg_s3().await)
    } else {
      None
    };
    let app = App::new().await;
    let port = app.port();
    tokio::spawn(app.run());

    let client = Client::new();
    let mut server = Server {
      port,
      client,
      cookie: String::new(),
      token: String::new(),
      _dir: dir,
      _containers: containers,
    };
    for _ in 0..100 {
      if let Ok(res) = server.client.get(server.url("/health")).send().await
        && res.status().is_success()
      {
        break;
      }
      tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Setup, the password is RSA encrypted like the frontend does
    let key: Value = server
      .client
      .get(server.url("/auth/password"))
      .send()
      .await
      .unwrap()
      .json()
      .await
      .unwrap();
    let public_key = RsaPublicKey::from_pkcs1_pem(key["key"].as_str().unwrap()).unwrap();
    let password = public_key
      .encrypt(&mut OsRng, Pkcs1v15Encrypt, b"hunter2pass")
      .unwrap();
    let res = server
      .post(
        "/setup",
        json!({
          "admin_username": "admin",
          "admin_email": "admin@example.com",
          "admin_password": BASE64_STANDARD.encode(password),
        }),
      )
      .await;
    assert_eq!(res.status(), StatusCode::OK, "setup");
    server.cookie = res
      .headers()
      .get_all("set-cookie")
      .iter()
      .filter_map(|v| v.to_str().ok()?.split(';').next())
      .find(|c| c.starts_with("centaurus_jwt="))
      .expect("auth cookie")
      .to_string();

    let res = server
      .post(
        "/cache/management",
        json!({ "name": "main", "public": true, "quota": 1, "sig_key": PUBLIC_KEY }),
      )
      .await;
    assert_eq!(res.status(), StatusCode::OK, "create cache");
    let uuid = res.json::<Value>().await.unwrap()["uuid"]
      .as_str()
      .unwrap()
      .to_string();
    let res = server
      .post(
        &format!("/cache/management/{uuid}"),
        json!({
          "name": "main", "priority": 40, "public": true, "quota": 1024 * 1024 * 1024, "sig_key": PUBLIC_KEY,
          "allow_force_push": false, "eviction_policy": "LeastRecentlyUsed", "downstream_caches": [],
        }),
      )
      .await;
    assert_eq!(res.status(), StatusCode::OK, "configure cache");

    let res = server
      .post(
        "/token",
        json!({ "name": "ci", "exp": "2099-01-01T00:00:00Z" }),
      )
      .await;
    assert_eq!(res.status(), StatusCode::OK, "create token");
    server.token = res.json::<Value>().await.unwrap()["token"]
      .as_str()
      .unwrap()
      .to_string();

    server
  }

  fn cache_url(&self) -> String {
    self.url("/nix/main")
  }

  async fn narinfo(&self, path: &str) -> Option<String> {
    let hash = path
      .strip_prefix("/nix/store/")
      .unwrap()
      .split('-')
      .next()
      .unwrap();
    let res = self
      .client
      .get(format!("{}/{hash}.narinfo", self.cache_url()))
      .send()
      .await
      .unwrap();
    match res.status() {
      StatusCode::OK => Some(res.text().await.unwrap()),
      StatusCode::NOT_FOUND => None,
      status => panic!("narinfo {path}: {status}"),
    }
  }

  /// Run the CLI binary (non-tty) against this server
  fn cli(&self, dir: &Path, args: &[&str]) -> Output {
    let config = dir.join("config.json");
    std::fs::write(
      &config,
      json!({ "app_url": format!("http://localhost:{}/", self.port), "token": self.token })
        .to_string(),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_hibernation"))
      .arg("--config")
      .arg(&config)
      .args(args)
      .env("HIBERNATION_SIGNING_KEY", SECRET_KEY)
      .env("LOG_LEVEL", "info")
      .stdin(Stdio::null())
      .output()
      .unwrap();
    eprintln!(
      "hibernation {args:?}: {}\n{}{}",
      out.status,
      String::from_utf8_lossy(&out.stdout),
      String::from_utf8_lossy(&out.stderr)
    );
    out
  }
}

/// Fresh store paths via the daemon: `root` references `a` and `b`
fn make_paths() -> HashMap<String, String> {
  let id = Uuid::new_v4();
  let expr = format!(
    r#"let a = builtins.toFile "hib-a" "{id}-a"; b = builtins.toFile "hib-b" "{id}-b";
       in builtins.toJSON {{ inherit a b; root = builtins.toFile "hib-root" "${{a}} ${{b}}"; }}"#
  );
  let out = Command::new("nix")
    .args(["eval", "--raw", "--impure", "--expr", &expr])
    .args(["--extra-experimental-features", "nix-command"])
    .output()
    .unwrap();
  assert!(
    out.status.success(),
    "{}",
    String::from_utf8_lossy(&out.stderr)
  );
  serde_json::from_slice(&out.stdout).unwrap()
}

fn output(out: &Output) -> String {
  format!(
    "{}{}",
    String::from_utf8_lossy(&out.stdout),
    String::from_utf8_lossy(&out.stderr)
  )
}

fn base(path: &str) -> &str {
  path.strip_prefix("/nix/store/").unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn push_paths_from_daemon() {
  if nix_available() {
    push_flow(Server::start(false).await).await;
  }
}

/// Same flow on the production-like stack (Postgres + S3, needs Docker)
#[tokio::test(flavor = "multi_thread")]
async fn push_paths_from_daemon_pg_s3() {
  if nix_available() {
    push_flow(Server::start(true).await).await;
  }
}

async fn push_flow(server: Server) {
  let dir = TempDir::new().unwrap();
  let paths = make_paths();
  let (a, b, root) = (&paths["a"], &paths["b"], &paths["root"]);

  // --no-deps: only the root
  let out = server.cli(dir.path(), &["push", "main", root, "--no-deps"]);
  assert!(out.status.success());
  let narinfo = server.narinfo(root).await.expect("root pushed");
  assert!(server.narinfo(a).await.is_none());
  assert!(server.narinfo(b).await.is_none());

  assert!(
    narinfo.contains(&format!("StorePath: {root}\n")),
    "{narinfo}"
  );
  assert!(
    narinfo.contains(&format!("References: {} {}\n", base(a), base(b)))
      || narinfo.contains(&format!("References: {} {}\n", base(b), base(a))),
    "{narinfo}"
  );
  assert!(narinfo.contains("Compression: zstd\n"), "{narinfo}");
  assert!(narinfo.contains("Sig: testing:"), "{narinfo}");

  // With deps: a and b, the root already exists
  let out = server.cli(dir.path(), &["push", "main", root]);
  assert!(out.status.success());
  assert!(output(&out).contains("Pushing 2 paths"), "{}", output(&out));
  assert!(server.narinfo(a).await.is_some());
  assert!(server.narinfo(b).await.is_some());

  // Nothing left
  let out = server.cli(dir.path(), &["push", "main", root]);
  assert!(out.status.success());
  assert!(output(&out).contains("All paths already exist"));

  // nix accepts the signatures (fingerprint incl. reference order) and NAR hashes of everything we pushed
  let out = Command::new("nix")
    .args(["store", "verify", "--store", &server.cache_url()])
    .args(["--trusted-public-keys", PUBLIC_KEY, "--sigs-needed", "1"])
    .args(["--extra-experimental-features", "nix-command", "-r", root])
    .output()
    .unwrap();
  assert!(
    out.status.success(),
    "nix store verify: {}",
    String::from_utf8_lossy(&out.stderr)
  );

  // And can copy it into a fresh store
  let store = TempDir::new().unwrap();
  let out = Command::new("nix")
    .args(["copy", "--from", &server.cache_url(), "--to"])
    .arg(store.path())
    .args([
      "--trusted-public-keys",
      PUBLIC_KEY,
      "--extra-experimental-features",
      "nix-command",
      root,
    ])
    .output()
    .unwrap();
  assert!(
    out.status.success(),
    "nix copy: {}",
    String::from_utf8_lossy(&out.stderr)
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn push_errors() {
  if !nix_available() {
    return;
  }
  let server = Server::start(false).await;
  let dir = TempDir::new().unwrap();
  let paths = make_paths();

  let out = server.cli(dir.path(), &["push", "nope", &paths["a"]]);
  assert_eq!(out.status.code(), Some(1));
  assert!(output(&out).contains("Cache not found"));

  let out = server.cli(dir.path(), &["push", "main", &paths["a"], "--force"]);
  assert_eq!(out.status.code(), Some(1));
  assert!(output(&out).contains("Force push not allowed"));

  // Invalid and missing paths are skipped, nothing to push
  let missing = "/nix/store/00000000000000000000000000000000-missing";
  let out = server.cli(dir.path(), &["push", "main", "not-a-path", missing]);
  assert!(out.status.success());
  assert!(output(&out).contains("Invalid store path: not-a-path"));
  assert!(output(&out).contains("not found in local store"));
  assert!(output(&out).contains("Collected 0 paths"));
}
