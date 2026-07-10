use chrono::Utc;
use futures::TryStreamExt;
use mongodb::{Collection, Database, IndexModel, bson::doc, options::IndexOptions};

use crate::{
    infra::telemetry,
    models::id::{CardId, DeckId, PlayerId},
};

const COLLECTION_NAME: &str = "CardDeckDefinitions";

#[derive(Clone)]
pub struct CardDecksRepository {
    decks: Collection<CardDeckDto>,
}

impl CardDecksRepository {
    pub fn new(database: &Database) -> Self {
        Self {
            decks: database.collection(COLLECTION_NAME),
        }
    }

    pub async fn ensure_indexes(&self) -> mongodb::error::Result<()> {
        telemetry::db_query(COLLECTION_NAME, "create_index.unique_deck_id", async {
            self.decks
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "deck_id": 1 })
                        .options(IndexOptions::builder().unique(true).build())
                        .build(),
                )
                .await
        })
        .await?;

        telemetry::db_query(COLLECTION_NAME, "create_index.creator_active", async {
            self.decks
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "creator_id": 1, "active": 1, "created_at": -1 })
                        .build(),
                )
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn insert(&self, deck: CardDeckDto) -> mongodb::error::Result<()> {
        telemetry::db_query(COLLECTION_NAME, "insert_one", async {
            self.decks.insert_one(deck).await
        })
        .await?;

        Ok(())
    }

    pub async fn replace(&self, deck: CardDeckDto) -> mongodb::error::Result<()> {
        telemetry::db_query(COLLECTION_NAME, "replace_one.deck_id", async {
            self.decks
                .replace_one(doc! { "deck_id": deck.deck_id.as_str() }, deck)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn active_deck_by_id(
        &self,
        deck_id: &DeckId,
    ) -> mongodb::error::Result<Option<CardDeckDto>> {
        telemetry::db_query(COLLECTION_NAME, "find_one.active_by_id", async {
            self.decks
                .find_one(doc! { "deck_id": deck_id.as_str(), "active": true })
                .await
        })
        .await
    }

    pub async fn active_decks(&self) -> mongodb::error::Result<Vec<CardDeckDto>> {
        telemetry::db_query(COLLECTION_NAME, "find.active", async {
            let cursor = self
                .decks
                .find(doc! { "active": true })
                .sort(doc! { "created_at": -1 })
                .await?;

            cursor.try_collect().await
        })
        .await
    }

    pub async fn active_playable_decks(&self) -> mongodb::error::Result<Vec<CardDeckDto>> {
        telemetry::db_query(COLLECTION_NAME, "find.active_playable", async {
            let cursor = self
                .decks
                .find(doc! {
                    "active": true,
                    "status": CardDeckStatus::Valid.as_str(),
                })
                .sort(doc! { "created_at": -1 })
                .await?;

            cursor.try_collect().await
        })
        .await
    }

    pub async fn active_deck_exists(&self, deck_id: &DeckId) -> mongodb::error::Result<bool> {
        telemetry::db_query(COLLECTION_NAME, "find_one.active_exists", async {
            self.decks
                .find_one(doc! {
                    "deck_id": deck_id.as_str(),
                    "active": true,
                    "status": CardDeckStatus::Valid.as_str(),
                })
                .await
        })
        .await
        .map(|deck| deck.is_some())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CardDeckDto {
    pub deck_id: DeckId,
    pub kind: CardDeckKind,
    pub name: String,
    pub description: String,
    pub creator_id: PlayerId,
    pub card_ids: Vec<CardId>,
    pub generic_card_ids: Vec<CardId>,
    pub mercenary_card_ids: std::collections::HashMap<crate::models::id::MercenaryId, Vec<CardId>>,
    pub status: CardDeckStatus,
    pub active: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl CardDeckDto {
    pub fn new(input: NewCardDeck) -> Self {
        let now = Utc::now().timestamp();

        Self {
            deck_id: input.deck_id,
            kind: input.kind,
            name: input.name,
            description: input.description,
            creator_id: input.creator_id,
            card_ids: input.card_ids,
            generic_card_ids: input.generic_card_ids,
            mercenary_card_ids: input.mercenary_card_ids,
            status: input.status,
            active: true,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn creator_id(&self) -> PlayerId {
        self.creator_id.clone()
    }
}

pub struct NewCardDeck {
    pub deck_id: DeckId,
    pub kind: CardDeckKind,
    pub name: String,
    pub description: String,
    pub creator_id: PlayerId,
    pub card_ids: Vec<CardId>,
    pub generic_card_ids: Vec<CardId>,
    pub mercenary_card_ids: std::collections::HashMap<crate::models::id::MercenaryId, Vec<CardId>>,
    pub status: CardDeckStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardDeckKind {
    Official,
    Community,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardDeckStatus {
    Draft,
    #[default]
    Valid,
}

impl CardDeckStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Valid => "valid",
        }
    }
}

impl CardDeckKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Official => "official",
            Self::Community => "community",
        }
    }
}

impl std::str::FromStr for CardDeckKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "official" => Ok(Self::Official),
            "community" => Ok(Self::Community),
            _ => Err("kind must be official or community".to_string()),
        }
    }
}
