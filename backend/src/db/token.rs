use chrono::Utc;
use entity::{prelude::*, token};
use sea_orm::{ActiveValue::Set, IntoActiveModel, prelude::*};

pub struct TokenTable<'db> {
  db: &'db DatabaseConnection,
}

impl<'db> TokenTable<'db> {
  pub fn new(db: &'db DatabaseConnection) -> Self {
    Self { db }
  }

  pub async fn insert(
    &self,
    user: Uuid,
    name: String,
    token: String,
    exp: DateTime,
  ) -> Result<token::Model, DbErr> {
    let active_model = token::ActiveModel {
      id: Set(Uuid::now_v7()),
      user_id: Set(user),
      name: Set(name),
      exp: Set(exp),
      token: Set(token),
      ..Default::default()
    };
    active_model.insert(self.db).await
  }

  pub async fn get_by_user(&self, user: Uuid) -> Result<Vec<token::Model>, DbErr> {
    Token::find()
      .filter(token::Column::UserId.eq(user))
      .all(self.db)
      .await
  }

  pub async fn invalidate(&self, user: Uuid, id: Uuid) -> Result<(), DbErr> {
    Token::delete_by_id(id)
      .filter(token::Column::UserId.eq(user))
      .exec(self.db)
      .await?;

    Ok(())
  }

  pub async fn invalidate_expired(&self, user: Uuid) -> Result<u64, DbErr> {
    Ok(
      Token::delete_many()
        .filter(token::Column::UserId.eq(user))
        .filter(token::Column::Exp.lt(Utc::now().naive_utc()))
        .exec(self.db)
        .await?
        .rows_affected,
    )
  }

  /// Returns `false` if no token with this id belongs to `user`.
  pub async fn replace(&self, user: Uuid, id: Uuid, hash: String) -> Result<bool, DbErr> {
    let res = Token::update_many()
      .col_expr(token::Column::Token, Expr::value(hash))
      .filter(token::Column::Id.eq(id))
      .filter(token::Column::UserId.eq(user))
      .exec(self.db)
      .await?;
    Ok(res.rows_affected > 0)
  }

  pub async fn get_by_token(&self, token: &str) -> Result<token::Model, DbErr> {
    Token::find()
      .filter(token::Column::Token.eq(token))
      .one(self.db)
      .await?
      .ok_or(DbErr::RecordNotFound(format!(
        "Token with value {} not found",
        token
      )))
  }

  pub async fn by_id(&self, id: Uuid) -> Result<token::Model, DbErr> {
    Token::find_by_id(id)
      .one(self.db)
      .await?
      .ok_or(DbErr::RecordNotFound(format!(
        "Token with id {} not found",
        id
      )))
  }

  pub async fn by_id_user(&self, id: Uuid, user: Uuid) -> Result<Option<token::Model>, DbErr> {
    Token::find_by_id(id)
      .filter(token::Column::UserId.eq(user))
      .one(self.db)
      .await
  }

  pub async fn get_by_name(&self, user: Uuid, name: &str) -> Result<token::Model, DbErr> {
    Token::find()
      .filter(token::Column::UserId.eq(user))
      .filter(token::Column::Name.eq(name))
      .one(self.db)
      .await?
      .ok_or(DbErr::RecordNotFound(format!(
        "Token with name {} not found for user {}",
        name, user
      )))
  }

  pub async fn token_used(&self, id: Uuid) -> Result<(), DbErr> {
    let mut token = self.by_id(id).await?.into_active_model();
    token.last_used = Set(Some(Utc::now().naive_utc()));
    token.update(self.db).await?;
    Ok(())
  }

  /// Returns `false` if no token with this id belongs to `user`.
  pub async fn update(
    &self,
    user: Uuid,
    id: Uuid,
    name: String,
    exp: DateTime,
  ) -> Result<bool, DbErr> {
    let res = Token::update_many()
      .col_expr(token::Column::Name, Expr::value(name))
      .col_expr(token::Column::Exp, Expr::value(exp))
      .filter(token::Column::Id.eq(id))
      .filter(token::Column::UserId.eq(user))
      .exec(self.db)
      .await?;
    Ok(res.rows_affected > 0)
  }
}
