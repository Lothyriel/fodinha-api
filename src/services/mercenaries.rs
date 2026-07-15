use chrono::Utc;
use sha2::{Digest, Sha256};

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
    power_card_registry: fodinha_power::PowerCardRegistryStore,
}

#[derive(Debug)]
pub struct UpsertMercenaryInput {
    pub mercenary_id: MercenaryId,
    pub name: String,
    pub subtitle: String,
    pub description: String,
    pub style: String,
    pub temper: String,
    pub banner: Option<Vec<u8>>,
    pub icon: Option<Vec<u8>>,
    pub passive_script: Option<Vec<u8>>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MercenaryResponse {
    pub id: MercenaryId,
    pub version: i64,
    pub name: String,
    pub subtitle: String,
    pub description: String,
    pub style: String,
    pub temper: String,
    pub creator_id: PlayerId,
    pub banner_url: Option<String>,
    pub icon_url: Option<String>,
    pub base_life: usize,
    pub initial_mana: usize,
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
        power_card_registry: fodinha_power::PowerCardRegistryStore,
    ) -> Self {
        Self {
            mercenaries,
            storage,
            users,
            power_card_registry,
        }
    }

    pub async fn load_mercenary_registry(&self) -> Result<usize, MercenaryError> {
        let mercenaries = self.mercenaries.active_mercenaries().await?;
        let mut definitions = Vec::with_capacity(mercenaries.len());

        for mercenary in mercenaries {
            let script_object_key = mercenary.script_object_key.clone();
            let script = self.storage.get_bytes(&script_object_key).await?;
            let script = String::from_utf8(script)
                .map_err(|error| MercenaryError::Script(error.to_string()))?;

            power_lua::validate_mercenary_passive_script_execution(&script, &script_object_key)
                .map_err(|error| MercenaryError::Script(error.to_string()))?;

            definitions.push(mercenary_definition_input(&mercenary, script)?);
        }

        let count = definitions.len();
        self.power_card_registry
            .replace_mercenary_definitions(definitions)
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
        let icon = normalized
            .icon
            .ok_or_else(|| MercenaryError::Invalid("icon image is required".to_string()))?;
        let passive_script = normalized
            .passive_script
            .ok_or_else(|| MercenaryError::Invalid("passive lua script is required".to_string()))?;
        let script = String::from_utf8(passive_script)
            .map_err(|error| MercenaryError::Script(error.to_string()))?;
        let banner_object_key = mercenary_asset_object_key(&banner, "png");
        let icon_object_key = mercenary_asset_object_key(&icon, "png");
        let script_object_key = mercenary_asset_object_key(script.as_bytes(), "lua");
        power_lua::validate_mercenary_passive_script_execution(&script, &script_object_key)
            .map_err(|error| MercenaryError::Script(error.to_string()))?;

        tokio::try_join!(
            self.storage.put_if_absent(
                &script_object_key,
                script.clone().into_bytes(),
                SCRIPT_OBJECT_CONTENT_TYPE,
            ),
            self.storage
                .put_if_absent(&banner_object_key, banner, IMAGE_OBJECT_CONTENT_TYPE),
            self.storage
                .put_if_absent(&icon_object_key, icon, IMAGE_OBJECT_CONTENT_TYPE),
        )?;

        let mercenary = MercenaryDto::new(NewMercenary {
            mercenary_id: normalized.mercenary_id,
            name: normalized.name,
            subtitle: normalized.subtitle,
            description: normalized.description,
            style: normalized.style,
            temper: normalized.temper,
            creator_id,
            icon_content_type: Some(IMAGE_OBJECT_CONTENT_TYPE.to_string()),
            banner_content_type: Some(IMAGE_OBJECT_CONTENT_TYPE.to_string()),
            banner_object_key,
            icon_object_key,
            script_object_key,
        });

        self.mercenaries.insert(mercenary.clone()).await?;
        self.power_card_registry
            .upsert_mercenary_definition(mercenary_definition_input(&mercenary, script.clone())?)
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
        let previous_version = mercenary.version;
        let normalized = normalize_input(input)?;
        let next_version = previous_version + 1;
        let previous_banner_object_key = mercenary.banner_object_key.clone();
        let previous_icon_object_key = mercenary.icon_object_key.clone();
        let previous_script_object_key = mercenary.script_object_key.clone();

        let script = match normalized.passive_script {
            Some(script) => String::from_utf8(script)
                .map_err(|error| MercenaryError::Script(error.to_string()))?,
            None => {
                let script = self.storage.get_bytes(&previous_script_object_key).await?;

                String::from_utf8(script)
                    .map_err(|error| MercenaryError::Script(error.to_string()))?
            }
        };
        let script_object_key = mercenary_asset_object_key(script.as_bytes(), "lua");

        power_lua::validate_mercenary_passive_script_execution(&script, &script_object_key)
            .map_err(|error| MercenaryError::Script(error.to_string()))?;

        let banner = if let Some(banner) = normalized.banner {
            if banner.is_empty() {
                return Err(MercenaryError::Invalid(
                    "banner image is required".to_string(),
                ));
            }

            banner
        } else {
            self.storage.get_bytes(&previous_banner_object_key).await?
        };
        let banner_object_key = mercenary_asset_object_key(&banner, "png");

        let icon = if let Some(icon) = normalized.icon {
            if icon.is_empty() {
                return Err(MercenaryError::Invalid(
                    "icon image is required".to_string(),
                ));
            }

            mercenary.icon_content_type = Some(IMAGE_OBJECT_CONTENT_TYPE.to_string());
            icon
        } else {
            self.storage.get_bytes(&previous_icon_object_key).await?
        };
        let icon_object_key = mercenary_asset_object_key(&icon, "png");

        tokio::try_join!(
            self.storage.put_if_absent(
                &script_object_key,
                script.clone().into_bytes(),
                SCRIPT_OBJECT_CONTENT_TYPE
            ),
            self.storage
                .put_if_absent(&banner_object_key, banner, IMAGE_OBJECT_CONTENT_TYPE),
            self.storage
                .put_if_absent(&icon_object_key, icon, IMAGE_OBJECT_CONTENT_TYPE),
        )?;

        mercenary.name = normalized.name;
        mercenary.subtitle = normalized.subtitle;
        mercenary.description = normalized.description;
        mercenary.style = normalized.style;
        mercenary.temper = normalized.temper;
        mercenary.banner_content_type = Some(IMAGE_OBJECT_CONTENT_TYPE.to_string());
        mercenary.updated_at = Utc::now().timestamp();
        mercenary.version = next_version;
        mercenary.banner_object_key = banner_object_key;
        mercenary.icon_object_key = icon_object_key;
        mercenary.script_object_key = script_object_key;

        self.mercenaries.insert(mercenary.clone()).await?;
        self.mercenaries
            .deactivate(&mercenary_id, previous_version)
            .await?;
        self.power_card_registry
            .upsert_mercenary_definition(mercenary_definition_input(&mercenary, script.clone())?)
            .map_err(|error| MercenaryError::Script(error.to_string()))?;

        self.response(mercenary, script)
    }

