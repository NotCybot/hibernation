//! `GET /api/nix/{cache}/nix-cache-info` and routing of unknown nix endpoints.
mod common;
mod nix_common;

use common::{CacheOptions, TestServer};
use nix_common::{get_status, header, nix, send};
use reqwest::{Method, StatusCode};

#[tokio::test]
async fn cache_info_body_and_content_type() {
  let (server, _) = TestServer::start_with_admin().await;
  let opts = CacheOptions {
    priority: 7,
    ..Default::default()
  };
  server.create_cache("main", &opts).await;

  let resp = send(nix(&server, Method::GET, "main", "nix-cache-info")).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(header(&resp, "content-type"), Some("text/x-nix-cache-info"));
  assert_eq!(
    resp.text().await.unwrap(),
    "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 7\n"
  );
}

#[tokio::test]
async fn cache_info_priority_zero_and_edit_is_visible_immediately() {
  let (server, _) = TestServer::start_with_admin().await;
  let mut opts = CacheOptions {
    priority: 0,
    ..Default::default()
  };
  let id = server.create_cache("main", &opts).await;
  let body = || async {
    send(nix(&server, Method::GET, "main", "nix-cache-info"))
      .await
      .text()
      .await
      .unwrap()
  };
  assert!(body().await.ends_with("Priority: 0\n"));

  // The lookup is cached for 30s, editing the cache must invalidate it
  opts.priority = 99;
  let resp = server
    .post(&format!("/cache/management/{id}"), opts.edit_body("main"))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert!(body().await.ends_with("Priority: 99\n"));
}

#[tokio::test]
async fn cache_info_name_is_case_insensitive() {
  let (server, _) = TestServer::start_with_admin().await;
  server.create_cache("main", &CacheOptions::default()).await;

  for name in ["main", "MAIN", "Main", "mAiN"] {
    assert_eq!(
      get_status(nix(&server, Method::GET, name, "nix-cache-info")).await,
      StatusCode::OK,
      "{name}"
    );
  }
}

#[tokio::test]
async fn cache_info_unknown_cache_is_not_found() {
  let (server, _) = TestServer::start_with_admin().await;
  server.create_cache("main", &CacheOptions::default()).await;

  for name in ["other", "mai", "main2", "%20"] {
    assert_eq!(
      get_status(nix(&server, Method::GET, name, "nix-cache-info")).await,
      StatusCode::NOT_FOUND,
      "{name}"
    );
  }
}

#[tokio::test]
async fn cache_info_head_has_headers_without_body() {
  let (server, _) = TestServer::start_with_admin().await;
  server.create_cache("main", &CacheOptions::default()).await;

  let resp = send(nix(&server, Method::HEAD, "main", "nix-cache-info")).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(header(&resp, "content-type"), Some("text/x-nix-cache-info"));
  assert!(resp.bytes().await.unwrap().is_empty());

  assert_eq!(
    get_status(nix(&server, Method::HEAD, "nope", "nix-cache-info")).await,
    StatusCode::NOT_FOUND
  );
}

#[tokio::test]
async fn cache_info_of_deleted_cache_is_not_found() {
  let (server, _) = TestServer::start_with_admin().await;
  let id = server.create_cache("main", &CacheOptions::default()).await;
  assert_eq!(
    get_status(nix(&server, Method::GET, "main", "nix-cache-info")).await,
    StatusCode::OK
  );

  let resp = server
    .delete("/cache/management", serde_json::json!({ "uuid": id }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(
    get_status(nix(&server, Method::GET, "main", "nix-cache-info")).await,
    StatusCode::NOT_FOUND
  );
}

#[tokio::test]
async fn unknown_nix_endpoints_are_not_found() {
  let (server, _) = TestServer::start_with_admin().await;
  server.create_cache("main", &CacheOptions::default()).await;

  // nix probes these for content addressed derivations and build logs; missing must be a plain 404
  for path in [
    "realisations/sha256:abc!out.doi",
    "log/abc-foo.drv",
    "nar",
    "nar/",
    "debuginfo/abc",
    "abc.ls",
    "nar/abc.nar.zst/extra",
    "a/b/c/d",
  ] {
    assert_eq!(
      get_status(nix(&server, Method::GET, "main", path)).await,
      StatusCode::NOT_FOUND,
      "{path}"
    );
  }
  for path in ["/nix/main", "/nix/main/", "/nix/nope"] {
    assert_eq!(
      get_status(server.raw(Method::GET, path)).await,
      StatusCode::NOT_FOUND,
      "{path}"
    );
  }
  // Uploading is not part of the read API
  assert_eq!(
    get_status(nix(&server, Method::PUT, "main", "abc.narinfo")).await,
    StatusCode::METHOD_NOT_ALLOWED
  );
}
