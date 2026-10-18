//! Access control of the nix endpoints: every credential form nix/curl can send, user/group/permission based
//! access, and invalidation of the cached lookups.
mod common;
mod nix_common;

use base64::{Engine, prelude::BASE64_STANDARD};
use common::{CacheOptions, JWT_COOKIE_NAME, TestNar, TestServer};
use nix_common::{
  User, create_group, create_user, edit_group, edit_user, get_status, mapping, nix,
};
use reqwest::{Method, RequestBuilder, StatusCode};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, sea_query::Expr};
use uuid::Uuid;

struct Ctx {
  server: TestServer,
  cache: Uuid,
  nar: TestNar,
  /// Admin CLI token
  token: String,
  /// Admin JWT
  jwt: String,
}

async fn private_cache() -> Ctx {
  let (server, _) = TestServer::start_with_admin().await;
  let cache = server
    .create_cache(
      "private",
      &CacheOptions {
        public: false,
        ..Default::default()
      },
    )
    .await;
  let (token, _) = server.create_cli_token("ci").await;
  let nar = TestNar::new("secret-1.0", b"secret");
  server.push_path(&token, "private", &nar).await;
  let jwt = server.cookie(JWT_COOKIE_NAME).unwrap();
  Ctx {
    server,
    cache,
    nar,
    token,
    jwt,
  }
}

impl Ctx {
  /// The three nix endpoints of the private cache
  fn paths(&self) -> [String; 3] {
    [
      "nix-cache-info".to_string(),
      format!("{}.narinfo", self.nar.hash),
      self.nar.nar_url(),
    ]
  }

  /// Status of every endpoint (GET) plus narinfo HEAD, with `auth` applied
  async fn statuses(&self, auth: impl Fn(RequestBuilder) -> RequestBuilder) -> Vec<StatusCode> {
    let mut out = Vec::new();
    for path in self.paths() {
      out.push(get_status(auth(nix(&self.server, Method::GET, "private", &path))).await);
    }
    let head = nix(&self.server, Method::HEAD, "private", &self.paths()[1]);
    out.push(get_status(auth(head)).await);
    out
  }

  async fn assert_all(
    &self,
    auth: impl Fn(RequestBuilder) -> RequestBuilder,
    status: StatusCode,
    what: &str,
  ) {
    assert_eq!(self.statuses(auth).await, vec![status; 4], "{what}");
  }

  /// Statuses when authenticating as `user` with its JWT cookie and with its CLI token
  async fn assert_user(&self, user: &User, status: StatusCode, what: &str) {
    let jwt = user.jwt.clone();
    self
      .assert_all(
        move |r| r.header("Cookie", format!("{JWT_COOKIE_NAME}={jwt}")),
        status,
        &format!("{what} (jwt)"),
      )
      .await;
    let token = user.token.clone();
    self
      .assert_all(
        move |r| r.bearer_auth(&token),
        status,
        &format!("{what} (cli token)"),
      )
      .await;
  }
}

fn basic(user: &str, password: &str) -> String {
  format!(
    "Basic {}",
    BASE64_STANDARD.encode(format!("{user}:{password}"))
  )
}

// ---- credential forms ----

#[tokio::test]
async fn private_cache_requires_auth() {
  let c = private_cache().await;
  c.assert_all(|r| r, StatusCode::UNAUTHORIZED, "anonymous")
    .await;
}

#[tokio::test]
async fn bearer_cli_token_and_jwt() {
  let c = private_cache().await;
  let token = c.token.clone();
  c.assert_all(move |r| r.bearer_auth(&token), StatusCode::OK, "cli token")
    .await;
  let jwt = c.jwt.clone();
  c.assert_all(move |r| r.bearer_auth(&jwt), StatusCode::OK, "jwt")
    .await;
}

#[tokio::test]
async fn jwt_cookie() {
  let c = private_cache().await;
  let jwt = c.jwt.clone();
  c.assert_all(
    move |r| r.header("Cookie", format!("{JWT_COOKIE_NAME}={jwt}")),
    StatusCode::OK,
    "cookie",
  )
  .await;
}

