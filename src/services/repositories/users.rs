use std::collections::HashMap;

use futures::TryStreamExt;
use mongodb::{Collection, Database, IndexModel, bson::doc};

use crate::{
    infra::{AnonymousUserClaims, GoogleUserClaims, UserClaims, UserRole, telemetry},
    models::id::PlayerId,
};

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
        let role = resolved_role(existing.as_ref(), user);
        let user = user.clone().with_role(role);
        let user = mongodb::bson::to_bson(&StoredUserClaims::from(&user))?;
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
#[serde(tag = "type", content = "data")]
enum StoredUserClaims {
    Anonymous(StoredAnonymousUserClaims),
    Google(StoredGoogleUserClaims),
}

impl StoredUserClaims {
    fn into_user(self, role: UserRole) -> UserClaims {
        match self {
            StoredUserClaims::Anonymous(claims) => UserClaims::Anonymous(AnonymousUserClaims {
                id: claims.id,
                data: claims.data,
                role,
            }),
            StoredUserClaims::Google(claims) => UserClaims::Google(GoogleUserClaims {
                email: claims.email,
                name: claims.name,
                picture: claims.picture,
                nickname: claims.nickname,
                picture_override: claims.picture_override,
                role,
            }),
        }
    }
}

impl From<&UserClaims> for StoredUserClaims {
    fn from(value: &UserClaims) -> Self {
        match value {
            UserClaims::Anonymous(claims) => Self::Anonymous(StoredAnonymousUserClaims {
                id: claims.id.clone(),
                data: claims.data.clone(),
            }),
            UserClaims::Google(claims) => Self::Google(StoredGoogleUserClaims {
                email: claims.email.clone(),
                name: claims.name.clone(),
                picture: claims.picture.clone(),
                nickname: claims.nickname.clone(),
                picture_override: claims.picture_override.clone(),
            }),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredAnonymousUserClaims {
    id: PlayerId,
    data: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredGoogleUserClaims {
    email: PlayerId,
    name: String,
    picture: String,
    nickname: Option<String>,
    picture_override: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct UserDto {
    player_id: String,
    user: StoredUserClaims,
    role: UserRole,
    refresh_token: Option<String>,
    refresh_token_expires_at: Option<i64>,
}

impl UserDto {
    fn into_user(self) -> UserClaims {
        self.user.into_user(self.role)
    }
}

fn resolved_role(existing: Option<&UserDto>, user: &UserClaims) -> UserRole {
    existing
        .map(|existing| existing.role)
        .unwrap_or_else(|| user.role())
}

#[cfg(test)]
mod tests {
    use super::{StoredGoogleUserClaims, StoredUserClaims, UserDto, resolved_role};
    use crate::{
        infra::{AnonymousUserClaims, GoogleUserClaims, UserClaims, UserRole},
        models::id::PlayerId,
    };

    fn anonymous_with_role(player_id: &str, role: UserRole) -> UserClaims {
        UserClaims::Anonymous(AnonymousUserClaims {
            id: PlayerId(player_id.into()),
            data: serde_json::json!({ "nickname": player_id }),
            role,
        })
    }

    fn google_with_role(player_id: &str, role: UserRole) -> UserClaims {
        UserClaims::Google(GoogleUserClaims {
            email: PlayerId(player_id.into()),
            name: player_id.to_string(),
            picture: "picture".to_string(),
            nickname: None,
            picture_override: None,
            role,
        })
    }

    fn stored_google(player_id: &str) -> StoredUserClaims {
        StoredUserClaims::Google(StoredGoogleUserClaims {
            email: PlayerId(player_id.into()),
            name: player_id.to_string(),
            picture: "picture".to_string(),
            nickname: None,
            picture_override: None,
        })
    }

    #[test]
    fn resolved_role_uses_incoming_role_for_new_user() {
        let user = anonymous_with_role("admin-guest", UserRole::Admin);

        assert_eq!(resolved_role(None, &user), UserRole::Admin);
    }

    #[test]
    fn resolved_role_prefers_existing_stored_role() {
        let existing = UserDto {
            player_id: "player-id".to_string(),
            user: StoredUserClaims::from(&anonymous_with_role("player-id", UserRole::Player)),
            role: UserRole::Admin,
            refresh_token: None,
            refresh_token_expires_at: None,
        };
        let incoming = anonymous_with_role("player-id", UserRole::Player);

        assert_eq!(resolved_role(Some(&existing), &incoming), UserRole::Admin);
    }

    #[test]
    fn user_dto_into_user_uses_outer_role_only() {
        let existing = UserDto {
            player_id: "fastjonh@gmail.com".to_string(),
            user: stored_google("fastjonh@gmail.com"),
            role: UserRole::Admin,
            refresh_token: None,
            refresh_token_expires_at: None,
        };

        let user = existing.into_user();

        assert_eq!(user.role(), UserRole::Admin);
    }

    #[test]
    fn resolved_role_ignores_nested_role_and_uses_outer_role_only() {
        let existing = UserDto {
            player_id: "fastjonh@gmail.com".to_string(),
            user: stored_google("fastjonh@gmail.com"),
            role: UserRole::Player,
            refresh_token: None,
            refresh_token_expires_at: None,
        };
        let incoming = google_with_role("fastjonh@gmail.com", UserRole::Admin);

        assert_eq!(resolved_role(Some(&existing), &incoming), UserRole::Player);
    }

    #[test]
    fn stored_user_claims_do_not_serialize_nested_role() {
        let user = StoredUserClaims::from(&google_with_role("fastjonh@gmail.com", UserRole::Admin));
        let value = serde_json::to_value(user).expect("stored user claims should serialize");

        assert_eq!(
            value,
            serde_json::json!({
                "type": "Google",
                "data": {
                    "email": "fastjonh@gmail.com",
                    "name": "fastjonh@gmail.com",
                    "picture": "picture",
                    "nickname": null,
                    "picture_override": null
                }
            })
        );
    }
}
