use centaurus::{
  backend::auth::permission::Permission,
  db::tables::group::GroupTable,
  error::{ErrorReportStatusExt, Result},
};
use entity::{
  cache, cache_access, downstream_cache, group_user, nar, nar_info,
  sea_orm_active_enums::{AccessType, EvictionPolicy},
};
use http::StatusCode;
use migration::{ExprTrait, Func, Query, WindowStatement};
use schemars::JsonSchema;
use sea_orm::{
  ActiveValue::Set, Condition, FromQueryResult, IntoActiveModel, Iterable, JoinType, QuerySelect,
  QueryTrait, TransactionTrait, prelude::*,
};
use serde::Serialize;
use url::Url;

use crate::utils::{CacheEdit, CacheView};

#[derive(Serialize, FromQueryResult, JsonSchema)]
pub struct CacheInfo {
  #[serde(rename = "uuid")]
  id: Uuid,
  name: String,
  #[serde(serialize_with = "serialize_i64_or_zero")]
  size: Option<i64>,
  quota: i64,
  public: bool,
}

#[derive(FromQueryResult)]
pub struct CacheDetailsQuery {
  id: Uuid,
  name: String,
  size: Option<i64>,
  quota: i64,
  public: bool,
  public_signing_key: String,
  priority: i32,
  nar_count: i64,
  allow_force_push: bool,
  eviction_policy: EvictionPolicy,
  has_write_access: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct CacheDetails {
  #[serde(rename = "uuid")]
  id: Uuid,
  name: String,
  #[serde(serialize_with = "serialize_i64_or_zero")]
  size: Option<i64>,
  quota: i64,
  public: bool,
  #[serde(rename = "sig_key")]
  public_signing_key: String,
  priority: i32,
  nar_count: i64,
  allow_force_push: bool,
  eviction_policy: EvictionPolicy,
  has_write_access: bool,
  downstream_caches: Vec<Url>,
}

#[derive(Serialize, FromQueryResult, JsonSchema)]
pub struct SimpleCacheInfo {
  #[serde(rename = "uuid")]
  id: Uuid,
  name: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveColumn)]
enum QueryAs {
  GroupId,
}

pub struct CacheTable<'db> {
  db: &'db DatabaseConnection,
}

impl<'db> CacheTable<'db> {
  pub fn new(db: &'db DatabaseConnection) -> Self {
    Self { db }
  }

  pub async fn list_caches(&self, user: Uuid) -> Result<Vec<CacheInfo>> {
    let mut query =
      cache::Entity::find().join(JoinType::LeftJoin, cache::Relation::CacheAccess.def());

    query = apply_user_filter(query, self.db, user, AccessType::View).await?;

    Ok(
      query
        .group_by(cache::Column::Id)
        .select_only()
        .columns(cache::Column::iter())
        .column_as(size_subquery(), "size")
        .into_model::<CacheInfo>()
        .all(self.db)
        .await?,
    )
  }

  pub async fn list_caches_simple(&self, user: Uuid) -> Result<Vec<SimpleCacheInfo>> {
    let mut query =
      cache::Entity::find().join(JoinType::LeftJoin, cache::Relation::CacheAccess.def());

    query = apply_user_filter(query, self.db, user, AccessType::View).await?;

    Ok(
      query
        .select_only()
        .column(cache::Column::Id)
        .column(cache::Column::Name)
        // the cache_access join yields one row per access entry
        .distinct()
        .into_model::<SimpleCacheInfo>()
        .all(self.db)
        .await?,
    )
  }

  pub async fn edit_cache_ids(&self, user: Uuid) -> Result<Vec<Uuid>> {
    let mut query =
      cache::Entity::find().join(JoinType::LeftJoin, cache::Relation::CacheAccess.def());

    query = apply_user_filter(query, self.db, user, AccessType::Edit).await?;

    Ok(
      query
        .select_only()
        .column(cache::Column::Id)
        // the cache_access join yields one row per access entry
        .distinct()
        .into_tuple::<Uuid>()
        .all(self.db)
        .await?,
    )
  }

