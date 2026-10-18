//! Shared harness for the backend integration tests.
//!
//! Every test boots a *real* server on an OS-assigned port (`PORT=0`) backed by a SQLite database file and local
//! file storage, both inside a per-test [`TempDir`], then drives it over HTTP with `reqwest`. The database is a file
//! (not `sqlite::memory:`) so tests can open a second connection with [`TestServer::db`] to seed or inspect rows
//! and call internal functions (cleanup/dedupe jobs) against the server's real data. `cargo nextest` runs each test in its
//! own process, so the process-global environment set by [`prepare_env`] is isolated per test.
//!
//! The server sets its auth cookies with the `Secure` flag, which `reqwest`'s cookie store refuses over plain HTTP,
//! so [`TestServer`] keeps its own tiny cookie jar.
//!
//! Downstream caches: new caches default to `https://cache.nixos.org`. [`TestServer::create_cache`] always replaces
//! that list (empty by default, or a local [`MockServer`] via [`mock_downstream`]) so tests never hit the network.
//!
//! [`TestServer::start_pg`] / [`TestServer::start_s3`] / [`TestServer::start_pg_s3`] boot the same server against a
//! throwaway Postgres / S3 (Adobe S3Mock) container via testcontainers (needs Docker) instead of sqlite / the local
//! filesystem. The containers live as long as the [`TestServer`]. [`TestServer::db`] and [`TestServer::file_storage`]
//! always use the exact database and storage config the server was started with.
//!
//! Run with the `test` feature so centaurus uses fast RSA keys: `cargo nextest run -p backend --all-features`
#![allow(dead_code)]

use std::{
  collections::HashMap,
  sync::{
    Mutex,
    atomic::{AtomicU32, Ordering},
  },
  time::Duration,
};

use backend::{App, config::Config};
use base64::{Engine, prelude::BASE64_STANDARD};
use centaurus::{
  db::{
    config::DBConfig,
    init::{Connection, connect_db},
  },
  storage::FileStorage,
};
use futures_util::StreamExt;
use harmonia_store_core::store_path::StorePath;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use rsa::{Pkcs1v15Encrypt, RsaPublicKey, pkcs1::DecodeRsaPublicKey, rand_core::OsRng};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shared::{api::push::CHUNK_SIZE, hash::to_nix_base32, sig::KeyPair};
use tempfile::TempDir;
use testcontainers_modules::{
  postgres::Postgres,
  testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
  },
};
use tokio::{spawn, time::sleep};
use uuid::Uuid;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

/// The auth cookie centaurus sets on a successful login/setup.
pub const JWT_COOKIE_NAME: &str = "centaurus_jwt";

pub const ADMIN_NAME: &str = "admin";
pub const ADMIN_EMAIL: &str = "admin@example.com";
pub const ADMIN_PASSWORD: &str = "hunter2pass";

/// Same key as `secret.key` / `public.key` in the repo root.
pub const TEST_SECRET_KEY: &str = "testing:iDTfQV9yVOwU/VFPVW6Zh/l9Ibdh1W6KurGI6KAVF/hKRlL9bp2evH0Bj/+LceiF4gbzl7sSgS8Fcj8fTiIeJA==";
pub const TEST_PUBLIC_KEY: &str = "testing:SkZS/W6dnrx9AY//i3HoheIG85e7EoEvBXI/H04iHiQ=";

pub fn test_key() -> KeyPair {
  KeyPair::from_string(TEST_SECRET_KEY).unwrap()
}

/// A fresh random signing key, e.g. to sign with a key the cache does not trust.
pub fn random_key(name: &str) -> KeyPair {
  let kp = ed25519_compact::KeyPair::generate();
  KeyPair::from_string(&format!("{name}:{}", BASE64_STANDARD.encode(*kp))).unwrap()
}

fn db_url(dir: &std::path::Path) -> String {
  format!("sqlite://{}/db.sqlite?mode=rwc", dir.display())
}

fn storage_path(dir: &std::path::Path) -> std::path::PathBuf {
  dir.join("storage")
}

