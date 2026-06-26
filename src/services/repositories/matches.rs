use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{Collection, Database, IndexModel, bson::doc, options::IndexOptions};

use crate::{
    infra::telemetry,
    models::{
        game::{GameSettings, MatchEvent},
        id::*,
    },
};

#[derive(Clone)]
pub struct MatchesRepository {
    events: Collection<MatchEventDto>,
    metadata: Collection<MatchMetadataDto>,
}

impl MatchesRepository {
    pub fn new(database: &Database) -> Self {
        Self {
            events: database.collection("MatchEvents"),
            metadata: database.collection("MatchMetadata"),
        }
    }

    pub async fn ensure_indexes(&self) -> mongodb::error::Result<()> {
        telemetry::db_query(
            "MatchEvents",
            "create_index.unique_match_id_sequence",
            async {
                self.events
                    .create_index(
                        IndexModel::builder()
                            .keys(doc! { "match_id": 1, "sequence": 1 })
                            .options(IndexOptions::builder().unique(true).build())
                            .build(),
                    )
                    .await
            },
        )
        .await?;

        telemetry::db_query("MatchMetadata", "create_index.unique_match_id", async {
            self.metadata
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "match_id": 1 })
                        .options(IndexOptions::builder().unique(true).build())
                        .build(),
                )
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn append_event(
        &self,
        match_id: &MatchId,
        sequence: usize,
        event: MatchEvent,
    ) -> mongodb::error::Result<()> {
        telemetry::db_query("MatchEvents", "insert_one", async {
            self.events
                .insert_one(MatchEventDto::new(match_id, sequence, event))
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn load_events(
        &self,
        match_id: &MatchId,
    ) -> mongodb::error::Result<Vec<MatchEventDto>> {
        telemetry::db_query("MatchEvents", "find", async {
            let cursor = self
                .events
                .find(doc! { "match_id": match_id.as_str() })
                .sort(doc! { "sequence": 1 })
                .await?;

            cursor.try_collect().await
        })
        .await
    }

    pub async fn create_metadata(
        &self,
        match_id: &MatchId,
        settings: GameSettings,
        creator_id: Option<&PlayerId>,
    ) -> mongodb::error::Result<()> {
        telemetry::db_query("MatchMetadata", "insert_one", async {
            self.metadata
                .insert_one(MatchMetadataDto::new(match_id, settings, creator_id))
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn add_metadata_player(
        &self,
        match_id: &MatchId,
        player_id: &PlayerId,
    ) -> mongodb::error::Result<()> {
        let updated_at = current_timestamp();

        telemetry::db_query("MatchMetadata", "update_one.add_player", async {
            self.metadata
                .update_one(
                    doc! { "match_id": match_id.as_str() },
                    doc! {
                        "$addToSet": { "players": player_id.as_str() },
                        "$set": { "updated_at": updated_at },
                    },
                )
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn remove_metadata_player(
        &self,
        match_id: &MatchId,
        player_id: &PlayerId,
    ) -> mongodb::error::Result<()> {
        let updated_at = current_timestamp();

        telemetry::db_query("MatchMetadata", "update_one.remove_player", async {
            self.metadata
                .update_one(
                    doc! { "match_id": match_id.as_str() },
                    doc! {
                        "$pull": {
                            "players": player_id.as_str(),
                            "ready_players": player_id.as_str(),
                        },
                        "$set": { "updated_at": updated_at },
                    },
                )
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn delete_metadata(&self, match_id: &MatchId) -> mongodb::error::Result<()> {
        telemetry::db_query("MatchMetadata", "delete_one", async {
            self.metadata
                .delete_one(doc! { "match_id": match_id.as_str() })
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn set_metadata_player_ready(
        &self,
        match_id: &MatchId,
        player_id: &PlayerId,
        ready: bool,
    ) -> mongodb::error::Result<()> {
        let updated_at = current_timestamp();
        let update = match ready {
            true => doc! {
                "$addToSet": { "ready_players": player_id.as_str() },
                "$set": { "updated_at": updated_at },
            },
            false => doc! {
                "$pull": { "ready_players": player_id.as_str() },
                "$set": { "updated_at": updated_at },
            },
        };

        telemetry::db_query("MatchMetadata", "update_one.set_ready", async {
            self.metadata
                .update_one(doc! { "match_id": match_id.as_str() }, update)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn touch_metadata(&self, match_id: &MatchId) -> mongodb::error::Result<()> {
        let updated_at = current_timestamp();

        telemetry::db_query("MatchMetadata", "update_one.touch", async {
            self.metadata
                .update_one(
                    doc! { "match_id": match_id.as_str() },
                    doc! { "$set": { "updated_at": updated_at } },
                )
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn mark_metadata_playing(&self, match_id: &MatchId) -> mongodb::error::Result<()> {
        self.set_metadata_status(match_id, MatchMetadataStatus::Playing)
            .await
    }

    pub async fn mark_metadata_finished(&self, match_id: &MatchId) -> mongodb::error::Result<()> {
        self.set_metadata_status(match_id, MatchMetadataStatus::Finished)
            .await
    }

    pub async fn active_metadata(
        &self,
        match_id: &MatchId,
    ) -> mongodb::error::Result<Option<MatchMetadataDto>> {
        telemetry::db_query("MatchMetadata", "find_one.active_by_match", async {
            self.metadata
                .find_one(doc! {
                    "match_id": match_id.as_str(),
                    "status": { "$ne": MatchMetadataStatus::Finished.as_str() },
                })
                .await
        })
        .await
    }

    pub async fn active_metadata_for_player(
        &self,
        player_id: &PlayerId,
    ) -> mongodb::error::Result<Option<MatchMetadataDto>> {
        telemetry::db_query("MatchMetadata", "find_one.active_by_player", async {
            self.metadata
                .find_one(doc! {
                    "players": player_id.as_str(),
                    "status": { "$ne": MatchMetadataStatus::Finished.as_str() },
                })
                .await
        })
        .await
    }

    pub async fn waiting_match_ids(&self) -> mongodb::error::Result<Vec<MatchId>> {
        let metadata: Vec<MatchMetadataDto> =
            telemetry::db_query("MatchMetadata", "find.waiting", async {
                let cursor = self
                    .metadata
                    .find(doc! { "status": MatchMetadataStatus::Waiting.as_str() })
                    .await?;

                cursor.try_collect().await
            })
            .await?;

        Ok(metadata.into_iter().map(|m| m.match_id()).collect())
    }

    pub async fn finished_match_ids(&self) -> mongodb::error::Result<Vec<MatchId>> {
        let metadata: Vec<MatchMetadataDto> =
            telemetry::db_query("MatchMetadata", "find.finished", async {
                let cursor = self
                    .metadata
                    .find(doc! { "status": MatchMetadataStatus::Finished.as_str() })
                    .await?;

                cursor.try_collect().await
            })
            .await?;

        Ok(metadata.into_iter().map(|m| m.match_id()).collect())
    }

    async fn set_metadata_status(
        &self,
        match_id: &MatchId,
        status: MatchMetadataStatus,
    ) -> mongodb::error::Result<()> {
        let updated_at = current_timestamp();

        telemetry::db_query("MatchMetadata", "update_one.set_status", async {
            self.metadata
                .update_one(
                    doc! { "match_id": match_id.as_str() },
                    doc! { "$set": { "status": status.as_str(), "updated_at": updated_at } },
                )
                .await
        })
        .await?;

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct MatchMetadataDto {
    pub match_id: String,
    pub status: MatchMetadataStatus,
    pub settings: Option<GameSettings>,
    #[serde(default)]
    pub creator_id: Option<String>,
    #[serde(default)]
    pub players: Vec<String>,
    #[serde(default)]
    pub ready_players: Vec<String>,
    #[serde(default)]
    pub updated_at: i64,
}

impl MatchMetadataDto {
    fn new(match_id: &MatchId, settings: GameSettings, creator_id: Option<&PlayerId>) -> Self {
        Self {
            match_id: match_id.as_str().to_string(),
            status: MatchMetadataStatus::Waiting,
            settings: Some(settings),
            creator_id: creator_id.map(|id| id.as_str().to_string()),
            players: Vec::new(),
            ready_players: Vec::new(),
            updated_at: current_timestamp(),
        }
    }

    pub fn match_id(&self) -> MatchId {
        LobbyId(Arc::from(self.match_id.as_str()))
    }

    pub fn creator_id(&self) -> Option<PlayerId> {
        self.creator_id
            .as_deref()
            .map(|player_id| PlayerId(Arc::from(player_id)))
    }

    pub fn is_waiting_stale(&self, timeout: std::time::Duration) -> bool {
        self.status == MatchMetadataStatus::Waiting
            && current_timestamp().saturating_sub(self.updated_at) >= timeout.as_secs() as i64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMetadataStatus {
    Waiting,
    Playing,
    Finished,
}

impl MatchMetadataStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Playing => "playing",
            Self::Finished => "finished",
        }
    }
}

fn current_timestamp() -> i64 {
    Utc::now().timestamp()
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct MatchEventDto {
    pub ts: DateTime<Utc>,
    pub match_id: Arc<str>,
    pub sequence: usize,
    pub event: MatchEvent,
}

impl MatchEventDto {
    fn new(match_id: &MatchId, sequence: usize, event: MatchEvent) -> Self {
        Self {
            ts: Utc::now(),
            match_id: match_id.0.clone(),
            sequence,
            event,
        }
    }
}
