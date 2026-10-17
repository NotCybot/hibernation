use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
  async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .create_table(
        Table::create()
          .table(NarUpload::Table)
          .if_not_exists()
          .col(pk_uuid(NarUpload::Id))
          .col(string_null(NarUpload::MultipartId))
          .col(date_time(NarUpload::UpdatedAt))
          .to_owned(),
      )
      .await
  }

  async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .drop_table(Table::drop().table(NarUpload::Table).to_owned())
      .await
  }
}

#[derive(DeriveIden)]
pub enum NarUpload {
  Table,
  Id,
  MultipartId,
  UpdatedAt,
}
