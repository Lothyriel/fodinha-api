use std::sync::Arc;

use chrono::{DateTime, Utc};
use mongodb::{Collection, Database, bson::doc};

use crate::models::{Card, id::*};

#[derive(Clone)]
pub struct GamesRepository {
    games: Collection<GameDto>,
    sets: Collection<SetDto>,
    rounds: Collection<RoundDto>,
    turns: Collection<TurnDto>,
}

impl GamesRepository {
    pub fn new(database: &Database) -> Self {
        Self {
            games: database.collection("Games"),
            sets: database.collection("Sets"),
            rounds: database.collection("Rounds"),
            turns: database.collection("Turns"),
        }
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
