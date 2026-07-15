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
        telemetry::db_query(
            COLLECTION_NAME,
            "create_index.unique_mercenary_version",
            async {
                self.mercenaries
                    .create_index(
                        IndexModel::builder()
                            .keys(doc! { "mercenary_id": 1, "version": 1 })
                            .options(IndexOptions::builder().unique(true).build())
                            .build(),
                    )
                    .await
            },
        )
        .await?;

        self.mercenaries
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "mercenary_id": 1, "version": -1 })
                    .build(),
            )
            .await?;
        self.mercenaries
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "mercenary_id": 1, "active": 1, "version": -1 })
                    .build(),
            )
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

    pub async fn deactivate(
        &self,
        mercenary_id: &MercenaryId,
        version: i64,
    ) -> mongodb::error::Result<()> {
        self.mercenaries
            .update_one(
                doc! { "mercenary_id": mercenary_id.as_str(), "version": version },
                doc! { "$set": { "active": false } },
            )
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
    pub version: i64,
    pub name: String,
    pub subtitle: String,
    pub description: String,
    pub style: String,
    pub temper: String,
    pub creator_id: PlayerId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner_content_type: Option<String>,
    pub banner_object_key: String,
    pub icon_object_key: String,
    pub script_object_key: String,
    pub active: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl MercenaryDto {
    pub fn new(input: NewMercenary) -> Self {
        let now = Utc::now().timestamp();

        Self {
            mercenary_id: input.mercenary_id,
            version: 1,
            name: input.name,
            subtitle: input.subtitle,
            description: input.description,
            style: input.style,
            temper: input.temper,
            creator_id: input.creator_id,
            icon_content_type: input.icon_content_type,
            banner_content_type: input.banner_content_type,
            banner_object_key: input.banner_object_key,
            icon_object_key: input.icon_object_key,
            script_object_key: input.script_object_key,
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
    pub icon_content_type: Option<String>,
    pub banner_content_type: Option<String>,
    pub banner_object_key: String,
    pub icon_object_key: String,
    pub script_object_key: String,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{MercenaryDto, NewMercenary};
    use crate::models::id::{MercenaryId, PlayerId};

    fn mercenary() -> MercenaryDto {
        MercenaryDto::new(NewMercenary {
            mercenary_id: MercenaryId(Arc::from("mercenary-1")),
            name: "Mercenary".to_string(),
            subtitle: "Subtitle".to_string(),
            description: "Description".to_string(),
            style: "Bid".to_string(),
            temper: "Aggressive".to_string(),
            creator_id: PlayerId(Arc::from("creator")),
            icon_content_type: Some("image/png".to_string()),
            banner_content_type: Some("image/png".to_string()),
            banner_object_key: "mercenary-assets/banner.png".to_string(),
            icon_object_key: "mercenary-assets/icon.png".to_string(),
            script_object_key: "mercenary-assets/script.lua".to_string(),
        })
    }

    #[test]
    fn mercenary_serialization_stores_asset_keys() {
        let value = serde_json::to_value(mercenary()).unwrap();
        let object = value.as_object().unwrap();

        assert!(object.contains_key("banner_object_key"));
        assert!(object.contains_key("icon_object_key"));
        assert!(object.contains_key("script_object_key"));
    }

    #[test]
    fn legacy_mercenaries_without_versions_are_rejected() {
        let mut value = serde_json::to_value(mercenary()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("version");

        assert!(serde_json::from_value::<MercenaryDto>(value).is_err());
    }
}
