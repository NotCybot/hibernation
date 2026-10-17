pub use sea_orm_migration::prelude::*;

mod m20260317_135750_token;
mod m20260319_192505_cache;
mod m20260319_193259_nar;
mod m20260319_194332_nar_info;
mod m20260320_164249_cache_access;
mod m20260806_134347_nar_info_last_accessed_not_null;
mod m20261017_120000_nar_upload;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
  fn migrations() -> Vec<Box<dyn MigrationTrait>> {
    vec![
      Box::new(centaurus::db::migrations::m0_key::Migration),
      Box::new(centaurus::db::migrations::m1_invalid_jwt::Migration),
      Box::new(centaurus::db::migrations::m2_settings::Migration),
      Box::new(centaurus::db::migrations::m3_user::Migration),
      Box::new(centaurus::db::migrations::m4_groups::Migration),
      Box::new(centaurus::db::migrations::m5_setup::Migration),
      Box::new(m20260317_135750_token::Migration),
      Box::new(m20260319_192505_cache::Migration),
      Box::new(m20260319_193259_nar::Migration),
      Box::new(m20260319_194332_nar_info::Migration),
      Box::new(m20260320_164249_cache_access::Migration),
      Box::new(centaurus::db::migrations::m6_user_oidc_subject::Migration),
      Box::new(m20260806_134347_nar_info_last_accessed_not_null::Migration),
      Box::new(m20261017_120000_nar_upload::Migration),
    ]
  }
}
