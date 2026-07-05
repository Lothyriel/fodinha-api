pub mod fodinha_classic;
pub mod fodinha_power;
pub mod power_lua;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{
    infra::UserClaims,
    models::{GameError, Turn, id::PlayerId},
    services::{GameInfoDto, PowerCardDto},
};

pub use fodinha_classic::{BiddingState, DealState, DealingMode, DeckShuffle, GameOutcome, NewSet};

#[derive(Debug, Clone)]
pub struct AppliedTurn {
    pub state: DealState,
    pub power_decks: Option<IndexMap<PlayerId, Vec<PowerCardDto>>>,
}

#[derive(Debug, Clone)]
pub enum AppliedGameChange {
    BidPlaced {
        player_id: PlayerId,
        bid: usize,
        state: BiddingState,
    },
    TurnPlayed(AppliedTurn),
    PowerCardPlayed(fodinha_power::PowerCardOutcome),
}

impl From<fodinha_classic::AppliedGameChange> for AppliedGameChange {
    fn from(change: fodinha_classic::AppliedGameChange) -> Self {
        match change {
            fodinha_classic::AppliedGameChange::BidPlaced {
                player_id,
                bid,
                state,
            } => Self::BidPlaced {
                player_id,
                bid,
                state,
            },
            fodinha_classic::AppliedGameChange::TurnPlayed(state) => {
                Self::TurnPlayed(AppliedTurn {
                    state,
                    power_decks: None,
                })
            }
        }
    }
}

impl From<fodinha_power::AppliedGameChange> for AppliedGameChange {
    fn from(change: fodinha_power::AppliedGameChange) -> Self {
        match change {
            fodinha_power::AppliedGameChange::BidPlaced {
                player_id,
                bid,
                state,
            } => Self::BidPlaced {
                player_id,
                bid,
                state,
            },
            fodinha_power::AppliedGameChange::TurnPlayed { state, power_decks } => {
                Self::TurnPlayed(AppliedTurn { state, power_decks })
            }
            fodinha_power::AppliedGameChange::PowerCardPlayed(outcome) => {
                Self::PowerCardPlayed(outcome)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameType {
    FodinhaClassic,
    FodinhaPower,
}

impl std::fmt::Display for GameType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::FodinhaClassic => "fodinha_classic",
            Self::FodinhaPower => "fodinha_power",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "game_type", content = "settings", rename_all = "snake_case")]
pub enum GameSettings {
    FodinhaClassic(fodinha_classic::GameSettings),
    FodinhaPower(fodinha_power::GameSettings),
}

impl Default for GameSettings {
    fn default() -> Self {
        Self::FodinhaClassic(fodinha_classic::GameSettings::default())
    }
}

impl GameSettings {
    pub fn game_type(&self) -> GameType {
        match self {
            Self::FodinhaClassic(_) => GameType::FodinhaClassic,
            Self::FodinhaPower(_) => GameType::FodinhaPower,
        }
    }

    pub fn max_players(&self) -> usize {
        match self {
            Self::FodinhaClassic(_) => fodinha_classic::MAX_PLAYER_COUNT,
            Self::FodinhaPower(_) => fodinha_power::MAX_PLAYER_COUNT,
        }
    }
}

impl<'de> Deserialize<'de> for GameSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        #[derive(Deserialize)]
        #[serde(tag = "game_type", content = "settings", rename_all = "snake_case")]
        enum TaggedGameSettings {
            FodinhaClassic(fodinha_classic::GameSettings),
            FodinhaPower(fodinha_power::GameSettings),
        }

        if value.get("game_type").is_some() {
            return match serde_json::from_value::<TaggedGameSettings>(value)
                .map_err(serde::de::Error::custom)?
            {
                TaggedGameSettings::FodinhaClassic(settings) => Ok(Self::FodinhaClassic(settings)),
                TaggedGameSettings::FodinhaPower(settings) => Ok(Self::FodinhaPower(settings)),
            };
        }

        serde_json::from_value::<fodinha_classic::GameSettings>(value)
            .map(Self::FodinhaClassic)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "game_type", content = "event", rename_all = "snake_case")]
