use centaurus::db::init::Connection;

use crate::db::cache::CacheTable;
use crate::db::group_ext::GroupTableExt;
use crate::db::nar::NarTable;
use crate::db::token::TokenTable;
use crate::db::user_ext::UserTableExt;

pub mod cache;
pub mod group_ext;
pub mod nar;
pub mod token;
pub mod user_ext;

pub trait DBTrait {
  fn group_ext(&self) -> GroupTableExt<'_>;
  fn user_ext(&self) -> UserTableExt<'_>;
  fn token(&self) -> TokenTable<'_>;
  fn cache(&self) -> CacheTable<'_>;
  fn nar(&self) -> NarTable<'_>;
}

impl DBTrait for Connection {
  fn group_ext(&self) -> GroupTableExt<'_> {
    GroupTableExt::new(self)
  }

  fn user_ext(&self) -> UserTableExt<'_> {
    UserTableExt::new(self)
  }

  fn token(&self) -> TokenTable<'_> {
    TokenTable::new(self)
  }

  fn cache(&self) -> CacheTable<'_> {
    CacheTable::new(self)
  }

  fn nar(&self) -> NarTable<'_> {
    NarTable::new(self)
  }
}

#[cfg(test)]
pub mod test {
  use centaurus::db::init::Connection;
  use migration::{Migrator, MigratorTrait};
  use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};
  use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner},
  };

  /// Starts a throwaway Postgres container (same major version as production) and runs all migrations on it.
  /// Keep the container alive for the duration of the test, dropping it removes postgres.
  pub async fn test_pg_db() -> (Connection, ContainerAsync<Postgres>) {
    let container = Postgres::default()
      .with_tag("18-alpine")
      .start()
      .await
      .expect("start postgres container");
    let url = format!(
      "postgres://postgres:postgres@{}:{}/postgres",
      container.get_host().await.unwrap(),
      container.get_host_port_ipv4(5432).await.unwrap()
    );
    let conn = Database::connect(url).await.expect("connect to postgres");
    Migrator::up(&conn, None).await.expect("run migrations");
    (Connection(conn), container)
  }

  async fn pg_type_exists(conn: &Connection, name: &str) -> bool {
    conn
      .query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT 1 FROM pg_type WHERE typname = $1",
        [name.into()],
      ))
      .await
      .unwrap()
      .is_some()
  }

  #[tokio::test]
  async fn pg_migrations_down_then_up() {
    let (conn, _pg) = test_pg_db().await;
    for ty in ["access_type", "eviction_policy"] {
      assert!(pg_type_exists(&conn, ty).await, "{ty} after up");
    }

    // every migration's `down` must work on postgres, including the enum drops
    Migrator::down(&conn.0, None).await.unwrap();
    for ty in ["access_type", "eviction_policy"] {
      assert!(!pg_type_exists(&conn, ty).await, "{ty} after down");
    }
    assert!(
      Migrator::get_applied_migrations(&conn.0)
        .await
        .unwrap()
        .is_empty()
    );
    let leftover = conn
      .query_all_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' \
         AND table_name <> 'seaql_migrations'",
      ))
      .await
      .unwrap();
    let leftover: Vec<String> = leftover
      .iter()
      .map(|r| r.try_get("", "table_name").unwrap())
      .collect();
    assert!(leftover.is_empty(), "tables left after down: {leftover:?}");

    Migrator::up(&conn.0, None).await.unwrap();
    for ty in ["access_type", "eviction_policy"] {
      assert!(pg_type_exists(&conn, ty).await, "{ty} after re-up");
    }
  }

  /// SQLite can't run centaurus' `m4_groups` down (multi-table `DROP TABLE`), so roll back to just after `m5_setup`:
  /// every hibernation migration (and centaurus `m6`) must still go down and back up.
  #[tokio::test]
  async fn sqlite_migrations_down_then_up() {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    let total = Migrator::migrations().len();
    let keep = Migrator::migrations()
      .iter()
      .position(|m| m.name() == "m5_setup")
      .unwrap()
      + 1;
    Migrator::down(&conn, Some((total - keep) as u32))
      .await
      .unwrap();
    let applied = Migrator::get_applied_migrations(&conn).await.unwrap();
    assert_eq!(applied.last().unwrap().name(), "m5_setup");
    Migrator::up(&conn, None).await.unwrap();
    assert_eq!(
      Migrator::get_applied_migrations(&conn).await.unwrap().len(),
      total
    );
  }
}
