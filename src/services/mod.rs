use manager::PlayerId;

use crate::models::Card;

pub mod manager;
pub mod repositories;

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq)]
pub struct GameInfoDto {
    pub info: Vec<PlayerInfoDto>,
    pub deck: Vec<Card>,
    pub upcard: Card,
    pub current_player: String,
    pub stage: GameStageDto,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq)]
#[serde(tag = "type", content = "data")]
pub enum GameStageDto {
    Bidding { possible_bids: Vec<usize> },
    Dealing,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq)]
pub struct PlayerInfoDto {
    pub id: PlayerId,
    pub lifes: usize,
    pub rounds: usize,
    pub bid: Option<usize>,
}