  pub async fn cache_details(&self, uuid: Uuid, user: Uuid) -> Result<Option<CacheDetails>> {
    let access = self.cache_user_access(user, uuid).await?;

    let mut query =
      cache::Entity::find_by_id(uuid).join(JoinType::LeftJoin, cache::Relation::CacheAccess.def());

    query = apply_user_filter(query, self.db, user, AccessType::View).await?;

    let nar_count = Query::select()
      .expr(nar_info::Column::Id.count().cast_as("BIGINT"))
      .from(nar_info::Entity)
      .and_where(
        Expr::col((nar_info::Entity, nar_info::Column::CacheId))
          .equals((cache::Entity, cache::Column::Id)),
      )
      .to_owned();

    let res = query
      .group_by(cache::Column::Id)
      .select_only()
      .columns(cache::Column::iter())
      .column_as(size_subquery(), "size")
      .column_as(
        Expr::SubQuery(None, Box::new(nar_count.into())),
        "nar_count",
      )
      .column_as(
        Expr::val(access == Some(AccessType::Edit)),
        "has_write_access",
      )
      .into_model::<CacheDetailsQuery>()
      .one(self.db)
      .await?;

    let Some(res) = res else {
      return Ok(None);
    };

    let downstream_caches = self.downstream_caches(res.id).await?;

    Ok(Some(CacheDetails {
      id: res.id,
      name: res.name,
      size: res.size,
      quota: res.quota,
      public: res.public,
      public_signing_key: res.public_signing_key,
      priority: res.priority,
      nar_count: res.nar_count,
      allow_force_push: res.allow_force_push,
      eviction_policy: res.eviction_policy,
      has_write_access: res.has_write_access,
      downstream_caches: downstream_caches
        .into_iter()
        .map(|dc| dc.url.parse().unwrap())
        .collect(),
    }))
  }

  pub async fn by_name(&self, name: String) -> Result<Option<cache::Model>> {
    Ok(
      cache::Entity::find()
        .filter(Func::lower(Expr::col(cache::Column::Name)).eq(name.to_lowercase()))
        .one(self.db)
        .await?,
    )
  }

  pub async fn by_id_filtered(
    &self,
    uuid: Uuid,
    user: Uuid,
    access_type: AccessType,
  ) -> Result<Option<cache::Model>> {
    let mut query =
      cache::Entity::find_by_id(uuid).join(JoinType::LeftJoin, cache::Relation::CacheAccess.def());

    query = apply_user_filter(query, self.db, user, access_type).await?;

    Ok(query.one(self.db).await?)
  }

  pub async fn by_id(&self, uuid: Uuid) -> Result<Option<cache::Model>> {
    Ok(cache::Entity::find_by_id(uuid).one(self.db).await?)
  }

  pub async fn by_name_filtered(
    &self,
    name: String,
    user: Uuid,
    access_type: AccessType,
  ) -> Result<Option<cache::Model>> {
    // Case-insensitive like `by_name`, names only differing in case can not be created
    let mut query = cache::Entity::find()
      .filter(Func::lower(Expr::col(cache::Column::Name)).eq(name.to_lowercase()))
      .join(JoinType::LeftJoin, cache::Relation::CacheAccess.def());

    query = apply_user_filter(query, self.db, user, access_type).await?;

    Ok(query.one(self.db).await?)
  }

  pub async fn create_cache(
    &self,
    name: String,
    public: bool,
    quota: i64,
    sig_key: String,
    user: Uuid,
  ) -> centaurus::error::Result<Uuid> {
    self
      .db
      .transaction::<_, Uuid, DbErr>(|db| {
        Box::pin(async move {
          let cache = cache::ActiveModel {
            id: Set(Uuid::now_v7()),
            name: Set(name),
            public: Set(public),
            quota: Set(quota),
            public_signing_key: Set(sig_key),
            priority: Set(50),
            allow_force_push: Set(false),
            eviction_policy: Set(EvictionPolicy::LeastRecentlyUsed),
          };
          let res = cache.insert(db).await?;

          let cache_access = cache_access::ActiveModel {
            cache_id: Set(res.id),
            user_id: Set(Some(user)),
            group_id: Set(None),
            access_type: Set(AccessType::Edit),
            ..Default::default()
          };
          cache_access.insert(db).await?;

          let downstream_cache = downstream_cache::ActiveModel {
            id: Set(Uuid::now_v7()),
            cache_id: Set(res.id),
            url: Set("https://cache.nixos.org".to_string()),
          };
          downstream_cache.insert(db).await?;

          Ok(res.id)
        })
      })
      .await
      .status_context(StatusCode::INTERNAL_SERVER_ERROR, "DB Error")
  }

  pub async fn delete_cache(&self, uuid: Uuid) -> Result<()> {
    cache::Entity::delete_by_id(uuid).exec(self.db).await?;

    Ok(())
  }

