//! End-to-end smoke test of the harness: every Postgres-flavoured query the push/serve/evict/dedupe paths use
//! has to work on the SQLite test database.
mod common;

use backend::db::DBTrait;
use common::{CacheOptions, TestNar, TestServer, downstream_has, mock_downstream};
use reqwest::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn push_serve_evict_dedupe() {
  let (server, _) = TestServer::start_with_admin().await;
  // Rate-limited login route works over the real TCP listener (connect info for the IP key extractor)
  server.clear_cookies();
  let resp = server
    .login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD)
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert!(server.has_cookie(common::JWT_COOKIE_NAME));

  let downstream = mock_downstream().await;
  let opts = CacheOptions {
    downstream_caches: vec![downstream.uri()],
    ..Default::default()
  };
  let cache = server.create_cache("main", &opts).await;
  let (token, _) = server.create_cli_token("ci").await;

  // Push and serve
  let a = TestNar::new("hello-1.0", b"hello world");
  let pushed = server.push_path(&token, "main", &a).await;
  assert!(!pushed.skipped && !pushed.nar_found);
  assert_eq!(pushed.cache, Some(cache));
  // Already present: /push/info answers 204 (missing_paths query)
  assert!(server.push_path(&token, "main", &a).await.skipped);

  let resp = server
    .raw(Method::GET, "/nix/main/nix-cache-info")
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(
    resp.text().await.unwrap(),
    "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 40\n"
  );

  let resp = server
    .raw(Method::GET, &format!("/nix/main/{}.narinfo", a.hash))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  let narinfo = resp.text().await.unwrap();
  assert!(narinfo.contains(&format!("StorePath: {}\n", a.full_path())));
  assert!(narinfo.contains(&format!("URL: {}\n", a.nar_url())));
  assert!(narinfo.contains(&format!("NarHash: sha256:{}\n", a.nar_hash)));
  assert!(narinfo.contains(&format!("Sig: {}\n", a.signature)));

  let resp = server
    .raw(Method::GET, &format!("/nix/main/{}", a.nar_url()))
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(resp.bytes().await.unwrap(), a.file);

  // Downstream filtering goes to the local mock, never the network
  let b = TestNar::new("upstream-1.0", b"from upstream");
  downstream_has(&downstream, &b).await;
  assert!(server.push_path(&token, "main", &b).await.skipped);

  // Same NAR under another store path reuses the stored nar (create_nar UPDATE ... RETURNING)
  let dup = TestNar::new("hello-copy-1.0", b"hello world");
  assert!(server.push_path(&token, "main", &dup).await.nar_found);
  let db = server.db().await;
  assert!(db.nar().find_duplicate_nars().await.unwrap().is_empty());
  backend::cache::dedupe::run_dedupe(&db).await;
  backend::cache::cleanup::run_cleanup(&db, &server.file_storage().await).await;
  backend::cache::cleanup::run_sweep(&db, &server.file_storage().await)
    .await
    .unwrap();

  // Eviction on push: quota only fits one of the next two paths besides nothing else
  let resp = server
    .delete(&format!("/cache/management/{cache}"), json!({}))
    .await;
  assert_eq!(resp.status(), StatusCode::OK, "clear cache");
  let c = TestNar::new("c-1.0", &[1u8; 4096]);
  let d = TestNar::new("d-1.0", &[2u8; 4096]);
  let tight = CacheOptions {
    quota: (c.file_size() + d.file_size() - 1) as i64,
    ..opts.clone()
  };
  let resp = server
    .post(
      &format!("/cache/management/{cache}"),
      tight.edit_body("main"),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  server.push_path(&token, "main", &c).await;
  server.push_path(&token, "main", &d).await;
  let status = |nar: &TestNar| {
    let req = server.raw(Method::GET, &format!("/nix/main/{}.narinfo", nar.hash));
    async move { req.send().await.unwrap().status() }
  };
  assert_eq!(status(&c).await, StatusCode::NOT_FOUND, "c evicted");
  assert_eq!(status(&d).await, StatusCode::OK);

  // Eviction on quota edit
  let zero = CacheOptions {
    quota: 0,
    eviction_policy: "OldestFirst",
    ..opts
  };
  let resp = server
    .post(
      &format!("/cache/management/{cache}"),
      zero.edit_body("main"),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(status(&d).await, StatusCode::NOT_FOUND, "d evicted");
}

#[test]
fn nar_serialization_matches_nix() {
  // `nix-store --dump` of a file containing "hello world" (no trailing newline)
  let nar = common::nar_regular_file(b"hello world", false);
  assert_eq!(nar.len(), 128);
  assert_eq!(
    shared::hash::to_nix_base32(&<sha2::Sha256 as sha2::Digest>::digest(&nar)),
    "14r07wnk5h2bl40v1nknwkwrzh5li7z4bl02vcsqh52vxwm08idf"
  );
}
