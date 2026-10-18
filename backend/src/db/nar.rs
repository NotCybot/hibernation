use std::collections::HashSet;

use chrono::Utc;
use entity::{nar, nar_info, nar_info_reference, nar_upload};
use harmonia_store_core::store_path::StorePath;
use migration::{ExprTrait, Func, LikeExpr};
use schemars::JsonSchema;
use sea_orm::{
  ActiveValue::Set, FromQueryResult, IntoActiveModel, JoinType, QueryOrder, QuerySelect,
  QueryTrait, TransactionTrait, prelude::*,
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, JsonSchema)]
pub enum SearchOrder {
  Asc,
  Desc,
}

#[derive(Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
pub enum SearchSort {
  StorePath,
  Created,
  Accessed,
  Size,
  AccessCount,
}

#[derive(FromQueryResult)]
pub struct SearchResult {
  pub store_path: String,
  pub created_at: chrono::NaiveDateTime,
  pub last_accessed_at: chrono::NaiveDateTime,
  pub size: i64,
  pub accessed: i64,
}

#[derive(FromQueryResult)]
pub struct NarInfoData {
  pub id: Uuid,
  pub store_path: String,
  pub compression: String,
  pub hash: String,
  pub size: i64,
  pub nar_hash: String,
  pub nar_size: i64,
  pub deriver: Option<String>,
  pub signature: String,
}

pub struct NarTable<'db> {
  db: &'db DatabaseConnection,
}

impl<'db> NarTable<'db> {
  pub fn new(db: &'db DatabaseConnection) -> Self {
    Self { db }
  }

  #[allow(clippy::too_many_arguments)]
  pub async fn create_path(
    &self,
    nar_id: Uuid,
    cache: Uuid,
    store_path: String,
    store_path_hash: String,
    nar_hash: String,
    nar_size: u64,
    file_hash: String,
    file_size: u64,
    deriver: Option<String>,
    signature: String,
    references: Vec<String>,
  ) -> centaurus::error::Result<()> {
    self
      .db
      .transaction::<_, (), DbErr>(|db| {
        Box::pin(async move {
          // nar_id is either an existing nar (refreshed by create_nar, which keeps the orphan cleanup away)
          // or the placeholder of a fresh upload that still needs its data
          let nar = nar::Entity::find_by_id(nar_id)
            .one(db)
            .await?
            .ok_or(DbErr::RecordNotFound("Nar not found".to_string()))?;
          let nar = if !nar.nar_hash.is_empty() {
            nar
          } else {
            let mut nar = nar.into_active_model();
            nar.hash = Set(file_hash.clone());
            nar.size = Set(file_size as i64);
            nar.nar_hash = Set(nar_hash.clone());
            nar.nar_size = Set(nar_size as i64);
            nar.update(db).await?
          };

          let now = chrono::Utc::now().naive_utc();

          let nar_info = nar_info::ActiveModel {
            id: Set(Uuid::now_v7()),
            nar_id: Set(nar.id),
            cache_id: Set(cache),
            compression: Set("zst".to_string()),
            store_path: Set(store_path),
            store_path_hash: Set(store_path_hash),
            deriver: Set(deriver),
            signature: Set(signature),
            created_at: Set(now),
            last_accessed_at: Set(now),
            accessed: Set(0),
          };
          let nar_info = nar_info.insert(db).await?;

          let mut nar_refernces = Vec::new();
          for reference in references {
            let reference = nar_info_reference::ActiveModel {
              id: Set(Uuid::now_v7()),
              nar_info_id: Set(nar_info.id),
              store_path: Set(reference),
            };
            nar_refernces.push(reference);
          }

          if !nar_refernces.is_empty() {
            nar_info_reference::Entity::insert_many(nar_refernces)
              .exec(db)
              .await?;
          }

          Ok(())
        })
      })
      .await
      .map_err(|e| match e {
        // A concurrent push of the same path won the race (unique cache_id + store_path_hash)
        sea_orm::TransactionError::Transaction(e)
          if matches!(
            e.sql_err(),
            Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
          ) =>
        {
          centaurus::anyhow!(BAD_REQUEST, "Store path is already in the cache")
        }
        e => centaurus::anyhow!(INTERNAL_SERVER_ERROR, "DB Error: {e}"),
      })
  }

  pub async fn is_store_path_in_cache(&self, cache: Uuid, store_path: &str) -> Result<bool, DbErr> {
    let count = nar_info::Entity::find()
      .filter(nar_info::Column::CacheId.eq(cache))
      .filter(nar_info::Column::StorePath.eq(store_path.to_string()))
      .count(self.db)
      .await?;

    Ok(count > 0)
  }