  pub async fn downstream_caches(&self, uuid: Uuid) -> Result<Vec<downstream_cache::Model>> {
    Ok(
      downstream_cache::Entity::find()
        .filter(downstream_cache::Column::CacheId.eq(uuid))
        .all(self.db)
        .await?,
    )
  }

  pub async fn cache_user_access(&self, user: Uuid, cache: Uuid) -> Result<Option<AccessType>> {
    let user_permissions = GroupTable::new(self.db).get_user_permissions(user).await?;
    if user_permissions.contains(&CacheEdit::name().to_string()) {
      return Ok(Some(AccessType::Edit));
    }

    let access = cache_access::Entity::find()
      .filter(cache_access::Column::CacheId.eq(cache))
      .filter(
        cache_access::Column::UserId.eq(Some(user)).or(
          cache_access::Column::GroupId.in_subquery(
            group_user::Entity::find()
              .filter(group_user::Column::UserId.eq(user))
              .select_only()
              .column_as(group_user::Column::GroupId, QueryAs::GroupId)
              .into_query(),
          ),
        ),
      )
      .select_only()
      .column(cache_access::Column::AccessType)
      .distinct()
      .into_tuple::<AccessType>()
      .all(self.db)
      .await?;

    if access.contains(&AccessType::Edit) {
      Ok(Some(AccessType::Edit))
    } else if access.contains(&AccessType::View)
      || user_permissions.contains(&CacheView::name().to_string())
    {
      Ok(Some(AccessType::View))
    } else {
      Ok(None)
    }
  }

  #[allow(clippy::too_many_arguments)]
  pub async fn edit_cache(
    &self,
    name: String,
    public: bool,
    quota: i64,
    sig_key: String,
    priority: i32,
    allow_force_push: bool,
    eviction_policy: EvictionPolicy,
    downstream_caches: Vec<Url>,
    cache_id: Uuid,
  ) -> Result<()> {
    let mut cache = cache::Entity::find_by_id(cache_id)
      .one(self.db)
      .await?
      .ok_or(DbErr::RecordNotFound("Cache not found".to_string()))?
      .into_active_model();

    cache.name = Set(name);
    cache.public = Set(public);
    cache.quota = Set(quota);
    cache.public_signing_key = Set(sig_key);
    cache.priority = Set(priority);
    cache.allow_force_push = Set(allow_force_push);
    cache.eviction_policy = Set(eviction_policy);

    cache.update(self.db).await?;

    downstream_cache::Entity::delete_many()
      .filter(downstream_cache::Column::CacheId.eq(cache_id))
      .exec(self.db)
      .await?;

    let mut new_downstreams = Vec::new();
    for url in downstream_caches {
      new_downstreams.push(downstream_cache::ActiveModel {
        id: Set(Uuid::now_v7()),
        cache_id: Set(cache_id),
        url: Set(url.to_string()),
      });
    }

    if !new_downstreams.is_empty() {
      downstream_cache::Entity::insert_many(new_downstreams)
        .exec(self.db)
        .await?;
    }

    Ok(())
  }

  pub async fn clear_cache(&self, cache_id: Uuid) -> Result<()> {
    nar_info::Entity::delete_many()
      .filter(nar_info::Column::CacheId.eq(cache_id))
      .exec(self.db)
      .await?;

    Ok(())
  }

  pub async fn cache_size(&self, cache_id: Uuid) -> Result<Option<i64>> {
    Ok(
      nar::Entity::find()
        .join(JoinType::InnerJoin, nar::Relation::NarInfo.def())
        .filter(nar_info::Column::CacheId.eq(cache_id))
        .select_only()
        .column_as(nar::Column::Size.sum().cast_as("BIGINT"), "total_size")
        .into_tuple::<Option<i64>>()
        .one(self.db)
        .await?
        .map(|size| size.unwrap_or(0)),
    )
  }

