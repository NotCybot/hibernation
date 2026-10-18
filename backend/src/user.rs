use aide::axum::{
  ApiRouter,
  routing::{get_with, put_with},
};
use axum::{
  Json,
  extract::{Path, Request},
  middleware::{self, Next},
  response::Response,
};
use centaurus::{
  backend::{
    auth::{
      jwt_auth::JwtAuth,
      permission::{Permission, UserEdit, UserView},
    },
    endpoints::user::{
      account,
      email::change_email_route,
      info,
      management::{
        convert_oidc_user_route, create_user_route, delete_user_route, list_groups_simple_route,
        list_users_route, mail_active_route, reset_user_avatar_route, reset_user_password_route,
      },
    },
    middleware::rate_limiter::RateLimiter,
  },
  bail,
  db::{init::Connection, tables::ConnectionExt},
  error::Result,
};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
  auth::cli_auth::CliTokenCache,
  db::{
    DBTrait,
    cache::SimpleCacheInfo,
    group_ext::{CacheMapping, same_mappings},
    user_ext::DetailUserInfo,
  },
  nix::NixCache,
  utils::{CacheEdit, UpdateMessage, Updater},
};

pub fn router(rate_limiter: &mut RateLimiter) -> ApiRouter {
  ApiRouter::new()
    .nest("/management", management())
    .nest("/account", account::router::<UpdateMessage>(rate_limiter))
    .nest("/info", info::router())
}

fn management() -> ApiRouter {
  ApiRouter::new()
    .api_route("/", list_users_route())
    .api_route("/", create_user_route::<UpdateMessage>())
    .api_route(
      "/",
      delete_user_route::<UpdateMessage>().layer(middleware::from_fn(clear_auth_caches)),
    )
    .api_route("/mail", mail_active_route())
    .api_route("/groups", list_groups_simple_route())
    .api_route("/avatar", reset_user_avatar_route::<UpdateMessage>())
    .api_route("/password", reset_user_password_route())
    .api_route("/email", change_email_route::<UpdateMessage>())
    .api_route("/convert-oidc", convert_oidc_user_route::<UpdateMessage>())
    .api_route("/", put_with(edit_user, |op| op.id("editUser")))
    .api_route("/{uuid}", get_with(user_info, |op| op.id("userInfoDetail")))
    .api_route(
      "/caches",
      get_with(list_caches_simple, |op| op.id("listCachesSimple")),
    )
}

/// The centaurus delete route knows nothing about our auth caches: drop them so a deleted user's cached CLI
/// tokens and cache access stop working immediately instead of after `LOOKUP_TTL`.
pub(crate) async fn clear_auth_caches(req: Request, next: Next) -> Response {
  let tokens = req.extensions().get::<CliTokenCache>().cloned();
  let nix = req.extensions().get::<NixCache>().cloned();
  let res = next.run(req).await;
  if res.status().is_success() {
    tokens.inspect(CliTokenCache::clear);
    nix.inspect(NixCache::clear);
  }
  res
}

#[derive(Deserialize, JsonSchema)]
struct UserViewPath {
  uuid: Uuid,
}

async fn user_info(
  _auth: JwtAuth<UserView>,
  db: Connection,
  Path(path): Path<UserViewPath>,
) -> Result<Json<DetailUserInfo>> {
  let info = db.user_ext().user_info(path.uuid).await?;
  let Some(info) = info else {
    bail!(NOT_FOUND, "User not found");
  };
  Ok(Json(info))
}

async fn list_caches_simple(
  auth: JwtAuth<UserView>,
  db: Connection,
) -> Result<Json<Vec<SimpleCacheInfo>>> {
  let users = db.cache().list_caches_simple(auth.user_id).await?;
  Ok(Json(users))
}

#[derive(Deserialize, JsonSchema)]
struct UserEditReq {
  uuid: Uuid,
  name: String,
  groups: Vec<Uuid>,
  caches: Vec<CacheMapping>,
}

async fn edit_user(
  auth: JwtAuth<UserEdit>,
  db: Connection,
  updater: Updater,
  nix_cache: NixCache,
  Json(req): Json<UserEditReq>,
) -> Result<()> {
  if req.name.trim().is_empty() {
    bail!(BAD_REQUEST, "Name cannot be empty");
  }

  let self_permissions = db.group().get_user_permissions(auth.user_id).await?;
  let target_permissions = db
    .group()
    .get_groups_permissions(req.groups.clone())
    .await?;
  let current_user_permissions = db.group().get_user_permissions(req.uuid).await?;

  if target_permissions
    .iter()
    .any(|p| !self_permissions.contains(p))
    || current_user_permissions
      .iter()
      .any(|p| !self_permissions.contains(p))
  {
    bail!(
      FORBIDDEN,
      "Cannot assign permissions that the editor does not have"
    );
  }

  let Some(admin_group) = db.setup().get_admin_group_id().await? else {
    bail!(INTERNAL_SERVER_ERROR, "Admin group is not set up");
  };

  if !req.groups.contains(&admin_group) && db.group().is_last_admin(admin_group, req.uuid).await? {
    bail!(CONFLICT, "Cannot remove the last user from the admin group");
  }

  let Some(user) = db.user_ext().user_info(req.uuid).await? else {
    bail!(NOT_FOUND, "User not found");
  };

  // only allow changing cache mappings if the user has the general cache edit permission
  if !same_mappings(&user.caches, &req.caches)
    && !self_permissions.contains(&CacheEdit::name().to_string())
  {
    bail!(
      FORBIDDEN,
      "Cannot edit cache mappings without the cache edit permission"
    );
  }

  db.user().edit_user(req.uuid, req.name, req.groups).await?;
  db.user_ext()
    .update_cache_mappings(req.uuid, user.caches, req.caches)
    .await?;
  nix_cache.clear();
  updater
    .broadcast(UpdateMessage::User { uuid: req.uuid })
    .await;

  Ok(())
}
