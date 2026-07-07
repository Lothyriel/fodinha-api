use chrono::Utc;

use crate::{
    infra::UserClaims,
    models::{
        game::{fodinha_power, power_lua},
        id::{MercenaryId, PlayerId},
    },
    services::{
        object_storage::{ObjectStorage, ObjectStorageError},
        repositories::{
            mercenaries::{MercenariesRepository, MercenaryDto, NewMercenary},
            users::UsersRepository,
        },
    },
};

const IMAGE_OBJECT_CONTENT_TYPE: &str = "image/png";
const SCRIPT_OBJECT_CONTENT_TYPE: &str = "text/x-lua";

#[derive(Clone)]
pub struct MercenariesService {
    mercenaries: MercenariesRepository,
    storage: ObjectStorage,
    users: UsersRepository,
}

#[derive(Debug)]
pub struct UpsertMercenaryInput {
    pub mercenary_id: MercenaryId,
    pub name: String,
    pub subtitle: String,
    pub description: String,
    pub deck: String,
    pub style: String,
    pub temper: String,
    pub banner: Option<Vec<u8>>,
    pub passive_script: Option<Vec<u8>>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MercenaryResponse {
    pub id: MercenaryId,
    pub name: String,
    pub subtitle: String,
    pub description: String,
    pub deck: String,
    pub style: String,
    pub temper: String,
    pub creator_id: PlayerId,
    pub banner_url: Option<String>,
    pub passive_script: String,
    pub created_at: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum MercenaryError {
    #[error("invalid mercenary: {0}")]
    Invalid(String),
    #[error("mercenary storage error: {0}")]
    Storage(#[from] ObjectStorageError),
    #[error("mercenary database error: {0}")]
    Database(#[from] mongodb::error::Error),
    #[error("mercenary script failed: {0}")]
    Script(String),
    #[error("mercenary forbidden: {0}")]
    Forbidden(String),
}

impl MercenariesService {
    pub fn new(
        mercenaries: MercenariesRepository,
        storage: ObjectStorage,
        users: UsersRepository,
    ) -> Self {
        Self {
            mercenaries,
            storage,
            users,
        }
    }

    pub async fn load_mercenary_registry(&self) -> Result<usize, MercenaryError> {
        let mercenaries = self.mercenaries.active_mercenaries().await?;
        let mut definitions = Vec::with_capacity(mercenaries.len());

        for mercenary in mercenaries {
            let script = self
                .storage
                .get_bytes(&mercenary.passive_script_object_key)
                .await?;
            let script = String::from_utf8(script)
                .map_err(|error| MercenaryError::Script(error.to_string()))?;

            definitions.push(mercenary_definition_input(&mercenary, script));
        }

        let count = definitions.len();
        fodinha_power::replace_mercenary_definitions(definitions)
            .map_err(|error| MercenaryError::Script(error.to_string()))?;

        Ok(count)
    }

    pub async fn list_mercenaries(&self) -> Result<Vec<MercenaryResponse>, MercenaryError> {
        let mercenaries = self.mercenaries.active_mercenaries().await?;

        self.responses(mercenaries).await
    }

    pub async fn create_mercenary(
        &self,
        creator_id: PlayerId,
        input: UpsertMercenaryInput,
    ) -> Result<MercenaryResponse, MercenaryError> {
        self.ensure_admin(&creator_id, "only admin users can create mercenaries")
            .await?;
        validate_mercenary_id(&input.mercenary_id)?;
        let normalized = normalize_input(input)?;
        let banner = normalized
            .banner
            .ok_or_else(|| MercenaryError::Invalid("banner image is required".to_string()))?;
        let passive_script = normalized
            .passive_script
            .ok_or_else(|| MercenaryError::Invalid("passive lua script is required".to_string()))?;
        let script = String::from_utf8(passive_script)
            .map_err(|error| MercenaryError::Script(error.to_string()))?;
        let banner_object_key = mercenary_banner_object_key(&normalized.mercenary_id);
        let script_object_key = mercenary_passive_script_object_key(&normalized.mercenary_id);

        power_lua::validate_mercenary_passive_script(&script, &script_object_key)
            .map_err(|error| MercenaryError::Script(error.to_string()))?;

        tokio::try_join!(
            self.storage.put(
                &script_object_key,
                script.clone().into_bytes(),
                SCRIPT_OBJECT_CONTENT_TYPE,
            ),
            self.storage
                .put(&banner_object_key, banner, IMAGE_OBJECT_CONTENT_TYPE),
        )?;

        let mercenary = MercenaryDto::new(NewMercenary {
            mercenary_id: normalized.mercenary_id,
            name: normalized.name,
            subtitle: normalized.subtitle,
            description: normalized.description,
            deck: normalized.deck,
            style: normalized.style,
            temper: normalized.temper,
            creator_id,
            banner_object_key: Some(banner_object_key),
            passive_script_object_key: script_object_key,
            banner_content_type: Some(IMAGE_OBJECT_CONTENT_TYPE.to_string()),
        });

        self.mercenaries.insert(mercenary.clone()).await?;
        fodinha_power::upsert_mercenary_definition(mercenary_definition_input(
            &mercenary,
            script.clone(),
        ))
        .map_err(|error| MercenaryError::Script(error.to_string()))?;

        self.response(mercenary, script)
    }

    pub async fn update_mercenary(
        &self,
        editor_id: PlayerId,
        mercenary_id: MercenaryId,
        input: UpsertMercenaryInput,
    ) -> Result<MercenaryResponse, MercenaryError> {
        self.ensure_admin(&editor_id, "only admin users can edit mercenaries")
            .await?;
        if input.mercenary_id != mercenary_id {
            return Err(MercenaryError::Invalid(
                "mercenary id cannot be changed".to_string(),
            ));
        }

        let mut mercenary = self
            .mercenaries
            .active_mercenary_by_id(&mercenary_id)
            .await?
            .ok_or_else(|| MercenaryError::Invalid("mercenary not found".to_string()))?;
        let normalized = normalize_input(input)?;
        let banner_object_key = mercenary
            .banner_object_key
            .clone()
            .unwrap_or_else(|| mercenary_banner_object_key(&mercenary_id));
        let script_object_key = mercenary.passive_script_object_key.clone();

        let script = match normalized.passive_script {
            Some(script) => String::from_utf8(script)
                .map_err(|error| MercenaryError::Script(error.to_string()))?,
            None => {
                let script = self.storage.get_bytes(&script_object_key).await?;

                String::from_utf8(script)
                    .map_err(|error| MercenaryError::Script(error.to_string()))?
            }
        };

        power_lua::validate_mercenary_passive_script(&script, &script_object_key)
            .map_err(|error| MercenaryError::Script(error.to_string()))?;

        if let Some(banner) = normalized.banner {
            if banner.is_empty() {
                return Err(MercenaryError::Invalid(
                    "banner image is required".to_string(),
                ));
            }

            self.storage
                .put(&banner_object_key, banner, IMAGE_OBJECT_CONTENT_TYPE)
                .await?;
        }

        self.storage
            .put(
                &script_object_key,
                script.clone().into_bytes(),
                SCRIPT_OBJECT_CONTENT_TYPE,
            )
            .await?;

        mercenary.name = normalized.name;
        mercenary.subtitle = normalized.subtitle;
        mercenary.description = normalized.description;
        mercenary.deck = normalized.deck;
        mercenary.style = normalized.style;
        mercenary.temper = normalized.temper;
        mercenary.banner_object_key = Some(banner_object_key);
        mercenary.banner_content_type = Some(IMAGE_OBJECT_CONTENT_TYPE.to_string());
        mercenary.updated_at = Utc::now().timestamp();

        self.mercenaries.replace(mercenary.clone()).await?;
        fodinha_power::upsert_mercenary_definition(mercenary_definition_input(
            &mercenary,
            script.clone(),
        ))
        .map_err(|error| MercenaryError::Script(error.to_string()))?;

        self.response(mercenary, script)
    }

    async fn responses(
        &self,
        mercenaries: Vec<MercenaryDto>,
    ) -> Result<Vec<MercenaryResponse>, MercenaryError> {
        let mut responses = Vec::with_capacity(mercenaries.len());

        for mercenary in mercenaries {
            let script = self
                .storage
                .get_bytes(&mercenary.passive_script_object_key)
                .await?;
            let script = String::from_utf8(script)
                .map_err(|error| MercenaryError::Script(error.to_string()))?;

            responses.push(self.response(mercenary, script)?);
        }

        Ok(responses)
    }

    fn response(
        &self,
        mercenary: MercenaryDto,
        script: String,
    ) -> Result<MercenaryResponse, MercenaryError> {
        Ok(MercenaryResponse {
            id: mercenary.mercenary_id,
            name: mercenary.name,
            subtitle: mercenary.subtitle,
            description: mercenary.description,
            deck: mercenary.deck,
            style: mercenary.style,
            temper: mercenary.temper,
            creator_id: mercenary.creator_id,
            banner_url: mercenary
                .banner_object_key
                .as_deref()
                .and_then(|key| self.storage.public_url(key)),
            passive_script: script,
            created_at: mercenary.created_at,
        })
    }

    async fn ensure_admin(
        &self,
        creator_id: &PlayerId,
        message: &str,
    ) -> Result<(), MercenaryError> {
        let user = self.users.user(creator_id.as_str()).await?;

        if user.as_ref().is_some_and(UserClaims::is_admin) {
            return Ok(());
        }

        Err(MercenaryError::Forbidden(message.to_string()))
    }
}

fn mercenary_definition_input(
    mercenary: &MercenaryDto,
    script: String,
) -> fodinha_power::MercenaryDefinitionInput {
    fodinha_power::MercenaryDefinitionInput {
        id: mercenary.mercenary_id.clone(),
        name: mercenary.name.clone(),
        passive_script: script,
        passive_source: mercenary.passive_script_object_key.clone(),
    }
}

fn normalize_input(input: UpsertMercenaryInput) -> Result<UpsertMercenaryInput, MercenaryError> {
    let name = input.name.trim().to_string();
    let subtitle = input.subtitle.trim().to_string();
    let description = input.description.trim().to_string();
    let deck = input.deck.trim().to_string();
    let style = input.style.trim().to_string();
    let temper = input.temper.trim().to_string();

    if name.is_empty() {
        return Err(MercenaryError::Invalid("name is required".to_string()));
    }

    if subtitle.is_empty() {
        return Err(MercenaryError::Invalid("subtitle is required".to_string()));
    }

    Ok(UpsertMercenaryInput {
        mercenary_id: input.mercenary_id,
        name,
        subtitle,
        description,
        deck,
        style,
        temper,
        banner: input.banner,
        passive_script: input.passive_script,
    })
}

fn validate_mercenary_id(mercenary_id: &MercenaryId) -> Result<(), MercenaryError> {
    let value = mercenary_id.as_str();

    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(MercenaryError::Invalid(
            "mercenary id must use letters, numbers, dash, or underscore".to_string(),
        ));
    }

    Ok(())
}

fn mercenary_banner_object_key(mercenary_id: &MercenaryId) -> String {
    format!("mercenaries/{}/banner.png", mercenary_id.as_str())
}

fn mercenary_passive_script_object_key(mercenary_id: &MercenaryId) -> String {
    format!("mercenaries/{}/passive.lua", mercenary_id.as_str())
}
