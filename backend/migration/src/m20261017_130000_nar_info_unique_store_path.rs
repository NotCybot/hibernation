use sea_orm_migration::prelude::*;

use crate::m20260319_194332_nar_info::NarInfo;

#[derive(DeriveMigrationName)]
pub struct Migration;

const INDEX_NAME: &str = "nar_info.cache_id_store_path_hash";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
  async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    // Concurrent pushes of the same path could insert it twice, keep the first row of each (references cascade)
    manager
      .get_connection()
      .execute_unprepared(
        "DELETE FROM nar_info WHERE EXISTS (SELECT 1 FROM nar_info b WHERE b.cache_id = nar_info.cache_id \
         AND b.store_path_hash = nar_info.store_path_hash AND b.id < nar_info.id)",
      )
      .await?;

    manager
      .create_index(
        Index::create()
          .if_not_exists()
          .name(INDEX_NAME)
          .table(NarInfo::Table)
          .col(NarInfo::CacheId)
          .col(NarInfo::StorePathHash)
          .unique()
          .to_owned(),
      )
      .await
  }

  async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .drop_index(
        Index::drop()
          .name(INDEX_NAME)
          .table(NarInfo::Table)
          .to_owned(),
      )
      .await
  }
}