pub enum GameEvent {
    FodinhaClassic(fodinha_classic::MatchEvent),
    FodinhaPower(fodinha_power::MatchEvent),
}

impl GameEvent {
    pub fn game_type(&self) -> GameType {
        match self {
            Self::FodinhaClassic(_) => GameType::FodinhaClassic,
            Self::FodinhaPower(_) => GameType::FodinhaPower,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum MatchEvent {
    MatchCreated { settings: GameSettings },
    PlayerJoined { user_claims: UserClaims },
    PlayerStatusChanged { player_id: PlayerId, ready: bool },
    Game(GameEvent),
}

impl<'de> Deserialize<'de> for MatchEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        #[derive(Deserialize)]
        #[serde(tag = "type", content = "data")]
        enum TaggedMatchEvent {
            MatchCreated { settings: GameSettings },
            PlayerJoined { user_claims: UserClaims },
            PlayerStatusChanged { player_id: PlayerId, ready: bool },
            Game(GameEvent),
        }

        match serde_json::from_value::<TaggedMatchEvent>(value.clone()) {
            Ok(TaggedMatchEvent::MatchCreated { settings }) => Ok(Self::MatchCreated { settings }),
            Ok(TaggedMatchEvent::PlayerJoined { user_claims }) => {
                Ok(Self::PlayerJoined { user_claims })
            }
            Ok(TaggedMatchEvent::PlayerStatusChanged { player_id, ready }) => {
                Ok(Self::PlayerStatusChanged { player_id, ready })
            }
            Ok(TaggedMatchEvent::Game(event)) => Ok(Self::Game(event)),
            Err(new_error) => serde_json::from_value::<fodinha_classic::MatchEvent>(value)
                .map(|event| Self::Game(GameEvent::FodinhaClassic(event)))
                .map_err(|legacy_error| {
                    serde::de::Error::custom(format!(
                        "invalid match event: {new_error}; legacy fodinha_classic event: {legacy_error}"
                    ))
                }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "game_type", content = "command", rename_all = "snake_case")]
pub enum GameCommand {
    FodinhaClassic(fodinha_classic::GameCommand),
    FodinhaPower(fodinha_power::GameCommand),
}

impl GameCommand {
    pub fn game_type(&self) -> GameType {
        match self {
            Self::FodinhaClassic(_) => GameType::FodinhaClassic,
            Self::FodinhaPower(_) => GameType::FodinhaPower,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::FodinhaClassic(command) => command.kind(),
            Self::FodinhaPower(command) => command.kind(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Game {
    FodinhaClassic(fodinha_classic::Game),
    FodinhaPower(fodinha_power::Game),
}

impl Game {
    pub fn start_match_event(
        players: &[PlayerId],
        settings: GameSettings,
    ) -> Result<MatchEvent, GameError> {
        match settings {
            GameSettings::FodinhaClassic(settings) => {
                fodinha_classic::Game::start_match_event(players, settings)
                    .map(GameEvent::FodinhaClassic)
                    .map(MatchEvent::Game)
            }
            GameSettings::FodinhaPower(settings) => {
                fodinha_power::Game::start_match_event(players, settings)
                    .map(GameEvent::FodinhaPower)
                    .map(MatchEvent::Game)
            }
        }
    }

    pub fn game_type(&self) -> GameType {
        match self {
            Self::FodinhaClassic(_) => GameType::FodinhaClassic,
            Self::FodinhaPower(_) => GameType::FodinhaPower,
        }
    }

    pub fn is_finished(&self) -> bool {
        match self {
            Self::FodinhaClassic(game) => game.is_finished(),
            Self::FodinhaPower(game) => game.is_finished(),
        }
    }

    pub fn get_game_info(&self, player_id: &PlayerId) -> GameInfoDto {
        match self {
            Self::FodinhaClassic(game) => game.get_game_info(player_id),
            Self::FodinhaPower(game) => game.get_game_info(player_id),
        }
    }

    pub fn get_bidding_player(&self) -> PlayerId {
        match self {
            Self::FodinhaClassic(game) => game.get_bidding_player(),
            Self::FodinhaPower(game) => game.get_bidding_player(),
        }
    }

    pub fn get_possible_bids(&self) -> Vec<usize> {
        match self {
            Self::FodinhaClassic(game) => game.get_possible_bids(),
            Self::FodinhaPower(game) => game.get_possible_bids(),
        }
    }

    pub fn apply_match_event(&mut self, event: GameEvent) -> Result<AppliedGameChange, GameError> {
        match (self, event) {
            (Self::FodinhaClassic(game), GameEvent::FodinhaClassic(event)) => {
                Ok(game.apply_match_event(event).into())
            }
            (Self::FodinhaPower(game), GameEvent::FodinhaPower(event)) => {
                Ok(game.apply_match_event(event).into())
            }
            _ => Err(GameError::InvalidStage),
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum LobbyState {
    Playing(Game),
    NotStarted(GameSettings),
}

#[derive(Debug, thiserror::Error)]
pub enum GameCommandError {
    #[error("invalid game type: expected {expected}, got {actual}")]
    WrongGameType {
        expected: GameType,
        actual: GameType,
    },
    #[error("invalid deal: {0}")]
    Deal(#[from] crate::models::DealError),
    #[error("invalid bid: {0}")]
    Bid(#[from] crate::models::BiddingError),
    #[error("invalid power card: {0}")]
    Power(#[from] fodinha_power::PowerCardError),
}

impl Game {
    pub fn validate_command(
        &self,
        player_id: &PlayerId,
        command: GameCommand,
    ) -> Result<MatchEvent, GameCommandError> {
        let expected = self.game_type();
        let actual = command.game_type();

        if expected != actual {
            return Err(GameCommandError::WrongGameType { expected, actual });
        }

        match (self, command) {
            (Self::FodinhaClassic(game), GameCommand::FodinhaClassic(command)) => match command {
                fodinha_classic::GameCommand::PlayTurn { card } => game
                    .validate_turn(Turn {
                        player_id: player_id.clone(),
                        card,
                    })
                    .map(GameEvent::FodinhaClassic)
                    .map(MatchEvent::Game)
                    .map_err(GameCommandError::Deal),
                fodinha_classic::GameCommand::PutBid { bid } => game
                    .validate_bid(player_id, bid)
                    .map(GameEvent::FodinhaClassic)
                    .map(MatchEvent::Game)
                    .map_err(GameCommandError::Bid),
            },
            (Self::FodinhaPower(game), GameCommand::FodinhaPower(command)) => match command {
                fodinha_power::GameCommand::PlayTurn { card } => game
                    .validate_turn(Turn {
                        player_id: player_id.clone(),
                        card,
                    })
                    .map(GameEvent::FodinhaPower)
                    .map(MatchEvent::Game)
                    .map_err(GameCommandError::Deal),
                fodinha_power::GameCommand::PutBid { bid } => game
                    .validate_bid(player_id, bid)
                    .map(GameEvent::FodinhaPower)
                    .map(MatchEvent::Game)
                    .map_err(GameCommandError::Bid),
                fodinha_power::GameCommand::UsePowerCard {
                    card_id,
                    target_player_id,
                } => game
                    .validate_power_card(player_id, &card_id, target_player_id)
                    .map(GameEvent::FodinhaPower)
                    .map(MatchEvent::Game)
                    .map_err(GameCommandError::Power),
            },
            _ => unreachable!("wrong game type was checked before command dispatch"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use crate::models::{
        Card, Rank, Suit,
        game::{fodinha_classic, fodinha_power},
        id::PlayerId,
    };

    use super::*;

    #[test]
    fn fodinha_settings_serializes_only_lifes() {
        let settings = GameSettings::FodinhaClassic(fodinha_classic::GameSettings { lifes: 5 });

        let document = mongodb::bson::to_document(&settings).unwrap();
        let inner = document.get_document("settings").unwrap();

        assert_eq!(document.get_str("game_type"), Ok("fodinha_classic"));
        assert_eq!(inner.len(), 1);
        assert_eq!(inner.get_i64("lifes"), Ok(5));
        assert!(!inner.contains_key("cards_count"));
        assert!(!inner.contains_key("mode"));
        assert!(!inner.contains_key("max_players"));
    }

    #[test]
    fn fodinha_power_settings_serializes_only_lifes() {
        let settings = GameSettings::FodinhaPower(fodinha_power::GameSettings { lifes: 50 });

        let document = mongodb::bson::to_document(&settings).unwrap();
        let inner = document.get_document("settings").unwrap();

        assert_eq!(document.get_str("game_type"), Ok("fodinha_power"));
        assert_eq!(inner.len(), 1);
        assert_eq!(inner.get_i64("lifes"), Ok(50));
    }

    #[test]
    fn game_event_round_trips_through_bson() {
        let player_id = PlayerId(Arc::from("player-1"));
        let event = MatchEvent::Game(GameEvent::FodinhaClassic(
            fodinha_classic::MatchEvent::BidPlaced {
                player_id: player_id.clone(),
                bid: 1,
            },
        ));

        let document = mongodb::bson::to_document(&event).unwrap();
        let decoded: MatchEvent = mongodb::bson::from_document(document).unwrap();

        match decoded {
            MatchEvent::Game(GameEvent::FodinhaClassic(
                fodinha_classic::MatchEvent::BidPlaced {
                    player_id: decoded_player_id,
                    bid,
                },
            )) => {
                assert_eq!(decoded_player_id, player_id);
                assert_eq!(bid, 1);
            }
            decoded => panic!("unexpected decoded event: {decoded:?}"),
        }
    }

    #[test]
    fn power_game_event_round_trips_through_bson() {
        let player_id = PlayerId(Arc::from("player-1"));
        let event = MatchEvent::Game(GameEvent::FodinhaPower(
            fodinha_power::MatchEvent::PowerCardPlayed {
                player_id: player_id.clone(),
                card: fodinha_power::PowerCard {
                    id: "heal_10".to_string(),
                    name: "Heal 10".to_string(),
                    description: "Restore 10 lives to yourself.".to_string(),
                    requires_target: false,
                },
                target_player_id: None,
                effects: fodinha_power::PowerCardEffects {
                    lifes: HashMap::from([(player_id.clone(), 60)]),
                },
            },
        ));

        let document = mongodb::bson::to_document(&event).unwrap();
        let decoded: MatchEvent = mongodb::bson::from_document(document).unwrap();

        match decoded {
            MatchEvent::Game(GameEvent::FodinhaPower(
                fodinha_power::MatchEvent::PowerCardPlayed {
                    player_id: decoded_player_id,
                    effects,
                    ..
                },
            )) => {
                assert_eq!(decoded_player_id, player_id);
                assert_eq!(effects.lifes.get(&player_id), Some(&60));
            }
            decoded => panic!("unexpected decoded event: {decoded:?}"),
        }
    }

    #[test]
    fn legacy_fodinha_event_deserializes_as_typed_game_event() {
        let player_id = PlayerId(Arc::from("player-1"));
        let legacy_event = fodinha_classic::MatchEvent::TurnPlayed {
            turn: crate::models::Turn {
                player_id: player_id.clone(),
                card: Card::new(Rank::Four, Suit::Golds),
            },
            next_set: None,
        };

        let document = mongodb::bson::to_document(&legacy_event).unwrap();
        let decoded: MatchEvent = mongodb::bson::from_document(document).unwrap();

        match decoded {
            MatchEvent::Game(GameEvent::FodinhaClassic(
                fodinha_classic::MatchEvent::TurnPlayed { turn, next_set },
            )) => {
                assert_eq!(turn.player_id, player_id);
                assert_eq!(next_set, None);
            }
            decoded => panic!("unexpected decoded legacy event: {decoded:?}"),
        }
    }
}
