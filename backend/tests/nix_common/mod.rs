//! Helpers shared by the `nix_*` integration tests (users, groups, sessions, raw nix requests).
#![allow(dead_code)]

use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::{ADMIN_EMAIL, ADMIN_PASSWORD, JWT_COOKIE_NAME, TestServer, unique};

pub const USER_PASSWORD: &str = "userpass123";

/// A non admin user plus its credentials.
pub struct User {
  pub id: Uuid,
  pub name: String,
  pub email: String,
  /// JWT session cookie value
  pub jwt: String,
  /// 32 char CLI token
  pub token: String,
}

/// Create a user (no groups) as the admin, log in as it to grab a JWT and a CLI token, then log back in as admin.
pub async fn create_user(server: &TestServer) -> User {
  let name = unique("user");
  let email = format!("{name}@example.com");
  let password = server.encrypt_password(USER_PASSWORD).await;
  let resp = server
    .post(
      "/user/management",
      json!({ "name": name, "email": email, "password": password }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "create user");
  let id = Uuid::parse_str(
    resp.json::<Value>().await.unwrap()["uuid"]
      .as_str()
      .unwrap(),
  )
  .unwrap();

  server.clear_cookies();
  assert_eq!(
    server.login(&email, USER_PASSWORD).await.status(),
    StatusCode::OK
  );
  let jwt = server.cookie(JWT_COOKIE_NAME).expect("user jwt");
  let (token, _) = server.create_cli_token(&unique("token")).await;
  relogin_admin(server).await;

  User {
    id,
    name,
    email,
    jwt,
    token,
  }
}

pub async fn relogin_admin(server: &TestServer) {
  server.clear_cookies();
  assert_eq!(
    server.login(ADMIN_EMAIL, ADMIN_PASSWORD).await.status(),
    StatusCode::OK
  );
}

pub fn mapping(cache: Uuid, cache_name: &str, access: &str) -> Value {
  json!({ "uuid": cache, "name": cache_name, "access_type": access })
}

/// `PUT /user/management` keeping the user name, setting groups and cache mappings.
pub async fn edit_user(server: &TestServer, user: &User, groups: &[Uuid], caches: Vec<Value>) {
  let resp = server
    .put(
      "/user/management",
      json!({ "uuid": user.id, "name": user.name, "groups": groups, "caches": caches }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "edit user");
}

pub async fn create_group(server: &TestServer) -> Uuid {
  let resp = server
    .post("/group", json!({ "name": unique("group") }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "create group");
  Uuid::parse_str(
    resp.json::<Value>().await.unwrap()["uuid"]
      .as_str()
      .unwrap(),
  )
  .unwrap()
}

/// `PUT /group` replacing permissions, members and cache mappings.
pub async fn edit_group(
  server: &TestServer,
  group: Uuid,
  permissions: &[&str],
  users: &[Uuid],
  caches: Vec<Value>,
) {
  let resp = server
    .put(
      "/group",
      json!({
        "uuid": group,
        "name": format!("g-{group}"),
        "permissions": permissions,
        "users": users,
        "caches": caches,
      }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "edit group");
}

/// A cookie-less request to a nix endpoint.
pub fn nix(server: &TestServer, method: Method, cache: &str, path: &str) -> RequestBuilder {
  server.raw(method, &format!("/nix/{cache}/{path}"))
}

pub async fn get_status(req: RequestBuilder) -> StatusCode {
  req.send().await.unwrap().status()
}

pub async fn send(req: RequestBuilder) -> Response {
  req.send().await.unwrap()
}

pub fn header<'a>(resp: &'a Response, name: &str) -> Option<&'a str> {
  resp.headers().get(name).map(|v| v.to_str().unwrap())
}

// ---- real `nix` client e2e, shared by `nix_client.rs` (sqlite + local fs) and `pg_s3.rs` (postgres + s3) ----

use std::{path::Path, process::Command};

use crate::common::{CacheOptions, TEST_PUBLIC_KEY, TestNar, random_key};

pub fn nix_available() -> bool {
  let ok = Command::new("nix")
    .arg("--version")
    .output()
    .is_ok_and(|o| o.status.success());
  if !ok {
    eprintln!("nix not found on PATH, skipping nix client e2e test");
  }
  ok
}

/// Run `nix <args>` isolated from the user's narinfo disk cache. Returns (success, stdout, stderr).
pub fn run_nix(home: &Path, args: &[&str]) -> (bool, String, String) {
  let out = Command::new("nix")
    .env("XDG_CACHE_HOME", home.join("cache"))
    .env("XDG_STATE_HOME", home.join("state"))
    .env("XDG_DATA_HOME", home.join("data"))
    .args(["--extra-experimental-features", "nix-command"])
    .args(["--option", "narinfo-cache-negative-ttl", "0"])
    .args(["--option", "narinfo-cache-positive-ttl", "0"])
    .args(args)
    .output()
    .expect("run nix");
  (
    out.status.success(),
    String::from_utf8_lossy(&out.stdout).into_owned(),
    String::from_utf8_lossy(&out.stderr).into_owned(),
  )
}

pub struct Ctx {
  pub server: TestServer,
  pub token: String,
  pub tmp: tempfile::TempDir,
  pub dep: TestNar,
  pub top: TestNar,
}

/// Admin setup on `server`, a `main` cache, and `top` (referencing `dep`) pushed to it.
pub async fn setup(server: TestServer, public: bool) -> Ctx {
  let (server, _) = TestServer::with_admin(server).await;
  server
    .create_cache(
      "main",
      &CacheOptions {
        public,
        ..Default::default()
      },
    )
    .await;
  let (token, _) = server.create_cli_token("ci").await;
  let dep = TestNar::new("nix-e2e-dep-1.0", b"dependency contents\n");
  let top = TestNar::new("nix-e2e-top-1.0", b"top level contents\n")
    .references(&[&dep.store_path])
    .deriver(&format!("{}-nix-e2e-top-1.0.drv", dep.hash));
  server.push_path(&token, "main", &dep).await;
  server.push_path(&token, "main", &top).await;
  Ctx {
    server,
    token,
    tmp: tempfile::TempDir::new().unwrap(),
    dep,
    top,
  }
}

impl Ctx {
  pub fn store(&self) -> String {
    self.server.url("/nix/main")
  }

  fn root(&self, name: &str) -> String {
    format!("local?root={}", self.tmp.path().join(name).display())
  }

  pub fn copy(&self, name: &str, key: &str, extra: &[&str]) -> (bool, String, String) {
    let store = self.store();
    let to = self.root(name);
    let mut args = vec![
      "copy",
      "--from",
      &store,
      "--to",
      &to,
      "--option",
      "trusted-public-keys",
      key,
    ];
    args.extend_from_slice(extra);
    let top = self.top.full_path();
    args.push(&top);
    run_nix(self.tmp.path(), &args)
  }

  async fn nar_id(&self, nar: &TestNar) -> Uuid {
    use backend::db::DBTrait;
    let db = self.server.db().await;
    let info = db.cache().by_name("main".into()).await.unwrap().unwrap();
    db.nar()
      .get_nar(info.id, &nar.file_hash, "zst")
      .await
      .unwrap()
      .unwrap()
      .0
  }
}

pub async fn store_info_and_path_info(server: TestServer) {
  let c = setup(server, true).await;
  let store = c.store();

  let (ok, out, err) = run_nix(c.tmp.path(), &["store", "info", "--store", &store]);
  assert!(ok, "store info: {err}");
  // printed on stderr by current nix versions
  assert!(
    format!("{out}{err}").contains(&format!("Store URL: {store}")),
    "{out}{err}"
  );

  let (ok, out, err) = run_nix(
    c.tmp.path(),
    &["path-info", "--json", "--store", &store, &c.top.full_path()],
  );
  assert!(ok, "path-info: {err}");
  let info: Value = serde_json::from_str(&out).unwrap();
  let entry = info
    .as_object()
    .and_then(|m| m.values().next().cloned())
    .or_else(|| info.as_array().and_then(|a| a.first().cloned()))
    .unwrap();
  assert_eq!(entry["narSize"], c.top.nar_size());
  assert_eq!(entry["downloadSize"], c.top.file_size());
  assert_eq!(entry["compression"], "zstd");
  assert!(
    entry["references"].to_string().contains(&c.dep.store_path),
    "{entry}"
  );
  assert!(
    entry["signatures"].to_string().contains("testing:"),
    "{entry}"
  );

  // a missing path is reported as such, not as a protocol error
  let missing = TestNar::new("nix-e2e-missing-1.0", b"");
  let (ok, _, err) = run_nix(
    c.tmp.path(),
    &["path-info", "--store", &store, &missing.full_path()],
  );
  assert!(!ok);
  assert!(
    err.contains("is not valid") || err.contains("does not exist"),
    "{err}"
  );
}

pub async fn copy_verifies_hash_and_signature(server: TestServer) {
  let c = setup(server, true).await;

  let (ok, _, err) = c.copy("good", TEST_PUBLIC_KEY, &[]);
  assert!(ok, "copy: {err}");
  let real = c.tmp.path().join("good/nix/store");
  assert_eq!(
    std::fs::read(real.join(&c.top.store_path)).unwrap(),
    b"top level contents\n"
  );
  // the closure came along
  assert_eq!(
    std::fs::read(real.join(&c.dep.store_path)).unwrap(),
    b"dependency contents\n"
  );

  // untrusted key: nix refuses the paths
  let other = random_key("other").export_public_key();
  let (ok, _, err) = c.copy("bad", &other, &[]);
  assert!(!ok, "copy with untrusted key must fail");
  assert!(
    err.contains("signature") || err.contains("trusted"),
    "{err}"
  );
}

pub async fn copy_from_private_cache_with_netrc(server: TestServer) {
  let c = setup(server, false).await;

  let (ok, _, _) = c.copy("anon", TEST_PUBLIC_KEY, &[]);
  assert!(!ok, "anonymous copy from private cache must fail");

  let netrc = c.tmp.path().join("netrc");
  std::fs::write(
    &netrc,
    format!("machine localhost login nix password {}\n", c.token),
  )
  .unwrap();
  let netrc = netrc.display().to_string();
  let (ok, _, err) = c.copy(
    "authed",
    TEST_PUBLIC_KEY,
    &["--option", "netrc-file", &netrc],
  );
  assert!(ok, "copy with netrc: {err}");
  assert!(
    c.tmp
      .path()
      .join("authed/nix/store")
      .join(&c.top.store_path)
      .exists()
  );
}

pub async fn copy_rejects_tampered_nar(server: TestServer) {
  let c = setup(server, true).await;
  // Replace the stored dependency with another valid zstd NAR of the same size: transfer succeeds, NarHash does not
  let fake = (b'a'..=b'z')
    .map(|ch| {
      let mut contents = b"dependency contents\n".to_vec();
      contents[0] = ch;
      zstd::encode_all(&crate::common::nar_regular_file(&contents, false)[..], 3).unwrap()
    })
    .find(|f| f.len() == c.dep.file.len() && *f != c.dep.file)
    .expect("same size fake");
  // through the server's storage config, so this works for local fs and S3 alike
  c.server
    .file_storage()
    .await
    .save_file(
      &mut &fake[..],
      format!("{}.nar", c.nar_id(&c.dep).await).into(),
    )
    .await
    .unwrap();

  let (ok, _, err) = c.copy("tampered", TEST_PUBLIC_KEY, &[]);
  assert!(!ok, "tampered nar must be rejected");
  assert!(err.contains("hash mismatch"), "{err}");
}

/// The real nix client tests against a server booted by `$start` (an expression giving a fresh [`TestServer`]).
#[macro_export]
macro_rules! nix_client_tests {
  ($start:expr) => {
    #[tokio::test(flavor = "multi_thread")]
    async fn nix_store_info_and_path_info() {
      if nix_common::nix_available() {
        nix_common::store_info_and_path_info($start).await;
      }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn nix_copy_verifies_hash_and_signature() {
      if nix_common::nix_available() {
        nix_common::copy_verifies_hash_and_signature($start).await;
      }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn nix_copy_from_private_cache_with_netrc() {
      if nix_common::nix_available() {
        nix_common::copy_from_private_cache_with_netrc($start).await;
      }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn nix_copy_rejects_tampered_nar() {
      if nix_common::nix_available() {
        nix_common::copy_rejects_tampered_nar($start).await;
      }
    }
  };
}