  pub async fn evict(
    &self,
    cache: Uuid,
    to_evict: i64,
    eviction_policy: EvictionPolicy,
  ) -> Result<()> {
    let cumulative_size = "cumulative_size";
    let mut window = WindowStatement::new();

    match eviction_policy {
      EvictionPolicy::LeastRecentlyUsed => {
        window
          .order_by(
            (nar_info::Entity, nar_info::Column::LastAccessedAt),
            sea_orm::Order::Asc,
          )
          .order_by(
            (nar_info::Entity, nar_info::Column::CreatedAt),
            sea_orm::Order::Asc,
          );
      }
      EvictionPolicy::LeastFrequentlyUsed => {
        window
          .order_by(
            (nar_info::Entity, nar_info::Column::Accessed),
            sea_orm::Order::Asc,
          )
          .order_by(
            (nar_info::Entity, nar_info::Column::CreatedAt),
            sea_orm::Order::Asc,
          );
      }
      EvictionPolicy::OldestFirst => {
        window.order_by(
          (nar_info::Entity, nar_info::Column::CreatedAt),
          sea_orm::Order::Asc,
        );
      }
    }

    let inner = Query::select()
      .expr_as(Expr::col((nar_info::Entity, nar_info::Column::Id)), "id")
      .expr_as(Expr::col((nar::Entity, nar::Column::Size)), "size")
      .from(nar_info::Entity)
      .and_where(nar_info::Column::CacheId.eq(cache))
      .left_join(
        nar::Entity,
        Expr::col((nar_info::Entity, nar_info::Column::NarId))
          .equals((nar::Entity, nar::Column::Id)),
      )
      .expr_window_as(
        Func::sum(Expr::col(nar::Column::Size)),
        window,
        cumulative_size,
      )
      .to_owned();

    let outer = Query::select()
      .column(nar_info::Column::Id)
      .from_subquery(inner, "ordered_nars")
      .and_where(
        Expr::col(cumulative_size)
          .sub(Expr::col(nar::Column::Size))
          .lt(to_evict),
      )
      .to_owned();

    let delete_query = Query::delete()
      .from_table(nar_info::Entity)
      .and_where(nar_info::Column::Id.in_subquery(outer))
      .to_owned();

    let builder = self.db.get_database_backend();
    self.db.query_all_raw(builder.build(&delete_query)).await?;

    Ok(())
  }
}

/// Summed nar size of the outer query's cache. A subquery instead of joins, the cache_access join needed for the
/// permission filter would otherwise multiply the sum by the number of access rows
fn size_subquery() -> Expr {
  let query = Query::select()
    .expr(nar::Column::Size.sum().cast_as("BIGINT"))
    .from(nar_info::Entity)
    .inner_join(
      nar::Entity,
      Expr::col((nar_info::Entity, nar_info::Column::NarId)).equals((nar::Entity, nar::Column::Id)),
    )
    .and_where(
      Expr::col((nar_info::Entity, nar_info::Column::CacheId))
        .equals((cache::Entity, cache::Column::Id)),
    )
    .to_owned();
  Expr::SubQuery(None, Box::new(query.into()))
}

fn serialize_i64_or_zero<S>(
  num: &Option<i64>,
  serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
  S: serde::Serializer,
{
  let num = num.unwrap_or(0);
  serializer.serialize_i64(num)
}

async fn apply_user_filter<Q: QueryFilter>(
  mut query: Q,
  db: &DatabaseConnection,
  user: Uuid,
  access_type: AccessType,
) -> Result<Q> {
  let permission = match access_type {
    AccessType::View => crate::utils::CacheView::name(),
    AccessType::Edit => crate::utils::CacheEdit::name(),
  };

  let hash_general_permission = GroupTable::new(db)
    .user_hash_permissions(user, permission)
    .await?;

  if !hash_general_permission {
    // If the user doesn't have the general permission, they must have specific access to the cache
    let group_ids = group_user::Entity::find()
      .filter(group_user::Column::UserId.eq(user))
      .select_only()
      .column_as(group_user::Column::GroupId, QueryAs::GroupId)
      .into_values::<Uuid, QueryAs>()
      .all(db)
      .await?;

    let mut user_cond = cache_access::Column::UserId.eq(user);
    let mut group_cond = cache_access::Column::GroupId.is_in(group_ids);
    let mut cond = Condition::any();

    // if only read access is requested, also include entries with edit access
    if access_type == AccessType::Edit {
      user_cond = user_cond.and(cache_access::Column::AccessType.eq(AccessType::Edit));
      group_cond = group_cond.and(cache_access::Column::AccessType.eq(AccessType::Edit));
    } else {
      // Only allow public caches if read access is sufficient. write requires explicit permissions
      cond = cond.add(cache::Column::Public.eq(true))
    }

    query = query.filter(cond.add(user_cond).add(group_cond));
  };

  Ok(query)
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn size_serializes_missing_as_zero() {
    let info = |size| CacheInfo {
      id: Uuid::nil(),
      name: "main".into(),
      size,
      quota: 1,
      public: true,
    };
    assert_eq!(serde_json::to_value(info(None)).unwrap()["size"], 0);
    assert_eq!(serde_json::to_value(info(Some(42))).unwrap()["size"], 42);
  }
}
