pub mod fodinha_classic;
pub mod fodinha_power;
pub mod power_lua;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use std::collections::HashMap;

use crate::{
    infra::UserClaims,
    models::{
        Card, GameError, Turn,
        id::{CardId, PlayerId},
    },
    services::{GameInfoDto, PlayerManaDto, PowerCardDto},
};

pub use fodinha_classic::{BiddingState, DealState, DealingMode, DeckShuffle, GameOutcome, NewSet};

#[derive(Debug, Clone)]
pub struct AppliedTurn {
    pub state: DealState,
    pub power_decks: Option<IndexMap<PlayerId, Vec<PowerCardDto>>>,
    pub mana: Option<HashMap<PlayerId, PlayerManaDto>>,
}

#[derive(Debug, Clone)]
pub enum AppliedGameChange {
    BidPlaced {
        player_id: PlayerId,
        bid: usize,
        state: BiddingState,
        mana: HashMap<PlayerId, PlayerManaDto>,
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
                mana: HashMap::new(),
            },
            fodinha_classic::AppliedGameChange::TurnPlayed(state) => {
                Self::TurnPlayed(AppliedTurn {
                    state,
                    power_decks: None,
                    mana: None,
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
                mana,
            } => Self::BidPlaced {
                player_id,
                bid,
                state,
                mana,
            },
            fodinha_power::AppliedGameChange::TurnPlayed {
                state,
                power_decks,
                mana,
            } => Self::TurnPlayed(AppliedTurn {
                state,
                power_decks,
                mana,
            }),
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

#[allow(clippy::large_enum_variant)]
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

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MatchEvent {
    MatchCreated { settings: GameSettings },
    PlayerJoined { user_claims: UserClaims },
    PlayerStatusChanged { player_id: PlayerId, ready: bool },
    Game(GameEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum InferredGameCommand {
    PlayTurn {
        card: Card,
    },
    PutBid {
        bid: usize,
    },
    UsePowerCard {
        card_id: CardId,
        target_player_id: Option<PlayerId>,
    },
}

impl InferredGameCommand {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::PlayTurn { .. } => "game.inferred.play_turn",
            Self::PutBid { .. } => "game.inferred.put_bid",
            Self::UsePowerCard { .. } => "game.inferred.use_power_card",
        }
    }

    fn wire_type(&self) -> &'static str {
        match self {
            Self::PlayTurn { .. } => "PlayTurn",
            Self::PutBid { .. } => "PutBid",
            Self::UsePowerCard { .. } => "UsePowerCard",
        }
    }

    fn into_game_command(self, game_type: GameType) -> Result<GameCommand, GameCommandError> {
        match game_type {
            GameType::FodinhaClassic => match self {
                Self::PlayTurn { card } => Ok(GameCommand::FodinhaClassic(
                    fodinha_classic::GameCommand::PlayTurn { card },
                )),
                Self::PutBid { bid } => Ok(GameCommand::FodinhaClassic(
                    fodinha_classic::GameCommand::PutBid { bid },
                )),
                command @ Self::UsePowerCard { .. } => Err(GameCommandError::UnsupportedCommand {
                    game_type,
                    command: command.wire_type(),
                }),
            },
            GameType::FodinhaPower => match self {
                Self::PlayTurn { card } => Ok(GameCommand::FodinhaPower(
                    fodinha_power::GameCommand::PlayTurn { card },
                )),
                Self::PutBid { bid } => Ok(GameCommand::FodinhaPower(
                    fodinha_power::GameCommand::PutBid { bid },
                )),
                Self::UsePowerCard {
                    card_id,
                    target_player_id,
                } => Ok(GameCommand::FodinhaPower(
                    fodinha_power::GameCommand::UsePowerCard {
                        card_id,
                        target_player_id,
                    },
                )),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub enum GameCommand {
    FodinhaClassic(fodinha_classic::GameCommand),
    FodinhaPower(fodinha_power::GameCommand),
    Inferred(InferredGameCommand),
}

impl GameCommand {
    pub fn game_type(&self) -> Option<GameType> {
        match self {
            Self::FodinhaClassic(_) => Some(GameType::FodinhaClassic),
            Self::FodinhaPower(_) => Some(GameType::FodinhaPower),
            Self::Inferred(_) => None,
        }
    }

    pub fn into_typed(self, game_type: GameType) -> Result<Self, GameCommandError> {
        match self {
            Self::Inferred(command) => command.into_game_command(game_type),
            command => Ok(command),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::FodinhaClassic(command) => command.kind(),
            Self::FodinhaPower(command) => command.kind(),
            Self::Inferred(command) => command.kind(),
        }
    }
}

impl Serialize for GameCommand {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        #[serde(tag = "game_type", content = "command", rename_all = "snake_case")]
        enum TaggedGameCommand<'a> {
            FodinhaClassic(&'a fodinha_classic::GameCommand),
            FodinhaPower(&'a fodinha_power::GameCommand),
        }

        #[derive(Serialize)]
        struct InferredGameCommandEnvelope<'a> {
            command: &'a InferredGameCommand,
        }

        match self {
            Self::FodinhaClassic(command) => {
                TaggedGameCommand::FodinhaClassic(command).serialize(serializer)
            }
            Self::FodinhaPower(command) => {
                TaggedGameCommand::FodinhaPower(command).serialize(serializer)
            }
            Self::Inferred(command) => {
                InferredGameCommandEnvelope { command }.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for GameCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        #[derive(Deserialize)]
        #[serde(tag = "game_type", content = "command", rename_all = "snake_case")]
        enum TaggedGameCommand {
            FodinhaClassic(fodinha_classic::GameCommand),
            FodinhaPower(fodinha_power::GameCommand),
        }

        #[derive(Deserialize)]
        struct InferredGameCommandEnvelope {
            command: InferredGameCommand,
        }

        if value.get("game_type").is_some() {
            return match serde_json::from_value::<TaggedGameCommand>(value)
                .map_err(serde::de::Error::custom)?
            {
                TaggedGameCommand::FodinhaClassic(command) => Ok(Self::FodinhaClassic(command)),
                TaggedGameCommand::FodinhaPower(command) => Ok(Self::FodinhaPower(command)),
            };
        }

        serde_json::from_value::<InferredGameCommandEnvelope>(value)
            .map(|envelope| Self::Inferred(envelope.command))
            .map_err(serde::de::Error::custom)
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Game {
    FodinhaClassic(fodinha_classic::Game),
    FodinhaPower(fodinha_power::Game),
}

impl Game {
    pub fn start_match_event(
        players: &[PlayerId],
        settings: GameSettings,
        power_card_registry: &fodinha_power::PowerCardRegistry,
    ) -> Result<MatchEvent, GameError> {
        match settings {
            GameSettings::FodinhaClassic(settings) => {
                fodinha_classic::Game::start_match_event(players, settings)
                    .map(GameEvent::FodinhaClassic)
                    .map(MatchEvent::Game)
            }
            GameSettings::FodinhaPower(settings) => {
                fodinha_power::Game::start_match_event(players, settings, power_card_registry)
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
    #[error("missing game type for unresolved command")]
    UnresolvedGameType,
    #[error("command {command} is not valid for {game_type}")]
    UnsupportedCommand {
        game_type: GameType,
        command: &'static str,
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
        let actual = command
            .game_type()
            .ok_or(GameCommandError::UnresolvedGameType)?;

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
    fn fodinha_power_settings_serializes_lifes_and_power_deck() {
        let settings = GameSettings::FodinhaPower(fodinha_power::GameSettings {
            lifes: 50,
            power_deck_id: crate::models::id::DeckId(Arc::from("test_deck")),
            player_mercenaries: HashMap::new(),
        });

        let document = mongodb::bson::to_document(&settings).unwrap();
        let inner = document.get_document("settings").unwrap();

        assert_eq!(document.get_str("game_type"), Ok("fodinha_power"));
        assert_eq!(inner.len(), 3);
        assert_eq!(inner.get_i64("lifes"), Ok(50));
        assert_eq!(inner.get_str("power_deck_id"), Ok("test_deck"));
        assert_eq!(inner.get_document("player_mercenaries").unwrap().len(), 0);
    }

    #[test]
    fn client_game_command_deserializes_without_game_type() {
        let message = serde_json::json!({
            "type": "GameCommand",
            "data": {
                "command": {
                    "type": "PutBid",
                    "data": { "bid": 2 }
                }
            }
        });

        let command = match serde_json::from_value::<crate::models::commands::ClientCommand>(
            message,
        )
        .unwrap()
        {
            crate::models::commands::ClientCommand::GameCommand(command) => command,
            command => panic!("unexpected command: {command:?}"),
        };

        let command = command.into_typed(GameType::FodinhaPower).unwrap();

        match command {
            GameCommand::FodinhaPower(fodinha_power::GameCommand::PutBid { bid }) => {
                assert_eq!(bid, 2);
            }
            command => panic!("unexpected command: {command:?}"),
        }
    }

    #[test]
    fn typed_game_command_deserializes_existing_envelope() {
        let command = serde_json::json!({
            "game_type": "fodinha_classic",
            "command": {
                "type": "PutBid",
                "data": { "bid": 1 }
            }
        });

        let command = serde_json::from_value::<GameCommand>(command).unwrap();

        match command {
            GameCommand::FodinhaClassic(fodinha_classic::GameCommand::PutBid { bid }) => {
                assert_eq!(bid, 1);
            }
            command => panic!("unexpected command: {command:?}"),
        }
    }

    #[test]
    fn inferred_power_card_command_is_rejected_for_classic() {
        let command = GameCommand::Inferred(InferredGameCommand::UsePowerCard {
            card_id: crate::models::id::CardId(Arc::from("heal_10")),
            target_player_id: None,
        });

        let error = command.into_typed(GameType::FodinhaClassic).unwrap_err();

        assert!(matches!(
            error,
            GameCommandError::UnsupportedCommand {
                game_type: GameType::FodinhaClassic,
                command: "UsePowerCard"
            }
        ));
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
                    id: crate::models::id::CardId(Arc::from("heal_10")),
                    name: "Heal 10".to_string(),
                    description: "Restore 10 lives to yourself.".to_string(),
                    mana_cost: 2,
                    card_type: fodinha_power::PowerCardType::Instant,
                    image_url: None,
                },
                target_player_id: None,
                effects: fodinha_power::PowerCardEffects {
                    lifes: HashMap::from([(player_id.clone(), 60)]),
                    mana: HashMap::new(),
                    decks: HashMap::new(),
                    power_decks: HashMap::new(),
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
}