  pub async fn find_nar(&self, nar_hash: &str, nar_size: u64) -> Result<Option<nar::Model>, DbErr> {
    nar::Entity::find()
      .filter(nar::Column::NarHash.eq(nar_hash))
      .filter(nar::Column::NarSize.eq(nar_size as i64))
      .one(self.db)
      .await
  }

  pub async fn create_nar(
    &self,
    nar_id: Uuid,
    nar_hash: &str,
    nar_size: u64,
  ) -> Result<Option<nar::Model>, DbErr> {
    // Single statement so the orphan cleanup can not delete the nar between finding and refreshing it
    if let Some(existing) = nar::Entity::update_many()
      .col_expr(nar::Column::CreatedAt, Expr::value(Utc::now().naive_utc()))
      .filter(nar::Column::NarHash.eq(nar_hash))
      .filter(nar::Column::NarSize.eq(nar_size as i64))
      .exec_with_returning(self.db)
      .await?
      .into_iter()
      .next()
    {
      return Ok(Some(existing));
    }

    let nar = nar::ActiveModel {
      id: Set(nar_id),
      hash: Set("".to_string()),
      size: Set(0),
      nar_hash: Set("".to_string()),
      nar_size: Set(0),
      created_at: Set(chrono::Utc::now().naive_utc()),
    };
    nar.insert(self.db).await?;

    Ok(None)
  }

  pub async fn create_upload(
    &self,
    nar_id: Uuid,
    multipart_id: Option<String>,
  ) -> Result<(), DbErr> {
    nar_upload::ActiveModel {
      id: Set(nar_id),
      multipart_id: Set(multipart_id),
      updated_at: Set(Utc::now().naive_utc()),
    }
    .insert(self.db)
    .await?;
    Ok(())
  }

  pub async fn touch_upload(&self, nar_id: Uuid) -> Result<(), DbErr> {
    nar_upload::Entity::update_many()
      .col_expr(
        nar_upload::Column::UpdatedAt,
        Expr::value(Utc::now().naive_utc()),
      )
      .filter(nar_upload::Column::Id.eq(nar_id))
      .exec(self.db)
      .await?;
    Ok(())
  }

  pub async fn delete_upload(&self, nar_id: Uuid) -> Result<(), DbErr> {
    nar_upload::Entity::delete_by_id(nar_id)
      .exec(self.db)
      .await?;
    Ok(())
  }

  /// Uploads without activity for an hour, e.g. abandoned by the client or lost due to a server restart
  pub async fn stale_uploads(&self) -> Result<Vec<nar_upload::Model>, DbErr> {
    let cut_off = Utc::now() - chrono::Duration::hours(1);

    nar_upload::Entity::find()
      .filter(nar_upload::Column::UpdatedAt.lt(cut_off.naive_utc()))
      .all(self.db)
      .await
  }

  /// Ids of all nars and of all uploads, every file in the storage belongs to one of them
  pub async fn known_ids(&self) -> Result<(HashSet<Uuid>, HashSet<Uuid>), DbErr> {
    // Uploads first: a finishing upload creates its nar entry before deleting its upload entry, so a file missing
    // from this read already has a nar entry the next read sees
    let uploads = nar_upload::Entity::find()
      .select_only()
      .column(nar_upload::Column::Id)
      .into_tuple::<Uuid>()
      .all(self.db)
      .await?;
    let nars = nar::Entity::find()
      .select_only()
      .column(nar::Column::Id)
      .into_tuple::<Uuid>()
      .all(self.db)
      .await?;
    Ok((nars.into_iter().collect(), uploads.into_iter().collect()))
  }

  pub async fn nar_exists(&self, nar_id: Uuid) -> Result<bool, DbErr> {
    Ok(nar::Entity::find_by_id(nar_id).count(self.db).await? > 0)
  }

  pub async fn orphan_nars(&self) -> Result<Vec<nar::Model>, DbErr> {
    let cut_off = (chrono::Utc::now() - chrono::Duration::seconds(600)).naive_utc();

    nar::Entity::find()
      .left_join(nar_info::Entity)
      .filter(nar_info::Column::Id.is_null())
      .filter(nar::Column::CreatedAt.lt(cut_off))
      .all(self.db)
      .await
  }

