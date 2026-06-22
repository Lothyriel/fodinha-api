use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{Collection, Database, bson::doc};

use crate::models::{Card, game::DomainEvent, id::*};

#[derive(Clone)]
pub struct GamesRepository {
    events: Collection<GameEventDto>,
    metadata: Collection<GameMetadataDto>,
    games: Collection<GameDto>,
    sets: Collection<SetDto>,
    rounds: Collection<RoundDto>,
    turns: Collection<TurnDto>,
}

impl GamesRepository {
    pub fn new(database: &Database) -> Self {
        Self {
            events: database.collection("GameEvents"),
            metadata: database.collection("GameMetadata"),
            games: database.collection("Games"),
            sets: database.collection("Sets"),
            rounds: database.collection("Rounds"),
            turns: database.collection("Turns"),
        }
    }

    pub async fn append_event(
        &self,
        game_id: &LobbyId,
        sequence: usize,
        event: DomainEvent,
    ) -> mongodb::error::Result<()> {
        let event = GameEventDto::new(game_id, sequence, event);

        self.events.insert_one(event).await?;

        Ok(())
    }

    pub async fn load_events(
        &self,
        game_id: &LobbyId,
    ) -> mongodb::error::Result<Vec<GameEventDto>> {
        let cursor = self
            .events
            .find(doc! { "game_id": game_id.as_str() })
            .sort(doc! { "sequence": 1 })
            .await?;

        cursor.try_collect().await
    }

    pub async fn create_metadata(&self, game_id: &LobbyId) -> mongodb::error::Result<()> {
        self.metadata
            .insert_one(GameMetadataDto::new(game_id))
            .await?;

        Ok(())
    }

    pub async fn add_metadata_player(
        &self,
        game_id: &LobbyId,
        player_id: &PlayerId,
    ) -> mongodb::error::Result<()> {
        self.metadata
            .update_one(
                doc! { "game_id": game_id.as_str() },
                doc! { "$addToSet": { "players": player_id.as_str() } },
            )
            .await?;

        Ok(())
    }

    pub async fn mark_metadata_playing(&self, game_id: &LobbyId) -> mongodb::error::Result<()> {
        self.set_metadata_status(game_id, GameMetadataStatus::Playing)
            .await
    }

    pub async fn mark_metadata_finished(&self, game_id: &LobbyId) -> mongodb::error::Result<()> {
        self.set_metadata_status(game_id, GameMetadataStatus::Finished)
            .await
    }

    pub async fn active_metadata(
        &self,
        game_id: &LobbyId,
    ) -> mongodb::error::Result<Option<GameMetadataDto>> {
        self.metadata
            .find_one(doc! {
                "game_id": game_id.as_str(),
                "status": { "$ne": GameMetadataStatus::Finished.as_str() },
            })
            .await
    }

    pub async fn active_metadata_for_player(
        &self,
        player_id: &PlayerId,
    ) -> mongodb::error::Result<Option<GameMetadataDto>> {
        self.metadata
            .find_one(doc! {
                "players": player_id.as_str(),
                "status": { "$ne": GameMetadataStatus::Finished.as_str() },
            })
            .await
    }

    pub async fn waiting_game_ids(&self) -> mongodb::error::Result<Vec<LobbyId>> {
        let cursor = self
            .metadata
            .find(doc! { "status": GameMetadataStatus::Waiting.as_str() })
            .await?;

        let metadata: Vec<GameMetadataDto> = cursor.try_collect().await?;

        Ok(metadata.into_iter().map(|m| m.lobby_id()).collect())
    }

    async fn set_metadata_status(
        &self,
        game_id: &LobbyId,
        status: GameMetadataStatus,
    ) -> mongodb::error::Result<()> {
        self.metadata
            .update_one(
                doc! { "game_id": game_id.as_str() },
                doc! { "$set": { "status": status.as_str() } },
            )
            .await?;

        Ok(())
    }

    pub fn insert_game(&self, game: GameDto) {
        let repo = self.games.clone();

        tokio::spawn(async move {
            if let Err(e) = repo.insert_one(game).await {
                tracing::error!("Error inserting game | {e}");
            }
        });
    }

    pub fn insert_set(&self, set: SetDto) {
        let repo = self.sets.clone();

        tokio::spawn(async move {
            if let Err(e) = repo.insert_one(set).await {
                tracing::error!("Error inserting game | {e}");
            }
        });
    }

    pub fn insert_round(&self, round: RoundDto) {
        let repo = self.rounds.clone();

        tokio::spawn(async move {
            if let Err(e) = repo.insert_one(round).await {
                tracing::error!("Error inserting game | {e}");
            }
        });
    }

    pub fn insert_turn(&self, turn: TurnDto) {
        let repo = self.turns.clone();

        tokio::spawn(async move {
            if let Err(e) = repo.insert_one(turn).await {
                tracing::error!("Error inserting turn | {e}");
            }
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct GameMetadataDto {
    pub game_id: String,
    pub status: GameMetadataStatus,
    pub players: Vec<String>,
}

impl GameMetadataDto {
    fn new(game_id: &LobbyId) -> Self {
        Self {
            game_id: game_id.as_str().to_string(),
            status: GameMetadataStatus::Waiting,
            players: Vec::new(),
        }
    }

    pub fn lobby_id(&self) -> LobbyId {
        LobbyId(Arc::from(self.game_id.as_str()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GameMetadataStatus {
    Waiting,
    Playing,
    Finished,
}

impl GameMetadataStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Playing => "playing",
            Self::Finished => "finished",
        }
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct GameEventDto {
    pub ts: DateTime<Utc>,
    pub game_id: Arc<str>,
    pub sequence: usize,
    pub event: DomainEvent,
}

impl GameEventDto {
    fn new(game_id: &LobbyId, sequence: usize, event: DomainEvent) -> Self {
        Self {
            ts: Utc::now(),
            game_id: game_id.0.clone(),
            sequence,
            event,
        }
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct GameDto {
    pub ts: DateTime<Utc>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct SetDto {
    pub ts: DateTime<Utc>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct RoundDto {
    pub ts: DateTime<Utc>,
    pub trump: Card,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct TurnDto {
    ts: DateTime<Utc>,
    i: usize,
    set_id: Arc<str>,
    round_id: Arc<str>,
    game_id: Arc<str>,
    player_id: Arc<str>,
    card: Card,
}

impl TurnDto {
    pub fn new(
        game_id: &LobbyId,
        player_id: &PlayerId,
        set_id: &Uid,
        round_id: &Uid,
        card: Card,
        i: usize,
    ) -> Self {
        Self {
            ts: Utc::now(),
            game_id: game_id.0.clone(),
            set_id: set_id.0.clone(),
            round_id: round_id.0.clone(),
            player_id: player_id.0.clone(),
            i,
            card,
        }
    }
}