#[tokio::test]
async fn token_query_parameter() {
  let c = private_cache().await;
  for (query, status) in [
    (format!("token={}", c.token), StatusCode::OK),
    (format!("token={}", c.jwt), StatusCode::OK),
    (format!("other=1&token={}", c.token), StatusCode::OK),
    ("other=x".to_string(), StatusCode::UNAUTHORIZED),
    ("token=".to_string(), StatusCode::UNAUTHORIZED),
    ("token=garbage".to_string(), StatusCode::UNAUTHORIZED),
  ] {
    for path in c.paths() {
      let url = c.server.nix_url("private", &format!("{path}?{query}"));
      assert_eq!(
        get_status(c.server.client.get(url)).await,
        status,
        "{path}?{query}"
      );
    }
  }
}

#[tokio::test]
async fn basic_auth_password_with_any_username() {
  // What nix sends from a netrc file
  let c = private_cache().await;
  for user in ["", "x", "admin", "some user"] {
    let value = basic(user, &c.token);
    c.assert_all(
      move |r| r.header("Authorization", value.clone()),
      StatusCode::OK,
      &format!("basic {user:?}"),
    )
    .await;
  }
  let value = basic("nix", &c.jwt);
  c.assert_all(
    move |r| r.header("Authorization", value.clone()),
    StatusCode::OK,
    "basic jwt",
  )
  .await;
  let value = basic(&c.token, "wrong");
  c.assert_all(
    move |r| r.header("Authorization", value.clone()),
    StatusCode::UNAUTHORIZED,
    "token as username",
  )
  .await;
}

#[tokio::test]
async fn invalid_credentials_are_anonymous() {
  let c = private_cache().await;
  let wrong_token = "a".repeat(32);
  let mut flipped = c.token.clone();
  let last = if flipped.ends_with('a') { "b" } else { "a" };
  flipped.replace_range(31.., last);
  let tampered_jwt = format!("{}x", c.jwt);
  for (what, value) in [
    ("wrong 32 char token", format!("Bearer {wrong_token}")),
    ("one char changed", format!("Bearer {flipped}")),
    ("31 chars", format!("Bearer {}", &c.token[..31])),
    ("33 chars", format!("Bearer {}x", c.token)),
    ("tampered jwt", format!("Bearer {tampered_jwt}")),
    ("garbage", "Bearer garbage".to_string()),
    ("basic wrong", basic("x", &wrong_token)),
    ("unknown scheme", format!("Token {}", c.token)),
  ] {
    c.assert_all(
      move |r| r.header("Authorization", value.clone()),
      StatusCode::UNAUTHORIZED,
      what,
    )
    .await;
  }
  c.assert_all(
    |r| r.header("Cookie", format!("{JWT_COOKIE_NAME}=garbage")),
    StatusCode::UNAUTHORIZED,
    "garbage cookie",
  )
  .await;
}

#[tokio::test]
async fn expired_cli_token_is_anonymous() {
  let c = private_cache().await;
  let (expired, id) = c.server.create_cli_token("old").await;
  let db = c.server.db().await;
  entity::token::Entity::update_many()
    .col_expr(
      entity::token::Column::Exp,
      Expr::value((chrono::Utc::now() - chrono::Duration::hours(1)).naive_utc()),
    )
    .filter(entity::token::Column::Id.eq(id))
    .exec(&*db)
    .await
    .unwrap();

  c.assert_all(
    move |r| r.bearer_auth(&expired),
    StatusCode::UNAUTHORIZED,
    "expired",
  )
  .await;
}

#[tokio::test]
async fn invalid_or_expired_credentials_on_public_cache_still_work() {
  let (server, _) = TestServer::start_with_admin().await;
  server
    .create_cache("public", &CacheOptions::default())
    .await;
  let (token, id) = server.create_cli_token("ci").await;
  let nar = TestNar::new("open-1.0", b"open");
  server.push_path(&token, "public", &nar).await;
  let db = server.db().await;
  entity::token::Entity::update_many()
    .col_expr(
      entity::token::Column::Exp,
      Expr::value((chrono::Utc::now() - chrono::Duration::hours(1)).naive_utc()),
    )
    .filter(entity::token::Column::Id.eq(id))
    .exec(&*db)
    .await
    .unwrap();

  for value in [
    "Bearer garbage".to_string(),
    format!("Bearer {}", "a".repeat(32)),
    format!("Bearer {token}"),
    basic("x", "y"),
  ] {
    for path in [
      "nix-cache-info".to_string(),
      format!("{}.narinfo", nar.hash),
      nar.nar_url(),
    ] {
      assert_eq!(
        get_status(nix(&server, Method::GET, "public", &path).header("Authorization", &value))
          .await,
        StatusCode::OK,
        "{value} {path}"
      );
    }
  }
}

