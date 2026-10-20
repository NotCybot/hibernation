use aide::axum::routing::{delete_with, get_with, post_with};
use std::ops::Deref;

use aide::{OperationIo, axum::ApiRouter};
use axum::{
  Extension, Json,
  extract::{FromRequestParts, Path},
};
use centaurus::{backend::auth::jwt_auth::JwtAuth, bail, db::init::Connection, error::Result};
use chrono::{DateTime, Utc};
use entity::sea_orm_active_enums::{AccessType, EvictionPolicy};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use shared::sig::PublicKey;
use url::Url;
use uuid::Uuid;

use crate::{
  cache::state::CacheEvictionState,
  db::{
    DBTrait,
    cache::{CacheDetails, CacheInfo},
    nar::{self, SearchOrder, SearchSort},
  },
  nix::NixCache,
  utils::CacheCreate,
  utils::{UpdateMessage, Updater},
};

pub fn router() -> ApiRouter {
  ApiRouter::new()
    .api_route("/", get_with(list_caches, |op| op.id("listCaches")))
    .api_route("/", post_with(create_cache, |op| op.id("createCache")))
    .api_route("/", delete_with(delete_cache, |op| op.id("deleteCache")))
    .api_route(
      "/overview",
      get_with(cache_overview, |op| op.id("cacheOverview")),
    )
    .api_route(
      "/{uuid}",
      get_with(cache_details, |op| op.id("cacheDetails")),
    )
    .api_route(
      "/{uuid}/search",
      post_with(search_store_paths, |op| op.id("searchStorePaths")),
    )
    .api_route("/{uuid}", post_with(edit_cache, |op| op.id("editCache")))
    .api_route(
      "/{uuid}",
      delete_with(clear_cache, |op| op.id("clearCache")),
    )
    .api_route(
      "/{uuid}/path",
      delete_with(delete_path, |op| op.id("deletePath")),
    )
}

#[derive(FromRequestParts, Clone, OperationIo)]
#[from_request(via(Extension))]
pub struct CacheRegex(Regex);

impl Deref for CacheRegex {
  type Target = Regex;

  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl Default for CacheRegex {
  fn default() -> Self {
    Self::new()
  }
}

impl CacheRegex {
  pub fn new() -> Self {
    // Cache names must be 1-63 characters, can contain letters, numbers, and hyphens,
    // but cannot start or end with a hyphen or be purely numeric.
    let regex = Regex::new(r"^[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?$").unwrap();
    CacheRegex(regex)
  }

