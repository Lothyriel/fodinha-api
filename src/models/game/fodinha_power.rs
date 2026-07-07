use std::{
    collections::HashMap,
    sync::{LazyLock, RwLock},
};

use indexmap::IndexMap;

use crate::{
    models::{
        BiddingError, Card, DealError, GameError, Turn,
        game::{
            BiddingState, DealState, DeckShuffle, NewSet, fodinha_classic,
            power_lua::{
                PowerScriptError, PowerScriptInput, PowerScriptOutput, ScriptManaState,
                ScriptPlayerState, ScriptPowerCardState,
            },
        },
        id::{CardId, DeckId, PlayerId},
        util::DeterministicRng,
    },
    services::{GameInfoDto, PlayerManaDto, PowerCardDto},
};

const LIFE_LOSS_PER_BID_DIFFERENCE: usize = 10;
const POWER_CARDS_PER_PLAYER: usize = 1;
const INITIAL_MANA_POOL: usize = 2;
const MANA_POOL_GAIN_PER_SET: usize = 1;
const MANA_REGEN_PER_BIDDING_TURN: usize = 1;

pub const DEFAULT_INITIAL_LIFES: usize = 50;
pub const MIN_INITIAL_LIFES: usize = 10;
pub const MAX_INITIAL_LIFES: usize = 100;
pub const MAX_PLAYER_COUNT: usize = fodinha_classic::MAX_PLAYER_COUNT;

