use std::collections::{HashMap, HashSet};

use chrono::Utc;

use crate::{
    infra::UserClaims,
    models::{
        game::{
            fodinha_power::{
                self, PowerCardDefinitionInput, PowerCardType, PowerDeckDefinitionInput,
            },
            power_lua,
        },
        id::{CardId, DeckId, PlayerId, gen_cardid, gen_deckid},
    },
    services::{
        object_storage::{ObjectStorage, ObjectStorageError},
        repositories::{
            card_decks::{
                CardDeckDto, CardDeckKind, CardDeckStatus, CardDecksRepository, NewCardDeck,
            },
            card_definitions::{
                CardDefinitionAssetDto, CardDefinitionAssetStatus, CardDefinitionDto,
                CardDefinitionKind, CardDefinitionsRepository, NewCardDefinition,
            },
            mercenaries::MercenariesRepository,
            users::UsersRepository,
        },
    },
};

const IMAGE_OBJECT_CONTENT_TYPE: &str = "image/png";
const SCRIPT_OBJECT_CONTENT_TYPE: &str = "text/x-lua";

#[derive(Clone)]
pub struct CardDefinitionsService {
    cards: CardDefinitionsRepository,
    decks: CardDecksRepository,
    mercenaries: MercenariesRepository,
    storage: ObjectStorage,
    users: UsersRepository,
    power_card_registry: fodinha_power::PowerCardRegistryStore,
}

#[derive(Debug)]
pub struct CreateCardDefinitionInput {
    pub kind: CardDefinitionKind,
    pub name: String,
    pub description: String,
    pub image: Vec<u8>,
    pub script: Vec<u8>,
}

#[derive(Debug)]
pub struct CreateCardDefinitionAssetInput {
    pub image: Vec<u8>,
    pub script: Vec<u8>,
}

#[derive(Debug)]
pub struct CreateCardDefinitionFromAssetInput {
    pub asset_id: CardId,
    pub kind: CardDefinitionKind,
    pub name: String,
    pub description: String,
}

