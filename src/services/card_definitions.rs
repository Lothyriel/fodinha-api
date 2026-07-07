use std::collections::{HashMap, HashSet};

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
            card_decks::{CardDeckDto, CardDeckKind, CardDecksRepository, NewCardDeck},
            card_definitions::{
                CardDefinitionDto, CardDefinitionKind, CardDefinitionsRepository, NewCardDefinition,
            },
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
    storage: ObjectStorage,
    users: UsersRepository,
}

#[derive(Debug)]
pub struct CreateCardDefinitionInput {
    pub kind: CardDefinitionKind,
    pub name: String,
    pub description: String,
    pub life: Option<i32>,
    pub card_type: PowerCardType,
    pub image: Vec<u8>,
    pub script: Vec<u8>,
}

#[derive(Debug)]
pub struct CreatePowerDeckInput {
    pub kind: CardDeckKind,
    pub name: String,
    pub description: String,
    pub card_ids: Vec<CardId>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CardDefinitionResponse {
    pub id: CardId,
    pub kind: CardDefinitionKind,
    pub name: String,
    pub description: String,
    pub life: Option<i32>,
    #[serde(rename = "type")]
    pub card_type: PowerCardType,
    pub creator_id: PlayerId,
    pub creator: Option<UserClaims>,
    pub image_url: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PowerDeckResponse {
    pub id: DeckId,
    pub kind: CardDeckKind,
    pub name: String,
    pub description: String,
    pub creator_id: PlayerId,
    pub creator: Option<UserClaims>,
    pub card_ids: Vec<CardId>,
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
        storage: ObjectStorage,
        users: UsersRepository,
    ) -> Self {
        Self {
            cards,
            decks,
            storage,
            users,
        }
    }

    pub async fn load_power_card_registry(&self) -> Result<usize, CardDefinitionError> {
        let cards = self.cards.active_cards().await?;
        let decks = self.decks.active_decks().await?;
        let mut definitions = Vec::new();

        for card in cards {
            let script = self.storage.get_bytes(&card.script_object_key).await?;
            let script = String::from_utf8(script)
                .map_err(|error| CardDefinitionError::Script(error.to_string()))?;

            definitions.push(self.definition_input(&card, script));
        }

        let count = definitions.len();
        let decks = decks
            .into_iter()
            .map(|deck| PowerDeckDefinitionInput {
                id: deck.deck_id,
                card_ids: deck.card_ids,
            })
            .collect();

        if count > 0 {
            fodinha_power::replace_power_card_registry(definitions, decks)?;
        }

        Ok(count)
    }

    pub async fn power_deck_exists(&self, deck_id: &DeckId) -> mongodb::error::Result<bool> {
        self.decks.active_deck_exists(deck_id).await
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
        let image_object_key = format!("card-definitions/{}/card.png", card_id.as_str());
        let script_object_key = format!("card-definitions/{}/effect.lua", card_id.as_str());

        power_lua::validate_power_card_script(&script, &script_object_key)
            .map_err(|error| CardDefinitionError::Script(error.to_string()))?;

        let definition = PowerCardDefinitionInput {
            id: card_id.clone(),
            name: name.to_string(),
            description: description.to_string(),
            card_type: input.card_type,
            image_url: self.storage.public_url(&image_object_key),
            script: script.clone(),
            source: script_object_key.clone(),
        };

        self.storage
            .put(
                &script_object_key,
                script.into_bytes(),
                SCRIPT_OBJECT_CONTENT_TYPE,
            )
            .await?;
        self.storage
            .put(&image_object_key, input.image, IMAGE_OBJECT_CONTENT_TYPE)
            .await?;

        let card = CardDefinitionDto::new(NewCardDefinition {
            card_id,
            kind: input.kind,
            name: name.to_string(),
            description: description.to_string(),
            life: input.life,
            card_type: input.card_type,
            creator_id,
            image_object_key: Some(image_object_key),
            script_object_key,
            image_content_type: Some(IMAGE_OBJECT_CONTENT_TYPE.to_string()),
        });

        self.cards.insert(card.clone()).await?;
        fodinha_power::upsert_power_card_definition(definition)?;

        let mut cards = self.hydrate_cards(vec![card]).await?;

        Ok(cards.remove(0))
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
        let card_ids = input
            .card_ids
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        if name.is_empty() {
            return Err(CardDefinitionError::Invalid("name is required".to_string()));
        }

        if card_ids.is_empty() {
            return Err(CardDefinitionError::Invalid(
                "select at least one card".to_string(),
            ));
        }

        let active_cards = self.cards.active_cards_by_ids(&card_ids).await?;

        if active_cards.len() != card_ids.len() {
            return Err(CardDefinitionError::Invalid(
                "deck contains invalid cards".to_string(),
            ));
        }

        let deck = CardDeckDto::new(NewCardDeck {
            deck_id: gen_deckid(),
            kind: input.kind,
            name: name.to_string(),
            description: description.to_string(),
            creator_id,
            card_ids,
        });

        self.decks.insert(deck.clone()).await?;
        fodinha_power::upsert_power_deck_definition(PowerDeckDefinitionInput {
            id: deck.deck_id.clone(),
            card_ids: deck.card_ids.clone(),
        });

        let mut decks = self.hydrate_decks(vec![deck]).await?;

        Ok(decks.remove(0))
    }

    pub async fn list_cards(&self) -> Result<Vec<CardDefinitionResponse>, CardDefinitionError> {
        let cards = self.cards.active_cards().await?;

        self.hydrate_cards(cards).await
    }

    pub async fn list_decks(&self) -> Result<Vec<PowerDeckResponse>, CardDefinitionError> {
        let decks = self.decks.active_decks().await?;

        self.hydrate_decks(decks).await
    }

    async fn hydrate_cards(
        &self,
        cards: Vec<CardDefinitionDto>,
    ) -> Result<Vec<CardDefinitionResponse>, CardDefinitionError> {
        let creator_ids = cards
            .iter()
            .map(|card| card.creator_id.as_str().to_string())
            .collect::<Vec<_>>();
        let creators = self.users.users_by_id(&creator_ids).await?;

        Ok(self.card_responses(cards, &creators))
    }

    async fn hydrate_decks(
        &self,
        decks: Vec<CardDeckDto>,
    ) -> Result<Vec<PowerDeckResponse>, CardDefinitionError> {
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
        let creator_ids = decks
            .iter()
            .map(|deck| deck.creator_id.as_str().to_string())
            .chain(
                cards
                    .iter()
                    .map(|card| card.creator_id.as_str().to_string()),
            )
            .collect::<Vec<_>>();
        let creators = self.users.users_by_id(&creator_ids).await?;

        let mut responses = decks
            .into_iter()
            .map(|deck| {
                let cards = deck
                    .card_ids
                    .iter()
                    .filter_map(|card_id| cards_by_id.get(card_id).cloned())
                    .collect::<Vec<_>>();
                let card_responses = self.card_responses(cards, &creators);

                PowerDeckResponse {
                    id: deck.deck_id,
                    kind: deck.kind,
                    name: deck.name,
                    description: deck.description,
                    creator_id: deck.creator_id.clone(),
                    creator: creators.get(deck.creator_id.as_str()).cloned(),
                    card_ids: deck.card_ids,
                    card_count: card_responses.len(),
                    cards: card_responses,
                    created_at: deck.created_at,
                }
            })
            .collect::<Vec<_>>();

        responses.sort_by_key(|deck| std::cmp::Reverse(deck.created_at));

        Ok(responses)
    }

    fn card_responses(
        &self,
        cards: Vec<CardDefinitionDto>,
        creators: &HashMap<String, UserClaims>,
    ) -> Vec<CardDefinitionResponse> {
        cards
            .into_iter()
            .map(|card| CardDefinitionResponse {
                id: card.card_id,
                kind: card.kind,
                name: card.name,
                description: card.description,
                life: card.life,
                card_type: card.card_type,
                creator_id: card.creator_id.clone(),
                creator: creators.get(card.creator_id.as_str()).cloned(),
                image_url: card
                    .image_object_key
                    .as_deref()
                    .and_then(|key| self.storage.public_url(key)),
                created_at: card.created_at,
            })
            .collect()
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
    ) -> PowerCardDefinitionInput {
        PowerCardDefinitionInput {
            id: card.card_id.clone(),
            name: card.name.clone(),
            description: card.description.clone(),
            card_type: card.card_type,
            image_url: card
                .image_object_key
                .as_deref()
                .and_then(|key| self.storage.public_url(key)),
            script,
            source: card.script_object_key.clone(),
        }
    }
}
