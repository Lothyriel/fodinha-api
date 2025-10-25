use chrono::{DateTime, Utc};
use mongodb::{bson::doc, error::Result, Collection, Database};

use crate::models::Card;

#[derive(Clone)]
pub struct GamesRepository {
    games: Collection<GameDto>,
    turns: Collection<TurnDto>,
}

impl GamesRepository {
    pub fn new(database: &Database) -> Self {
        Self {
            games: database.collection("Games"),
            turns: database.collection("Turns"),
        }
    }

    pub async fn insert_game(&self, game: &GameDto) -> Result<()> {
        self.games.insert_one(game).await?;

        Ok(())
    }

    pub async fn insert_turn(&self, turn: &TurnDto) -> Result<()> {
        self.turns.insert_one(turn).await?;

        Ok(())
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct GameDto {
    started_at: DateTime<Utc>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct TurnDto {
    game_id: String,
    player_id: String,
    time: DateTime<Utc>,
    card: Card,
}