#[derive(Debug)]
pub struct UpdateCardDefinitionInput {
    pub kind: Option<CardDefinitionKind>,
    pub name: String,
    pub description: String,
    pub image: Option<Vec<u8>>,
    pub script: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct CreatePowerDeckInput {
    pub kind: CardDeckKind,
    pub name: String,
    pub description: String,
    pub card_ids: Vec<CardId>,
    pub generic_card_ids: Vec<CardId>,
    pub mercenary_card_ids: HashMap<crate::models::id::MercenaryId, Vec<CardId>>,
    pub status: Option<CardDeckStatus>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CardDefinitionResponse {
    pub id: CardId,
    pub kind: CardDefinitionKind,
    pub name: String,
    pub description: String,
    pub life: Option<i32>,
    pub mana_cost: usize,
    pub quantity: usize,
    #[serde(rename = "type")]
    pub card_type: PowerCardType,
    pub creator_id: PlayerId,
    pub image_url: Option<String>,
    pub script: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CardDefinitionAssetResponse {
    pub asset_id: CardId,
    pub image_url: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PowerDeckResponse {
    pub id: DeckId,
    pub kind: CardDeckKind,
    pub status: CardDeckStatus,
    pub name: String,
    pub description: String,
    pub creator_id: PlayerId,
    pub card_ids: Vec<CardId>,
    pub generic_card_ids: Vec<CardId>,
    pub mercenary_card_ids: HashMap<crate::models::id::MercenaryId, Vec<CardId>>,
    pub validation_errors: Vec<String>,
    pub card_count: usize,
    pub cards: Vec<CardDefinitionResponse>,
    pub created_at: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum CardDefinitionError {
    #[error("invalid card definition: {0}")]
    Invalid(String),
    #[error("card definition storage error: {0}")]
    Storage(#[from] ObjectStorageError),
    #[error("card definition database error: {0}")]
    Database(#[from] mongodb::error::Error),
    #[error("card definition script failed: {0}")]
    Script(String),
    #[error("card definition forbidden: {0}")]
    Forbidden(String),
    #[error("card definitions failed: {0}")]
    Definitions(#[from] fodinha_power::PowerCardDefinitionError),
}

impl CardDefinitionsService {
    pub fn new(
        cards: CardDefinitionsRepository,
        decks: CardDecksRepository,
        mercenaries: MercenariesRepository,
        storage: ObjectStorage,
        users: UsersRepository,
        power_card_registry: fodinha_power::PowerCardRegistryStore,
    ) -> Self {
        Self {
            cards,
            decks,
            mercenaries,
            storage,
            users,
            power_card_registry,
        }
    }

    pub async fn load_power_card_registry(&self) -> Result<usize, CardDefinitionError> {
        let cards = self.cards.active_cards().await?;
        let decks = self.decks.active_playable_decks().await?;
        let mut definitions = Vec::new();

        for card in cards {
            let script = self.storage.get_bytes(&card.script_object_key).await?;
            let script = String::from_utf8(script)
                .map_err(|error| CardDefinitionError::Script(error.to_string()))?;

            definitions.push(self.definition_input(&card, script)?);
        }

        let count = definitions.len();
        let mut deck_definitions = Vec::new();

        for deck in decks {
            let validation_errors =
                if deck.generic_card_ids.is_empty() && deck.mercenary_card_ids.is_empty() {
                    Vec::new()
                } else {
                    self.deck_validation_errors().await?
                };

            if validation_errors.is_empty() {
                deck_definitions.push(PowerDeckDefinitionInput {
                    id: deck.deck_id,
                    card_ids: deck.card_ids,
                    generic_card_ids: deck.generic_card_ids,
                    mercenary_card_ids: deck.mercenary_card_ids,
                });
            }
        }

        self.power_card_registry
            .replace_power_card_registry(definitions, deck_definitions)?;

        Ok(count)
    }

    pub async fn power_deck_exists(&self, deck_id: &DeckId) -> mongodb::error::Result<bool> {
        let decks = self.decks.active_playable_decks().await?;

        for deck in decks {
            if &deck.deck_id != deck_id {
                continue;
            }

            if deck.generic_card_ids.is_empty() && deck.mercenary_card_ids.is_empty() {
                return Ok(true);
            }

            let validation_errors = match self.deck_validation_errors().await {
                Ok(validation_errors) => validation_errors,
                Err(CardDefinitionError::Database(error)) => return Err(error),
                Err(_) => return Ok(false),
            };

            return Ok(validation_errors.is_empty());
        }

        Ok(false)
    }

    pub async fn create_card(
        &self,
        creator_id: PlayerId,
        input: CreateCardDefinitionInput,
    ) -> Result<CardDefinitionResponse, CardDefinitionError> {
        self.ensure_can_create_card_kind(&creator_id, input.kind)
            .await?;

        let name = input.name.trim();
        let description = input.description.trim();

        if name.is_empty() {
            return Err(CardDefinitionError::Invalid("name is required".to_string()));
        }

        if description.is_empty() {
            return Err(CardDefinitionError::Invalid(
                "description is required".to_string(),
            ));
        }

        if input.image.is_empty() {
            return Err(CardDefinitionError::Invalid(
                "image is required".to_string(),
            ));
        }

        if input.script.is_empty() {
            return Err(CardDefinitionError::Invalid(
                "lua script is required".to_string(),
            ));
        }

        let script = String::from_utf8(input.script)
            .map_err(|error| CardDefinitionError::Script(error.to_string()))?;
        let card_id = gen_cardid();
        let image_object_key = card_image_object_key(&card_id);
        let script_object_key = card_script_object_key(&card_id);

        let script_definition =
            power_lua::parse_power_card_script_definition(&script, &script_object_key)
                .map_err(|error| CardDefinitionError::Script(error.to_string()))?;

        let definition = PowerCardDefinitionInput {
            id: card_id.clone(),
            name: name.to_string(),
            description: description.to_string(),
            mana_cost: script_definition.mana_cost,
            card_type: script_definition.card_type,
            quantity: script_definition.quantity,
            image_url: self.storage.public_url(&image_object_key),
            script: script.clone(),
            source: script_object_key.clone(),
        };

        tokio::try_join!(
            self.storage.put(
                &script_object_key,
                script.clone().into_bytes(),
                SCRIPT_OBJECT_CONTENT_TYPE,
            ),
            self.storage
                .put(&image_object_key, input.image, IMAGE_OBJECT_CONTENT_TYPE),
        )?;

        let card = CardDefinitionDto::new(NewCardDefinition {
            card_id: card_id.clone(),
            kind: input.kind,
            name: name.to_string(),
            description: description.to_string(),
            life: None,
            mana_cost: script_definition.mana_cost,
            card_type: script_definition.card_type,
            creator_id: creator_id.clone(),
            image_object_key: Some(image_object_key),
            script_object_key,
            image_content_type: Some(IMAGE_OBJECT_CONTENT_TYPE.to_string()),
        });

        self.cards.insert(card.clone()).await?;
        self.power_card_registry
            .upsert_power_card_definition(definition)?;

        self.card_response(card, script)
    }

    pub async fn create_card_asset(
        &self,
        creator_id: PlayerId,
        input: CreateCardDefinitionAssetInput,
    ) -> Result<CardDefinitionAssetResponse, CardDefinitionError> {
        if input.image.is_empty() {
            return Err(CardDefinitionError::Invalid(
                "image is required".to_string(),
            ));
        }

        if input.script.is_empty() {
            return Err(CardDefinitionError::Invalid(
                "lua script is required".to_string(),
            ));
        }

        let asset_id = gen_cardid();
        let image_object_key = card_image_object_key(&asset_id);
        let script_object_key = card_script_object_key(&asset_id);
        let script = String::from_utf8(input.script)
            .map_err(|error| CardDefinitionError::Script(error.to_string()))?;

        power_lua::parse_power_card_script_definition(&script, &script_object_key)
            .map_err(|error| CardDefinitionError::Script(error.to_string()))?;

        tokio::try_join!(
            self.storage.put(
                &script_object_key,
                script.into_bytes(),
                SCRIPT_OBJECT_CONTENT_TYPE,
            ),
            self.storage
                .put(&image_object_key, input.image, IMAGE_OBJECT_CONTENT_TYPE),
        )?;

        self.cards
            .insert_asset(CardDefinitionAssetDto {
                asset_id: asset_id.clone(),
                creator_id,
                image_object_key: image_object_key.clone(),
                script_object_key: script_object_key.clone(),
                status: CardDefinitionAssetStatus::Pending,
                created_at: Utc::now().timestamp(),
            })
            .await?;

        Ok(CardDefinitionAssetResponse {
            asset_id,
            image_url: self.storage.public_url(&image_object_key),
        })
    }

    pub async fn create_card_from_asset(
        &self,
        creator_id: PlayerId,
        input: CreateCardDefinitionFromAssetInput,
    ) -> Result<CardDefinitionResponse, CardDefinitionError> {
        self.ensure_can_create_card_kind(&creator_id, input.kind)
            .await?;

        let name = input.name.trim();
        let description = input.description.trim();

        let asset = self
            .cards
            .pending_asset_by_id(&input.asset_id)
            .await?
            .ok_or_else(|| {
                CardDefinitionError::Invalid("card asset not found or expired".to_string())
            })?;

        if asset.creator_id != creator_id {
            return Err(CardDefinitionError::Forbidden(
                "only the asset creator can use this asset".to_string(),
            ));
        }

        if name.is_empty() {
            return Err(CardDefinitionError::Invalid("name is required".to_string()));
        }

        if description.is_empty() {
            return Err(CardDefinitionError::Invalid(
                "description is required".to_string(),
            ));
        }

        let card_id = input.asset_id;
        let image_object_key = asset.image_object_key;
        let script_object_key = asset.script_object_key;
        let script = self.storage.get_bytes(&script_object_key).await?;
        let script = String::from_utf8(script)
            .map_err(|error| CardDefinitionError::Script(error.to_string()))?;

        let script_definition =
            power_lua::parse_power_card_script_definition(&script, &script_object_key)
                .map_err(|error| CardDefinitionError::Script(error.to_string()))?;

        let definition = PowerCardDefinitionInput {
            id: card_id.clone(),
            name: name.to_string(),
            description: description.to_string(),
            mana_cost: script_definition.mana_cost,
            card_type: script_definition.card_type,
            quantity: script_definition.quantity,
            image_url: self.storage.public_url(&image_object_key),
            script: script.clone(),
            source: script_object_key.clone(),
        };

        let card = CardDefinitionDto::new(NewCardDefinition {
            card_id: card_id.clone(),
            kind: input.kind,
            name: name.to_string(),
            description: description.to_string(),
            life: None,
            mana_cost: script_definition.mana_cost,
            card_type: script_definition.card_type,
            creator_id: creator_id.clone(),
            image_object_key: Some(image_object_key),
            script_object_key,
            image_content_type: Some(IMAGE_OBJECT_CONTENT_TYPE.to_string()),
        });

        self.cards.insert(card.clone()).await?;
        self.cards.delete_asset(&card_id).await?;
        self.power_card_registry
            .upsert_power_card_definition(definition)?;

        self.card_response(card, script)
    }

    pub async fn cleanup_expired_assets(&self, before: i64) -> Result<usize, CardDefinitionError> {
        let assets = self.cards.expired_pending_assets(before).await?;
        let mut deleted = 0;

        for asset in assets {
            tokio::try_join!(
                self.storage.delete(&asset.image_object_key),
                self.storage.delete(&asset.script_object_key),
            )?;
            self.cards.delete_asset(&asset.asset_id).await?;
            deleted += 1;
        }

        Ok(deleted)
    }

    pub async fn update_card(
        &self,
        editor_id: PlayerId,
        card_id: CardId,
        input: UpdateCardDefinitionInput,
    ) -> Result<CardDefinitionResponse, CardDefinitionError> {
        let mut card = self
            .cards
            .active_card_by_id(&card_id)
            .await?
            .ok_or_else(|| CardDefinitionError::Invalid("card not found".to_string()))?;

        if card.creator_id != editor_id {
            return Err(CardDefinitionError::Forbidden(
                "only the card creator can edit this card".to_string(),
            ));
        }

        let kind = input.kind.unwrap_or(card.kind);

        if card.kind == CardDefinitionKind::Official || kind == CardDefinitionKind::Official {
            self.ensure_admin(&editor_id, "only admin users can edit official cards")
                .await?;
        }

        let name = input.name.trim();
        let description = input.description.trim();

        if name.is_empty() {
            return Err(CardDefinitionError::Invalid("name is required".to_string()));
        }

        if description.is_empty() {
            return Err(CardDefinitionError::Invalid(
                "description is required".to_string(),
            ));
        }

        let image_object_key = card
            .image_object_key
            .clone()
            .unwrap_or_else(|| card_image_object_key(&card_id));
        let script_object_key = card.script_object_key.clone();
        let script = match input.script {
            Some(script) => {
                if script.is_empty() {
                    return Err(CardDefinitionError::Invalid(
                        "lua script is required".to_string(),
                    ));
                }

                String::from_utf8(script)
                    .map_err(|error| CardDefinitionError::Script(error.to_string()))?
            }
            None => {
                let script = self.storage.get_bytes(&script_object_key).await?;

                String::from_utf8(script)
                    .map_err(|error| CardDefinitionError::Script(error.to_string()))?
            }
        };

        let script_definition =
            power_lua::parse_power_card_script_definition(&script, &script_object_key)
                .map_err(|error| CardDefinitionError::Script(error.to_string()))?;

        if let Some(image) = input.image {
            if image.is_empty() {
                return Err(CardDefinitionError::Invalid(
                    "image is required".to_string(),
                ));
            }

            self.storage
                .put(&image_object_key, image, IMAGE_OBJECT_CONTENT_TYPE)
                .await?;
        }

        self.storage
            .put(
                &script_object_key,
                script.clone().into_bytes(),
                SCRIPT_OBJECT_CONTENT_TYPE,
            )
            .await?;

        card.kind = kind;
        card.name = name.to_string();
        card.description = description.to_string();
        card.life = None;
        card.mana_cost = script_definition.mana_cost;
        card.card_type = script_definition.card_type;
        card.image_object_key = Some(image_object_key);
        card.image_content_type = Some(IMAGE_OBJECT_CONTENT_TYPE.to_string());
        card.updated_at = Utc::now().timestamp();

        let definition = self.definition_input(&card, script.clone())?;

        self.cards.replace(card.clone()).await?;
        self.power_card_registry
            .upsert_power_card_definition(definition)?;

        self.card_response(card, script)
    }

    pub async fn create_deck(
        &self,
        creator_id: PlayerId,
        input: CreatePowerDeckInput,
    ) -> Result<PowerDeckResponse, CardDefinitionError> {
        self.ensure_can_create_deck_kind(&creator_id, input.kind)
            .await?;

        let name = input.name.trim();
        let description = input.description.trim();
        let generic_card_ids = unique_card_ids(input.generic_card_ids);
        let mercenary_card_ids = input
            .mercenary_card_ids
            .into_iter()
            .map(|(mercenary_id, card_ids)| (mercenary_id, unique_card_ids(card_ids)))
            .filter(|(_, card_ids)| !card_ids.is_empty())
            .collect::<HashMap<_, _>>();
        let is_partitioned = !generic_card_ids.is_empty() || !mercenary_card_ids.is_empty();
        let card_ids = if is_partitioned {
            generic_card_ids
                .iter()
                .cloned()
                .chain(
                    mercenary_card_ids
                        .values()
                        .flat_map(|card_ids| card_ids.iter().cloned()),
                )
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            unique_card_ids(input.card_ids)
        };

        if name.is_empty() {
            return Err(CardDefinitionError::Invalid("name is required".to_string()));
        }

        let active_cards = self.cards.active_cards_by_ids(&card_ids).await?;

        if active_cards.len() != card_ids.len() {
            return Err(CardDefinitionError::Invalid(
                "deck contains invalid cards".to_string(),
            ));
        }

        let validation_errors = if is_partitioned {
            self.deck_validation_errors().await?
        } else {
            Vec::new()
        };
        let status = if input.status == Some(CardDeckStatus::Draft) || !validation_errors.is_empty()
        {
            CardDeckStatus::Draft
        } else {
            CardDeckStatus::Valid
        };

        let deck = CardDeckDto::new(NewCardDeck {
            deck_id: gen_deckid(),
            kind: input.kind,
            name: name.to_string(),
            description: description.to_string(),
            creator_id: creator_id.clone(),
            card_ids,
            generic_card_ids,
            mercenary_card_ids,
            status,
        });

        self.decks.insert(deck.clone()).await?;

        if deck.status == CardDeckStatus::Valid {
            self.power_card_registry
                .upsert_power_deck_definition(PowerDeckDefinitionInput {
                    id: deck.deck_id.clone(),
                    card_ids: deck.card_ids.clone(),
                    generic_card_ids: deck.generic_card_ids.clone(),
                    mercenary_card_ids: deck.mercenary_card_ids.clone(),
                });
        }

        let mut decks = self.hydrate_decks(vec![deck], Some(&creator_id)).await?;

        Ok(decks.remove(0))
    }

    pub async fn list_cards(&self) -> Result<Vec<CardDefinitionResponse>, CardDefinitionError> {
        let cards = self.cards.active_cards().await?;

        self.card_responses(cards).await
    }

    pub async fn list_decks(
        &self,
        viewer_id: &PlayerId,
    ) -> Result<Vec<PowerDeckResponse>, CardDefinitionError> {
        let decks = self.decks.active_decks().await?;

        self.hydrate_decks(decks, Some(viewer_id)).await
    }

    async fn hydrate_decks(
        &self,
        decks: Vec<CardDeckDto>,
        viewer_id: Option<&PlayerId>,
    ) -> Result<Vec<PowerDeckResponse>, CardDefinitionError> {
        let decks = decks
            .into_iter()
            .filter(|deck| {
                deck.status == CardDeckStatus::Valid
                    || viewer_id.is_some_and(|viewer_id| deck.creator_id == *viewer_id)
            })
            .collect::<Vec<_>>();
        let card_ids = decks
            .iter()
            .flat_map(|deck| deck.card_ids.iter().cloned())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let cards = self.cards.active_cards_by_ids(&card_ids).await?;
        let cards_by_id = cards
            .iter()
            .cloned()
            .map(|card| (card.card_id.clone(), card))
            .collect::<HashMap<_, _>>();
        let deck_cards = decks
            .into_iter()
            .map(|deck| {
                let cards = deck
                    .card_ids
                    .iter()
                    .filter_map(|card_id| cards_by_id.get(card_id).cloned())
                    .collect::<Vec<_>>();

                (deck, cards)
            })
            .collect::<Vec<_>>();

        let mut responses = Vec::with_capacity(deck_cards.len());

        for (deck, cards) in deck_cards {
            let card_responses = self.card_responses(cards).await?;
            let validation_errors =
                if deck.generic_card_ids.is_empty() && deck.mercenary_card_ids.is_empty() {
                    Vec::new()
                } else {
                    self.deck_validation_errors().await?
                };

            responses.push(PowerDeckResponse {
                id: deck.deck_id,
                kind: deck.kind,
                status: deck.status,
                name: deck.name,
                description: deck.description,
                creator_id: deck.creator_id.clone(),
                card_ids: deck.card_ids,
                generic_card_ids: deck.generic_card_ids,
                mercenary_card_ids: deck.mercenary_card_ids,
                validation_errors,
                card_count: card_responses.len(),
                cards: card_responses,
                created_at: deck.created_at,
            });
        }

        responses.sort_by_key(|deck| std::cmp::Reverse(deck.created_at));

        Ok(responses)
    }

    async fn card_responses(
        &self,
        cards: Vec<CardDefinitionDto>,
    ) -> Result<Vec<CardDefinitionResponse>, CardDefinitionError> {
        let mut responses = Vec::with_capacity(cards.len());

        for card in cards {
            let script = self.storage.get_bytes(&card.script_object_key).await?;
            let script = String::from_utf8(script)
                .map_err(|error| CardDefinitionError::Script(error.to_string()))?;

            responses.push(self.card_response(card, script)?);
        }

        Ok(responses)
    }

    fn card_response(
        &self,
        card: CardDefinitionDto,
        script: String,
    ) -> Result<CardDefinitionResponse, CardDefinitionError> {
        let script_definition =
            power_lua::parse_power_card_script_definition(&script, &card.script_object_key)
                .map_err(|error| CardDefinitionError::Script(error.to_string()))?;

        Ok(CardDefinitionResponse {
            id: card.card_id,
            kind: card.kind,
            name: card.name,
            description: card.description,
            life: card.life,
            mana_cost: script_definition.mana_cost,
            quantity: script_definition.quantity,
            card_type: script_definition.card_type,
            creator_id: card.creator_id.clone(),
            image_url: card
                .image_object_key
                .as_deref()
                .and_then(|key| self.storage.public_url(key)),
            script,
            created_at: card.created_at,
        })
    }

    async fn deck_validation_errors(&self) -> Result<Vec<String>, CardDefinitionError> {
        let mercenaries = self.mercenaries.active_mercenaries().await?;
        let mut errors = Vec::new();

        if mercenaries.is_empty() {
            errors.push("create at least one mercenary before validating a deck".to_string());
        }

        Ok(errors)
    }

    async fn ensure_can_create_card_kind(
        &self,
        creator_id: &PlayerId,
        kind: CardDefinitionKind,
    ) -> Result<(), CardDefinitionError> {
        if kind == CardDefinitionKind::Official {
            self.ensure_admin(creator_id, "only admin users can create official cards")
                .await?;
        }

        Ok(())
    }

    async fn ensure_can_create_deck_kind(
        &self,
        creator_id: &PlayerId,
        kind: CardDeckKind,
    ) -> Result<(), CardDefinitionError> {
        if kind == CardDeckKind::Official {
            self.ensure_admin(creator_id, "only admin users can create official decks")
                .await?;
        }

        Ok(())
    }

    async fn ensure_admin(
        &self,
        creator_id: &PlayerId,
        message: &str,
    ) -> Result<(), CardDefinitionError> {
        let user = self.users.user(creator_id.as_str()).await?;

        if user.as_ref().is_some_and(UserClaims::is_admin) {
            return Ok(());
        }

        Err(CardDefinitionError::Forbidden(message.to_string()))
    }

    fn definition_input(
        &self,
        card: &CardDefinitionDto,
        script: String,
    ) -> Result<PowerCardDefinitionInput, CardDefinitionError> {
        let script_definition =
            power_lua::parse_power_card_script_definition(&script, &card.script_object_key)
                .map_err(|error| CardDefinitionError::Script(error.to_string()))?;

        Ok(PowerCardDefinitionInput {
            id: card.card_id.clone(),
            name: card.name.clone(),
            description: card.description.clone(),
            mana_cost: script_definition.mana_cost,
            card_type: script_definition.card_type,
            quantity: script_definition.quantity,
            image_url: card
                .image_object_key
                .as_deref()
                .and_then(|key| self.storage.public_url(key)),
            script,
            source: card.script_object_key.clone(),
        })
    }
}

fn unique_card_ids(card_ids: Vec<CardId>) -> Vec<CardId> {
    card_ids
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

fn card_image_object_key(card_id: &CardId) -> String {
    format!("card-definitions/{}/card.png", card_id.as_str())
}

fn card_script_object_key(card_id: &CardId) -> String {
    format!("card-definitions/{}/effect.lua", card_id.as_str())
}
