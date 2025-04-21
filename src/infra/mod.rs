pub mod auth;
pub mod game;
pub mod lobby;

use std::collections::HashMap;

use auth::UserClaims;
use axum::http::StatusCode;

use crate::{
    models::{Card, Turn},
    services::{
        manager::{PlayerId, PlayerStatus},
        GameInfoDto,
    },
};

pub async fn fallback_handler() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "this resource doesn't exist")
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
#[serde(tag = "type", content = "data")]
pub enum ClientMessage {
    Game(ClientGameMessage),
    Auth { token: String },
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug)]
#[serde(tag = "type", content = "data")]
pub enum ClientGameMessage {
    PlayTurn { card: Card },
    PutBid { bid: usize },
    PlayerStatusChange { ready: bool },
    Reconnect,
}

#[derive(serde::Serialize)]
pub struct GetLobbyDto {
    pub id: PlayerId,
    pub player_count: usize,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct JoinLobbyDto {
    pub id: PlayerId,
    pub players: Vec<PlayerStatus>,
    pub should_reconnect: bool,
}

pub type PlayerPoints = HashMap<PlayerId, usize>;

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq)]
#[serde(tag = "type", content = "data")]
pub enum ServerMessage {
    PlayerTurn {
        player_id: PlayerId,
    },
    TurnPlayed {
        pile: Vec<Turn>,
    },
    PlayerBidded {
        player_id: PlayerId,
        bid: usize,
    },
    PlayerBiddingTurn {
        player_id: PlayerId,
        possible_bids: Vec<usize>,
    },
    PlayerStatusChange {
        player_id: PlayerId,
        ready: bool,
    },
    RoundEnded(PlayerPoints),
    PlayerDeck(Vec<Card>),
    SetStart {
        upcard: Card,
    },
    SetEnded {
        lifes: PlayerPoints,
    },
    GameEnded {
        lifes: PlayerPoints,
    },
    PlayerJoined(UserClaims),
    Reconnect(GameInfoDto),
    Error {
        msg: String,
    },
}