  pub fn is_valid(&self, name: &str) -> bool {
    self.is_match(name) && name.len() <= 63 && !name.bytes().all(|b| b.is_ascii_digit())
  }
}

async fn list_caches(auth: JwtAuth, db: Connection) -> Result<Json<Vec<CacheInfo>>> {
  Ok(Json(db.cache().list_caches(auth.user_id).await?))
}

#[derive(Serialize, JsonSchema)]
struct CacheOverview {
  most_accessed: Vec<OverviewPath>,
  recent_pushes: Vec<OverviewPath>,
}

#[derive(Serialize, JsonSchema)]
struct OverviewPath {
  cache_uuid: Uuid,
  cache_name: String,
  store_path: String,
  created_at: DateTime<Utc>,
  last_accessed_at: DateTime<Utc>,
  size: i64,
  accessed: i64,
}

impl From<nar::OverviewPath> for OverviewPath {
  fn from(entry: nar::OverviewPath) -> Self {
    Self {
      cache_uuid: entry.cache_id,
      cache_name: entry.cache_name,
      store_path: entry.store_path,
      created_at: DateTime::from_naive_utc_and_offset(entry.created_at, Utc),
      last_accessed_at: DateTime::from_naive_utc_and_offset(entry.last_accessed_at, Utc),
      size: entry.size,
      accessed: entry.accessed,
    }
  }
}

/// Paths are only listed for caches with edit access, like the search
async fn cache_overview(auth: JwtAuth, db: Connection) -> Result<Json<CacheOverview>> {
  let caches = db.cache().edit_cache_ids(auth.user_id).await?;
  let most_accessed = db.nar().most_accessed_paths(caches.clone()).await?;
  let recent_pushes = db.nar().recent_paths(caches).await?;

  Ok(Json(CacheOverview {
    most_accessed: most_accessed.into_iter().map(Into::into).collect(),
    recent_pushes: recent_pushes.into_iter().map(Into::into).collect(),
  }))
}

#[derive(Deserialize, JsonSchema)]
struct CachePath {
  uuid: Uuid,
}

async fn cache_details(
  auth: JwtAuth,
  Path(path): Path<CachePath>,
  db: Connection,
) -> Result<Json<CacheDetails>> {
  let Some(details) = db.cache().cache_details(path.uuid, auth.user_id).await? else {
    bail!(NOT_FOUND, "Cache not found");
  };

  Ok(Json(details))
}

#[derive(Deserialize, JsonSchema)]
struct CreateCacheRequest {
  name: String,
  public: bool,
  quota: i64,
  sig_key: String,
}

#[derive(Serialize, JsonSchema)]
struct CreateCacheResponse {
  uuid: Uuid,
}

async fn create_cache(
  auth: JwtAuth<CacheCreate>,
  db: Connection,
  updater: Updater,
  regex: CacheRegex,
  Json(req): Json<CreateCacheRequest>,
) -> Result<Json<CreateCacheResponse>> {
  if !regex.is_valid(&req.name) {
    bail!(BAD_REQUEST, "Invalid cache name format");
  }

  if db.cache().by_name(req.name.clone()).await?.is_some() {
    bail!(CONFLICT, "Cache with this name already exists");
  }

  if PublicKey::from_string(&req.sig_key).is_none() {
    bail!(NOT_ACCEPTABLE, "Invalid signature key format");
  }

  let quota = req.quota.max(0).saturating_mul(1024 * 1024); // Convert from MiB to bytes, ensuring non-negative
  let uuid = db
    .cache()
    .create_cache(req.name, req.public, quota, req.sig_key, auth.user_id)
    .await?;
  updater.broadcast(UpdateMessage::Cache { uuid }).await;

  Ok(Json(CreateCacheResponse { uuid }))
}

#[derive(Deserialize, JsonSchema)]
struct DeleteCacheRequest {
  uuid: Uuid,
}

async fn delete_cache(
  auth: JwtAuth,
  db: Connection,
  updater: Updater,
  nix_cache: NixCache,
  Json(req): Json<DeleteCacheRequest>,
) -> Result<()> {
  if db.cache().cache_user_access(auth.user_id, req.uuid).await? != Some(AccessType::Edit) {
    bail!(FORBIDDEN, "Insufficient permissions");
  }

  db.cache().delete_cache(req.uuid).await?;
  nix_cache.clear();
  updater
    .broadcast(UpdateMessage::Cache { uuid: req.uuid })
    .await;
  Ok(())
}

#[derive(Deserialize, JsonSchema)]
struct SearchStorePathsRequest {
  query: String,
  sort: SearchSort,
  order: SearchOrder,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SearchResult {
  store_path: String,
  created_at: DateTime<Utc>,
  last_accessed_at: DateTime<Utc>,
  size: i64,
  accessed: i64,
}

async fn search_store_paths(
  auth: JwtAuth,
  Path(path): Path<CachePath>,
  db: Connection,
  Json(req): Json<SearchStorePathsRequest>,
) -> Result<Json<Vec<SearchResult>>> {
  if db
    .cache()
    .cache_user_access(auth.user_id, path.uuid)
    .await?
    != Some(AccessType::Edit)
  {
    bail!(FORBIDDEN, "Insufficient permissions");
  }

  let paths = db
    .nar()
    .search_store_paths(path.uuid, req.query, req.order, req.sort)
    .await?
    .into_iter()
    .map(|entry| SearchResult {
      store_path: entry.store_path,
      created_at: DateTime::from_naive_utc_and_offset(entry.created_at, Utc),
      last_accessed_at: DateTime::from_naive_utc_and_offset(entry.last_accessed_at, Utc),
      size: entry.size,
      accessed: entry.accessed,
    })
    .collect();

  Ok(Json(paths))
}

#[derive(Deserialize, JsonSchema)]
struct EditCacheRequest {
  name: String,
  priority: i32,
  public: bool,
  quota: i64,
  sig_key: String,
  allow_force_push: bool,
  eviction_policy: EvictionPolicy,
  downstream_caches: Vec<Url>,
}

#[allow(clippy::too_many_arguments)]
async fn edit_cache(
  auth: JwtAuth,
  Path(path): Path<CachePath>,
  db: Connection,
  updater: Updater,
  lock: CacheEvictionState,
  regex: CacheRegex,
  nix_cache: NixCache,
  Json(mut req): Json<EditCacheRequest>,
) -> Result<()> {
  if req.priority < 0 {
    bail!(BAD_REQUEST, "Priority must be non-negative");
  }

  if !regex.is_valid(&req.name) {
    bail!(BAD_REQUEST, "Invalid cache name format");
  }

  if PublicKey::from_string(&req.sig_key).is_none() {
    bail!(NOT_ACCEPTABLE, "Invalid signature key format");
  }

  if req.quota < 0 {
    bail!(BAD_REQUEST, "Quota must be non-negative");
  }

  if db
    .cache()
    .cache_user_access(auth.user_id, path.uuid)
    .await?
    != Some(AccessType::Edit)
  {
    bail!(FORBIDDEN, "Insufficient permissions");
  }

  if db
    .cache()
    .by_name(req.name.clone())
    .await?
    .map_or_else(|| false, |c| c.id != path.uuid)
  {
    bail!(CONFLICT, "Cache with this name already exists");
  }

  let lock = lock.lock_cache(path.uuid).await;
  if db.cache().by_id(path.uuid).await?.is_none() {
    bail!("Cache not found");
  }

  let Some(size) = db.cache().cache_size(path.uuid).await? else {
    bail!("Cache not found");
  };

  let diff = size - req.quota;
  if diff > 0 {
    db.cache()
      .evict(path.uuid, diff, req.eviction_policy.clone())
      .await?;
  }

  req.downstream_caches.sort_unstable();
  req.downstream_caches.dedup();

  db.cache()
    .edit_cache(
      req.name,
      req.public,
      req.quota,
      req.sig_key,
      req.priority,
      req.allow_force_push,
      req.eviction_policy,
      req.downstream_caches,
      path.uuid,
    )
    .await?;

  drop(lock); // Release the cache lock as soon as possible
  nix_cache.clear();

  updater
    .broadcast(UpdateMessage::Cache { uuid: path.uuid })
    .await;

  Ok(())
}

async fn clear_cache(auth: JwtAuth, Path(path): Path<CachePath>, db: Connection) -> Result<()> {
  if db
    .cache()
    .cache_user_access(auth.user_id, path.uuid)
    .await?
    != Some(AccessType::Edit)
  {
    bail!(FORBIDDEN, "Insufficient permissions");
  }

  db.cache().clear_cache(path.uuid).await?;
  Ok(())
}

#[derive(Deserialize, JsonSchema)]
struct DeletePathRequest {
  store_path: String,
}

async fn delete_path(
  auth: JwtAuth,
  Path(path): Path<CachePath>,
  db: Connection,
  Json(req): Json<DeletePathRequest>,
) -> Result<()> {
  if db
    .cache()
    .cache_user_access(auth.user_id, path.uuid)
    .await?
    != Some(AccessType::Edit)
  {
    bail!(FORBIDDEN, "Insufficient permissions");
  }

  db.nar().delete_path(path.uuid, &req.store_path).await?;
  Ok(())
}

#[cfg(test)]
mod test {
  use super::CacheRegex;

  #[test]
  fn cache_name_validation() {
    let regex = CacheRegex::default();
    for name in ["a", "main", "My-Cache", "a1", "1a", "0-0", &"a".repeat(63)] {
      assert!(regex.is_valid(name), "{name} should be valid");
    }
    for name in [
      "",
      "-a",
      "a-",
      "-",
      "a_b",
      "a.b",
      "a b",
      "ä",
      "1",
      "123",
      &"a".repeat(64),
    ] {
      assert!(!regex.is_valid(name), "{name} should be invalid");
    }
  }
}
