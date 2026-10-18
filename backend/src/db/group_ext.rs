use centaurus::{
  db::tables::group::{GroupTable, SimpleUserInfo},
  error::Result,
};
use entity::{cache, cache_access, group, sea_orm_active_enums::AccessType};
use schemars::JsonSchema;
use sea_orm::{FromQueryResult, IntoActiveModel, JoinType, QuerySelect, Set, prelude::*};
use serde::{Deserialize, Serialize};

pub struct GroupTableExt<'db> {
  db: &'db DatabaseConnection,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct GroupDetails {
  pub id: Uuid,
  pub name: String,
  pub permissions: Vec<String>,
  pub users: Vec<SimpleUserInfo>,
  pub caches: Vec<CacheMapping>,
}

#[derive(Serialize, Deserialize, FromQueryResult, PartialEq, JsonSchema)]
pub struct CacheMapping {
  pub uuid: Uuid,
  pub name: String,
  pub access_type: AccessType,
}

/// Whether two mapping lists grant the same access, ignoring order.
pub fn same_mappings(a: &[CacheMapping], b: &[CacheMapping]) -> bool {
  a.len() == b.len() && a.iter().all(|m| b.contains(m)) && b.iter().all(|m| a.contains(m))
}

impl<'db> GroupTableExt<'db> {
  pub fn new(db: &'db DatabaseConnection) -> Self {
    Self { db }
  }

  async fn cache_access_for_group(&self, group: Uuid) -> Result<Vec<CacheMapping>> {
    Ok(
      cache_access::Entity::find()
        .join(JoinType::InnerJoin, cache_access::Relation::Cache.def())
        .filter(cache_access::Column::GroupId.eq(group))
        .select_only()
        .column_as(cache_access::Column::CacheId, "uuid")
        .column_as(cache::Column::Name, "name")
        .column_as(cache_access::Column::AccessType, "access_type")
        .into_model::<CacheMapping>()
        .all(self.db)
        .await?,
    )
  }

  pub async fn group_info(&self, group_id: Uuid) -> Result<Option<GroupDetails>> {
    let group = group::Entity::find_by_id(group_id).one(self.db).await?;
    let Some(group) = group else {
      return Ok(None);
    };

    let group_table = GroupTable::new(self.db);
    let permissions = group_table.get_group_permissions(group_id).await?;
    let users = group_table.get_group_users(group_id).await?;
    let caches = self.cache_access_for_group(group_id).await?;

    Ok(Some(GroupDetails {
      id: group.id,
      name: group.name,
      permissions,
      users,
      caches,
    }))
  }

  pub async fn update_cache_mappings(
    &self,
    group: Uuid,
    old_mappings: Vec<CacheMapping>,
    new_mappings: Vec<CacheMapping>,
  ) -> Result<()> {
    // Delete old mappings
    let cache_ids_to_delete: Vec<Uuid> = old_mappings.iter().map(|m| m.uuid).collect();
    cache_access::Entity::delete_many()
      .filter(cache_access::Column::GroupId.eq(group))
      .filter(cache_access::Column::CacheId.is_in(cache_ids_to_delete))
      .exec(self.db)
      .await?;

    // Insert new mappings
    let mut models = Vec::new();
    for mapping in new_mappings {
      let model = cache_access::ActiveModel {
        group_id: Set(Some(group)),
        cache_id: Set(mapping.uuid),
        access_type: Set(mapping.access_type),
        ..Default::default()
      }
      .into_active_model();
      models.push(model);
    }

    if models.is_empty() {
      return Ok(());
    }

    cache_access::Entity::insert_many(models)
      .exec(self.db)
      .await?;

    Ok(())
  }
}
