use chrono::Utc;
use futures::TryStreamExt;
use mongodb::{Collection, Database, IndexModel, bson::doc, options::IndexOptions};

use crate::{
    infra::telemetry,
    models::{
        game::fodinha_power::PowerCardType,
        id::{CardId, PlayerId},
    },
};

const COLLECTION_NAME: &str = "CardDefinitions";

#[derive(Clone)]
pub struct CardDefinitionsRepository {
    cards: Collection<CardDefinitionDto>,
}

impl CardDefinitionsRepository {
    pub fn new(database: &Database) -> Self {
        Self {
            cards: database.collection(COLLECTION_NAME),
        }
    }

    pub async fn ensure_indexes(&self) -> mongodb::error::Result<()> {
        telemetry::db_query(COLLECTION_NAME, "create_index.unique_card_id", async {
            self.cards
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "card_id": 1 })
                        .options(IndexOptions::builder().unique(true).build())
                        .build(),
                )
                .await
        })
        .await?;

        telemetry::db_query(COLLECTION_NAME, "create_index.kind_creator_active", async {
            self.cards
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "kind": 1, "creator_id": 1, "active": 1, "created_at": -1 })
                        .build(),
                )
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn insert(&self, card: CardDefinitionDto) -> mongodb::error::Result<()> {
        telemetry::db_query(COLLECTION_NAME, "insert_one", async {
            self.cards.insert_one(card).await
        })
        .await?;

        Ok(())
    }

    pub async fn active_cards(&self) -> mongodb::error::Result<Vec<CardDefinitionDto>> {
        telemetry::db_query(COLLECTION_NAME, "find.active", async {
            let cursor = self
                .cards
                .find(doc! { "active": true })
                .sort(doc! { "created_at": -1 })
                .await?;

            cursor.try_collect().await
        })
        .await
    }

    pub async fn active_cards_by_ids(
        &self,
        card_ids: &[CardId],
    ) -> mongodb::error::Result<Vec<CardDefinitionDto>> {
        let card_ids = card_ids.iter().map(CardId::as_str).collect::<Vec<_>>();

        telemetry::db_query(COLLECTION_NAME, "find.active_by_ids", async {
            let cursor = self
                .cards
                .find(doc! {
                    "active": true,
                    "card_id": { "$in": card_ids },
                })
                .await?;

            cursor.try_collect().await
        })
        .await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardDefinitionKind {
    Official,
    Community,
}

impl CardDefinitionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Official => "official",
            Self::Community => "community",
        }
    }
}

impl std::str::FromStr for CardDefinitionKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "official" => Ok(Self::Official),
            "community" => Ok(Self::Community),
            _ => Err("kind must be official or community".to_string()),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CardDefinitionDto {
    pub card_id: CardId,
    pub kind: CardDefinitionKind,
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub life: Option<i32>,
    #[serde(rename = "type")]
    pub card_type: PowerCardType,
    pub creator_id: PlayerId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_object_key: Option<String>,
    pub script_object_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_content_type: Option<String>,
    pub active: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl CardDefinitionDto {
    pub fn new(input: NewCardDefinition) -> Self {
        let now = Utc::now().timestamp();

        Self {
            card_id: input.card_id,
            kind: input.kind,
            name: input.name,
            description: input.description,
            life: input.life,
            card_type: input.card_type,
            creator_id: input.creator_id,
            image_object_key: input.image_object_key,
            script_object_key: input.script_object_key,
            image_content_type: input.image_content_type,
            active: true,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn creator_id(&self) -> PlayerId {
        self.creator_id.clone()
    }
}

pub struct NewCardDefinition {
    pub card_id: CardId,
    pub kind: CardDefinitionKind,
    pub name: String,
    pub description: String,
    pub life: Option<i32>,
    pub card_type: PowerCardType,
    pub creator_id: PlayerId,
    pub image_object_key: Option<String>,
    pub script_object_key: String,
    pub image_content_type: Option<String>,
}