    async fn responses(
        &self,
        mercenaries: Vec<MercenaryDto>,
    ) -> Result<Vec<MercenaryResponse>, MercenaryError> {
        let mut responses = Vec::with_capacity(mercenaries.len());

        for mercenary in mercenaries {
            let script_object_key = mercenary.script_object_key.clone();
            let script = self.storage.get_bytes(&script_object_key).await?;
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
        let banner_object_key = mercenary.banner_object_key.clone();
        let icon_object_key = mercenary.icon_object_key.clone();
        let script_object_key = mercenary.script_object_key.clone();
        let passive_definition =
            power_lua::parse_mercenary_passive_definition(&script, &script_object_key)
                .map_err(|error| MercenaryError::Script(error.to_string()))?;

        Ok(MercenaryResponse {
            id: mercenary.mercenary_id,
            version: mercenary.version,
            name: mercenary.name,
            subtitle: mercenary.subtitle,
            description: mercenary.description,
            style: mercenary.style,
            temper: mercenary.temper,
            creator_id: mercenary.creator_id,
            banner_url: self.storage.public_url(&banner_object_key),
            icon_url: self.storage.public_url(&icon_object_key),
            base_life: passive_definition.base_life,
            initial_mana: passive_definition.initial_mana,
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
) -> Result<fodinha_power::MercenaryDefinitionInput, MercenaryError> {
    let script_object_key = mercenary.script_object_key.clone();
    let passive_definition =
        power_lua::parse_mercenary_passive_definition(&script, &script_object_key)
            .map_err(|error| MercenaryError::Script(error.to_string()))?;

    Ok(fodinha_power::MercenaryDefinitionInput {
        id: mercenary.mercenary_id.clone(),
        version: mercenary.version,
        name: mercenary.name.clone(),
        base_life: passive_definition.base_life,
        initial_mana: passive_definition.initial_mana,
        passive_script: script,
        passive_source: script_object_key,
    })
}

fn normalize_input(input: UpsertMercenaryInput) -> Result<UpsertMercenaryInput, MercenaryError> {
    let name = input.name.trim().to_string();
    let subtitle = input.subtitle.trim().to_string();
    let description = input.description.trim().to_string();
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
        style,
        temper,
        banner: input.banner,
        icon: input.icon,
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

fn mercenary_asset_object_key(bytes: &[u8], extension: &str) -> String {
    let digest = Sha256::digest(bytes);
    format!("mercenary-assets/{digest:x}.{extension}")
}

#[cfg(test)]
mod tests {
    use super::mercenary_asset_object_key;
    #[test]
    fn mercenary_object_keys_are_derived_from_id() {
        assert_eq!(
            mercenary_asset_object_key(b"asset", "png"),
            mercenary_asset_object_key(b"asset", "png")
        );
    }
}
