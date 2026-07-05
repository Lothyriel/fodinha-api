use std::collections::HashMap;

use indexmap::IndexMap;

use crate::{
    models::{
        BiddingError, Card, DealError, GameError, Turn,
        game::{
            BiddingState, DealState, DeckShuffle, NewSet, fodinha_classic,
            power_lua::{PowerScriptError, PowerScriptInput, PowerScriptOutput, ScriptPlayerState},
        },
        id::PlayerId,
        util::DeterministicRng,
    },
    services::{GameInfoDto, PowerCardDto},
};

const LIFE_LOSS_PER_BID_DIFFERENCE: usize = 10;
const POWER_CARDS_PER_PLAYER: usize = 1;
const POWER_CARD_RNG_SEQUENCE_MULTIPLIER: u64 = 0x517C_C1B7_2722_0A95;
pub const MAX_PLAYER_COUNT: usize = fodinha_classic::MAX_PLAYER_COUNT;

#[derive(Debug, Clone)]
pub struct Game {
    core: fodinha_classic::Game,
    power_decks: IndexMap<PlayerId, Vec<PowerCard>>,
    power_seed: i64,
    next_power_shuffle_sequence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GameSettings {
    pub lifes: usize,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self { lifes: 50 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PowerSet {
    pub shuffle: DeckShuffle,
    pub decks: IndexMap<PlayerId, Vec<PowerCard>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PowerCard {
    pub id: String,
    pub name: String,
    pub description: String,
    pub requires_target: bool,
}

impl PowerCard {
    pub fn to_dto(&self) -> PowerCardDto {
        PowerCardDto {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            requires_target: self.requires_target,
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
#[serde(tag = "type", content = "data")]
pub enum GameCommand {
    PlayTurn {
        card: Card,
    },
    PutBid {
        bid: usize,
    },
    UsePowerCard {
        card_id: String,
        target_player_id: Option<PlayerId>,
    },
}

impl GameCommand {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::PlayTurn { .. } => "game.fodinha_power.play_turn",
            Self::PutBid { .. } => "game.fodinha_power.put_bid",
            Self::UsePowerCard { .. } => "game.fodinha_power.use_power_card",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MatchEvent {
    GameStarted {
        settings: GameSettings,
        set: NewSet,
        power_set: PowerSet,
    },
    BidPlaced {
        player_id: PlayerId,
        bid: usize,
    },
    TurnPlayed {
        turn: Turn,
        next_set: Option<NewSet>,
        next_power_set: Option<PowerSet>,
    },
    PowerCardPlayed {
        player_id: PlayerId,
        card: PowerCard,
        target_player_id: Option<PlayerId>,
        effects: PowerCardEffects,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PowerCardEffects {
    pub lifes: HashMap<PlayerId, usize>,
}

#[derive(Debug, Clone)]
pub enum AppliedGameChange {
    BidPlaced {
        player_id: PlayerId,
        bid: usize,
        state: BiddingState,
    },
    TurnPlayed {
        state: DealState,
        power_decks: Option<IndexMap<PlayerId, Vec<PowerCardDto>>>,
    },
    PowerCardPlayed(PowerCardOutcome),
}

#[derive(Debug, Clone)]
pub struct PowerCardOutcome {
    pub player_id: PlayerId,
    pub card: PowerCardDto,
    pub target_player_id: Option<PlayerId>,
    pub lifes: HashMap<PlayerId, usize>,
    pub ended: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PowerCardError {
    #[error("power cards can only be used during bidding")]
    BiddingStageRequired,
    #[error("not your turn")]
    NotYourTurn,
    #[error("invalid player")]
    InvalidPlayer,
    #[error("invalid target")]
    InvalidTarget,
    #[error("target is required")]
    TargetRequired,
    #[error("invalid power card")]
    InvalidPowerCard,
    #[error("power card script failed: {0}")]
    Script(#[from] PowerScriptError),
}

struct PowerCardDefinition {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    requires_target: bool,
    script: &'static str,
}

impl PowerCardDefinition {
    fn to_card(&self) -> PowerCard {
        PowerCard {
            id: self.id.to_string(),
            name: self.name.to_string(),
            description: self.description.to_string(),
            requires_target: self.requires_target,
        }
    }
}

impl Game {
    pub fn new(players: &[PlayerId], settings: GameSettings) -> Result<Self, GameError> {
        Self::new_with_seed(players, settings, rand::random())
    }

    pub fn new_with_seed(
        players: &[PlayerId],
        settings: GameSettings,
        seed: i64,
    ) -> Result<Self, GameError> {
        let event = Self::start_match_event_with_seed(players, settings, seed)?;

        match event {
            MatchEvent::GameStarted {
                settings,
                set,
                power_set,
            } => Self::from_started(players, settings, set, power_set),
            _ => unreachable!("start_match_event only emits GameStarted"),
        }
    }

    pub fn start_match_event(
        players: &[PlayerId],
        settings: GameSettings,
    ) -> Result<MatchEvent, GameError> {
        Self::start_match_event_with_seed(players, settings, rand::random())
    }

    pub fn start_match_event_with_seed(
        players: &[PlayerId],
        settings: GameSettings,
        seed: i64,
    ) -> Result<MatchEvent, GameError> {
        let classic_settings = Self::classic_settings(&settings);
        let classic_event =
            fodinha_classic::Game::start_match_event_with_seed(players, classic_settings, seed)?;
        let fodinha_classic::MatchEvent::GameStarted { set, .. } = classic_event else {
            unreachable!("classic start_match_event only emits GameStarted")
        };
        let power_set = Self::new_power_set(players, seed, 0);

        Ok(MatchEvent::GameStarted {
            settings,
            set,
            power_set,
        })
    }

    pub fn from_started(
        players: &[PlayerId],
        settings: GameSettings,
        set: NewSet,
        power_set: PowerSet,
    ) -> Result<Self, GameError> {
        let classic = fodinha_classic::Game::from_started_with_rules(
            players,
            Self::classic_settings(&settings),
            set,
            fodinha_classic::GameRules {
                life_loss_per_bid_difference: LIFE_LOSS_PER_BID_DIFFERENCE,
            },
        )?;

        Ok(Self {
            core: classic,
            power_decks: power_set.decks,
            power_seed: power_set.shuffle.seed,
            next_power_shuffle_sequence: power_set.shuffle.sequence.wrapping_add(1),
        })
    }

    pub fn validate_bid(
        &self,
        player_id: &PlayerId,
        bid: usize,
    ) -> Result<MatchEvent, BiddingError> {
        self.core
            .validate_bid(player_id, bid)
            .map(Self::from_classic_event)
    }

    pub fn validate_turn(&self, turn: Turn) -> Result<MatchEvent, DealError> {
        let event = self.core.validate_turn(turn)?;
        let fodinha_classic::MatchEvent::TurnPlayed { turn, next_set } = event else {
            unreachable!("validate_turn only emits TurnPlayed")
        };
        let next_power_set = next_set.as_ref().map(|set| {
            let players: Vec<_> = set.decks.keys().cloned().collect();

            self.new_power_set_for_game(&players)
        });

        Ok(MatchEvent::TurnPlayed {
            turn,
            next_set,
            next_power_set,
        })
    }

    pub fn validate_power_card(
        &self,
        player_id: &PlayerId,
        card_id: &str,
        target_player_id: Option<PlayerId>,
    ) -> Result<MatchEvent, PowerCardError> {
        if !self.core.is_bidding_stage() {
            return Err(PowerCardError::BiddingStageRequired);
        }

        if self.core.current_player().as_ref() != Some(player_id) {
            return Err(PowerCardError::NotYourTurn);
        }

        if !self.core.is_player_alive(player_id) {
            return Err(PowerCardError::InvalidPlayer);
        }

        let card = self
            .power_decks
            .get(player_id)
            .and_then(|deck| deck.iter().find(|card| card.id == card_id))
            .cloned()
            .ok_or(PowerCardError::InvalidPowerCard)?;

        let definition = power_card_definition(&card.id).ok_or(PowerCardError::InvalidPowerCard)?;

        if definition.requires_target && target_player_id.is_none() {
            return Err(PowerCardError::TargetRequired);
        }

        if let Some(target_player_id) = target_player_id.as_ref()
            && !self.core.is_player_alive(target_player_id)
        {
            return Err(PowerCardError::InvalidTarget);
        }

        let output = self.run_power_script(definition, player_id, target_player_id.clone())?;

        Ok(MatchEvent::PowerCardPlayed {
            player_id: player_id.clone(),
            card,
            target_player_id,
            effects: PowerCardEffects {
                lifes: output.lifes,
            },
        })
    }

    pub fn apply_match_event(&mut self, event: MatchEvent) -> AppliedGameChange {
        match event {
            MatchEvent::BidPlaced { player_id, bid } => {
                match self
                    .core
                    .apply_match_event(fodinha_classic::MatchEvent::BidPlaced { player_id, bid })
                {
                    fodinha_classic::AppliedGameChange::BidPlaced {
                        player_id,
                        bid,
                        state,
                    } => AppliedGameChange::BidPlaced {
                        player_id,
                        bid,
                        state,
                    },
                    _ => unreachable!("bid event applies as bid change"),
                }
            }
            MatchEvent::TurnPlayed {
                turn,
                next_set,
                next_power_set,
            } => {
                let state = match self
                    .core
                    .apply_match_event(fodinha_classic::MatchEvent::TurnPlayed { turn, next_set })
                {
                    fodinha_classic::AppliedGameChange::TurnPlayed(state) => state,
                    _ => unreachable!("turn event applies as turn change"),
                };
                let power_decks = next_power_set.as_ref().map(|set| {
                    self.apply_power_set(set);
                    dto_decks(&set.decks)
                });

                AppliedGameChange::TurnPlayed { state, power_decks }
            }
            MatchEvent::PowerCardPlayed {
                player_id,
                card,
                target_player_id,
                effects,
            } => {
                if let Some(deck) = self.power_decks.get_mut(&player_id)
                    && let Some(idx) = deck.iter().position(|held| held.id == card.id)
                {
                    deck.remove(idx);
                }

                self.core.apply_life_totals(&effects.lifes);

                AppliedGameChange::PowerCardPlayed(PowerCardOutcome {
                    player_id,
                    card: card.to_dto(),
                    target_player_id,
                    lifes: self.core.get_lifes(),
                    ended: self.core.is_finished(),
                })
            }
            MatchEvent::GameStarted { .. } => unreachable!("GameStarted is applied by facade"),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.core.is_finished()
    }

    pub fn get_game_info(&self, player_id: &PlayerId) -> GameInfoDto {
        let mut info = self.core.get_game_info(player_id);
        info.power_cards = Some(
            self.power_decks
                .get(player_id)
                .map(|deck| deck.iter().map(PowerCard::to_dto).collect())
                .unwrap_or_default(),
        );

        info
    }

    pub fn get_bidding_player(&self) -> PlayerId {
        self.core.get_bidding_player()
    }

    pub fn get_possible_bids(&self) -> Vec<usize> {
        self.core.get_possible_bids()
    }

    fn run_power_script(
        &self,
        definition: &PowerCardDefinition,
        owner_id: &PlayerId,
        target_player_id: Option<PlayerId>,
    ) -> Result<PowerScriptOutput, PowerCardError> {
        let players = self
            .core
            .get_player_snapshots()
            .into_iter()
            .filter(|(_, player)| player.lifes > 0)
            .map(|(player_id, player)| {
                (
                    player_id,
                    ScriptPlayerState {
                        lifes: player.lifes,
                        bid: player.bid,
                        rounds: player.rounds,
                    },
                )
            })
            .collect();

        Ok(super::power_lua::run_power_card_script(
            definition.script,
            PowerScriptInput {
                owner_id: owner_id.clone(),
                target_player_id,
                players,
            },
        )?)
    }

    fn classic_settings(settings: &GameSettings) -> fodinha_classic::GameSettings {
        fodinha_classic::GameSettings {
            lifes: settings.lifes,
        }
    }

    fn apply_power_set(&mut self, set: &PowerSet) {
        self.power_decks = set.decks.clone();
        self.power_seed = set.shuffle.seed;
        self.next_power_shuffle_sequence = set.shuffle.sequence.wrapping_add(1);
    }

    fn new_power_set_for_game(&self, players: &[PlayerId]) -> PowerSet {
        Self::new_power_set(players, self.power_seed, self.next_power_shuffle_sequence)
    }

    fn new_power_set(players: &[PlayerId], seed: i64, sequence: i64) -> PowerSet {
        let definitions = default_power_card_definitions();
        let needed_cards = players.len().saturating_mul(POWER_CARDS_PER_PLAYER);
        let mut deck = (0..needed_cards)
            .map(|idx| definitions[idx % definitions.len()].to_card())
            .collect::<Vec<_>>();

        shuffle_power_cards(&mut deck, seed, sequence);

        let decks = players
            .iter()
            .map(|player_id| {
                (
                    player_id.clone(),
                    deck.drain(..POWER_CARDS_PER_PLAYER.min(deck.len()))
                        .collect(),
                )
            })
            .collect();

        PowerSet {
            shuffle: DeckShuffle { seed, sequence },
            decks,
        }
    }

    fn from_classic_event(event: fodinha_classic::MatchEvent) -> MatchEvent {
        match event {
            fodinha_classic::MatchEvent::BidPlaced { player_id, bid } => {
                MatchEvent::BidPlaced { player_id, bid }
            }
            _ => unreachable!("only bid events are converted here"),
        }
    }
}

fn dto_decks(decks: &IndexMap<PlayerId, Vec<PowerCard>>) -> IndexMap<PlayerId, Vec<PowerCardDto>> {
    decks
        .iter()
        .map(|(player_id, deck)| {
            (
                player_id.clone(),
                deck.iter().map(PowerCard::to_dto).collect(),
            )
        })
        .collect()
}

fn power_card_definition(id: &str) -> Option<&'static PowerCardDefinition> {
    default_power_card_definitions()
        .iter()
        .find(|definition| definition.id == id)
}

fn default_power_card_definitions() -> &'static [PowerCardDefinition] {
    &[
        PowerCardDefinition {
            id: "heal_10",
            name: "Heal 10",
            description: "Restore 10 lives to yourself.",
            requires_target: false,
            script: "game.add_lives(card.owner_id, 10)",
        },
        PowerCardDefinition {
            id: "strike_10",
            name: "Strike 10",
            description: "Remove 10 lives from a target player.",
            requires_target: true,
            script: "game.add_lives(card.target_player_id, -10)",
        },
    ]
}

fn shuffle_power_cards(deck: &mut [PowerCard], seed: i64, sequence: i64) {
    let mut rng = DeterministicRng::with_sequence_multiplier(
        seed,
        sequence,
        POWER_CARD_RNG_SEQUENCE_MULTIPLIER,
    );

    for i in (1..deck.len()).rev() {
        deck.swap(i, rng.next_index(i + 1));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::models::id::PlayerId;

    use super::*;

    #[test]
    fn bid_mismatch_costs_ten_lives() {
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let mut game = Game::new_with_seed(&players, GameSettings::default(), 42).unwrap();

        game.apply_match_event(game.validate_bid(&player1, 1).unwrap());
        game.apply_match_event(game.validate_bid(&player2, 1).unwrap());

        let first_card = game.core.get_player_snapshots()[&player1].deck[0];
        let second_card = game.core.get_player_snapshots()[&player2].deck[0];

        game.apply_match_event(
            game.validate_turn(Turn {
                player_id: player1.clone(),
                card: first_card,
            })
            .unwrap(),
        );
        game.apply_match_event(
            game.validate_turn(Turn {
                player_id: player2.clone(),
                card: second_card,
            })
            .unwrap(),
        );

        let lifes = game.core.get_lifes();
        let life_values = lifes.values().copied().collect::<Vec<_>>();

        assert!(life_values.contains(&50));
        assert!(life_values.contains(&40));
    }

    #[test]
    fn power_card_script_applies_life_effect() {
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let mut game = Game::new_with_seed(&players, GameSettings::default(), 42).unwrap();

        game.power_decks.insert(
            player1.clone(),
            vec![power_card_definition("strike_10").unwrap().to_card()],
        );

        let event = game
            .validate_power_card(&player1, "strike_10", Some(player2.clone()))
            .unwrap();
        let AppliedGameChange::PowerCardPlayed(outcome) = game.apply_match_event(event) else {
            panic!("expected power card outcome");
        };

        assert_eq!(outcome.lifes.get(&player2), Some(&40));
        assert!(game.power_decks[&player1].is_empty());
    }
}
