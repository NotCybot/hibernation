//! Group management (`group.rs` + centaurus `/group`): CRUD, cache access, admin group protections, gates.
mod auth_common;
mod common;

use auth_common::{create_group, create_user, token_for, user_with_perms, uuid};
use chrono::{Duration, Utc};
use common::{CacheOptions, TestServer, unique};
use entity::{cache_access, sea_orm_active_enums::AccessType};
use reqwest::{Method, StatusCode};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde_json::{Value, json};
use uuid::Uuid;

async fn list(server: &TestServer) -> Value {
  let resp = server.get("/group").await;
  assert_eq!(resp.status(), StatusCode::OK);
  resp.json().await.unwrap()
}

async fn detail(server: &TestServer, id: Uuid) -> Value {
  let resp = server.get(&format!("/group/{id}")).await;
  assert_eq!(resp.status(), StatusCode::OK);
  resp.json().await.unwrap()
}

async fn admin_group(server: &TestServer) -> Uuid {
  uuid(&list(server).await["admin_group"])
}

/// `cache_access` rows owned by `group` (creating a cache also adds a row for the creating user).
async fn group_rows(server: &TestServer, group: Uuid) -> Vec<cache_access::Model> {
  cache_access::Entity::find()
    .filter(cache_access::Column::GroupId.eq(group))
    .all(&*server.db().await)
    .await
    .unwrap()
}

fn private() -> CacheOptions {
  CacheOptions {
    public: false,
    ..Default::default()
  }
}

fn edit_body(id: Uuid, name: &str, perms: &[&str], users: &[Uuid], caches: Value) -> Value {
  json!({ "uuid": id, "name": name, "permissions": perms, "users": users, "caches": caches })
}

// ---- CRUD ----

