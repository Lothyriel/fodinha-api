use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{Collection, Database, bson::doc};

use crate::models::{game::MatchEvent, id::*};

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

    pub async fn append_event(
        &self,
        match_id: &MatchId,
        sequence: usize,
        event: MatchEvent,
    ) -> mongodb::error::Result<()> {
        self.events
            .insert_one(MatchEventDto::new(match_id, sequence, event))
            .await?;

        Ok(())
    }

    pub async fn load_events(
        &self,
        match_id: &MatchId,
    ) -> mongodb::error::Result<Vec<MatchEventDto>> {
        let cursor = self
            .events
            .find(doc! { "match_id": match_id.as_str() })
            .sort(doc! { "sequence": 1 })
            .await?;

        cursor.try_collect().await
    }

    pub async fn create_metadata(&self, match_id: &MatchId) -> mongodb::error::Result<()> {
        self.metadata
            .insert_one(MatchMetadataDto::new(match_id))
            .await?;

        Ok(())
    }

    pub async fn add_metadata_player(
        &self,
        match_id: &MatchId,
        player_id: &PlayerId,
    ) -> mongodb::error::Result<()> {
        self.metadata
            .update_one(
                doc! { "match_id": match_id.as_str() },
                doc! { "$addToSet": { "players": player_id.as_str() } },
            )
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
        self.metadata
            .find_one(doc! {
                "match_id": match_id.as_str(),
                "status": { "$ne": MatchMetadataStatus::Finished.as_str() },
            })
            .await
    }

    pub async fn active_metadata_for_player(
        &self,
        player_id: &PlayerId,
    ) -> mongodb::error::Result<Option<MatchMetadataDto>> {
        self.metadata
            .find_one(doc! {
                "players": player_id.as_str(),
                "status": { "$ne": MatchMetadataStatus::Finished.as_str() },
            })
            .await
    }

    pub async fn waiting_match_ids(&self) -> mongodb::error::Result<Vec<MatchId>> {
        let cursor = self
            .metadata
            .find(doc! { "status": MatchMetadataStatus::Waiting.as_str() })
            .await?;

        let metadata: Vec<MatchMetadataDto> = cursor.try_collect().await?;

        Ok(metadata.into_iter().map(|m| m.match_id()).collect())
    }

    async fn set_metadata_status(
        &self,
        match_id: &MatchId,
        status: MatchMetadataStatus,
    ) -> mongodb::error::Result<()> {
        self.metadata
            .update_one(
                doc! { "match_id": match_id.as_str() },
                doc! { "$set": { "status": status.as_str() } },
            )
            .await?;

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct MatchMetadataDto {
    pub match_id: String,
    pub status: MatchMetadataStatus,
    pub players: Vec<String>,
}

impl MatchMetadataDto {
    fn new(match_id: &MatchId) -> Self {
        Self {
            match_id: match_id.as_str().to_string(),
            status: MatchMetadataStatus::Waiting,
            players: Vec::new(),
        }
    }

    pub fn match_id(&self) -> MatchId {
        LobbyId(Arc::from(self.match_id.as_str()))
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
