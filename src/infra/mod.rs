pub mod auth;
pub mod game;
pub mod lobby;

use std::collections::HashMap;

use auth::UserClaims;
use axum::http::StatusCode;

use crate::{
    models::{Card, Turn},
    services::{
        GameInfoDto,
        manager::{LobbyId, PlayerId, PlayerStatus},
    },
};

pub async fn fallback_handler() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "this resource doesn't exist")
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug)]
#[serde(tag = "type", content = "data")]
pub enum ClientMessage {
    PlayTurn { card: Card },
    PutBid { bid: usize },
    PlayerStatusChange { ready: bool },
    Reconnect,
}

#[derive(serde::Serialize)]
pub struct GetLobbyDto {
    pub id: LobbyId,
    pub player_count: usize,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct JoinLobbyDto {
    pub id: LobbyId,
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

const ALPHABET: &[char] = &[
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I',
    'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '_', 'a',
    'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't',
    'u', 'v', 'w', 'x', 'y', 'z', '-',
];

pub fn generate_playerid() -> PlayerId {
    PlayerId(nanoid::nanoid!(10, ALPHABET).into())
}

pub fn generate_lobbyid() -> LobbyId {
    LobbyId(nanoid::nanoid!(12, ALPHABET).into())
}