/// Configure the process environment for a throwaway server keeping its db and files in `dir`.
pub fn prepare_env(dir: &std::path::Path) {
  unsafe {
    std::env::set_var("PORT", "0");
    std::env::set_var("DB_URL", db_url(dir));
    std::env::set_var("SITE_URL", "http://localhost/");
    std::env::set_var("STORAGE_PATH", storage_path(dir));
    // keep logs quiet during tests
    std::env::set_var("LOG_LEVEL", "off");
  }
}

/// Bucket created in the S3Mock container and used by S3 test servers.
pub const S3_BUCKET: &str = "hibernation";

/// A container the server depends on, kept alive as long as the server.
pub enum TestContainer {
  Postgres(ContainerAsync<Postgres>),
  S3(ContainerAsync<GenericImage>),
}

/// Start a Postgres container (same major version as production), returns it with its connection URL.
pub async fn start_pg_container() -> (ContainerAsync<Postgres>, String) {
  let container = Postgres::default()
    .with_tag("18-alpine")
    .start()
    .await
    .expect("start postgres container");
  let url = format!(
    "postgres://postgres:postgres@{}:{}/postgres",
    container.get_host().await.unwrap(),
    container.get_host_port_ipv4(5432).await.unwrap()
  );
  (container, url)
}

/// Start an S3Mock container with an empty [`S3_BUCKET`], returns it with its endpoint (`http://host:port`).
pub async fn start_s3_container() -> (ContainerAsync<GenericImage>, String) {
  let container = GenericImage::new("adobe/s3mock", "5.2.3")
    .with_exposed_port(ContainerPort::Tcp(9090))
    .with_wait_for(WaitFor::message_on_stdout("Started S3MockApplication"))
    .with_env_var("COM_ADOBE_TESTING_S3MOCK_STORE_INITIAL_BUCKETS", S3_BUCKET)
    .start()
    .await
    .expect("start S3Mock container");
  let host = format!(
    "http://{}:{}",
    container.get_host().await.unwrap(),
    container.get_host_port_ipv4(9090).await.unwrap()
  );
  (container, host)
}

/// A running test server plus a client (with a hand-rolled cookie jar).
pub struct TestServer {
  pub port: u16,
  pub client: Client,
  /// Holds the sqlite db (`db.sqlite`) and the local file storage (`storage/`), deleted on drop. Present (but unused
  /// for that part) on Postgres / S3 servers too.
  pub dir: TempDir,
  cookies: Mutex<HashMap<String, String>>,
  /// The config the server was started with (`db_url`, `storage`)
  pub config: Config,
  containers: Vec<TestContainer>,
}

impl TestServer {
  /// Boot a fresh server with an empty database and wait for it to become ready.
  pub async fn start() -> TestServer {
    Self::start_with(|| {}).await
  }

  /// Like [`TestServer::start`], `env` runs after the defaults are set to override or add env vars.
  pub async fn start_with(env: impl FnOnce()) -> TestServer {
    Self::start_backends(false, false, env).await
  }

  /// Boot a server on a Postgres container (local file storage).
  pub async fn start_pg() -> TestServer {
    Self::start_backends(true, false, || {}).await
  }

  /// Boot a server with S3 file storage on an S3Mock container (sqlite database).
  pub async fn start_s3() -> TestServer {
    Self::start_backends(false, true, || {}).await
  }

  /// Boot a production-like server: Postgres database and S3 file storage.
  pub async fn start_pg_s3() -> TestServer {
    Self::start_backends(true, true, || {}).await
  }

