use crate::{
    infra::UserClaims,
    models::{
        Card, GameError,
        game::fodinha_power::PowerCardType,
        id::{CardId, PlayerId},
    },
};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<PowerCardStateDto>,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct PowerCardStateDto {
    pub ready: bool,
    pub reason: Option<String>,
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlayerStatsResponse {
    pub player_id: String,
    pub player: Option<UserClaims>,
    pub games_played: i64,
    pub matches_won: i64,
    pub rounds_won: i64,
    pub trump_cards: i64,
    pub bid_count: i64,
    pub total_bid: i64,
    pub average_bid: f64,
    pub bids_hit: i64,
    pub bids_missed: i64,
    pub bid_accuracy: f64,
    pub win_rate: f64,
    pub favorite_card: Option<Card>,
    pub favorite_card_wins: i64,
}
