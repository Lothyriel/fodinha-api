use std::collections::HashMap;

use futures::TryStreamExt;
use mongodb::{Collection, Database, IndexModel, bson::doc};

use crate::infra::{UserClaims, telemetry};

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
        telemetry::db_query("Users", "create_index", async {
            self.users
                .create_index(IndexModel::builder().keys(doc! { "player_id": 1 }).build())
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn upsert_user(&self, user: &UserClaims) -> mongodb::error::Result<()> {
        let dto = UserDto::new(user.clone());

        telemetry::db_query("Users", "replace_one.upsert", async {
            self.users
                .replace_one(doc! { "player_id": &dto.player_id }, &dto)
                .upsert(true)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn user(&self, player_id: &str) -> mongodb::error::Result<Option<UserClaims>> {
        telemetry::db_query("Users", "find_one", async {
            self.users.find_one(doc! { "player_id": player_id }).await
        })
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

        let users: Vec<UserDto> = telemetry::db_query("Users", "find.by_ids", async {
            let cursor = self
                .users
                .find(doc! { "player_id": { "$in": player_ids } })
                .await?;

            cursor.try_collect().await
        })
        .await?;

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