  /// Boot a server on Postgres and/or S3 containers (sqlite / local fs otherwise). `env` runs last, after the
  /// defaults and container env vars are set, e.g. to set `MAX_CONCURRENT_UPLOADS`.
  pub async fn start_backends(pg: bool, s3: bool, env: impl FnOnce()) -> TestServer {
    let dir = TempDir::new().expect("create temp dir");
    prepare_env(dir.path());
    let mut containers = Vec::new();
    // start both containers concurrently
    let (pg, s3) = tokio::join!(
      async {
        if pg {
          Some(start_pg_container().await)
        } else {
          None
        }
      },
      async {
        if s3 {
          Some(start_s3_container().await)
        } else {
          None
        }
      },
    );
    if let Some((container, url)) = pg {
      unsafe { std::env::set_var("DB_URL", url) };
      containers.push(TestContainer::Postgres(container));
    }
    if let Some((container, host)) = s3 {
      unsafe {
        std::env::set_var("S3_BUCKET", S3_BUCKET);
        std::env::set_var("S3_REGION", "us-east-1");
        std::env::set_var("S3_HOST", host);
        std::env::set_var("S3_ACCESS_KEY", "test");
        std::env::set_var("S3_SECRET_KEY", "test");
        std::env::set_var("S3_FORCE_PATH_STYLE", "true");
      }
      containers.push(TestContainer::S3(container));
    }
    env();

    let config = Config::parse();
    let app = App::from_config(config.clone()).await;
    let port = app.port();
    spawn(app.run());

    let client = Client::builder()
      .timeout(Duration::from_secs(30))
      .connect_timeout(Duration::from_secs(30))
      .build()
      .expect("build reqwest client");

    let server = TestServer {
      port,
      client,
      dir,
      cookies: Mutex::new(HashMap::new()),
      config,
      containers,
    };

    for _ in 0..100 {
      if let Ok(resp) = server.client.get(server.url("/health")).send().await
        && resp.status().is_success()
      {
        return server;
      }
      sleep(Duration::from_millis(50)).await;
    }
    panic!("server did not become ready in time");
  }

  /// Boot a server and complete first-run setup with the default admin (session cookie stored).
  pub async fn start_with_admin() -> (TestServer, Uuid) {
    Self::with_admin(TestServer::start().await).await
  }

  /// Complete first-run setup with the default admin on an already booted server, e.g.
  /// `TestServer::with_admin(TestServer::start_pg().await)`.
  pub async fn with_admin(server: TestServer) -> (TestServer, Uuid) {
    let id = server
      .setup_admin(ADMIN_NAME, ADMIN_EMAIL, ADMIN_PASSWORD)
      .await;
    (server, id)
  }

  pub fn is_pg(&self) -> bool {
    self.config.db_url.starts_with("postgres")
  }

  pub fn is_s3(&self) -> bool {
    self.config.storage.use_s3()
  }

  /// A second connection to the server's database (no migrations run).
  pub async fn db(&self) -> Connection {
    connect_db(&DBConfig::default(), &self.config.db_url).await
  }

  /// The local storage directory (`{id}.nar` files, `parts/`). Unused by S3 servers.
  pub fn storage_path(&self) -> std::path::PathBuf {
    assert!(!self.is_s3(), "storage_path() on an S3 server");
    storage_path(self.dir.path())
  }

  /// A [`FileStorage`] with the same storage config as the server (local dir or S3 bucket).
  pub async fn file_storage(&self) -> FileStorage {
    FileStorage::init(&self.config.storage)
      .await
      .expect("init storage")
  }

  /// Every object in the server's storage (local or S3) as a `/`-separated key, sorted.
  pub async fn storage_files(&self) -> Vec<String> {
    let storage = self.file_storage().await;
    let mut files: Vec<String> = storage
      .list(None)
      .map(|meta| meta.expect("list storage").location.to_string())
      .collect()
      .await;
    files.sort();
    files
  }

