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
const ASSETS_COLLECTION_NAME: &str = "CardDefinitionAssets";

#[derive(Clone)]
pub struct CardDefinitionsRepository {
    cards: Collection<CardDefinitionDto>,
    assets: Collection<CardDefinitionAssetDto>,
}

impl CardDefinitionsRepository {
    pub fn new(database: &Database) -> Self {
        Self {
            cards: database.collection(COLLECTION_NAME),
            assets: database.collection(ASSETS_COLLECTION_NAME),
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

        telemetry::db_query(
            ASSETS_COLLECTION_NAME,
            "create_index.pending_created_at",
            async {
                self.assets
                    .create_index(
                        IndexModel::builder()
                            .keys(doc! { "status": 1, "created_at": 1 })
                            .build(),
                    )
                    .await
            },
        )
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

    pub async fn replace(&self, card: CardDefinitionDto) -> mongodb::error::Result<()> {
        let card_id = card.card_id.as_str().to_string();

        telemetry::db_query(COLLECTION_NAME, "replace_one.card_id", async {
            self.cards
                .replace_one(doc! { "card_id": card_id }, card)
                .await
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

    pub async fn active_card_by_id(
        &self,
        card_id: &CardId,
    ) -> mongodb::error::Result<Option<CardDefinitionDto>> {
        telemetry::db_query(COLLECTION_NAME, "find_one.active_by_id", async {
            self.cards
                .find_one(doc! { "active": true, "card_id": card_id.as_str() })
                .await
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

    pub async fn insert_asset(&self, asset: CardDefinitionAssetDto) -> mongodb::error::Result<()> {
        telemetry::db_query(ASSETS_COLLECTION_NAME, "insert_one", async {
            self.assets.insert_one(asset).await
        })
        .await?;
        Ok(())
    }

    pub async fn pending_asset_by_id(
        &self,
        asset_id: &CardId,
    ) -> mongodb::error::Result<Option<CardDefinitionAssetDto>> {
        telemetry::db_query(ASSETS_COLLECTION_NAME, "find_one.pending_by_id", async {
            self.assets
                .find_one(doc! { "asset_id": asset_id.as_str(), "status": "pending" })
                .await
        })
        .await
    }

    pub async fn expired_pending_assets(
        &self,
        before: i64,
    ) -> mongodb::error::Result<Vec<CardDefinitionAssetDto>> {
        telemetry::db_query(ASSETS_COLLECTION_NAME, "find.expired_pending", async {
            let cursor = self
                .assets
                .find(doc! { "status": "pending", "created_at": { "$lt": before } })
                .await?;
            cursor.try_collect().await
        })
        .await
    }

    pub async fn delete_asset(&self, asset_id: &CardId) -> mongodb::error::Result<()> {
        telemetry::db_query(ASSETS_COLLECTION_NAME, "delete_one", async {
            self.assets
                .delete_one(doc! { "asset_id": asset_id.as_str() })
                .await
        })
        .await?;
        Ok(())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CardDefinitionAssetDto {
    pub asset_id: CardId,
    pub creator_id: PlayerId,
    pub image_object_key: String,
    pub script_object_key: String,
    pub status: CardDefinitionAssetStatus,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardDefinitionAssetStatus {
    Pending,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub life: Option<i32>,
    pub mana_cost: usize,
    #[serde(rename = "type")]
    pub card_type: PowerCardType,
    pub creator_id: PlayerId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_object_key: Option<String>,
    pub script_object_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
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
            mana_cost: input.mana_cost,
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
    pub mana_cost: usize,
    pub card_type: PowerCardType,
    pub creator_id: PlayerId,
    pub image_object_key: Option<String>,
    pub script_object_key: String,
    pub image_content_type: Option<String>,
}
