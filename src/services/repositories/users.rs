use std::collections::HashMap;

use futures::TryStreamExt;
use mongodb::{Collection, Database, IndexModel, bson::doc};

use crate::infra::UserClaims;

#[derive(Clone)]
pub struct UsersRepository {
    users: Collection<UserDto>,
}

impl UsersRepository {
    pub fn new(database: &Database) -> Self {
        Self {
            users: database.collection("Users"),
        }
    }

    pub async fn ensure_indexes(&self) -> mongodb::error::Result<()> {
        self.users
            .create_index(IndexModel::builder().keys(doc! { "player_id": 1 }).build())
            .await?;

        Ok(())
    }

    pub async fn upsert_user(&self, user: &UserClaims) -> mongodb::error::Result<()> {
        let dto = UserDto::new(user.clone());

        self.users
            .replace_one(doc! { "player_id": &dto.player_id }, &dto)
            .upsert(true)
            .await?;

        Ok(())
    }

    pub async fn user(&self, player_id: &str) -> mongodb::error::Result<Option<UserClaims>> {
        self.users
            .find_one(doc! { "player_id": player_id })
            .await
            .map(|user| user.map(|user| user.user))
    }

    pub async fn users_by_id(
        &self,
        player_ids: &[String],
    ) -> mongodb::error::Result<HashMap<String, UserClaims>> {
        if player_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let cursor = self
            .users
            .find(doc! { "player_id": { "$in": player_ids } })
            .await?;
        let users: Vec<UserDto> = cursor.try_collect().await?;

        Ok(users
            .into_iter()
            .map(|user| (user.player_id, user.user))
            .collect())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct UserDto {
    player_id: String,
    user: UserClaims,
}

impl UserDto {
    fn new(user: UserClaims) -> Self {
        Self {
            player_id: user.id().as_str().to_string(),
            user,
        }
    }
}
