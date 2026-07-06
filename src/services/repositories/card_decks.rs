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

    pub async fn active_deck_exists(&self, deck_id: &DeckId) -> mongodb::error::Result<bool> {
        telemetry::db_query(COLLECTION_NAME, "find_one.active_exists", async {
            self.decks
                .find_one(doc! { "deck_id": deck_id.as_str(), "active": true })
                .await
        })
        .await
        .map(|deck| deck.is_some())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CardDeckDto {
    pub deck_id: DeckId,
    pub name: String,
    pub description: String,
    pub creator_id: PlayerId,
    pub card_ids: Vec<CardId>,
    #[serde(default = "default_active")]
    pub active: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl CardDeckDto {
    pub fn new(input: NewCardDeck) -> Self {
        let now = Utc::now().timestamp();

        Self {
            deck_id: input.deck_id,
            name: input.name,
            description: input.description,
            creator_id: input.creator_id,
            card_ids: input.card_ids,
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
    pub name: String,
    pub description: String,
    pub creator_id: PlayerId,
    pub card_ids: Vec<CardId>,
}

fn default_active() -> bool {
    true
}
