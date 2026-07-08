use std::collections::HashMap;

use futures::TryStreamExt;
use mongodb::{Collection, Database, IndexModel, bson::doc};

use crate::infra::{UserClaims, UserRole, telemetry};

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

    pub async fn upsert_user(&self, user: &UserClaims) -> mongodb::error::Result<UserClaims> {
        let player_id = user.id();
        let existing = telemetry::db_query("Users", "find_one.before_upsert", async {
            self.users
                .find_one(doc! { "player_id": player_id.as_str() })
                .await
        })
        .await?;
        let role = existing.as_ref().map(UserDto::role).unwrap_or_default();
        let user = user.clone().with_role(role);
        let user = mongodb::bson::to_bson(&user)?;
        let role = mongodb::bson::to_bson(&role)?;

        telemetry::db_query("Users", "update_one.upsert", async {
            self.users
                .update_one(
                    doc! { "player_id": player_id.as_str() },
                    doc! {
                        "$set": {
                            "player_id": player_id.as_str(),
                            "user": user,
                            "role": role,
                        },
                    },
                )
                .upsert(true)
                .await
        })
        .await?;

        self.user(player_id.as_str())
            .await
            .map(|user| user.expect("upserted user should exist"))
    }

    pub async fn user(&self, player_id: &str) -> mongodb::error::Result<Option<UserClaims>> {
        telemetry::db_query("Users", "find_one", async {
            self.users.find_one(doc! { "player_id": player_id }).await
        })
        .await
        .map(|user| user.map(UserDto::into_user))
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
            .map(|user| (user.player_id.clone(), user.into_user()))
            .collect())
    }

    pub async fn store_refresh_token(
        &self,
        player_id: &str,
        token: &str,
        expires_at: i64,
    ) -> mongodb::error::Result<()> {
        telemetry::db_query("Users", "update_one.store_refresh_token", async {
            self.users
                .update_one(
                    doc! { "player_id": player_id },
                    doc! {
                        "$set": {
                            "refresh_token": token,
                            "refresh_token_expires_at": expires_at,
                        },
                    },
                )
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn refresh_session(
        &self,
        token: &str,
    ) -> mongodb::error::Result<Option<RefreshSession>> {
        let user = telemetry::db_query("Users", "find_one.refresh_token", async {
            self.users.find_one(doc! { "refresh_token": token }).await
        })
        .await?;

        Ok(user.and_then(|user| {
            Some(RefreshSession {
                player_id: user.player_id,
                expires_at: user.refresh_token_expires_at?,
            })
        }))
    }
}

#[derive(Debug, Clone)]
pub struct RefreshSession {
    pub player_id: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct UserDto {
    player_id: String,
    user: UserClaims,
    role: Option<UserRole>,
    refresh_token: Option<String>,
    refresh_token_expires_at: Option<i64>,
}

impl UserDto {
    fn role(&self) -> UserRole {
        self.role.unwrap_or_else(|| self.user.role())
    }

    fn into_user(self) -> UserClaims {
        let role = self.role();

        self.user.with_role(role)
    }
}
