//! End to end against the real nix client: the responses must be accepted by `nix` itself.
//! Skipped (passes with a note) when `nix` is not on PATH.
mod common;

use std::{path::Path, process::Command};

use common::{CacheOptions, TEST_PUBLIC_KEY, TestNar, TestServer, random_key};

fn nix_available() -> bool {
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
fn run_nix(home: &Path, args: &[&str]) -> (bool, String, String) {
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

struct Ctx {
  server: TestServer,
  token: String,
  tmp: tempfile::TempDir,
  dep: TestNar,
  top: TestNar,
}

async fn setup(public: bool) -> Ctx {
  let (server, _) = TestServer::start_with_admin().await;
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
  fn store(&self) -> String {
    self.server.url("/nix/main")
  }

  fn root(&self, name: &str) -> String {
    format!("local?root={}", self.tmp.path().join(name).display())
  }

  fn copy(&self, name: &str, key: &str, extra: &[&str]) -> (bool, String, String) {
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
}

#[tokio::test(flavor = "multi_thread")]
async fn nix_store_info_and_path_info() {
  if !nix_available() {
    return;
  }
  let c = setup(true).await;
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
  let info: serde_json::Value = serde_json::from_str(&out).unwrap();
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

#[tokio::test(flavor = "multi_thread")]
async fn nix_copy_verifies_hash_and_signature() {
  if !nix_available() {
    return;
  }
  let c = setup(true).await;

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

#[tokio::test(flavor = "multi_thread")]
async fn nix_copy_from_private_cache_with_netrc() {
  if !nix_available() {
    return;
  }
  let c = setup(false).await;

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

#[tokio::test(flavor = "multi_thread")]
async fn nix_copy_rejects_tampered_nar() {
  if !nix_available() {
    return;
  }
  let c = setup(true).await;
  // Replace the stored dependency with another valid zstd NAR of the same size: transfer succeeds, NarHash does not
  let dep_file = c
    .server
    .storage_path()
    .join(format!("{}.nar", nar_id(&c, &c.dep).await));
  let fake = (b'a'..=b'z')
    .map(|ch| {
      let mut contents = b"dependency contents\n".to_vec();
      contents[0] = ch;
      zstd::encode_all(&common::nar_regular_file(&contents, false)[..], 3).unwrap()
    })
    .find(|f| f.len() == c.dep.file.len() && *f != c.dep.file)
    .expect("same size fake");
  std::fs::write(&dep_file, &fake).unwrap();

  let (ok, _, err) = c.copy("tampered", TEST_PUBLIC_KEY, &[]);
  assert!(!ok, "tampered nar must be rejected");
  assert!(err.contains("hash mismatch"), "{err}");
}

async fn nar_id(c: &Ctx, nar: &TestNar) -> uuid::Uuid {
  use backend::db::DBTrait;
  let db = c.server.db().await;
  let info = db.cache().by_name("main".into()).await.unwrap().unwrap();
  db.nar()
    .get_nar(info.id, &nar.file_hash, "zst")
    .await
    .unwrap()
    .unwrap()
    .0
}
