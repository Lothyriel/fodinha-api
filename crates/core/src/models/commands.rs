use std::collections::HashMap;

use crate::{
    infra::UserClaims,
    models::{
        Card, Turn,
        game::{GameCommand, GameType},
        id::{DeckId, LobbyId, MercenaryId, PlayerId},
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

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Clone)]
pub enum LobbyInfo {
    NotStarted(WaitingLobbySnapshot),
    Playing(GameInfoDto),
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Clone)]
#[serde(tag = "type", content = "data")]
pub enum MatchSnapshot {
    Waiting(WaitingLobbySnapshot),
    Playing(PlayingMatchSnapshot),
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Clone)]
pub struct WaitingLobbySnapshot {
    pub players: HashMap<PlayerId, PlayerStatus>,
    pub settings: WaitingLobbySettingsDto,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Clone)]
#[serde(tag = "game_type", rename_all = "snake_case")]
pub enum WaitingLobbySettingsDto {
    FodinhaClassic,
    FodinhaPower {
        power_deck_id: DeckId,
        life_multiplier: f64,
    },
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

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Clone)]
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
    PlayersLifesChanged(PlayerPoints),
    PlayerBiddingTurn {
        player_id: PlayerId,
        possible_bids: Vec<usize>,
    },
    PlayerPowerTurn {
        player_id: PlayerId,
        phase: crate::services::PowerPhaseDto,
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
    DeckRevealed {
        target_player_id: PlayerId,
        cards: Vec<Card>,
    },
    PlayerPowerCards(Vec<PowerCardDto>),
    PowerCardPlayed {
        player_id: PlayerId,
        card: PowerCardDto,
        targets: Vec<PlayerId>,
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn classic_waiting_settings_have_no_power_placeholders() {
        let value = serde_json::to_value(WaitingLobbySettingsDto::FodinhaClassic).unwrap();

        assert_eq!(value, serde_json::json!({ "game_type": "fodinha_classic" }));
    }

    #[test]
    fn power_waiting_settings_round_trip_as_typed_payload() {
        let settings = WaitingLobbySettingsDto::FodinhaPower {
            power_deck_id: DeckId(Arc::from("starter")),
            life_multiplier: 2.0,
        };
        let value = serde_json::to_value(&settings).unwrap();

        assert_eq!(
            value,
            serde_json::json!({
                "game_type": "fodinha_power",
                "power_deck_id": "starter",
                "life_multiplier": 2.0
            })
        );
        assert_eq!(
            serde_json::from_value::<WaitingLobbySettingsDto>(value).unwrap(),
            settings
        );
    }
}