  /// Deletes the nar only if it is still an orphan. Checked in the same statement so a concurrent upload
  /// reusing the nar (which refreshes created_at first) never loses its data. Returns whether it was deleted.
  pub async fn delete_orphan_nar(&self, nar_id: Uuid) -> Result<bool, DbErr> {
    let cut_off = (chrono::Utc::now() - chrono::Duration::seconds(600)).naive_utc();
    let res = nar::Entity::delete_many()
      .filter(nar::Column::Id.eq(nar_id))
      .filter(nar::Column::CreatedAt.lt(cut_off))
      .filter(
        nar::Column::Id.not_in_subquery(
          nar_info::Entity::find()
            .select_only()
            .column(nar_info::Column::NarId)
            .into_query(),
        ),
      )
      .exec(self.db)
      .await?;
    Ok(res.rows_affected > 0)
  }

  pub async fn find_duplicate_nars(&self) -> Result<Vec<String>, DbErr> {
    let referenced_ids = nar_info::Entity::find()
      .select_only()
      .column(nar_info::Column::NarId)
      .distinct()
      .into_query();

    nar::Entity::find()
      .select_only()
      .column(nar::Column::NarHash)
      .filter(nar::Column::Id.in_subquery(referenced_ids))
      .group_by(nar::Column::NarHash)
      .having(nar::Column::NarHash.count().gt(1))
      .into_tuple::<String>()
      .all(self.db)
      .await
  }

  /// Only referenced nars, so dedupe never keeps an orphan the cleanup is about to delete.
  /// Oldest first (v7 ids), so dedupe keeps the nar the earliest narinfo URLs point at
  pub async fn find_nars_by_nar_hash(&self, nar_hash: &str) -> Result<Vec<Uuid>, DbErr> {
    nar::Entity::find()
      .select_only()
      .column(nar::Column::Id)
      .order_by_asc(nar::Column::Id)
      .filter(nar::Column::NarHash.eq(nar_hash))
      .filter(
        nar::Column::Id.in_subquery(
          nar_info::Entity::find()
            .select_only()
            .column(nar_info::Column::NarId)
            .into_query(),
        ),
      )
      .into_tuple::<Uuid>()
      .all(self.db)
      .await
  }

  pub async fn replace_nar_id(&self, old: &[Uuid], new: Uuid) -> Result<(), DbErr> {
    let old = old.to_vec();
    self
      .db
      .transaction::<_, (), DbErr>(|db| {
        Box::pin(async move {
          // Refreshing first locks the kept nar, a concurrent orphan cleanup then sees the new created_at and skips
          // it. If the cleanup deleted it first, moving the references fails on the foreign key and nothing changes
          nar::Entity::update_many()
            .col_expr(nar::Column::CreatedAt, Expr::value(Utc::now().naive_utc()))
            .filter(nar::Column::Id.eq(new))
            .exec(db)
            .await?;
          nar_info::Entity::update_many()
            .col_expr(nar_info::Column::NarId, Expr::value(new))
            .filter(nar_info::Column::NarId.is_in(old))
            .exec(db)
            .await?;
          Ok(())
        })
      })
      .await
      .map_err(|e| match e {
        sea_orm::TransactionError::Connection(e) | sea_orm::TransactionError::Transaction(e) => e,
      })
  }

  pub async fn missing_paths(
    &self,
    cache: Uuid,
    paths: Vec<StorePath>,
  ) -> Result<Vec<StorePath>, DbErr> {
    if paths.is_empty() {
      return Ok(vec![]);
    }

    let paths = paths.into_iter().map(|p| p.to_string()).collect::<Vec<_>>();

    let mut query = sea_orm::sea_query::Query::select();
    query
      .column(("inputs_paths", "column1"))
      .from_values(paths, "inputs_paths")
      .left_join(
        nar_info::Entity,
        Expr::col(("inputs_paths", "column1"))
          .equals((nar_info::Entity, nar_info::Column::StorePath))
          .and(Expr::col((nar_info::Entity, nar_info::Column::CacheId)).eq(cache)),
      )
      .and_where(Expr::col((nar_info::Entity, nar_info::Column::StorePath)).is_null());

    let builder = self.db.get_database_backend();
    let missing = self
      .db
      .query_all_raw(builder.build(&query))
      .await?
      .into_iter()
      .map(|row| StorePath::from_base_path(&row.try_get_by_index::<String>(0).unwrap()).unwrap())
      .collect();

    Ok(missing)
  }