  /// In-progress S3 multipart uploads as `(key, upload id)`, via S3Mock's unsigned `GET /{bucket}?uploads`.
  pub async fn s3_multipart_uploads(&self) -> Vec<(String, String)> {
    assert!(
      self.is_s3(),
      "s3_multipart_uploads() on a local storage server"
    );
    let host = self.config.storage.s3_host.as_ref().unwrap();
    let xml = self
      .client
      .get(format!("{host}/{S3_BUCKET}?uploads"))
      .send()
      .await
      .unwrap()
      .error_for_status()
      .unwrap()
      .text()
      .await
      .unwrap();
    fn tag<'a>(s: &'a str, name: &str) -> &'a str {
      let start = s.find(&format!("<{name}>")).unwrap() + name.len() + 2;
      &s[start..start + s[start..].find("</").unwrap()]
    }
    xml
      .split("<Upload>")
      .skip(1)
      .map(|u| (tag(u, "Key").to_string(), tag(u, "UploadId").to_string()))
      .collect()
  }

  /// Full URL under the `/api` prefix, e.g. `url("/auth/config")`.
  pub fn url(&self, path: &str) -> String {
    format!("http://localhost:{}/api{}", self.port, path)
  }

  /// Full URL with no `/api` prefix.
  pub fn root_url(&self, path: &str) -> String {
    format!("http://localhost:{}{}", self.port, path)
  }

  /// URL of a nix binary cache endpoint, e.g. `nix_url("main", "nix-cache-info")`.
  pub fn nix_url(&self, cache: &str, path: &str) -> String {
    self.url(&format!("/nix/{cache}/{path}"))
  }

  fn cookie_header(&self) -> Option<String> {
    let jar = self.cookies.lock().unwrap();
    if jar.is_empty() {
      return None;
    }
    Some(
      jar
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; "),
    )
  }

  /// Capture every `Set-Cookie` into the jar (handling deletions via empty value or `Max-Age=0`).
  fn capture_cookies(&self, resp: &Response) {
    let mut jar = self.cookies.lock().unwrap();
    for value in resp.headers().get_all("set-cookie") {
      let Ok(value) = value.to_str() else { continue };
      let mut parts = value.split(';');
      let Some(pair) = parts.next() else { continue };
      let Some((name, val)) = pair.split_once('=') else {
        continue;
      };
      let name = name.trim().to_string();
      let val = val.trim().to_string();

      let deleted = val.is_empty()
        || parts.any(|attr| {
          let attr = attr.trim().to_ascii_lowercase();
          attr == "max-age=0" || attr.starts_with("expires=thu, 01 jan 1970")
        });

      if deleted {
        jar.remove(&name);
      } else {
        jar.insert(name, val);
      }
    }
  }

  /// Send a request with the cookie jar attached, capturing any cookies set.
  pub async fn send(&self, req: RequestBuilder) -> Response {
    let req = match self.cookie_header() {
      Some(cookie) => req.header("Cookie", cookie),
      None => req,
    };
    let resp = req.send().await.expect("request failed");
    self.capture_cookies(&resp);
    resp
  }

  /// A request under `/api` *without* cookies, add `.bearer_auth(token)` etc. and `.send()` it yourself.
  pub fn raw(&self, method: Method, path: &str) -> RequestBuilder {
    self.client.request(method, self.url(path))
  }

  pub async fn get(&self, path: &str) -> Response {
    self.send(self.client.get(self.url(path))).await
  }

  pub async fn post(&self, path: &str, body: Value) -> Response {
    self
      .send(self.client.post(self.url(path)).json(&body))
      .await
  }

  pub async fn put(&self, path: &str, body: Value) -> Response {
    self.send(self.client.put(self.url(path)).json(&body)).await
  }

  pub async fn delete(&self, path: &str, body: Value) -> Response {
    self
      .send(self.client.delete(self.url(path)).json(&body))
      .await
  }

  /// Fetch the password-transfer RSA key and encrypt `plaintext` like the frontend (RSA-PKCS1v15 + base64).
  pub async fn encrypt_password(&self, plaintext: &str) -> String {
    let resp = self.get("/auth/password").await;
    assert_eq!(resp.status(), StatusCode::OK, "fetch password key");
    let key: Value = resp.json().await.expect("key json");
    let pem = key["key"].as_str().expect("key pem");

    let public_key = RsaPublicKey::from_pkcs1_pem(pem).expect("parse rsa pub key");
    let ciphertext = public_key
      .encrypt(&mut OsRng, Pkcs1v15Encrypt, plaintext.as_bytes())
      .expect("rsa encrypt");
    BASE64_STANDARD.encode(ciphertext)
  }

  /// Run first-run setup, creating the admin and storing its session. Returns the admin user id.
  pub async fn setup_admin(&self, username: &str, email: &str, password: &str) -> Uuid {
    let encrypted = self.encrypt_password(password).await;
    let resp = self
      .post(
        "/setup",
        json!({
          "admin_username": username,
          "admin_email": email,
          "admin_password": encrypted,
        }),
      )
      .await;
    assert_eq!(resp.status(), StatusCode::OK, "setup should succeed");
    assert!(
      self.has_cookie(JWT_COOKIE_NAME),
      "setup must set the auth cookie"
    );

    let body: Value = resp.json().await.expect("setup json");
    Uuid::parse_str(body["user"].as_str().expect("user id")).expect("parse user id")
  }

  /// Log in via the password endpoint (cookies captured automatically).
  pub async fn login(&self, email: &str, password: &str) -> Response {
    let encrypted = self.encrypt_password(password).await;
    self
      .post(
        "/auth/password",
        json!({ "email": email, "password": encrypted }),
      )
      .await
  }

  pub fn has_cookie(&self, name: &str) -> bool {
    self.cookies.lock().unwrap().contains_key(name)
  }

  pub fn cookie(&self, name: &str) -> Option<String> {
    self.cookies.lock().unwrap().get(name).cloned()
  }

  /// Drop all stored cookies so subsequent requests are unauthenticated.
  pub fn clear_cookies(&self) {
    self.cookies.lock().unwrap().clear();
  }

  /// Create a cache via `/cache/management` as the logged-in user, then apply `opts` via the edit endpoint.
  /// Returns the cache id.
  pub async fn create_cache(&self, name: &str, opts: &CacheOptions) -> Uuid {
    let resp = self
      .post(
        "/cache/management",
        json!({ "name": name, "public": opts.public, "quota": 1, "sig_key": opts.sig_key }),
      )
      .await;
    assert_eq!(resp.status(), StatusCode::OK, "create cache {name}");
    let body: Value = resp.json().await.unwrap();
    let uuid = Uuid::parse_str(body["uuid"].as_str().unwrap()).unwrap();

    let resp = self
      .post(&format!("/cache/management/{uuid}"), opts.edit_body(name))
      .await;
    assert_eq!(resp.status(), StatusCode::OK, "configure cache {name}");
    uuid
  }

  /// Create a CLI token via `/token` as the logged-in user. Returns `(secret, token id)`.
  pub async fn create_cli_token(&self, name: &str) -> (String, Uuid) {
    let exp = chrono::Utc::now() + chrono::Duration::days(1);
    self.create_cli_token_exp(name, exp).await
  }

  pub async fn create_cli_token_exp(
    &self,
    name: &str,
    exp: chrono::DateTime<chrono::Utc>,
  ) -> (String, Uuid) {
    let resp = self
      .post("/token", json!({ "name": name, "exp": exp }))
      .await;
    assert_eq!(resp.status(), StatusCode::OK, "create token {name}");
    let body: Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 32);
    (
      token,
      Uuid::parse_str(body["uuid"].as_str().unwrap()).unwrap(),
    )
  }

  /// Drive the full push flow like the CLI and assert every step succeeds:
  /// `POST /cache/push/info`, `POST /cache/push`, `POST /cache/push/{id}/{part}` per chunk, `PUT /cache/push/{id}`.
  pub async fn push_path(&self, token: &str, cache: &str, nar: &TestNar) -> PushOutcome {
    self.push_path_opts(token, cache, nar, false).await
  }

  pub async fn push_path_opts(
    &self,
    token: &str,
    cache: &str,
    nar: &TestNar,
    force: bool,
  ) -> PushOutcome {
    let resp = self
      .raw(Method::POST, "/cache/push/info")
      .bearer_auth(token)
      .json(&json!({ "cache": cache, "paths": [nar.store_path], "force": force }))
      .send()
      .await
      .unwrap();
    if resp.status() == StatusCode::NO_CONTENT {
      return PushOutcome {
        skipped: true,
        cache: None,
        upload: None,
        nar_found: false,
      };
    }
    assert_eq!(resp.status(), StatusCode::OK, "push info");
    let info: Value = resp.json().await.unwrap();
    assert_eq!(info["paths"], json!([nar.store_path]));
    let cache_id = Uuid::parse_str(info["cache"].as_str().unwrap()).unwrap();

    let resp = self
      .raw(Method::POST, "/cache/push")
      .bearer_auth(token)
      .json(&nar.upload_request(cache_id, force))
      .send()
      .await
      .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "upload path");
    let body: Value = resp.json().await.unwrap();
    let upload = Uuid::parse_str(body["uuid"].as_str().unwrap()).unwrap();
    let nar_found = body["nar_found"].as_bool().unwrap();

    if !nar_found {
      for (part, chunk) in nar.chunks(CHUNK_SIZE).into_iter().enumerate() {
        let resp = self
          .raw(Method::POST, &format!("/cache/push/{upload}/{part}"))
          .bearer_auth(token)
          .body(chunk)
          .send()
          .await
          .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "upload chunk {part}");
      }
    }

    let resp = self
      .raw(Method::PUT, &format!("/cache/push/{upload}"))
      .bearer_auth(token)
      .json(&nar.finish_request())
      .send()
      .await
      .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "upload finish");

    PushOutcome {
      skipped: false,
      cache: Some(cache_id),
      upload: Some(upload),
      nar_found,
    }
  }
}

