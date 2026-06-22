use std::collections::HashMap;

use crate::{
    infra::UserClaims,
    models::{
        Card, Turn,
        id::{LobbyId, PlayerId},
        lobby::PlayerStatus,
    },
    services::GameInfoDto,
};

#[derive(serde::Serialize)]
pub struct GetLobbyDto {
    pub id: LobbyId,
    pub player_count: usize,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct CreateLobbyResponse {
    pub lobby_id: LobbyId,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum LobbyInfo {
    NotStarted(HashMap<PlayerId, PlayerStatus>),
    Playing(GameInfoDto),
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(tag = "type", content = "data")]
pub enum MatchSnapshot {
    Waiting(HashMap<PlayerId, PlayerStatus>),
    Playing(GameInfoDto),
}

type PlayerPoints = HashMap<PlayerId, usize>;

#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug)]
#[serde(tag = "type", content = "data")]
pub enum ClientCommand {
    PlayTurn { card: Card },
    PutBid { bid: usize },
    PlayerStatusChange { ready: bool },
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug)]
#[serde(tag = "type", content = "data")]
pub enum GameCommand {
    PlayTurn { card: Card },
    PutBid { bid: usize },
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq, Clone)]
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
    Snapshot(MatchSnapshot),
    Error {
        msg: String,
    },
}
