use crate::{
    infra::AuthError,
    models::{
        BiddingError, Card, DealError, GameError,
        game::{GameCommandError, fodinha_power::PowerCardType},
        id::{CardId, PlayerId},
    },
};

pub mod card_definitions;
pub mod manager;
pub mod matches;
pub mod mercenaries;
pub mod object_storage;
pub mod repositories;
pub mod stats;
pub(crate) mod tasks;

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct GameInfoDto {
    pub info: Vec<PlayerInfoDto>,
    pub deck: Option<Vec<Card>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_cards: Option<Vec<PowerCardDto>>,
    pub upcard: Option<Card>,
    pub current_player: String,
    pub stage: GameStageDto,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct PowerCardDto {
    pub id: CardId,
    pub name: String,
    pub description: String,
    pub mana_cost: usize,
    #[serde(rename = "type")]
    pub card_type: PowerCardType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct PlayerManaDto {
    pub current: usize,
    pub max: usize,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(tag = "type", content = "data")]
pub enum GameStageDto {
    Bidding { possible_bids: Vec<usize> },
    Power { phase: PowerPhaseDto },
    Dealing,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum PowerPhaseDto {
    First,
    Second,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct PlayerInfoDto {
    pub id: PlayerId,
    pub lifes: usize,
    pub rounds: Option<usize>,
    pub bid: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mana: Option<PlayerManaDto>,
}

#[derive(thiserror::Error, Debug)]
pub enum ManagerError {
    #[error("Player disconnected | {0}")]
    PlayerDisconnected(String),
    #[error("Error processing deal: {0:?}")]
    Deal(#[from] DealError),
    #[error("Error processing bid: {0:?}")]
    Bid(#[from] BiddingError),
    #[error("Invalid websocket message type")]
    InvalidWebsocketMessageType,
    #[error("Invalid game command | {0}")]
    GameCommand(#[from] GameCommandError),
    #[error("Unexpected valid json message: {0}")]
    UnexpectedMessage(#[from] serde_json::error::Error),
    #[error("Database error: {0}")]
    Database(#[from] mongodb::error::Error),
    #[error("Unauthorized | {0}")]
    Unauthorized(#[from] AuthError),
    #[error("Lobby error | {0}")]
    Lobby(#[from] LobbyError),
    #[error("Oneshot receiver disposed")]
    ReceiverDisposed,
}

#[derive(thiserror::Error, Debug)]
pub enum LobbyError {
    #[error("Invalid lobby id")]
    InvalidLobby,
    #[error("Invalid lobby settings | {0}")]
    InvalidSettings(String),
    #[error("This lobby is already playing")]
    GameAlreadyStarted,
    #[error("This player isn't in a lobby")]
    PlayerNotInLobby,
    #[error("Game didn't started yet")]
    GameNotStarted,
    #[error("This is not your lobby")]
    WrongLobby,
    #[error("Mercenary selection is required before readying up")]
    MercenaryRequired,
    #[error("Game error | {0}")]
    GameError(#[from] GameError),
}