/// Settings applied by [`TestServer::create_cache`]. Defaults: public, priority 40, 1 GiB quota, LRU, test key,
/// no force push, no downstream caches.
#[derive(Clone)]
pub struct CacheOptions {
  pub public: bool,
  pub priority: i32,
  /// In bytes
  pub quota: i64,
  /// `LeastRecentlyUsed`, `LeastFrequentlyUsed` or `OldestFirst`
  pub eviction_policy: &'static str,
  pub sig_key: String,
  pub allow_force_push: bool,
  pub downstream_caches: Vec<String>,
}

impl Default for CacheOptions {
  fn default() -> Self {
    Self {
      public: true,
      priority: 40,
      quota: 1024 * 1024 * 1024,
      eviction_policy: "LeastRecentlyUsed",
      sig_key: TEST_PUBLIC_KEY.to_string(),
      allow_force_push: false,
      downstream_caches: Vec::new(),
    }
  }
}

impl CacheOptions {
  /// Body for `POST /cache/management/{uuid}` (edit).
  pub fn edit_body(&self, name: &str) -> Value {
    json!({
      "name": name,
      "priority": self.priority,
      "public": self.public,
      "quota": self.quota,
      "sig_key": self.sig_key,
      "allow_force_push": self.allow_force_push,
      "eviction_policy": self.eviction_policy,
      "downstream_caches": self.downstream_caches,
    })
  }
}