  pub async fn search_store_paths(
    &self,
    cache: Uuid,
    query: String,
    order: SearchOrder,
    sort: SearchSort,
  ) -> Result<Vec<SearchResult>, DbErr> {
    let mut query = nar_info::Entity::find()
      .filter(nar_info::Column::CacheId.eq(cache))
      // `contains` is a case-sensitive LIKE on Postgres, lower both sides so search is case-insensitive everywhere
      .filter(
        Func::lower(Expr::col((nar_info::Entity, nar_info::Column::StorePath))).like(
          LikeExpr::new(format!(
            "%{}%",
            query
              .trim()
              .to_lowercase()
              .replace('\\', "\\\\")
              .replace('%', "\\%")
              .replace('_', "\\_")
          ))
          .escape('\\'),
        ),
      )
      .join(JoinType::LeftJoin, nar_info::Relation::Nar.def());

    let order = match order {
      SearchOrder::Asc => sea_orm::Order::Asc,
      SearchOrder::Desc => sea_orm::Order::Desc,
    };

    if sort == SearchSort::Size {
      query = query.order_by(nar::Column::Size, order);
    } else {
      let column = match sort {
        SearchSort::StorePath => nar_info::Column::StorePath,
        SearchSort::Created => nar_info::Column::CreatedAt,
        SearchSort::Accessed => nar_info::Column::LastAccessedAt,
        SearchSort::AccessCount => nar_info::Column::Accessed,
        SearchSort::Size => unreachable!(),
      };

      query = query.order_by(column, order);
    }

    query
      .select_only()
      .column(nar_info::Column::StorePath)
      .column(nar_info::Column::CreatedAt)
      .column(nar_info::Column::LastAccessedAt)
      .column(nar_info::Column::Accessed)
      .column(nar::Column::Size)
      .limit(100)
      .into_model::<SearchResult>()
      .all(self.db)
      .await
  }

  pub async fn delete_path(&self, cache: Uuid, store_path: &str) -> Result<(), DbErr> {
    nar_info::Entity::delete_many()
      .filter(nar_info::Column::CacheId.eq(cache))
      .filter(nar_info::Column::StorePath.eq(store_path.to_string()))
      .exec(self.db)
      .await?;

    Ok(())
  }

  pub async fn nar_info_data(
    &self,
    cache: Uuid,
    store_path_hash: &str,
  ) -> Result<Option<NarInfoData>, DbErr> {
    nar_info::Entity::find()
      .filter(nar_info::Column::CacheId.eq(cache))
      .filter(nar_info::Column::StorePathHash.eq(store_path_hash.to_string()))
      .join(JoinType::LeftJoin, nar_info::Relation::Nar.def())
      .select_only()
      .column(nar_info::Column::Id)
      .column(nar_info::Column::StorePath)
      .column(nar_info::Column::Compression)
      .column(nar::Column::NarHash)
      .column(nar::Column::NarSize)
      .column(nar::Column::Hash)
      .column(nar::Column::Size)
      .column(nar_info::Column::Deriver)
      .column(nar_info::Column::Signature)
      .into_model::<NarInfoData>()
      .one(self.db)
      .await
  }

  pub async fn nar_info_references(&self, nar_info_id: Uuid) -> Result<Vec<String>, DbErr> {
    nar_info_reference::Entity::find()
      .filter(nar_info_reference::Column::NarInfoId.eq(nar_info_id))
      .select_only()
      .column(nar_info_reference::Column::StorePath)
      .into_tuple::<String>()
      .all(self.db)
      .await
  }

  pub async fn get_nar(
    &self,
    cache: Uuid,
    file_hash: &str,
    compression: &str,
  ) -> Result<Option<(Uuid, i64)>, DbErr> {
    nar_info::Entity::find()
      .filter(nar_info::Column::CacheId.eq(cache))
      .join(JoinType::InnerJoin, nar_info::Relation::Nar.def())
      .filter(nar::Column::Hash.eq(file_hash))
      .filter(nar_info::Column::Compression.eq(compression))
      .select_only()
      .column(nar::Column::Id)
      .column(nar::Column::Size)
      .into_tuple::<(Uuid, i64)>()
      .one(self.db)
      .await
  }

  pub async fn nar_accessed(&self, nar_id: Uuid, cache: Uuid) -> Result<(), DbErr> {
    let model = nar_info::Entity::find()
      .filter(nar_info::Column::NarId.eq(nar_id))
      .filter(nar_info::Column::CacheId.eq(cache))
      .one(self.db)
      .await?
      .ok_or(DbErr::RecordNotFound("nar_info".to_string()))?;

    let accessed = model.accessed;
    let mut model = model.into_active_model();

    model.last_accessed_at = Set(chrono::Utc::now().naive_utc());
    model.accessed = Set(accessed + 1);

    model.save(self.db).await?;

    Ok(())
  }
}
