use crate::{
    infra::AuthError,
    models::{BiddingError, DealError, game::GameCommandError},
};

pub use fodinha_core::services::{
    GameInfoDto, GameStageDto, LobbyError, PlayerInfoDto, PlayerManaDto, PlayerStatsResponse,
    PowerCardDto, PowerCardStateDto, PowerPhaseDto,
};

pub mod card_definitions;
pub mod manager;
pub mod matches;
pub mod mercenaries;
pub mod object_storage;
pub use crate::infra::repositories;
pub mod stats;
pub(crate) mod tasks;

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