/// A local stand-in for a downstream cache: answers 404 to everything until mocks are mounted.
/// Use [`downstream_has`] to make it report store paths as present.
pub async fn mock_downstream() -> MockServer {
  MockServer::start().await
}

/// Make `server` answer `HEAD /{hash}.narinfo` with 200 for `nar`.
pub async fn downstream_has(server: &MockServer, nar: &TestNar) {
  Mock::given(method("HEAD"))
    .and(wiremock::matchers::path(format!("/{}.narinfo", nar.hash)))
    .respond_with(ResponseTemplate::new(200))
    .mount(server)
    .await;
}

#[derive(Debug, Clone)]
pub struct PushOutcome {
  /// `/push/info` answered 204: everything already present (cache or downstream)
  pub skipped: bool,
  pub cache: Option<Uuid>,
  pub upload: Option<Uuid>,
  /// The server already had this NAR (dedupe), no chunks were sent
  pub nar_found: bool,
}

/// A store path with a single regular file, serialized as a valid NAR, zstd compressed and signed like the CLI does.
#[derive(Clone)]
pub struct TestNar {
  /// Base path `<hash>-<name>`
  pub store_path: String,
  /// 32 char store path hash
  pub hash: String,
  pub nar: Vec<u8>,
  /// zstd compressed NAR, exactly what the server stores and serves
  pub file: Vec<u8>,
  /// nix base32 sha256 of `file`
  pub file_hash: String,
  /// nix base32 sha256 of `nar`
  pub nar_hash: String,
  pub references: Vec<String>,
  pub deriver: Option<String>,
  pub signature: String,
  key: KeyPair,
}