#[tokio::test]
async fn unknown_cache_is_not_found_before_auth() {
  let c = private_cache().await;
  assert_eq!(
    get_status(nix(&c.server, Method::GET, "nope", "nix-cache-info")).await,
    StatusCode::NOT_FOUND
  );
  // Malformed paths are rejected before the cache is even looked up
  assert_eq!(
    get_status(nix(&c.server, Method::GET, "private", "abc.nar")).await,
    StatusCode::NOT_FOUND
  );
  assert_eq!(
    get_status(nix(&c.server, Method::GET, "private", "nar/abc")).await,
    StatusCode::NOT_FOUND
  );
  // But a well formed missing path needs auth first
  assert_eq!(
    get_status(nix(&c.server, Method::GET, "private", "abc.narinfo")).await,
    StatusCode::UNAUTHORIZED
  );
  let token = c.token.clone();
  assert_eq!(
    get_status(nix(&c.server, Method::GET, "private", "abc.narinfo").bearer_auth(&token)).await,
    StatusCode::NOT_FOUND
  );
}

// ---- who has access ----

#[tokio::test]
async fn user_without_access_is_forbidden() {
  let c = private_cache().await;
  let user = create_user(&c.server).await;
  c.assert_user(&user, StatusCode::FORBIDDEN, "no access")
    .await;
}

#[tokio::test]
async fn user_level_view_and_edit_access() {
  let c = private_cache().await;
  let viewer = create_user(&c.server).await;
  let editor = create_user(&c.server).await;
  edit_user(
    &c.server,
    &viewer,
    &[],
    vec![mapping(c.cache, "private", "View")],
  )
  .await;
  edit_user(
    &c.server,
    &editor,
    &[],
    vec![mapping(c.cache, "private", "Edit")],
  )
  .await;

  c.assert_user(&viewer, StatusCode::OK, "view").await;
  c.assert_user(&editor, StatusCode::OK, "edit").await;
}

#[tokio::test]
async fn access_to_other_cache_does_not_grant_this_one() {
  let c = private_cache().await;
  let other = c
    .server
    .create_cache(
      "other",
      &CacheOptions {
        public: false,
        ..Default::default()
      },
    )
    .await;
  let user = create_user(&c.server).await;
  edit_user(&c.server, &user, &[], vec![mapping(other, "other", "Edit")]).await;
  c.assert_user(&user, StatusCode::FORBIDDEN, "other cache")
    .await;
}

#[tokio::test]
async fn group_level_access() {
  let c = private_cache().await;
  let user = create_user(&c.server).await;
  let group = create_group(&c.server).await;
  edit_group(
    &c.server,
    group,
    &[],
    &[user.id],
    vec![mapping(c.cache, "private", "View")],
  )
  .await;
  c.assert_user(&user, StatusCode::OK, "group view").await;
}

#[tokio::test]
async fn global_cache_permissions() {
  let c = private_cache().await;
  let viewer = create_user(&c.server).await;
  let editor = create_user(&c.server).await;
  let unrelated = create_user(&c.server).await;
  for (user, perm) in [
    (&viewer, "cache:view"),
    (&editor, "cache:edit"),
    (&unrelated, "cache:create"),
  ] {
    let group = create_group(&c.server).await;
    edit_group(&c.server, group, &[perm], &[user.id], vec![]).await;
  }

  c.assert_user(&viewer, StatusCode::OK, "cache:view").await;
  c.assert_user(&editor, StatusCode::OK, "cache:edit").await;
  c.assert_user(&unrelated, StatusCode::FORBIDDEN, "cache:create")
    .await;
}

// ---- cached lookups are invalidated by edits ----

