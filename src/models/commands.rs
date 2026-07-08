use std::collections::HashMap;

use crate::{
    infra::UserClaims,
    models::{
        Card, Turn,
        game::{GameCommand, GameType},
        id::{LobbyId, MercenaryId, PlayerId},
    },
    services::{GameInfoDto, PlayerManaDto, PowerCardDto},
};

#[derive(serde::Serialize)]
pub struct GetLobbyDto {
    pub id: LobbyId,
    pub game_type: GameType,
    pub player_count: usize,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct CreateLobbyResponse {
    pub lobby_id: LobbyId,
    pub game_type: GameType,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PlayerStatus {
    pub ready: bool,
    pub player: UserClaims,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mercenary_id: Option<MercenaryId>,
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
    Playing(PlayingMatchSnapshot),
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct PlayingMatchSnapshot {
    pub players: HashMap<PlayerId, PlayerStatus>,
    pub game: GameInfoDto,
}

type PlayerPoints = HashMap<PlayerId, usize>;
type PlayerMana = HashMap<PlayerId, PlayerManaDto>;

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
#[serde(tag = "type", content = "data")]
pub enum ClientCommand {
    GameCommand(GameCommand),
    PlayerStatusChange { ready: bool },
    SelectMercenary { mercenary_id: MercenaryId },
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
    PlayersManaChanged(PlayerMana),
    PlayerBiddingTurn {
        player_id: PlayerId,
        possible_bids: Vec<usize>,
    },
    PlayerStatusChange {
        player_id: PlayerId,
        ready: bool,
    },
    PlayerMercenarySelected {
        player_id: PlayerId,
        mercenary_id: MercenaryId,
    },
    RoundEnded(PlayerPoints),
    PlayerDeck(Vec<Card>),
    PlayerPowerCards(Vec<PowerCardDto>),
    PowerCardPlayed {
        player_id: PlayerId,
        card: PowerCardDto,
        target_player_id: Option<PlayerId>,
        lifes: PlayerPoints,
    },
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
    PlayerLeft {
        player_id: PlayerId,
    },
    Snapshot(MatchSnapshot),
    Error {
        msg: String,
    },
}