impl TestNar {
  /// `name` must be a valid store path name. The hash is derived from `name`, so equal names give equal paths.
  pub fn new(name: &str, contents: &[u8]) -> Self {
    let hash = to_nix_base32(&Sha256::digest(name.as_bytes())[..20]);
    let nar = nar_regular_file(contents, false);
    let file = zstd::encode_all(&nar[..], 3).unwrap();
    let mut test_nar = TestNar {
      store_path: format!("{hash}-{name}"),
      hash,
      file_hash: to_nix_base32(&Sha256::digest(&file)),
      nar_hash: to_nix_base32(&Sha256::digest(&nar)),
      nar,
      file,
      references: Vec::new(),
      deriver: None,
      signature: String::new(),
      key: test_key(),
    };
    test_nar.sign();
    test_nar
  }

  /// Base store paths (`<hash>-<name>`) this path references.
  pub fn references(mut self, references: &[&str]) -> Self {
    self.references = references.iter().map(|r| r.to_string()).collect();
    self.sign();
    self
  }

  /// Base store path of the deriver, e.g. `<hash>-foo.drv`.
  pub fn deriver(mut self, deriver: &str) -> Self {
    self.deriver = Some(deriver.to_string());
    self
  }

  pub fn signed_with(mut self, key: KeyPair) -> Self {
    self.key = key;
    self.sign();
    self
  }

  fn sign(&mut self) {
    let references = self
      .references
      .iter()
      .map(|r| StorePath::from_base_path(r).expect("valid reference"))
      .collect::<Vec<_>>();
    self.signature = self.key.sign(
      &StorePath::from_base_path(&self.store_path).expect("valid store path"),
      &self.nar_hash,
      self.nar_size(),
      &references,
    );
  }

  pub fn nar_size(&self) -> u64 {
    self.nar.len() as u64
  }

  pub fn file_size(&self) -> u64 {
    self.file.len() as u64
  }

  /// Full store path `/nix/store/<hash>-<name>`
  pub fn full_path(&self) -> String {
    format!("/nix/store/{}", self.store_path)
  }

  /// `nar/<file_hash>.nar.zst`, relative to the cache root as in the narinfo `URL:` field
  pub fn nar_url(&self) -> String {
    format!("nar/{}.nar.zst", self.file_hash)
  }

  /// Split `file` like the CLI does: always at least one (possibly empty) part.
  pub fn chunks(&self, size: usize) -> Vec<Vec<u8>> {
    if self.file.is_empty() {
      return vec![Vec::new()];
    }
    self.file.chunks(size).map(<[u8]>::to_vec).collect()
  }

  /// Body for `POST /cache/push`
  pub fn upload_request(&self, cache: Uuid, force: bool) -> Value {
    json!({
      "cache": cache,
      "force": force,
      "store_path": self.store_path,
      "nar_hash": self.nar_hash,
      "nar_size": self.nar_size(),
      "deriver": self.deriver,
      "references": self.references,
      "signature": self.signature,
    })
  }

  /// Body for `PUT /cache/push/{id}`
  pub fn finish_request(&self) -> Value {
    json!({ "file_hash": self.file_hash, "file_size": self.file_size() })
  }
}

/// NAR serialization of a single regular file.
pub fn nar_regular_file(contents: &[u8], executable: bool) -> Vec<u8> {
  fn str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u64).to_le_bytes());
    out.extend_from_slice(s);
    out.resize(out.len().next_multiple_of(8), 0);
  }

  let mut out = Vec::new();
  for s in [&b"nix-archive-1"[..], b"(", b"type", b"regular"] {
    str(&mut out, s);
  }
  if executable {
    str(&mut out, b"executable");
    str(&mut out, b"");
  }
  str(&mut out, b"contents");
  str(&mut out, contents);
  str(&mut out, b")");
  out
}

/// Monotonic counter for generating unique names within a test.
static COUNTER: AtomicU32 = AtomicU32::new(0);

pub fn unique(prefix: &str) -> String {
  format!("{prefix}-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}
