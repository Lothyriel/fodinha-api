use chrono::Utc;
use futures::TryStreamExt;
use mongodb::{Collection, Database, IndexModel, bson::doc, options::IndexOptions};

use crate::{
    infra::telemetry,
    models::id::{MercenaryId, PlayerId},
};

const COLLECTION_NAME: &str = "MercenaryDefinitions";

#[derive(Clone)]
pub struct MercenariesRepository {
    mercenaries: Collection<MercenaryDto>,
}

impl MercenariesRepository {
    pub fn new(database: &Database) -> Self {
        Self {
            mercenaries: database.collection(COLLECTION_NAME),
        }
    }

    pub async fn ensure_indexes(&self) -> mongodb::error::Result<()> {
        telemetry::db_query(COLLECTION_NAME, "create_index.unique_mercenary_id", async {
            self.mercenaries
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "mercenary_id": 1 })
                        .options(IndexOptions::builder().unique(true).build())
                        .build(),
                )
                .await
        })
        .await?;

        telemetry::db_query(COLLECTION_NAME, "create_index.active_created", async {
            self.mercenaries
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "active": 1, "created_at": -1 })
                        .build(),
                )
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn insert(&self, mercenary: MercenaryDto) -> mongodb::error::Result<()> {
        telemetry::db_query(COLLECTION_NAME, "insert_one", async {
            self.mercenaries.insert_one(mercenary).await
        })
        .await?;

        Ok(())
    }

    pub async fn replace(&self, mercenary: MercenaryDto) -> mongodb::error::Result<()> {
        let mercenary_id = mercenary.mercenary_id.as_str().to_string();

        telemetry::db_query(COLLECTION_NAME, "replace_one.mercenary_id", async {
            self.mercenaries
                .replace_one(doc! { "mercenary_id": mercenary_id }, mercenary)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn active_mercenaries(&self) -> mongodb::error::Result<Vec<MercenaryDto>> {
        telemetry::db_query(COLLECTION_NAME, "find.active", async {
            let cursor = self
                .mercenaries
                .find(doc! { "active": true })
                .sort(doc! { "created_at": -1 })
                .await?;

            cursor.try_collect().await
        })
        .await
    }

    pub async fn active_mercenary_by_id(
        &self,
        mercenary_id: &MercenaryId,
    ) -> mongodb::error::Result<Option<MercenaryDto>> {
        telemetry::db_query(COLLECTION_NAME, "find_one.active_by_id", async {
            self.mercenaries
                .find_one(doc! { "active": true, "mercenary_id": mercenary_id.as_str() })
                .await
        })
        .await
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MercenaryDto {
    pub mercenary_id: MercenaryId,
    pub name: String,
    pub subtitle: String,
    pub description: String,
    pub style: String,
    pub temper: String,
    pub creator_id: PlayerId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner_object_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_object_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_content_type: Option<String>,
    pub passive_script_object_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner_content_type: Option<String>,
    pub active: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl MercenaryDto {
    pub fn new(input: NewMercenary) -> Self {
        let now = Utc::now().timestamp();

        Self {
            mercenary_id: input.mercenary_id,
            name: input.name,
            subtitle: input.subtitle,
            description: input.description,
            style: input.style,
            temper: input.temper,
            creator_id: input.creator_id,
            banner_object_key: input.banner_object_key,
            icon_object_key: input.icon_object_key,
            icon_content_type: input.icon_content_type,
            passive_script_object_key: input.passive_script_object_key,
            banner_content_type: input.banner_content_type,
            active: true,
            created_at: now,
            updated_at: now,
        }
    }
}

pub struct NewMercenary {
    pub mercenary_id: MercenaryId,
    pub name: String,
    pub subtitle: String,
    pub description: String,
    pub style: String,
    pub temper: String,
    pub creator_id: PlayerId,
    pub banner_object_key: Option<String>,
    pub icon_object_key: Option<String>,
    pub icon_content_type: Option<String>,
    pub passive_script_object_key: String,
    pub banner_content_type: Option<String>,
}