#[derive(Debug, Clone)]
pub struct Game {
    core: fodinha_classic::Game,
    power_decks: IndexMap<PlayerId, Vec<PowerCard>>,
    mana: IndexMap<PlayerId, PlayerMana>,
    power_deck_id: DeckId,
    power_seed: i64,
    next_power_shuffle_sequence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GameSettings {
    pub lifes: usize,
    pub power_deck_id: DeckId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PowerSet {
    pub shuffle: DeckShuffle,
    pub decks: IndexMap<PlayerId, Vec<PowerCard>>,
    #[serde(default)]
    pub mana: IndexMap<PlayerId, PlayerMana>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlayerMana {
    pub current: usize,
    pub max: usize,
}

impl PlayerMana {
    fn initial() -> Self {
        Self {
            current: INITIAL_MANA_POOL,
            max: INITIAL_MANA_POOL,
        }
    }

    fn regenerate(&mut self) {
        self.current = self
            .current
            .saturating_add(MANA_REGEN_PER_BIDDING_TURN)
            .min(self.max);
    }

    fn increase_pool_for_set(&mut self) {
        self.max = self.max.saturating_add(MANA_POOL_GAIN_PER_SET);
        self.current = self.max;
    }

    fn to_dto(&self) -> PlayerManaDto {
        PlayerManaDto {
            current: self.current,
            max: self.max,
        }
    }
}

impl From<&PlayerMana> for ScriptManaState {
    fn from(mana: &PlayerMana) -> Self {
        Self {
            current: mana.current,
            max: mana.max,
        }
    }
}

impl From<ScriptManaState> for PlayerMana {
    fn from(mana: ScriptManaState) -> Self {
        Self {
            current: mana.current.min(mana.max),
            max: mana.max,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerCardType {
    Instant,
    Targetable,
    Interactive,
}

impl PowerCardType {
    pub fn needs_target(self) -> bool {
        matches!(self, Self::Targetable)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Instant => "instant",
            Self::Targetable => "targetable",
            Self::Interactive => "interactive",
        }
    }
}

impl std::str::FromStr for PowerCardType {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "instant" => Ok(Self::Instant),
            "targetable" => Ok(Self::Targetable),
            "interactive" => Ok(Self::Interactive),
            _ => Err("type must be instant, targetable, or interactive".to_string()),
        }
    }
}

impl std::fmt::Display for PowerCardType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PowerCard {
    pub id: CardId,
    pub name: String,
    pub description: String,
    pub mana_cost: usize,
    #[serde(rename = "type")]
    pub card_type: PowerCardType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}

impl PowerCard {
    pub fn to_dto(&self) -> PowerCardDto {
        PowerCardDto {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            mana_cost: self.mana_cost,
            card_type: self.card_type,
            image_url: self.image_url.clone(),
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
        card_id: CardId,
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
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        mana: HashMap<PlayerId, PlayerMana>,
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
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub lifes: HashMap<PlayerId, usize>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub mana: HashMap<PlayerId, PlayerMana>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub decks: HashMap<PlayerId, Vec<Card>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub power_decks: HashMap<PlayerId, Vec<PowerCard>>,
}

#[derive(Debug, Clone)]
pub enum AppliedGameChange {
    BidPlaced {
        player_id: PlayerId,
        bid: usize,
        state: BiddingState,
        mana: HashMap<PlayerId, PlayerManaDto>,
    },
    TurnPlayed {
        state: DealState,
        power_decks: Option<IndexMap<PlayerId, Vec<PowerCardDto>>>,
        mana: Option<HashMap<PlayerId, PlayerManaDto>>,
    },
    PowerCardPlayed(PowerCardOutcome),
}

#[derive(Debug, Clone)]
pub struct PowerCardOutcome {
    pub player_id: PlayerId,
    pub card: PowerCardDto,
    pub target_player_id: Option<PlayerId>,
    pub lifes: HashMap<PlayerId, usize>,
    pub mana: HashMap<PlayerId, PlayerManaDto>,
    pub decks: HashMap<PlayerId, Vec<Card>>,
    pub power_decks: HashMap<PlayerId, Vec<PowerCardDto>>,
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
    #[error("not enough mana")]
    NotEnoughMana,
    #[error("power card script failed: {0}")]
    Script(#[from] PowerScriptError),
    #[error("power card definitions failed: {0}")]
    Definitions(#[from] PowerCardDefinitionError),
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum PowerCardDefinitionError {
    #[error("no FodinhaPower card definitions were loaded for this deck")]
    MissingDefinitions,
    #[error("invalid FodinhaPower card definition {path}: {message}")]
    InvalidDefinition { path: String, message: String },
    #[error("duplicate FodinhaPower card id `{id}` in {path}")]
    DuplicateId { id: String, path: String },
}

#[derive(Debug, Clone)]
pub struct PowerCardDefinitionInput {
    pub id: CardId,
    pub name: String,
    pub description: String,
    pub mana_cost: usize,
    pub card_type: PowerCardType,
    pub image_url: Option<String>,
    pub script: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct PowerDeckDefinitionInput {
    pub id: DeckId,
    pub card_ids: Vec<CardId>,
}

#[derive(Debug, Clone)]
struct PowerCardDefinition {
    id: CardId,
    name: String,
    description: String,
    mana_cost: usize,
    card_type: PowerCardType,
    image_url: Option<String>,
    script: String,
    source: String,
}

impl PowerCardDefinition {
    fn from_input(input: PowerCardDefinitionInput) -> Result<Self, PowerCardDefinitionError> {
        super::power_lua::validate_power_card_script(&input.script, &input.source).map_err(
            |error| PowerCardDefinitionError::InvalidDefinition {
                path: input.source.clone(),
                message: error.to_string(),
            },
        )?;

        Ok(Self {
            id: input.id,
            name: input.name,
            description: input.description,
            mana_cost: input.mana_cost,
            card_type: input.card_type,
            image_url: input.image_url,
            script: input.script,
            source: input.source,
        })
    }

    fn to_card(&self) -> PowerCard {
        PowerCard {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            mana_cost: self.mana_cost,
            card_type: self.card_type,
            image_url: self.image_url.clone(),
        }
    }
}

impl From<ScriptPowerCardState> for PowerCard {
    fn from(card: ScriptPowerCardState) -> Self {
        Self {
            id: CardId(card.id.into()),
            name: card.name,
            description: card.description,
            mana_cost: card.mana_cost,
            card_type: card.card_type,
            image_url: card.image_url,
        }
    }
}

impl From<&PowerCard> for ScriptPowerCardState {
    fn from(card: &PowerCard) -> Self {
        Self {
            id: card.id.as_str().to_string(),
            name: card.name.clone(),
            description: card.description.clone(),
            mana_cost: card.mana_cost,
            card_type: card.card_type,
            image_url: card.image_url.clone(),
        }
    }
}

static POWER_CARD_DEFINITIONS: RwLock<Vec<PowerCardDefinition>> = RwLock::new(Vec::new());
static POWER_DECK_DEFINITIONS: LazyLock<RwLock<HashMap<DeckId, Vec<CardId>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn replace_power_card_registry(
    definitions: Vec<PowerCardDefinitionInput>,
    decks: Vec<PowerDeckDefinitionInput>,
) -> Result<(), PowerCardDefinitionError> {
    let definitions = validate_power_card_definitions(definitions)?;
    let mut registry = POWER_CARD_DEFINITIONS
        .write()
        .expect("power card definitions registry lock poisoned");
    let mut deck_registry = POWER_DECK_DEFINITIONS
        .write()
        .expect("power deck definitions registry lock poisoned");

    *registry = definitions;
    *deck_registry = decks
        .into_iter()
        .map(|deck| (deck.id, deck.card_ids))
        .collect();

    Ok(())
}

pub fn replace_power_card_definitions(
    deck_id: DeckId,
    definitions: Vec<PowerCardDefinitionInput>,
) -> Result<(), PowerCardDefinitionError> {
    let card_ids = definitions
        .iter()
        .map(|definition| definition.id.clone())
        .collect();

    replace_power_card_registry(
        definitions,
        vec![PowerDeckDefinitionInput {
            id: deck_id,
            card_ids,
        }],
    )
}

pub fn upsert_power_card_definition(
    definition: PowerCardDefinitionInput,
) -> Result<(), PowerCardDefinitionError> {
    let definition = PowerCardDefinition::from_input(definition)?;
    let mut registry = POWER_CARD_DEFINITIONS
        .write()
        .expect("power card definitions registry lock poisoned");

    if let Some(existing) = registry.iter_mut().find(|card| card.id == definition.id) {
        *existing = definition;
    } else {
        registry.push(definition);
    }

    Ok(())
}

pub fn upsert_power_deck_definition(definition: PowerDeckDefinitionInput) {
    POWER_DECK_DEFINITIONS
        .write()
        .expect("power deck definitions registry lock poisoned")
        .insert(definition.id, definition.card_ids);
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
        let power_set = Self::new_power_set(
            players,
            seed,
            0,
            &settings.power_deck_id,
            Self::initial_mana(players),
        )?;

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

        let mana = if power_set.mana.is_empty() {
            Self::initial_mana(players)
        } else {
            power_set.mana
        };

        Ok(Self {
            core: classic,
            power_decks: power_set.decks,
            mana,
            power_deck_id: settings.power_deck_id,
            power_seed: power_set.shuffle.seed,
            next_power_shuffle_sequence: power_set.shuffle.sequence.wrapping_add(1),
        })
    }

    pub fn validate_bid(
        &self,
        player_id: &PlayerId,
        bid: usize,
    ) -> Result<MatchEvent, BiddingError> {
        self.core.validate_bid(player_id, bid)?;

        Ok(MatchEvent::BidPlaced {
            player_id: player_id.clone(),
            bid,
            mana: self.mana_after_bid(player_id, bid),
        })
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
        card_id: &CardId,
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
            .and_then(|deck| deck.iter().find(|card| &card.id == card_id))
            .cloned()
            .ok_or(PowerCardError::InvalidPowerCard)?;

        let definition = power_card_definition(&self.power_deck_id, &card.id)?
            .ok_or(PowerCardError::InvalidPowerCard)?;

        if self
            .mana
            .get(player_id)
            .map(|mana| mana.current)
            .unwrap_or_default()
            < definition.mana_cost
        {
            return Err(PowerCardError::NotEnoughMana);
        }

        if definition.card_type.needs_target() && target_player_id.is_none() {
            return Err(PowerCardError::TargetRequired);
        }

        if let Some(target_player_id) = target_player_id.as_ref()
            && !self.core.is_player_alive(target_player_id)
        {
            return Err(PowerCardError::InvalidTarget);
        }

        let output = self.run_power_script(&definition, player_id, target_player_id.clone())?;
        let effects = self.power_card_effects(player_id, &definition, output);

        Ok(MatchEvent::PowerCardPlayed {
            player_id: player_id.clone(),
            card,
            target_player_id,
            effects,
        })
    }

    pub fn apply_match_event(&mut self, event: MatchEvent) -> AppliedGameChange {
        match event {
            MatchEvent::BidPlaced {
                player_id,
                bid,
                mana,
            } => {
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
                        mana: self.apply_mana_totals(&mana),
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
                let mana = next_power_set.as_ref().map(|set| mana_to_dto(&set.mana));

                AppliedGameChange::TurnPlayed {
                    state,
                    power_decks,
                    mana,
                }
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
                self.core.apply_decks(&effects.decks);
                let mana = self.apply_mana_totals(&effects.mana);
                self.apply_power_decks(&effects.power_decks);
                let power_decks = effects
                    .power_decks
                    .iter()
                    .map(|(player_id, deck)| {
                        (
                            player_id.clone(),
                            deck.iter().map(PowerCard::to_dto).collect(),
                        )
                    })
                    .collect();

                AppliedGameChange::PowerCardPlayed(PowerCardOutcome {
                    player_id,
                    card: card.to_dto(),
                    target_player_id,
                    lifes: self.core.get_lifes(),
                    mana,
                    decks: effects.decks,
                    power_decks,
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
        for player in &mut info.info {
            player.mana = self.mana.get(&player.id).map(PlayerMana::to_dto);
        }
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
                let power_cards = self
                    .power_decks
                    .get(&player_id)
                    .map(|deck| {
                        deck.iter()
                            .filter(|card| {
                                &player_id != owner_id || card.id.as_str() != definition.id.as_str()
                            })
                            .map(ScriptPowerCardState::from)
                            .collect()
                    })
                    .unwrap_or_default();

                (
                    player_id.clone(),
                    ScriptPlayerState {
                        lifes: player.lifes,
                        bid: player.bid,
                        rounds: player.rounds,
                        mana: self
                            .mana
                            .get(&player_id)
                            .map(ScriptManaState::from)
                            .unwrap_or(ScriptManaState { current: 0, max: 0 }),
                        cards: player.deck,
                        power_cards,
                    },
                )
            })
            .collect();

        Ok(super::power_lua::run_power_card_script(
            &definition.script,
            PowerScriptInput {
                card_id: definition.id.as_str().to_string(),
                mana_cost: definition.mana_cost,
                owner_id: owner_id.clone(),
                target_player_id,
                players,
            },
        )?)
    }

    fn power_card_effects(
        &self,
        owner_id: &PlayerId,
        definition: &PowerCardDefinition,
        output: PowerScriptOutput,
    ) -> PowerCardEffects {
        let mut mana = output
            .mana
            .into_iter()
            .map(|(player_id, mana)| (player_id, PlayerMana::from(mana)))
            .collect::<HashMap<_, _>>();
        let mut owner_mana = mana
            .remove(owner_id)
            .or_else(|| self.mana.get(owner_id).cloned())
            .unwrap_or_else(PlayerMana::initial);

        owner_mana.current = owner_mana.current.saturating_sub(definition.mana_cost);

        if self.mana.get(owner_id) != Some(&owner_mana) {
            mana.insert(owner_id.clone(), owner_mana);
        }

        let power_decks = output
            .power_cards
            .into_iter()
            .map(|(player_id, deck)| {
                (
                    player_id,
                    deck.into_iter().map(PowerCard::from).collect::<Vec<_>>(),
                )
            })
            .collect();

        PowerCardEffects {
            lifes: output.lifes,
            mana,
            decks: output.cards,
            power_decks,
        }
    }

    fn classic_settings(settings: &GameSettings) -> fodinha_classic::GameSettings {
        fodinha_classic::GameSettings {
            lifes: settings.lifes,
        }
    }

    fn apply_power_set(&mut self, set: &PowerSet) {
        self.power_decks = set.decks.clone();
        if !set.mana.is_empty() {
            self.mana = set.mana.clone();
        }
        self.power_seed = set.shuffle.seed;
        self.next_power_shuffle_sequence = set.shuffle.sequence.wrapping_add(1);
    }

    fn apply_mana_totals(
        &mut self,
        mana: &HashMap<PlayerId, PlayerMana>,
    ) -> HashMap<PlayerId, PlayerManaDto> {
        for (player_id, player_mana) in mana {
            if self.core.is_player_alive(player_id) {
                self.mana.insert(player_id.clone(), player_mana.clone());
            }
        }

        mana.iter()
            .map(|(player_id, mana)| (player_id.clone(), mana.to_dto()))
            .collect()
    }

    fn apply_power_decks(&mut self, power_decks: &HashMap<PlayerId, Vec<PowerCard>>) {
        for (player_id, deck) in power_decks {
            if self.core.is_player_alive(player_id) {
                self.power_decks.insert(player_id.clone(), deck.clone());
            }
        }
    }

    fn new_power_set_for_game(&self, players: &[PlayerId]) -> PowerSet {
        Self::new_power_set(
            players,
            self.power_seed,
            self.next_power_shuffle_sequence,
            &self.power_deck_id,
            self.next_set_mana(players),
        )
        .expect("FodinhaPower card definitions are loaded before the game starts")
    }

    fn new_power_set(
        players: &[PlayerId],
        seed: i64,
        sequence: i64,
        power_deck_id: &DeckId,
        mana: IndexMap<PlayerId, PlayerMana>,
    ) -> Result<PowerSet, PowerCardDefinitionError> {
        let definitions = power_card_definitions(power_deck_id)?;
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

        Ok(PowerSet {
            shuffle: DeckShuffle { seed, sequence },
            decks,
            mana,
        })
    }

    fn initial_mana(players: &[PlayerId]) -> IndexMap<PlayerId, PlayerMana> {
        players
            .iter()
            .map(|player_id| (player_id.clone(), PlayerMana::initial()))
            .collect()
    }

    fn next_set_mana(&self, players: &[PlayerId]) -> IndexMap<PlayerId, PlayerMana> {
        players
            .iter()
            .map(|player_id| {
                let mut mana = self
                    .mana
                    .get(player_id)
                    .cloned()
                    .unwrap_or_else(PlayerMana::initial);
                mana.increase_pool_for_set();

                (player_id.clone(), mana)
            })
            .collect()
    }

    fn mana_after_bid(&self, player_id: &PlayerId, bid: usize) -> HashMap<PlayerId, PlayerMana> {
        let mut core = self.core.clone();
        let change = core.apply_match_event(fodinha_classic::MatchEvent::BidPlaced {
            player_id: player_id.clone(),
            bid,
        });

        let fodinha_classic::AppliedGameChange::BidPlaced { state, .. } = change else {
            return HashMap::new();
        };

        let BiddingState::Active { next, .. } = state else {
            return HashMap::new();
        };

        let mut mana = self
            .mana
            .get(&next)
            .cloned()
            .unwrap_or_else(PlayerMana::initial);
        let previous = mana.clone();
        mana.regenerate();

        if mana == previous {
            HashMap::new()
        } else {
            HashMap::from([(next, mana)])
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

fn mana_to_dto(mana: &IndexMap<PlayerId, PlayerMana>) -> HashMap<PlayerId, PlayerManaDto> {
    mana.iter()
        .map(|(player_id, mana)| (player_id.clone(), mana.to_dto()))
        .collect()
}

fn power_card_definition(
    deck_id: &DeckId,
    id: &CardId,
) -> Result<Option<PowerCardDefinition>, PowerCardDefinitionError> {
    Ok(power_card_definitions(deck_id)?
        .iter()
        .find(|definition| &definition.id == id)
        .cloned())
}

fn power_card_definitions(
    deck_id: &DeckId,
) -> Result<Vec<PowerCardDefinition>, PowerCardDefinitionError> {
    let deck_card_ids = POWER_DECK_DEFINITIONS
        .read()
        .expect("power deck definitions registry lock poisoned")
        .get(deck_id)
        .cloned()
        .ok_or(PowerCardDefinitionError::MissingDefinitions)?;
    let registry = POWER_CARD_DEFINITIONS
        .read()
        .expect("power card definitions registry lock poisoned");
    let definitions = deck_card_ids
        .iter()
        .filter_map(|card_id| {
            registry
                .iter()
                .find(|definition| &definition.id == card_id)
                .cloned()
        })
        .collect::<Vec<_>>();

    if definitions.is_empty() {
        return Err(PowerCardDefinitionError::MissingDefinitions);
    }

    Ok(definitions)
}

fn validate_power_card_definitions(
    definitions: Vec<PowerCardDefinitionInput>,
) -> Result<Vec<PowerCardDefinition>, PowerCardDefinitionError> {
    let mut loaded = Vec::with_capacity(definitions.len());

    for definition in definitions {
        let definition = PowerCardDefinition::from_input(definition)?;

        if loaded
            .iter()
            .any(|existing: &PowerCardDefinition| existing.id == definition.id)
        {
            return Err(PowerCardDefinitionError::DuplicateId {
                id: definition.id.to_string(),
                path: definition.source,
            });
        }

        loaded.push(definition);
    }

    Ok(loaded)
}

fn shuffle_power_cards(deck: &mut [PowerCard], seed: i64, sequence: i64) {
    let mut rng = DeterministicRng::new(seed, sequence);

    for i in (1..deck.len()).rev() {
        deck.swap(i, rng.next_index(i + 1));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, MutexGuard};

    use crate::models::id::PlayerId;

    use super::*;

    const HEAL_10_SCRIPT: &str = r#"
return {
    effect = function(game, card)
        game.add_lives(card.owner_id, 10)
    end,
}
"#;

    const STRIKE_10_SCRIPT: &str = r#"
return {
    effect = function(game, card)
        game.add_lives(card.target_player_id, -10)
    end,
}
"#;

    static POWER_CARD_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn power_card_test_lock() -> MutexGuard<'static, ()> {
        POWER_CARD_TEST_LOCK
            .lock()
            .expect("power card test lock poisoned")
    }

    pub(crate) fn install_test_power_card_definitions() {
        replace_power_card_definitions(test_deck_id(), test_power_card_definitions())
            .expect("valid test power card definitions");
    }

    fn test_power_card_definitions() -> Vec<PowerCardDefinitionInput> {
        vec![
            PowerCardDefinitionInput {
                id: card_id("heal_10"),
                name: "Heal 10".to_string(),
                description: "Restore 10 lives to yourself.".to_string(),
                mana_cost: 2,
                card_type: PowerCardType::Instant,
                image_url: None,
                script: HEAL_10_SCRIPT.to_string(),
                source: "test/heal_10.lua".to_string(),
            },
            PowerCardDefinitionInput {
                id: card_id("strike_10"),
                name: "Strike 10".to_string(),
                description: "Remove 10 lives from a target player.".to_string(),
                mana_cost: 3,
                card_type: PowerCardType::Targetable,
                image_url: None,
                script: STRIKE_10_SCRIPT.to_string(),
                source: "test/strike_10.lua".to_string(),
            },
        ]
    }

    fn new_test_game(players: &[PlayerId]) -> Game {
        install_test_power_card_definitions();
        Game::new_with_seed(players, test_settings(), 42).unwrap()
    }

    fn card_id(value: &str) -> CardId {
        CardId(Arc::from(value))
    }

    fn test_deck_id() -> DeckId {
        DeckId(Arc::from("test_deck"))
    }

    fn test_settings() -> GameSettings {
        GameSettings {
            lifes: DEFAULT_INITIAL_LIFES,
            power_deck_id: test_deck_id(),
        }
    }

    fn test_players() -> [PlayerId; 2] {
        [PlayerId(Arc::from("P1")), PlayerId(Arc::from("P2"))]
    }

    #[test]
    fn loads_power_cards_from_runtime_registry() {
        let _lock = power_card_test_lock();
        install_test_power_card_definitions();
        let definitions = power_card_definitions(&test_deck_id()).unwrap();

        assert!(
            definitions
                .iter()
                .any(|definition| definition.id.as_str() == "heal_10")
        );
        assert!(
            definitions
                .iter()
                .any(|definition| definition.id.as_str() == "strike_10")
        );
        assert!(
            definitions
                .iter()
                .all(|definition| !definition.script.is_empty())
        );
    }

    #[test]
    fn game_starts_with_initial_mana_pool() {
        let _lock = power_card_test_lock();
        install_test_power_card_definitions();
        let players = test_players();

        let MatchEvent::GameStarted { power_set, .. } =
            Game::start_match_event_with_seed(&players, test_settings(), 42).unwrap()
        else {
            panic!("expected game started event");
        };

        assert_eq!(power_set.mana.len(), players.len());
        for player in players {
            assert_eq!(
                power_set.mana.get(&player),
                Some(&PlayerMana {
                    current: INITIAL_MANA_POOL,
                    max: INITIAL_MANA_POOL,
                })
            );
        }
    }

    #[test]
    fn bid_mismatch_costs_ten_lives() {
        let _lock = power_card_test_lock();
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);

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
        let _lock = power_card_test_lock();
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);

        game.power_decks.insert(
            player1.clone(),
            vec![
                power_card_definition(&test_deck_id(), &card_id("strike_10"))
                    .unwrap()
                    .unwrap()
                    .to_card(),
            ],
        );

        let event = game
            .validate_power_card(&player1, &card_id("strike_10"), Some(player2.clone()))
            .unwrap();
        let AppliedGameChange::PowerCardPlayed(outcome) = game.apply_match_event(event) else {
            panic!("expected power card outcome");
        };

        assert_eq!(outcome.lifes.get(&player2), Some(&40));
        assert!(game.power_decks[&player1].is_empty());
    }

    #[test]
    fn power_card_cost_is_deducted_from_mana() {
        let _lock = power_card_test_lock();
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);

        game.power_decks.insert(
            player1.clone(),
            vec![
                power_card_definition(&test_deck_id(), &card_id("strike_10"))
                    .unwrap()
                    .unwrap()
                    .to_card(),
            ],
        );

        let event = game
            .validate_power_card(&player1, &card_id("strike_10"), Some(player2))
            .unwrap();
        let AppliedGameChange::PowerCardPlayed(outcome) = game.apply_match_event(event) else {
            panic!("expected power card outcome");
        };

        assert_eq!(
            outcome.mana.get(&player1),
            Some(&PlayerManaDto { current: 2, max: 5 })
        );
        assert_eq!(game.mana[&player1].current, 2);
    }

    #[test]
    fn power_card_requires_enough_mana() {
        let _lock = power_card_test_lock();
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);

        game.power_decks.insert(
            player1.clone(),
            vec![
                power_card_definition(&test_deck_id(), &card_id("strike_10"))
                    .unwrap()
                    .unwrap()
                    .to_card(),
            ],
        );
        game.mana.insert(
            player1.clone(),
            PlayerMana {
                current: 2,
                max: INITIAL_MANA_POOL,
            },
        );

        assert!(matches!(
            game.validate_power_card(&player1, &card_id("strike_10"), Some(player2)),
            Err(PowerCardError::NotEnoughMana)
        ));
    }

    #[test]
    fn power_card_script_lookup_uses_selected_deck() {
        let _lock = power_card_test_lock();
        let custom_deck_id = DeckId(Arc::from("custom_deck"));

        replace_power_card_registry(
            test_power_card_definitions(),
            vec![
                PowerDeckDefinitionInput {
                    id: test_deck_id(),
                    card_ids: vec![card_id("heal_10")],
                },
                PowerDeckDefinitionInput {
                    id: custom_deck_id.clone(),
                    card_ids: vec![card_id("strike_10")],
                },
            ],
        )
        .expect("valid custom deck definitions");

        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            lifes: DEFAULT_INITIAL_LIFES,
            power_deck_id: custom_deck_id,
        };
        let mut game = Game::new_with_seed(&players, settings, 42).unwrap();

        let event = game
            .validate_power_card(&player1, &card_id("strike_10"), Some(player2.clone()))
            .unwrap();
        let AppliedGameChange::PowerCardPlayed(outcome) = game.apply_match_event(event) else {
            panic!("expected power card outcome");
        };

        assert_eq!(outcome.lifes.get(&player2), Some(&40));
    }

    #[test]
    fn validates_power_card_errors() {
        let _lock = power_card_test_lock();
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);

        game.power_decks.insert(
            player1.clone(),
            vec![
                power_card_definition(&test_deck_id(), &card_id("strike_10"))
                    .unwrap()
                    .unwrap()
                    .to_card(),
            ],
        );

        assert!(matches!(
            game.validate_power_card(&player1, &card_id("strike_10"), None),
            Err(PowerCardError::TargetRequired)
        ));

        assert!(matches!(
            game.validate_power_card(&player1, &card_id("missing"), Some(player2.clone())),
            Err(PowerCardError::InvalidPowerCard)
        ));

        assert!(matches!(
            game.validate_power_card(&player2, &card_id("strike_10"), Some(player1.clone())),
            Err(PowerCardError::NotYourTurn)
        ));

        game.apply_match_event(game.validate_bid(&player1, 1).unwrap());
        game.apply_match_event(game.validate_bid(&player2, 1).unwrap());

        assert!(matches!(
            game.validate_power_card(&player1, &card_id("strike_10"), Some(player2)),
            Err(PowerCardError::BiddingStageRequired)
        ));
    }

    #[test]
    fn applying_persisted_power_card_event_removes_card_and_can_end_game() {
        let _lock = power_card_test_lock();
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);
        let card = power_card_definition(&test_deck_id(), &card_id("strike_10"))
            .unwrap()
            .unwrap()
            .to_card();

        game.power_decks.insert(player1.clone(), vec![card.clone()]);

        let AppliedGameChange::PowerCardPlayed(outcome) =
            game.apply_match_event(MatchEvent::PowerCardPlayed {
                player_id: player1.clone(),
                card,
                target_player_id: Some(player2.clone()),
                effects: PowerCardEffects {
                    lifes: HashMap::from([(player2.clone(), 0)]),
                    mana: HashMap::new(),
                    decks: HashMap::new(),
                    power_decks: HashMap::new(),
                },
            })
        else {
            panic!("expected power card outcome");
        };

        assert!(game.power_decks[&player1].is_empty());
        assert_eq!(outcome.lifes.get(&player2), Some(&0));
        assert!(outcome.ended);
        assert!(game.is_finished());
    }

    #[test]
    fn next_set_refreshes_power_cards() {
        let _lock = power_card_test_lock();
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);

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

        let event = game
            .validate_turn(Turn {
                player_id: player2.clone(),
                card: second_card,
            })
            .unwrap();
        let MatchEvent::TurnPlayed {
            next_power_set: Some(_),
            ..
        } = &event
        else {
            panic!("expected next power set at set end");
        };

        let AppliedGameChange::TurnPlayed {
            power_decks: Some(power_decks),
            ..
        } = game.apply_match_event(event)
        else {
            panic!("expected refreshed power decks");
        };

        assert_eq!(power_decks.len(), 2);
        assert_eq!(power_decks[&player1].len(), POWER_CARDS_PER_PLAYER);
        assert_eq!(power_decks[&player2].len(), POWER_CARDS_PER_PLAYER);
        assert_eq!(game.power_decks[&player1].len(), POWER_CARDS_PER_PLAYER);
        assert_eq!(game.power_decks[&player2].len(), POWER_CARDS_PER_PLAYER);
    }

    #[test]
    fn bidding_turn_regenerates_next_players_mana() {
        let _lock = power_card_test_lock();
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);

        game.mana.insert(
            player2.clone(),
            PlayerMana {
                current: 4,
                max: INITIAL_MANA_POOL,
            },
        );

        let event = game.validate_bid(&player1, 1).unwrap();
        let MatchEvent::BidPlaced { mana, .. } = &event else {
            panic!("expected bid event");
        };

        assert_eq!(mana.get(&player2), Some(&PlayerMana { current: 5, max: 5 }));

        let AppliedGameChange::BidPlaced { mana, .. } = game.apply_match_event(event) else {
            panic!("expected bid change");
        };

        assert_eq!(
            mana.get(&player2),
            Some(&PlayerManaDto { current: 5, max: 5 })
        );
        assert_eq!(game.mana[&player2].current, 5);
    }

    #[test]
    fn next_set_increases_and_refills_mana_pool() {
        let _lock = power_card_test_lock();
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);

        game.mana.insert(
            player1.clone(),
            PlayerMana {
                current: 1,
                max: INITIAL_MANA_POOL,
            },
        );
        game.mana.insert(
            player2.clone(),
            PlayerMana {
                current: 2,
                max: INITIAL_MANA_POOL,
            },
        );

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

        let event = game
            .validate_turn(Turn {
                player_id: player2.clone(),
                card: second_card,
            })
            .unwrap();

        let AppliedGameChange::TurnPlayed {
            mana: Some(mana), ..
        } = game.apply_match_event(event)
        else {
            panic!("expected next-set mana update");
        };

        assert_eq!(
            mana.get(&player1),
            Some(&PlayerManaDto { current: 6, max: 6 })
        );
        assert_eq!(
            mana.get(&player2),
            Some(&PlayerManaDto { current: 6, max: 6 })
        );
        assert_eq!(game.mana[&player1].current, 6);
        assert_eq!(game.mana[&player1].max, 6);
        assert_eq!(game.mana[&player2].current, 6);
        assert_eq!(game.mana[&player2].max, 6);
    }

    #[test]
    fn next_set_mana_pool_has_no_global_cap() {
        let _lock = power_card_test_lock();
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);

        game.mana.insert(
            player1.clone(),
            PlayerMana {
                current: 4,
                max: 10,
            },
        );

        let mana = game.next_set_mana(&players);

        assert_eq!(
            mana.get(&player1),
            Some(&PlayerMana {
                current: 11,
                max: 11,
            })
        );
    }

    #[test]
    fn game_info_exposes_private_power_cards() {
        let _lock = power_card_test_lock();
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let game = new_test_game(&players);

        let info = game.get_game_info(&player1);

        assert_eq!(info.power_cards.unwrap().len(), POWER_CARDS_PER_PLAYER);
    }
}