#[tokio::test]
async fn group_crud_flow() {
  let (server, admin) = TestServer::start_with_admin().await;
  let initial = list(&server).await["groups"].as_array().unwrap().len();
  assert_eq!(initial, 1, "only the admin group");

  let name = unique("editors");
  let resp = server.post("/group", json!({ "name": name })).await;
  assert_eq!(resp.status(), StatusCode::OK);
  let id = uuid(&resp.json::<Value>().await.unwrap()["uuid"]);
  assert_eq!(list(&server).await["groups"].as_array().unwrap().len(), 2);

  let body = detail(&server, id).await;
  assert_eq!(uuid(&body["admin_group"]), admin_group(&server).await);
  assert_eq!(body["group"]["name"], name);
  assert_eq!(body["group"]["permissions"], json!([]));
  assert_eq!(body["group"]["users"], json!([]));
  assert_eq!(body["group"]["caches"], json!([]));

  let resp = server
    .put(
      "/group",
      edit_body(id, "renamed", &["group:view"], &[admin], json!([])),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body = detail(&server, id).await;
  assert_eq!(body["group"]["name"], "renamed");
  assert_eq!(body["group"]["permissions"], json!(["group:view"]));
  assert_eq!(uuid(&body["group"]["users"][0]["id"]), admin);

  let resp = server.delete("/group", json!({ "uuid": id })).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(
    list(&server).await["groups"].as_array().unwrap().len(),
    initial
  );
  let resp = server.get(&format!("/group/{id}")).await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_validation() {
  let (server, _) = TestServer::start_with_admin().await;
  let resp = server.post("/group", json!({ "name": "  " })).await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  let resp = server.post("/group", json!({ "name": "Admin" })).await;
  assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn edit_validation() {
  let (server, _) = TestServer::start_with_admin().await;
  let id = create_group(&server, &[], &[]).await;

  let resp = server
    .put("/group", edit_body(id, " ", &[], &[], json!([])))
    .await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  let resp = server
    .put("/group", edit_body(id, "Admin", &[], &[], json!([])))
    .await;
  assert_eq!(resp.status(), StatusCode::CONFLICT);
  let resp = server
    .put(
      "/group",
      edit_body(Uuid::new_v4(), &unique("g"), &[], &[], json!([])),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn users_endpoint_lists_users() {
  let (server, admin) = TestServer::start_with_admin().await;
  let (id, _) = create_user(&server, "member").await;
  let resp = server.get("/group/users").await;
  assert_eq!(resp.status(), StatusCode::OK);
  let text = resp.text().await.unwrap();
  assert!(text.contains(&admin.to_string()) && text.contains(&id.to_string()));
}

#[tokio::test]
async fn delete_group_removes_memberships_and_cache_access() {
  let (server, _) = TestServer::start_with_admin().await;
  let cache = server.create_cache("c", &private()).await;
  let (user, _) = create_user(&server, "member").await;
  let id = create_group(&server, &["user:view"], &[user]).await;
  let resp = server
    .put(
      "/group",
      edit_body(
        id,
        "g",
        &["user:view"],
        &[user],
        json!([{ "uuid": cache, "name": "c", "access_type": "View" }]),
      ),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);

  assert_eq!(group_rows(&server, id).await.len(), 1);
  assert_eq!(
    server
      .delete("/group", json!({ "uuid": id }))
      .await
      .status(),
    StatusCode::OK
  );
  assert!(group_rows(&server, id).await.is_empty());
  let info: Value = server
    .get(&format!("/user/management/{user}"))
    .await
    .json()
    .await
    .unwrap();
  assert_eq!(info["permissions"], json!([]));
}

// ---- admin group ----

#[tokio::test]
async fn admin_group_protections() {
  let (server, admin) = TestServer::start_with_admin().await;
  let admin_group = admin_group(&server).await;
  let perms = backend::utils::permissions();

  let resp = server
    .delete("/group", json!({ "uuid": admin_group }))
    .await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

  let fewer = &perms[1..];
  let resp = server
    .put(
      "/group",
      edit_body(admin_group, "Admin", fewer, &[admin], json!([])),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

  let resp = server
    .put(
      "/group",
      edit_body(admin_group, "Admin", &perms, &[], json!([])),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::NOT_ACCEPTABLE);

  // renaming with all permissions and a user is fine
  let resp = server
    .put(
      "/group",
      edit_body(admin_group, "Root", &perms, &[admin], json!([])),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(detail(&server, admin_group).await["group"]["name"], "Root");
}

// ---- permission escalation ----

#[tokio::test]
async fn editor_cannot_escalate() {
  let (server, _) = TestServer::start_with_admin().await;
  let editor = user_with_perms(&server, &["group:edit", "group:view"]).await;
  let strong = create_group(&server, &["user:edit"], &[]).await;
  let weak = create_group(&server, &[], &[]).await;

  // editing a group that holds permissions the editor lacks
  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/group",
      edit_body(strong, "s", &[], &[], json!([])),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);
  // granting permissions the editor lacks
  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/group",
      edit_body(weak, "w", &["user:edit"], &[], json!([])),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);
  // granting own permissions is fine
  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/group",
      edit_body(weak, "w", &["group:view"], &[editor.id], json!([])),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
}

// ---- cache access ----

#[tokio::test]
async fn edit_sets_and_replaces_cache_access() {
  let (server, _) = TestServer::start_with_admin().await;
  let id = create_group(&server, &[], &[]).await;
  let a = server.create_cache("a", &private()).await;
  let b = server.create_cache("b", &private()).await;

  let edit = |caches: Value| server.put("/group", edit_body(id, "g", &[], &[], caches));

  let resp = edit(json!([
    { "uuid": a, "name": "a", "access_type": "Edit" },
    { "uuid": b, "name": "b", "access_type": "View" },
  ]))
  .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let mut caches = detail(&server, id).await["group"]["caches"]
    .as_array()
    .unwrap()
    .clone();
  caches.sort_by_key(|c| c["name"].as_str().unwrap().to_string());
  assert_eq!(
    caches,
    vec![
      json!({ "uuid": a, "name": "a", "access_type": "Edit" }),
      json!({ "uuid": b, "name": "b", "access_type": "View" }),
    ]
  );

  let resp = edit(json!([{ "uuid": b, "name": "b", "access_type": "Edit" }])).await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(
    detail(&server, id).await["group"]["caches"],
    json!([{ "uuid": b, "name": "b", "access_type": "Edit" }])
  );

  assert_eq!(edit(json!([])).await.status(), StatusCode::OK);
  assert_eq!(detail(&server, id).await["group"]["caches"], json!([]));
  assert!(group_rows(&server, id).await.is_empty());
}

#[tokio::test]
async fn cache_access_requires_cache_edit() {
  let (server, _) = TestServer::start_with_admin().await;
  let cache = server.create_cache("a", &private()).await;
  let editor = user_with_perms(&server, &["group:edit", "group:view"]).await;
  let id = create_group(&server, &[], &[]).await;
  let mapping = json!([{ "uuid": cache, "name": "a", "access_type": "View" }]);

  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/group",
      edit_body(id, "g", &[], &[], mapping.clone()),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);

  // once set by the admin, the editor may edit other fields as long as caches stay unchanged
  assert_eq!(
    server
      .put("/group", edit_body(id, "g", &[], &[], mapping.clone()))
      .await
      .status(),
    StatusCode::OK
  );
  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/group",
      edit_body(id, "g2", &[], &[], mapping),
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
  let id = create_group(&server, &[], &[user.id]).await;

  let info = || {
    server
      .raw(Method::GET, "/nix/secret/nix-cache-info")
      .bearer_auth(&token)
      .send()
  };
  assert_eq!(info().await.unwrap().status(), StatusCode::FORBIDDEN);

  let resp = server
    .put(
      "/group",
      edit_body(
        id,
        "g",
        &[],
        &[user.id],
        json!([{ "uuid": cache, "name": "secret", "access_type": "View" }]),
      ),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(info().await.unwrap().status(), StatusCode::OK);

  // removing the user from the group revokes access immediately
  let resp = server
    .put(
      "/group",
      edit_body(
        id,
        "g",
        &[],
        &[],
        json!([{ "uuid": cache, "name": "secret", "access_type": "View" }]),
      ),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(info().await.unwrap().status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn caches_simple_lists_caches() {
  let (server, _) = TestServer::start_with_admin().await;
  let cache = server.create_cache("a", &private()).await;
  let resp = server.get("/group/caches").await;
  assert_eq!(resp.status(), StatusCode::OK);
  let body: Value = resp.json().await.unwrap();
  assert_eq!(body, json!([{ "uuid": cache, "name": "a" }]));
}

#[tokio::test]
async fn cache_access_check_requires_exactly_one_owner() {
  let (server, admin) = TestServer::start_with_admin().await;
  let cache = server.create_cache("a", &private()).await;
  let group = admin_group(&server).await;
  let db = server.db().await;

  let row = |user: Option<Uuid>, group: Option<Uuid>| cache_access::ActiveModel {
    cache_id: Set(cache),
    user_id: Set(user),
    group_id: Set(group),
    access_type: Set(AccessType::View),
    ..Default::default()
  };

  assert!(row(None, None).insert(&*db).await.is_err(), "neither");
  assert!(
    row(Some(admin), Some(group)).insert(&*db).await.is_err(),
    "both"
  );
  row(Some(admin), None).insert(&*db).await.unwrap();
  row(None, Some(group)).insert(&*db).await.unwrap();
}

// ---- permission gates ----

#[tokio::test]
async fn group_routes_are_permission_gated() {
  let (server, _) = TestServer::start_with_admin().await;
  let admin_group = admin_group(&server).await;
  let target = create_group(&server, &[], &[]).await;
  let nobody = user_with_perms(&server, &[]).await;
  let viewer = user_with_perms(&server, &["group:view"]).await;

  let routes = [
    (Method::GET, "/group".to_string(), json!(null), true),
    (
      Method::GET,
      format!("/group/{admin_group}"),
      json!(null),
      true,
    ),
    (Method::GET, "/group/users".to_string(), json!(null), true),
    (Method::GET, "/group/caches".to_string(), json!(null), true),
    (
      Method::POST,
      "/group".to_string(),
      json!({ "name": "x" }),
      false,
    ),
    (
      Method::PUT,
      "/group".to_string(),
      edit_body(target, "x", &[], &[], json!([])),
      false,
    ),
    (
      Method::DELETE,
      "/group".to_string(),
      json!({ "uuid": target }),
      false,
    ),
  ];
  for (method, path, body, view_only) in routes {
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
    let expected = if view_only {
      StatusCode::OK
    } else {
      StatusCode::FORBIDDEN
    };
    assert_eq!(resp.status(), expected, "{method} {path} viewer");
  }
}

#[tokio::test]
async fn reordered_cache_access_is_not_a_change() {
  let (server, _) = TestServer::start_with_admin().await;
  let a = server.create_cache("a", &private()).await;
  let b = server.create_cache("b", &private()).await;
  let editor = user_with_perms(&server, &["group:edit", "group:view"]).await;
  let id = create_group(&server, &[], &[]).await;
  let ma = json!({ "uuid": a, "name": "a", "access_type": "View" });
  let mb = json!({ "uuid": b, "name": "b", "access_type": "Edit" });

  let resp = server
    .put("/group", edit_body(id, "g", &[], &[], json!([ma, mb])))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/group",
      edit_body(id, "g2", &[], &[], json!([mb, ma])),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  let resp = editor
    .send(
      &server,
      Method::PUT,
      "/group",
      edit_body(id, "g3", &[], &[], json!([ma])),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
