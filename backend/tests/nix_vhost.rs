//! Virtual host routing: `<cache>.<site host>/<path>` is served as `/api/nix/<cache>/<path>`.
mod common;
mod nix_common;

use common::{CacheOptions, TestNar, TestServer};
use nix_common::header;
use reqwest::{Method, StatusCode};

async fn vhost_server() -> (TestServer, String, TestNar) {
  let server = TestServer::start_with(|| unsafe {
    std::env::set_var("VIRTUAL_HOST_ROUTING", "true");
    std::env::set_var("SITE_URL", "http://cache.example.com/");
  })
  .await;
  server
    .setup_admin(
      common::ADMIN_NAME,
      common::ADMIN_EMAIL,
      common::ADMIN_PASSWORD,
    )
    .await;
  server.create_cache("main", &CacheOptions::default()).await;
  let (token, _) = server.create_cli_token("ci").await;
  let nar = TestNar::new("hello-1.0", b"hello world");
  server.push_path(&token, "main", &nar).await;
  (server, token, nar)
}

fn on_host(server: &TestServer, method: Method, host: &str, path: &str) -> reqwest::RequestBuilder {
  server
    .client
    .request(method, server.root_url(path))
    .header("Host", host)
}

#[tokio::test]
async fn subdomain_is_rewritten_to_cache() {
  let (server, _, nar) = vhost_server().await;

  let resp = on_host(
    &server,
    Method::GET,
    "main.cache.example.com",
    "/nix-cache-info",
  )
  .send()
  .await
  .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(header(&resp, "content-type"), Some("text/x-nix-cache-info"));
  assert!(
    resp
      .text()
      .await
      .unwrap()
      .starts_with("StoreDir: /nix/store\n")
  );

  let resp = on_host(
    &server,
    Method::GET,
    "main.cache.example.com",
    &format!("/{}.narinfo", nar.hash),
  )
  .send()
  .await
  .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  assert!(
    resp
      .text()
      .await
      .unwrap()
      .contains(&format!("URL: {}\n", nar.nar_url()))
  );

  // The relative URL from the narinfo resolves against the virtual host root
  let resp = on_host(
    &server,
    Method::GET,
    "main.cache.example.com",
    &format!("/{}", nar.nar_url()),
  )
  .header("Range", "bytes=0-3")
  .send()
  .await
  .unwrap();
  assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
  assert_eq!(resp.bytes().await.unwrap(), nar.file[..4]);

  let resp = on_host(
    &server,
    Method::HEAD,
    "main.cache.example.com:8080",
    &format!("/{}.narinfo", nar.hash),
  )
  .send()
  .await
  .unwrap();
  assert_eq!(resp.status(), StatusCode::OK, "port in Host is ignored");
}

#[tokio::test]
async fn subdomain_is_case_insensitive_and_unknown_is_not_found() {
  let (server, _, _) = vhost_server().await;
  for (host, status) in [
    ("MAIN.cache.example.com", StatusCode::OK),
    ("other.cache.example.com", StatusCode::NOT_FOUND),
  ] {
    let resp = on_host(&server, Method::GET, host, "/nix-cache-info")
      .send()
      .await
      .unwrap();
    assert_eq!(resp.status(), status, "{host}");
  }
}

#[tokio::test]
async fn site_host_and_foreign_hosts_are_not_rewritten() {
  let (server, _, _) = vhost_server().await;
  // the site itself and unrelated hosts keep the normal routes
  for host in ["cache.example.com", "localhost", "main.other.org"] {
    let resp = on_host(&server, Method::GET, host, "/api/health")
      .send()
      .await
      .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "{host}");
    let resp = on_host(&server, Method::GET, host, "/api/nix/main/nix-cache-info")
      .send()
      .await
      .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "{host}");
  }
}

#[tokio::test]
async fn private_cache_via_vhost_uses_basic_auth() {
  let (server, token, _) = vhost_server().await;
  let id = server
    .create_cache(
      "secret",
      &CacheOptions {
        public: false,
        ..Default::default()
      },
    )
    .await;
  let _ = id;

  let anon = on_host(
    &server,
    Method::GET,
    "secret.cache.example.com",
    "/nix-cache-info",
  )
  .send()
  .await
  .unwrap();
  assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);
  let authed = on_host(
    &server,
    Method::GET,
    "secret.cache.example.com",
    "/nix-cache-info",
  )
  .basic_auth("nix", Some(&token))
  .send()
  .await
  .unwrap();
  assert_eq!(authed.status(), StatusCode::OK);
  let bearer = on_host(
    &server,
    Method::GET,
    "secret.cache.example.com",
    "/nix-cache-info",
  )
  .bearer_auth(&token)
  .send()
  .await
  .unwrap();
  assert_eq!(bearer.status(), StatusCode::OK);
}

#[tokio::test]
async fn vhost_routing_disabled_by_default() {
  let (server, _) = TestServer::start_with_admin().await;
  server.create_cache("main", &CacheOptions::default()).await;
  let resp = on_host(
    &server,
    Method::GET,
    "main.localhost.example.com",
    "/nix-cache-info",
  )
  .send()
  .await
  .unwrap();
  assert_ne!(resp.status(), StatusCode::OK);
}
