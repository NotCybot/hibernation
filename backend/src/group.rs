use aide::axum::ApiRouter;
use aide::axum::routing::{get_with, put_with};
use axum::{Json, extract::Path};
use centaurus::backend::auth::jwt_auth::JwtAuth;
use centaurus::backend::auth::permission::{GroupEdit, GroupView, Permission};
use centaurus::backend::endpoints::group::{
  create_group_route, delete_group_route, list_groups_route, list_users_simple_route,
};
use centaurus::db::tables::ConnectionExt;
use centaurus::{bail, db::init::Connection, error::Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::{
  DBTrait,
  cache::SimpleCacheInfo,
  group_ext::{CacheMapping, GroupDetails, same_mappings},
};
use crate::nix::NixCache;
use crate::utils::{CacheEdit, UpdateMessage, Updater};

pub fn router() -> ApiRouter {
  ApiRouter::new()
    .api_route("/", list_groups_route())
    .api_route("/", create_group_route::<UpdateMessage>())
    // deleting a group can revoke cache access and permissions
    .api_route(
      "/",
      delete_group_route::<UpdateMessage>()
        .layer(axum::middleware::from_fn(crate::user::clear_auth_caches)),
    )
    .api_route("/", put_with(edit_group, |op| op.id("editGroup")))
    .api_route("/{uuid}", get_with(group_info, |op| op.id("groupInfo")))
    .api_route("/users", list_users_simple_route())
    .api_route(
      "/caches",
      get_with(list_caches_simple, |op| op.id("listCachesSimpleGroup")),
    )
}

#[derive(Deserialize, JsonSchema)]
struct GroupViewPath {
  uuid: Uuid,
}

#[derive(Serialize, JsonSchema)]
struct GroupDetailsResponse {
  group: GroupDetails,
  admin_group: Uuid,
}

async fn group_info(
  _auth: JwtAuth<GroupView>,
  db: Connection,
  Path(path): Path<GroupViewPath>,
) -> Result<Json<GroupDetailsResponse>> {
  let info = db.group_ext().group_info(path.uuid).await?;
  let Some(info) = info else {
    bail!(NOT_FOUND, "Group not found");
  };

  let Some(admin_group) = db.setup().get_admin_group_id().await? else {
    bail!(INTERNAL_SERVER_ERROR, "Admin group not configured");
  };

  Ok(Json(GroupDetailsResponse {
    group: info,
    admin_group,
  }))
}

#[derive(Deserialize, JsonSchema)]
struct EditGroupRequest {
  uuid: Uuid,
  name: String,
  permissions: Vec<String>,
  users: Vec<Uuid>,
  caches: Vec<CacheMapping>,
}

async fn edit_group(
  auth: JwtAuth<GroupEdit>,
  db: Connection,
  updater: Updater,
  nix_cache: NixCache,
  Json(data): Json<EditGroupRequest>,
) -> Result<()> {
  if data.name.trim().is_empty() {
    bail!(BAD_REQUEST, "Group name cannot be empty");
  }

  if let Some(existing_group) = db.group().find_group_by_name(&data.name).await?
    && existing_group != data.uuid
  {
    bail!(CONFLICT, "A group with this name already exists");
  }

  let Some(group) = db.group_ext().group_info(data.uuid).await? else {
    bail!(NOT_FOUND, "Group not found");
  };

  let user_permissions = db.group().get_user_permissions(auth.user_id).await?;
  if group
    .permissions
    .iter()
    .any(|perm| !user_permissions.contains(perm))
  {
    bail!(
      FORBIDDEN,
      "Cannot edit a group with permissions you do not have"
    );
  }
  if data
    .permissions
    .iter()
    .any(|perm| !user_permissions.contains(perm))
  {
    bail!(
      FORBIDDEN,
      "Cannot assign permissions you do not have to a group"
    );
  }

  // only allow changing cache mappings if the user has the general cache edit permission
  if !same_mappings(&group.caches, &data.caches)
    && !user_permissions.contains(&CacheEdit::name().to_string())
  {
    bail!(
      FORBIDDEN,
      "Cannot edit cache mappings without the cache edit permission"
    );
  }

  if let Some(admin_group) = db.setup().get_admin_group_id().await?
    && admin_group == data.uuid
  {
    if group
      .permissions
      .iter()
      .any(|p| !data.permissions.contains(p))
    {
      bail!(BAD_REQUEST, "Cannot change permissions of the admin group");
    } else if data.users.is_empty() {
      bail!(NOT_ACCEPTABLE, "Admin group must have at least one user");
    }
  }

  let old_users = db.group().get_group_users_ids(data.uuid).await?;

  db.group()
    .edit_group(
      data.uuid,
      data.name,
      data.permissions.clone(),
      data.users.clone(),
    )
    .await?;
  db.group_ext()
    .update_cache_mappings(data.uuid, group.caches, data.caches)
    .await?;
  nix_cache.clear();

  updater
    .broadcast(UpdateMessage::Group { uuid: data.uuid })
    .await;

  let permissions_changed = group.permissions.len() != data.permissions.len()
    || group
      .permissions
      .iter()
      .any(|perm| !data.permissions.contains(perm));

  let mut users_to_notify = old_users.clone();
  users_to_notify.extend(data.users.clone());
  users_to_notify.sort_unstable();
  users_to_notify.dedup();

  // Only notify users that where added or removed
  if !permissions_changed {
    users_to_notify.retain(|user_id| {
      let in_old = old_users.contains(user_id);
      let in_new = data.users.contains(user_id);
      in_old != in_new
    });
  }

  for user_id in users_to_notify {
    updater
      .send_to(user_id, UpdateMessage::UserPermissions)
      .await;
  }

  Ok(())
}

async fn list_caches_simple(
  auth: JwtAuth<GroupView>,
  db: Connection,
) -> Result<Json<Vec<SimpleCacheInfo>>> {
  let users = db.cache().list_caches_simple(auth.user_id).await?;
  Ok(Json(users))
}
