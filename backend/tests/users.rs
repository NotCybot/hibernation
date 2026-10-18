//! User management (`user.rs` + centaurus `/user/*`): CRUD, cache mappings, permission gates.
mod auth_common;
mod common;

use auth_common::{USER_PASSWORD, create_group, create_user, token_for, user_with_perms, uuid};
use chrono::{Duration, Utc};
use common::{ADMIN_EMAIL, ADMIN_PASSWORD, CacheOptions, TestServer, unique};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

async fn admin_group(server: &TestServer) -> Uuid {
  let body: Value = server.get("/group").await.json().await.unwrap();
  uuid(&body["admin_group"])
}

async fn detail(server: &TestServer, id: Uuid) -> Value {
  let resp = server.get(&format!("/user/management/{id}")).await;
  assert_eq!(resp.status(), StatusCode::OK);
  resp.json().await.unwrap()
}

fn private() -> CacheOptions {
  CacheOptions {
    public: false,
    ..Default::default()
  }
}

// ---- list / create / detail / delete ----

#[tokio::test]
async fn create_detail_list_delete_flow() {
  let (server, admin) = TestServer::start_with_admin().await;
  let (id, email) = create_user(&server, "member").await;

  let users: Value = server.get("/user/management").await.json().await.unwrap();
  let text = users.to_string();
  assert!(text.contains(&admin.to_string()) && text.contains(&id.to_string()));

  let info = detail(&server, id).await;
  assert_eq!(uuid(&info["uuid"]), id);
  assert_eq!(info["name"], "member");
  assert_eq!(info["email"], email);
  assert_eq!(info["groups"], json!([]));
  assert_eq!(info["permissions"], json!([]));
  assert_eq!(info["caches"], json!([]));
  assert_eq!(info["oidc_user"], false);

  // the new user can log in
  auth_common::login_raw(&server, &email, USER_PASSWORD).await;

  let resp = server
    .delete("/user/management", json!({ "uuid": id }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let resp = server.get(&format!("/user/management/{id}")).await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_detail_has_admin_group_and_all_permissions() {
  let (server, admin) = TestServer::start_with_admin().await;
  let info = detail(&server, admin).await;
  let perms = info["permissions"].as_array().unwrap();
  for perm in backend::utils::permissions() {
    assert!(perms.contains(&json!(perm)), "{perm}");
  }
  assert_eq!(uuid(&info["groups"][0]["uuid"]), admin_group(&server).await);
}

#[tokio::test]
async fn detail_unknown_user_is_not_found() {
  let (server, _) = TestServer::start_with_admin().await;
  let resp = server
    .get(&format!("/user/management/{}", Uuid::new_v4()))
    .await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_validation() {
  let (server, _) = TestServer::start_with_admin().await;
  let password = server.encrypt_password("memberpass1").await;

  let cases = [
    (
      json!({ "name": " ", "email": "a@example.com", "password": password }),
      StatusCode::BAD_REQUEST,
    ),
    (
      json!({ "name": "a", "email": " ", "password": password }),
      StatusCode::BAD_REQUEST,
    ),
    (
      json!({ "name": "a", "email": ADMIN_EMAIL, "password": password }),
      StatusCode::CONFLICT,
    ),
    // no mailer configured, so a password is mandatory
    (
      json!({ "name": "a", "email": "a@example.com", "password": null }),
      StatusCode::BAD_REQUEST,
    ),
    (
      json!({ "name": "a", "email": "a@example.com", "password": "not base64!" }),
      StatusCode::BAD_REQUEST,
    ),
  ];
  for (body, status) in cases {
    let resp = server.post("/user/management", body.clone()).await;
    assert_eq!(resp.status(), status, "{body}");
  }
}

#[tokio::test]
async fn cannot_delete_last_admin() {
  let (server, admin) = TestServer::start_with_admin().await;
  let resp = server
    .delete("/user/management", json!({ "uuid": admin }))
    .await;
  assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn mail_and_groups_simple() {
  let (server, _) = TestServer::start_with_admin().await;
  let body: Value = server
    .get("/user/management/mail")
    .await
    .json()
    .await
    .unwrap();
  assert_eq!(body["active"], false);

  let group = create_group(&server, &[], &[]).await;
  let resp = server.get("/user/management/groups").await;
  assert_eq!(resp.status(), StatusCode::OK);
  let text = resp.text().await.unwrap();
  assert!(
    text.contains(&group.to_string()) && text.contains(&admin_group(&server).await.to_string())
  );
}

#[tokio::test]
async fn info_returns_own_profile() {
  let (server, admin) = TestServer::start_with_admin().await;
  let body: Value = server.get("/user/info").await.json().await.unwrap();
  assert_eq!(uuid(&body["uuid"]), admin);
  assert_eq!(body["email"], ADMIN_EMAIL);
  assert!(!body["permissions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn password_change_flow() {
  let (server, _) = TestServer::start_with_admin().await;
  let old_pw = server.encrypt_password(ADMIN_PASSWORD).await;
  let new_pw = server.encrypt_password("brandnewpass").await;
  let resp = server
    .post(
      "/user/account/password",
      json!({ "old_password": old_pw, "new_password": new_pw }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);

  server.clear_cookies();
  assert_eq!(
    server.login(ADMIN_EMAIL, ADMIN_PASSWORD).await.status(),
    StatusCode::UNAUTHORIZED
  );
  assert_eq!(
    server.login(ADMIN_EMAIL, "brandnewpass").await.status(),
    StatusCode::OK
  );
}

// ---- edit ----

#[tokio::test]
async fn edit_renames_and_replaces_groups() {
  let (server, _) = TestServer::start_with_admin().await;
  let (id, _) = create_user(&server, "member").await;
  let g1 = create_group(&server, &["user:view"], &[id]).await;
  let g2 = create_group(&server, &["group:view"], &[]).await;

  let resp = server
    .put(
      "/user/management",
      json!({ "uuid": id, "name": "renamed", "groups": [g2], "caches": [] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);

  let info = detail(&server, id).await;
  assert_eq!(info["name"], "renamed");
  let groups: Vec<Uuid> = info["groups"]
    .as_array()
    .unwrap()
    .iter()
    .map(|g| uuid(&g["uuid"]))
    .collect();
  assert_eq!(groups, vec![g2]);
  assert_eq!(info["permissions"], json!(["group:view"]));
  assert_ne!(g1, g2);
}

#[tokio::test]
async fn edit_validation() {
  let (server, admin) = TestServer::start_with_admin().await;
  let admin_group = admin_group(&server).await;

  let resp = server
    .put(
      "/user/management",
      json!({ "uuid": admin, "name": " ", "groups": [admin_group], "caches": [] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

  // removing the last admin from the admin group
  let resp = server
    .put(
      "/user/management",
      json!({ "uuid": admin, "name": "admin", "groups": [], "caches": [] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::CONFLICT);

  let resp = server
    .put(
      "/user/management",
      json!({ "uuid": Uuid::new_v4(), "name": "x", "groups": [], "caches": [] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn editor_cannot_escalate_or_edit_stronger_users() {
  let (server, admin) = TestServer::start_with_admin().await;
  let admin_group = admin_group(&server).await;
  let editor = user_with_perms(&server, &["user:edit", "user:view"]).await;
  let (target, _) = create_user(&server, "target").await;

  // assigning a group with permissions the editor lacks
  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/user/management",
      json!({ "uuid": target, "name": "t", "groups": [admin_group], "caches": [] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);

  // editing a user that has permissions the editor lacks
  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/user/management",
      json!({ "uuid": admin, "name": "admin", "groups": [admin_group], "caches": [] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);

  // a plain user is fine
  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/user/management",
      json!({ "uuid": target, "name": "ok", "groups": [], "caches": [] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
}

// ---- cache mappings ----

#[tokio::test]
async fn edit_sets_and_replaces_cache_mappings() {
  let (server, _) = TestServer::start_with_admin().await;
  let (id, _) = create_user(&server, "member").await;
  let a = server.create_cache("a", &private()).await;
  let b = server.create_cache("b", &private()).await;

  let edit = |caches: Value| {
    server.put(
      "/user/management",
      json!({ "uuid": id, "name": "member", "groups": [], "caches": caches }),
    )
  };

  let resp = edit(json!([
    { "uuid": a, "name": "a", "access_type": "View" },
    { "uuid": b, "name": "b", "access_type": "Edit" },
  ]))
  .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let info = detail(&server, id).await;
  let mut caches = info["caches"].as_array().unwrap().clone();
  caches.sort_by_key(|c| c["name"].as_str().unwrap().to_string());
  assert_eq!(
    caches,
    vec![
      json!({ "uuid": a, "name": "a", "access_type": "View" }),
      json!({ "uuid": b, "name": "b", "access_type": "Edit" }),
    ]
  );

  // replace semantics: old rows go away
  let resp = edit(json!([{ "uuid": a, "name": "a", "access_type": "Edit" }])).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let info = detail(&server, id).await;
  assert_eq!(
    info["caches"],
    json!([{ "uuid": a, "name": "a", "access_type": "Edit" }])
  );

  assert_eq!(edit(json!([])).await.status(), StatusCode::OK);
  assert_eq!(detail(&server, id).await["caches"], json!([]));
}

#[tokio::test]
async fn cache_mappings_require_cache_edit() {
  let (server, _) = TestServer::start_with_admin().await;
  let cache = server.create_cache("a", &private()).await;
  let editor = user_with_perms(&server, &["user:edit", "user:view"]).await;
  let (target, _) = create_user(&server, "target").await;
  let mapping = json!([{ "uuid": cache, "name": "a", "access_type": "View" }]);

  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/user/management",
      json!({ "uuid": target, "name": "t", "groups": [], "caches": mapping }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);

  // unchanged mappings are fine for the editor
  let resp = server
    .put(
      "/user/management",
      json!({ "uuid": target, "name": "t", "groups": [], "caches": mapping }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/user/management",
      json!({ "uuid": target, "name": "t2", "groups": [], "caches": mapping }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn edit_clears_nix_access_cache() {
  let (server, _) = TestServer::start_with_admin().await;
  let cache = server.create_cache("secret", &private()).await;
  let user = user_with_perms(&server, &[]).await;
  let (token, _) = token_for(&server, &user, "ci", Utc::now() + Duration::days(1)).await;

  let info = || {
    server
      .raw(Method::GET, "/nix/secret/nix-cache-info")
      .bearer_auth(&token)
      .send()
  };
  // denied, and the denial is cached for 30 s
  assert_eq!(info().await.unwrap().status(), StatusCode::FORBIDDEN);

  let resp = server
    .put(
      "/user/management",
      json!({ "uuid": user.id, "name": "user", "groups": [],
              "caches": [{ "uuid": cache, "name": "secret", "access_type": "View" }] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(info().await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn caches_simple_lists_visible_caches() {
  let (server, _) = TestServer::start_with_admin().await;
  server.create_cache("pub", &CacheOptions::default()).await;
  server.create_cache("hidden", &private()).await;
  let granted = server.create_cache("granted", &private()).await;

  let names = |v: Value| {
    let mut n: Vec<String> = v
      .as_array()
      .unwrap()
      .iter()
      .map(|c| c["name"].as_str().unwrap().to_string())
      .collect();
    n.sort();
    n
  };

  // cache:view sees everything
  let body: Value = server
    .get("/user/management/caches")
    .await
    .json()
    .await
    .unwrap();
  assert_eq!(names(body), ["granted", "hidden", "pub"]);

  // without cache:view only public caches and explicit grants are listed
  let (id, email) = create_user(&server, "viewer").await;
  let group = create_group(&server, &["user:view"], &[id]).await;
  let resp = server
    .put(
      "/user/management",
      json!({ "uuid": id, "name": "viewer", "groups": [group],
              "caches": [{ "uuid": granted, "name": "granted", "access_type": "View" }] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let jwt = auth_common::login_raw(&server, &email, USER_PASSWORD).await;
  let resp = server
    .raw(Method::GET, "/user/management/caches")
    .bearer_auth(jwt)
    .send()
    .await
    .unwrap();
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(names(resp.json().await.unwrap()), ["granted", "pub"]);
}

// ---- permission gates ----

#[tokio::test]
async fn management_routes_are_permission_gated() {
  let (server, admin) = TestServer::start_with_admin().await;
  let nobody = user_with_perms(&server, &[]).await;
  let viewer = user_with_perms(&server, &["user:view"]).await;
  let email = format!("{}@example.com", unique("x"));

  // (method, path, body, required permission)
  let routes = [
    (
      Method::GET,
      "/user/management".to_string(),
      json!(null),
      "user:view",
    ),
    (
      Method::GET,
      format!("/user/management/{admin}"),
      json!(null),
      "user:view",
    ),
    (
      Method::GET,
      "/user/management/groups".to_string(),
      json!(null),
      "user:view",
    ),
    (
      Method::GET,
      "/user/management/caches".to_string(),
      json!(null),
      "user:view",
    ),
    (
      Method::POST,
      "/user/management".to_string(),
      json!({ "name": "x", "email": email, "password": null }),
      "user:edit",
    ),
    (
      Method::PUT,
      "/user/management".to_string(),
      json!({ "uuid": admin, "name": "x", "groups": [], "caches": [] }),
      "user:edit",
    ),
    (
      Method::DELETE,
      "/user/management".to_string(),
      json!({ "uuid": admin }),
      "user:edit",
    ),
  ];
  for (method, path, body, perm) in routes {
    let resp = server
      .raw(method.clone(), &path)
      .json(&body)
      .send()
      .await
      .unwrap();
    assert!(
      resp.status().is_client_error() && resp.status() != StatusCode::FORBIDDEN,
      "{method} {path} no auth"
    );

    let resp = nobody
      .send(&server, method.clone(), &path, body.clone())
      .await;
    assert_eq!(
      resp.status(),
      StatusCode::FORBIDDEN,
      "{method} {path} no perms"
    );

    let resp = viewer.send(&server, method.clone(), &path, body).await;
    if perm == "user:view" {
      assert_eq!(resp.status(), StatusCode::OK, "{method} {path} viewer");
    } else {
      assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "{method} {path} viewer"
      );
    }
  }

  // no permission needed
  assert_eq!(
    nobody.get(&server, "/user/management/mail").await.status(),
    StatusCode::OK
  );
  assert_eq!(
    nobody.get(&server, "/user/info").await.status(),
    StatusCode::OK
  );
  assert!(
    server
      .raw(Method::GET, "/user/info")
      .send()
      .await
      .unwrap()
      .status()
      .is_client_error()
  );
}

#[tokio::test]
async fn reordered_cache_mappings_are_not_a_change() {
  let (server, _) = TestServer::start_with_admin().await;
  let a = server.create_cache("a", &private()).await;
  let b = server.create_cache("b", &private()).await;
  let editor = user_with_perms(&server, &["user:edit", "user:view"]).await;
  let (target, _) = create_user(&server, "target").await;
  let ma = json!({ "uuid": a, "name": "a", "access_type": "View" });
  let mb = json!({ "uuid": b, "name": "b", "access_type": "Edit" });

  let resp = server
    .put(
      "/user/management",
      json!({ "uuid": target, "name": "t", "groups": [], "caches": [ma, mb] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/user/management",
      json!({ "uuid": target, "name": "t2", "groups": [], "caches": [mb, ma] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  // dropping one is still a change
  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/user/management",
      json!({ "uuid": target, "name": "t3", "groups": [], "caches": [mb] }),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