#[tokio::test]
async fn making_cache_private_takes_effect_immediately() {
  let (server, _) = TestServer::start_with_admin().await;
  let opts = CacheOptions::default();
  let id = server.create_cache("main", &opts).await;
  let info = || get_status(nix(&server, Method::GET, "main", "nix-cache-info"));
  assert_eq!(info().await, StatusCode::OK);

  let private = CacheOptions {
    public: false,
    ..opts.clone()
  };
  let resp = server
    .post(
      &format!("/cache/management/{id}"),
      private.edit_body("main"),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(info().await, StatusCode::UNAUTHORIZED);

  let resp = server
    .post(&format!("/cache/management/{id}"), opts.edit_body("main"))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(info().await, StatusCode::OK);
}

#[tokio::test]
async fn renaming_cache_takes_effect_immediately() {
  let (server, _) = TestServer::start_with_admin().await;
  let opts = CacheOptions::default();
  let id = server.create_cache("main", &opts).await;
  assert_eq!(
    get_status(nix(&server, Method::GET, "main", "nix-cache-info")).await,
    StatusCode::OK
  );

  let resp = server
    .post(
      &format!("/cache/management/{id}"),
      opts.edit_body("renamed"),
    )
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  assert_eq!(
    get_status(nix(&server, Method::GET, "main", "nix-cache-info")).await,
    StatusCode::NOT_FOUND
  );
  assert_eq!(
    get_status(nix(&server, Method::GET, "renamed", "nix-cache-info")).await,
    StatusCode::OK
  );
}

#[tokio::test]
async fn removing_group_access_takes_effect_immediately() {
  let c = private_cache().await;
  let user = create_user(&c.server).await;
  let group = create_group(&c.server).await;
  edit_group(
    &c.server,
    group,
    &[],
    &[user.id],
    vec![mapping(c.cache, "private", "View")],
  )
  .await;
  c.assert_user(&user, StatusCode::OK, "granted").await;

  edit_group(&c.server, group, &[], &[user.id], vec![]).await;
  c.assert_user(&user, StatusCode::FORBIDDEN, "mapping removed")
    .await;

  edit_group(
    &c.server,
    group,
    &[],
    &[user.id],
    vec![mapping(c.cache, "private", "View")],
  )
  .await;
  c.assert_user(&user, StatusCode::OK, "granted again").await;
  edit_group(
    &c.server,
    group,
    &[],
    &[],
    vec![mapping(c.cache, "private", "View")],
  )
  .await;
  c.assert_user(&user, StatusCode::FORBIDDEN, "removed from group")
    .await;
}

#[tokio::test]
async fn removing_user_access_takes_effect_immediately() {
  let c = private_cache().await;
  let user = create_user(&c.server).await;
  edit_user(
    &c.server,
    &user,
    &[],
    vec![mapping(c.cache, "private", "View")],
  )
  .await;
  c.assert_user(&user, StatusCode::OK, "granted").await;

  edit_user(&c.server, &user, &[], vec![]).await;
  c.assert_user(&user, StatusCode::FORBIDDEN, "revoked").await;
}

#[tokio::test]
async fn access_lookups_are_cached_between_edits() {
  // Changes that bypass the API (or centaurus routes that don't clear NixCache) are only seen after the 30s TTL
  let c = private_cache().await;
  let user = create_user(&c.server).await;
  edit_user(
    &c.server,
    &user,
    &[],
    vec![mapping(c.cache, "private", "View")],
  )
  .await;
  c.assert_user(&user, StatusCode::OK, "granted").await;

  let db = c.server.db().await;
  db.execute_unprepared("DELETE FROM cache_access")
    .await
    .unwrap();
  db.execute_unprepared("UPDATE cache SET priority = 1")
    .await
    .unwrap();
  c.assert_user(&user, StatusCode::OK, "still cached").await;
  let token = c.token.clone();
  let body = nix(&c.server, Method::GET, "private", "nix-cache-info")
    .bearer_auth(token)
    .send()
    .await
    .unwrap()
    .text()
    .await
    .unwrap();
  assert!(body.ends_with("Priority: 40\n"), "cache row cached too");
}

#[tokio::test]
async fn deleting_group_revokes_access_immediately() {
  let c = private_cache().await;
  let user = create_user(&c.server).await;
  let group = create_group(&c.server).await;
  edit_group(
    &c.server,
    group,
    &[],
    &[user.id],
    vec![mapping(c.cache, "private", "View")],
  )
  .await;
  c.assert_user(&user, StatusCode::OK, "granted").await;

  let resp = c
    .server
    .delete("/group", serde_json::json!({ "uuid": group }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  c.assert_user(&user, StatusCode::FORBIDDEN, "group deleted")
    .await;
}

#[tokio::test]
async fn deleting_group_with_global_permission_revokes_access_immediately() {
  let c = private_cache().await;
  let user = create_user(&c.server).await;
  let group = create_group(&c.server).await;
  edit_group(&c.server, group, &["cache:view"], &[user.id], vec![]).await;
  c.assert_user(&user, StatusCode::OK, "granted").await;

  let resp = c
    .server
    .delete("/group", serde_json::json!({ "uuid": group }))
    .await;
  assert_eq!(resp.status(), StatusCode::OK);
  c.assert_user(&user, StatusCode::FORBIDDEN, "group deleted")
    .await;
}
