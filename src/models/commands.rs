use std::collections::HashMap;

use tokio::sync::oneshot;

use crate::{
    infra::UserClaims,
    models::{
        Card, Turn,
        game::GameSettings,
        id::{LobbyId, PlayerId},
        lobby::PlayerStatus,
    },
    services::{GameInfoDto, ManagerError},
};

#[derive(serde::Serialize)]
pub struct GetLobbyDto {
    pub id: LobbyId,
    pub player_count: usize,
}

pub enum Command {
    Lobby(LobbyCommand),
    Game(GameCommand, PlayerId),
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct CreateLobbyResponse {
    pub lobby_id: LobbyId,
}

pub type JoinLobbyResponse = Result<LobbyInfo, ManagerError>;

#[derive(serde::Serialize, serde::Deserialize)]
pub enum LobbyInfo {
    NotStarted(HashMap<PlayerId, PlayerStatus>),
    Playing(GameInfoDto),
}

pub enum LobbyCommand {
    CreateLobby(LobbyId, GameSettings),
    StatusChange {
        ready: bool,
    },
    JoinLobby {
        lobby_id: LobbyId,
        user_claims: UserClaims,
        respond: oneshot::Sender<JoinLobbyResponse>,
    },
    GetLobbies(oneshot::Sender<Vec<GetLobbyDto>>),
}

impl From<LobbyCommand> for Command {
    fn from(value: LobbyCommand) -> Self {
        Command::Lobby(value)
    }
}

type PlayerPoints = HashMap<PlayerId, usize>;

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
    TurnPlayed(Turn),
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
}
