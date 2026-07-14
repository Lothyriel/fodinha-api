use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{Arc, RwLock},
};

use indexmap::IndexMap;
use mlua_extras::mlua;

use crate::{
    models::{
        BiddingError, Card, DealError, GameError, Turn,
        game::{
            BiddingState, DealState, DeckShuffle, NewSet, fodinha_classic,
            power_lua::{
                DeckReveal, DrawPowerCardsFn, PassiveGameEvent, PassiveScriptInput,
                PowerScriptError, PowerScriptInput, PowerScriptOutput, ScriptManaState,
                ScriptPlayerState, ScriptPowerCardState,
            },
        },
        id::{CardId, DeckId, MercenaryId, PlayerId},
        util::DeterministicRng,
    },
    services::{GameInfoDto, GameStageDto, PlayerManaDto, PowerCardDto, PowerCardStateDto},
};

const LIFE_LOSS_PER_BID_DIFFERENCE: usize = 10;
const GENERIC_POWER_CARDS_PER_PLAYER: usize = 1;
const MERCENARY_POWER_CARDS_PER_PLAYER: usize = 1;
const INITIAL_MANA_POOL: usize = 2;
const MANA_REGEN_PER_BIDDING_TURN: usize = 1;

pub const DEFAULT_INITIAL_LIFES: usize = 50;
pub const MIN_INITIAL_LIFES: usize = 10;
pub const MAX_INITIAL_LIFES: usize = 100;
pub const MAX_PLAYER_COUNT: usize = fodinha_classic::MAX_PLAYER_COUNT;

#[derive(Debug, Clone)]
pub struct Game {
    core: fodinha_classic::Game,
    stage: PowerGameStage,
    power_decks: IndexMap<PlayerId, Vec<PowerCard>>,
    mana: IndexMap<PlayerId, PlayerMana>,
    registry: PowerCardRegistry,
    power_deck_id: DeckId,
    player_mercenaries: HashMap<PlayerId, MercenaryId>,
    power_seed: i64,
    draw_seed: i64,
    next_power_shuffle_sequence: i64,
    pending_set_resolution: Option<PendingSetResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PowerGameStage {
    Bidding,
    Power {
        phase: PowerPhase,
        pending_players: Vec<PlayerId>,
    },
    Dealing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerPhase {
    First,
    Second,
}

#[derive(Debug, Clone)]
struct PendingSetResolution {
    next_set: NewSet,
    next_power_set: PowerSet,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GameSettings {
    #[serde(default = "default_life_multiplier")]
    pub life_multiplier: f64,
    pub power_deck_id: DeckId,
    pub player_mercenaries: HashMap<PlayerId, MercenaryId>,
}

fn default_life_multiplier() -> f64 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PowerSet {
    pub shuffle: DeckShuffle,
    pub decks: IndexMap<PlayerId, Vec<PowerCard>>,
    pub mana: IndexMap<PlayerId, PlayerMana>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlayerMana {
    pub current: usize,
    pub max: usize,
}

impl PlayerMana {
    fn initial() -> Self {
        Self::with_max(INITIAL_MANA_POOL)
    }

    fn with_max(max: usize) -> Self {
        Self { current: max, max }
    }

    fn regenerate(&mut self) {
        self.current = self
            .current
            .saturating_add(MANA_REGEN_PER_BIDDING_TURN)
            .min(self.max);
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

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    lua_api_derive::LuaApiEnum,
)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(default = "default_power_card_usable")]
    pub usable: bool,
}

fn default_power_card_usable() -> bool {
    true
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
            state: None,
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
    SkipPowerPhase,
}

impl GameCommand {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::PlayTurn { .. } => "game.fodinha_power.play_turn",
            Self::PutBid { .. } => "game.fodinha_power.put_bid",
            Self::UsePowerCard { .. } => "game.fodinha_power.use_power_card",
            Self::SkipPowerPhase => "game.fodinha_power.skip_power_phase",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MatchEvent {
    GameStarted {
        settings: GameSettings,
        seed: i64,
        initial_mana: IndexMap<PlayerId, PlayerMana>,
        power_deck_version: i64,
        player_mercenary_versions: HashMap<PlayerId, i64>,
        draw_seed: i64,
        passive_effects: PowerCardEffects,
    },
    BidPlaced {
        player_id: PlayerId,
        bid: usize,
        mana: HashMap<PlayerId, PlayerMana>,
        passive_effects: PowerCardEffects,
    },
    TurnPlayed {
        turn: Turn,
        passive_effects: PowerCardEffects,
    },
    PowerCardPlayed {
        player_id: PlayerId,
        card: PowerCard,
        target_player_id: Option<PlayerId>,
        effects: PowerCardEffects,
        set_ended_effects: PowerCardEffects,
        set_started_effects: PowerCardEffects,
    },
    PowerPhaseSkipped {
        player_id: PlayerId,
        effects: PowerCardEffects,
        set_ended_effects: PowerCardEffects,
        set_started_effects: PowerCardEffects,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PowerCardEffects {
    pub lifes: HashMap<PlayerId, usize>,
    pub mana: HashMap<PlayerId, PlayerMana>,
    pub decks: HashMap<PlayerId, Vec<Card>>,
    pub power_decks: HashMap<PlayerId, Vec<PowerCard>>,
    #[serde(default)]
    pub deck_reveals: Vec<DeckReveal>,
}

impl PowerCardEffects {
    fn merge(&mut self, other: Self) {
        self.lifes.extend(other.lifes);
        self.mana.extend(other.mana);
        self.decks.extend(other.decks);
        self.power_decks.extend(other.power_decks);
        self.deck_reveals.extend(other.deck_reveals);
    }
}

#[derive(Debug, Clone)]
pub enum AppliedGameChange {
    BidPlaced {
        player_id: PlayerId,
        bid: usize,
        state: BiddingState,
        mana: HashMap<PlayerId, PlayerManaDto>,
        deck_reveals: Vec<DeckReveal>,
    },
    TurnPlayed {
        state: DealState,
        lifes: Option<HashMap<PlayerId, usize>>,
        power_decks: Option<IndexMap<PlayerId, Vec<PowerCardDto>>>,
        mana: Option<HashMap<PlayerId, PlayerManaDto>>,
        deck_reveals: Vec<DeckReveal>,
    },
    PowerCardPlayed(PowerCardOutcome),
    PowerPhaseSkipped(PowerPhaseSkipOutcome),
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
    pub deck_reveals: Vec<DeckReveal>,
    pub ended: bool,
    pub next_set: Option<NewSet>,
    pub next_power_set: Option<PowerSet>,
    pub next_set_passive_effects: PowerCardEffects,
}

#[derive(Debug, Clone)]
pub struct PowerPhaseSkipOutcome {
    pub player_id: PlayerId,
    pub lifes: HashMap<PlayerId, usize>,
    pub changed_lifes: HashMap<PlayerId, usize>,
    pub mana: HashMap<PlayerId, PlayerManaDto>,
    pub decks: HashMap<PlayerId, Vec<Card>>,
    pub power_decks: HashMap<PlayerId, Vec<PowerCardDto>>,
    pub deck_reveals: Vec<DeckReveal>,
    pub ended: bool,
    pub next_set: Option<NewSet>,
    pub next_power_set: Option<PowerSet>,
    pub next_set_passive_effects: PowerCardEffects,
}

#[derive(Debug, thiserror::Error)]
pub enum PowerCardError {
    #[error("power cards can only be used during the power phase")]
    PowerStageRequired,
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
    #[error("power card is disabled")]
    CardDisabled,
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
    #[error("missing mercenary definition `{mercenary_id}`")]
    MissingMercenaryDefinition { mercenary_id: String },
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
    pub quantity: usize,
    pub card_type: PowerCardType,
    pub image_url: Option<String>,
    pub script: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct PowerDeckDefinitionInput {
    pub id: DeckId,
    pub generic_card_ids: Vec<CardId>,
    pub mercenary_card_ids: HashMap<MercenaryId, Vec<CardId>>,
}

#[derive(Debug, Clone)]
pub struct MercenaryDefinitionInput {
    pub id: MercenaryId,
    pub name: String,
    pub base_life: usize,
    pub initial_mana: usize,
    pub passive_script: String,
    pub passive_source: String,
}

#[derive(Debug, Clone)]
struct PowerCardDefinition {
    id: CardId,
    name: String,
    description: String,
    mana_cost: usize,
    quantity: usize,
    card_type: PowerCardType,
    image_url: Option<String>,
    script: String,
    source: String,
    event_handlers: Vec<String>,
}

impl PowerCardDefinition {
    fn from_input(input: PowerCardDefinitionInput) -> Result<Self, PowerCardDefinitionError> {
        let script_definition =
            super::power_lua::parse_power_card_script_definition(&input.script, &input.source)
                .map_err(|error| PowerCardDefinitionError::InvalidDefinition {
                    path: input.source.clone(),
                    message: error.to_string(),
                })?;

        Ok(Self {
            id: input.id,
            name: input.name,
            description: input.description,
            mana_cost: script_definition.mana_cost,
            quantity: script_definition.quantity,
            card_type: script_definition.card_type,
            image_url: input.image_url,
            script: input.script,
            source: input.source,
            event_handlers: script_definition.event_handlers,
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
            usable: true,
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
            usable: card.usable,
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
            usable: card.usable,
        }
    }
}

#[derive(Debug, Clone)]
struct PowerDeckDefinition {
    generic_card_ids: Vec<CardId>,
    mercenary_card_ids: HashMap<MercenaryId, Vec<CardId>>,
}

impl PowerDeckDefinition {
    fn from_input(input: PowerDeckDefinitionInput) -> Self {
        Self {
            generic_card_ids: input.generic_card_ids,
            mercenary_card_ids: input.mercenary_card_ids,
        }
    }

    fn is_partitioned(&self) -> bool {
        !self.generic_card_ids.is_empty() || !self.mercenary_card_ids.is_empty()
    }

    fn selected_card_ids(&self) -> Vec<CardId> {
        self.generic_card_ids
            .iter()
            .cloned()
            .chain(
                self.mercenary_card_ids
                    .values()
                    .flat_map(|card_ids| card_ids.iter().cloned()),
            )
            .collect()
    }
}

#[derive(Debug, Clone)]
struct MercenaryDefinition {
    id: MercenaryId,
    base_life: usize,
    initial_mana: usize,
    passive_script: String,
    passive_source: String,
}

impl MercenaryDefinition {
    fn from_input(input: MercenaryDefinitionInput) -> Result<Self, PowerCardDefinitionError> {
        let passive_definition = super::power_lua::parse_mercenary_passive_definition(
            &input.passive_script,
            &input.passive_source,
        )
        .map_err(|error| PowerCardDefinitionError::InvalidDefinition {
            path: input.passive_source.clone(),
            message: error.to_string(),
        })?;

        Ok(Self {
            id: input.id,
            base_life: passive_definition.base_life,
            initial_mana: passive_definition.initial_mana,
            passive_script: input.passive_script,
            passive_source: input.passive_source,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct PowerCardRegistry {
    card_definitions: Arc<HashMap<CardId, PowerCardDefinition>>,
    deck_definitions: Arc<HashMap<DeckId, PowerDeckDefinition>>,
    mercenary_definitions: Arc<HashMap<MercenaryId, MercenaryDefinition>>,
}

impl PowerCardRegistry {
    pub fn replace_power_card_registry(
        &mut self,
        definitions: Vec<PowerCardDefinitionInput>,
        decks: Vec<PowerDeckDefinitionInput>,
    ) -> Result<(), PowerCardDefinitionError> {
        let definitions = validate_power_card_definitions(definitions)?;

        *Arc::make_mut(&mut self.card_definitions) = definitions
            .into_iter()
            .map(|definition| (definition.id.clone(), definition))
            .collect();
        *Arc::make_mut(&mut self.deck_definitions) = decks
            .into_iter()
            .map(|deck| (deck.id.clone(), PowerDeckDefinition::from_input(deck)))
            .collect();

        Ok(())
    }

    pub fn replace_power_card_definitions(
        &mut self,
        deck_id: DeckId,
        definitions: Vec<PowerCardDefinitionInput>,
    ) -> Result<(), PowerCardDefinitionError> {
        let generic_card_ids = definitions
            .iter()
            .map(|definition| definition.id.clone())
            .collect();

        self.replace_power_card_registry(
            definitions,
            vec![PowerDeckDefinitionInput {
                id: deck_id,
                generic_card_ids,
                mercenary_card_ids: HashMap::new(),
            }],
        )
    }

    pub fn upsert_power_card_definition(
        &mut self,
        definition: PowerCardDefinitionInput,
    ) -> Result<(), PowerCardDefinitionError> {
        let definition = PowerCardDefinition::from_input(definition)?;

        Arc::make_mut(&mut self.card_definitions).insert(definition.id.clone(), definition);

        Ok(())
    }

    pub fn upsert_power_deck_definition(&mut self, definition: PowerDeckDefinitionInput) {
        Arc::make_mut(&mut self.deck_definitions).insert(
            definition.id.clone(),
            PowerDeckDefinition::from_input(definition),
        );
    }

    pub fn replace_mercenary_definitions(
        &mut self,
        definitions: Vec<MercenaryDefinitionInput>,
    ) -> Result<(), PowerCardDefinitionError> {
        let mut loaded = HashMap::new();

        for definition in definitions {
            let definition = MercenaryDefinition::from_input(definition)?;

            if loaded.contains_key(&definition.id) {
                return Err(PowerCardDefinitionError::DuplicateId {
                    id: definition.id.to_string(),
                    path: definition.passive_source,
                });
            }

            loaded.insert(definition.id.clone(), definition);
        }

        *Arc::make_mut(&mut self.mercenary_definitions) = loaded;

        Ok(())
    }

    pub fn upsert_mercenary_definition(
        &mut self,
        definition: MercenaryDefinitionInput,
    ) -> Result<(), PowerCardDefinitionError> {
        let definition = MercenaryDefinition::from_input(definition)?;

        Arc::make_mut(&mut self.mercenary_definitions).insert(definition.id.clone(), definition);

        Ok(())
    }

    fn mercenary_definition(&self, id: &MercenaryId) -> Option<MercenaryDefinition> {
        self.mercenary_definitions.get(id).cloned()
    }

    fn power_deck_definition(
        &self,
        deck_id: &DeckId,
    ) -> Result<PowerDeckDefinition, PowerCardDefinitionError> {
        self.deck_definitions
            .get(deck_id)
            .cloned()
            .ok_or(PowerCardDefinitionError::MissingDefinitions)
    }

    fn power_card_definition(
        &self,
        deck_id: &DeckId,
        id: &CardId,
    ) -> Result<Option<PowerCardDefinition>, PowerCardDefinitionError> {
        Ok(self
            .power_card_definitions(deck_id)?
            .iter()
            .find(|definition| &definition.id == id)
            .cloned())
    }

    fn power_card_definitions(
        &self,
        deck_id: &DeckId,
    ) -> Result<Vec<PowerCardDefinition>, PowerCardDefinitionError> {
        let selected_card_ids = self.power_deck_definition(deck_id)?.selected_card_ids();

        self.power_card_definitions_from_ids(&selected_card_ids)
    }

    fn power_card_definitions_from_ids(
        &self,
        card_ids: &[CardId],
    ) -> Result<Vec<PowerCardDefinition>, PowerCardDefinitionError> {
        let definitions = card_ids
            .iter()
            .filter_map(|card_id| self.card_definitions.get(card_id).cloned())
            .collect::<Vec<_>>();

        if definitions.is_empty() {
            return Err(PowerCardDefinitionError::MissingDefinitions);
        }

        Ok(definitions)
    }

    fn draw_power_cards_for_player(
        &self,
        deck_id: &DeckId,
        player_mercenaries: &HashMap<PlayerId, MercenaryId>,
        player_id: &PlayerId,
        count: usize,
        (seed, sequence): (i64, i64),
        offset: usize,
    ) -> Result<Vec<PowerCard>, PowerCardDefinitionError> {
        let deck_definition = self.power_deck_definition(deck_id)?;
        let definitions =
            self.draw_power_card_definitions(&deck_definition, player_mercenaries, player_id)?;
        if definitions.is_empty() {
            return Ok(Vec::new());
        }
        let needed_cards = offset.saturating_add(count);
        let mut deck = (0..needed_cards)
            .map(|idx| definitions[idx % definitions.len()].to_card())
            .collect::<Vec<_>>();

        shuffle_power_cards(&mut deck, seed, sequence);

        Ok(deck.into_iter().skip(offset).take(count).collect())
    }

    fn draw_power_card_definitions(
        &self,
        deck_definition: &PowerDeckDefinition,
        player_mercenaries: &HashMap<PlayerId, MercenaryId>,
        player_id: &PlayerId,
    ) -> Result<Vec<PowerCardDefinition>, PowerCardDefinitionError> {
        if !deck_definition.is_partitioned() {
            return Ok(Vec::new());
        }

        let mut card_ids = deck_definition.generic_card_ids.clone();
        if let Some(mercenary_id) = player_mercenaries.get(player_id)
            && let Some(mercenary_card_ids) = deck_definition.mercenary_card_ids.get(mercenary_id)
        {
            card_ids.extend(mercenary_card_ids.iter().cloned());
        }

        if card_ids.is_empty() {
            Ok(Vec::new())
        } else {
            self.weighted_power_card_definitions_from_ids(&card_ids)
        }
    }

    fn weighted_power_card_definitions_from_ids(
        &self,
        card_ids: &[CardId],
    ) -> Result<Vec<PowerCardDefinition>, PowerCardDefinitionError> {
        let definitions = self.power_card_definitions_from_ids(card_ids)?;
        let definitions = definitions
            .into_iter()
            .flat_map(|definition| {
                let quantity = definition.quantity.max(1);

                std::iter::repeat_n(definition, quantity)
            })
            .collect::<Vec<_>>();

        if definitions.is_empty() {
            return Err(PowerCardDefinitionError::MissingDefinitions);
        }

        Ok(definitions)
    }
}

#[derive(Debug, Clone, Default)]
pub struct PowerCardRegistryStore {
    registry: Arc<RwLock<PowerCardRegistry>>,
}

impl PowerCardRegistryStore {
    pub fn snapshot(&self) -> PowerCardRegistry {
        self.registry
            .read()
            .expect("power card registry lock poisoned")
            .clone()
    }

    pub fn replace_power_card_registry(
        &self,
        definitions: Vec<PowerCardDefinitionInput>,
        decks: Vec<PowerDeckDefinitionInput>,
    ) -> Result<(), PowerCardDefinitionError> {
        self.registry
            .write()
            .expect("power card registry lock poisoned")
            .replace_power_card_registry(definitions, decks)
    }

    pub fn replace_power_card_definitions(
        &self,
        deck_id: DeckId,
        definitions: Vec<PowerCardDefinitionInput>,
    ) -> Result<(), PowerCardDefinitionError> {
        self.registry
            .write()
            .expect("power card registry lock poisoned")
            .replace_power_card_definitions(deck_id, definitions)
    }

    pub fn upsert_power_card_definition(
        &self,
        definition: PowerCardDefinitionInput,
    ) -> Result<(), PowerCardDefinitionError> {
        self.registry
            .write()
            .expect("power card registry lock poisoned")
            .upsert_power_card_definition(definition)
    }

    pub fn upsert_power_deck_definition(&self, definition: PowerDeckDefinitionInput) {
        self.registry
            .write()
            .expect("power card registry lock poisoned")
            .upsert_power_deck_definition(definition);
    }

    pub fn replace_mercenary_definitions(
        &self,
        definitions: Vec<MercenaryDefinitionInput>,
    ) -> Result<(), PowerCardDefinitionError> {
        self.registry
            .write()
            .expect("power card registry lock poisoned")
            .replace_mercenary_definitions(definitions)
    }

    pub fn upsert_mercenary_definition(
        &self,
        definition: MercenaryDefinitionInput,
    ) -> Result<(), PowerCardDefinitionError> {
        self.registry
            .write()
            .expect("power card registry lock poisoned")
            .upsert_mercenary_definition(definition)
    }
}

impl Game {
    pub fn new(
        players: &[PlayerId],
        settings: GameSettings,
        registry: PowerCardRegistry,
    ) -> Result<Self, GameError> {
        Self::new_with_seeds(players, settings, rand::random(), rand::random(), registry)
    }

    pub fn new_with_seed(
        players: &[PlayerId],
        settings: GameSettings,
        seed: i64,
        registry: PowerCardRegistry,
    ) -> Result<Self, GameError> {
        Self::new_with_seeds(players, settings, seed, seed, registry)
    }

    pub fn new_with_seeds(
        players: &[PlayerId],
        settings: GameSettings,
        seed: i64,
        draw_seed: i64,
        registry: PowerCardRegistry,
    ) -> Result<Self, GameError> {
        let event =
            Self::start_match_event_with_seeds(players, settings, seed, draw_seed, &registry)?;

        match event {
            MatchEvent::GameStarted {
                settings,
                seed,
                initial_mana,
                draw_seed,
                passive_effects,
                ..
            } => {
                let mut game =
                    Self::from_started(players, settings, seed, initial_mana, draw_seed, registry)?;
                game.apply_effects(&passive_effects);

                Ok(game)
            }
            _ => unreachable!("start_match_event only emits GameStarted"),
        }
    }

    pub fn start_match_event(
        players: &[PlayerId],
        settings: GameSettings,
        registry: &PowerCardRegistry,
    ) -> Result<MatchEvent, GameError> {
        Self::start_match_event_with_seeds(
            players,
            settings,
            rand::random(),
            rand::random(),
            registry,
        )
    }

    pub fn start_match_event_with_seed(
        players: &[PlayerId],
        settings: GameSettings,
        seed: i64,
        registry: &PowerCardRegistry,
    ) -> Result<MatchEvent, GameError> {
        Self::start_match_event_with_seeds(players, settings, seed, seed, registry)
    }

    pub fn start_match_event_with_seeds(
        players: &[PlayerId],
        settings: GameSettings,
        seed: i64,
        draw_seed: i64,
        registry: &PowerCardRegistry,
    ) -> Result<MatchEvent, GameError> {
        let initial_mana = Self::initial_mana(players, &settings.player_mercenaries, registry)?;
        let game = Self::from_started(
            players,
            settings.clone(),
            seed,
            initial_mana.clone(),
            draw_seed,
            registry.clone(),
        )?;
        let mut preview = game.clone();
        let mut passive_effects = preview
            .passive_effects(PassiveGameEvent::MatchStarted)
            .map_err(|error| GameError::PowerScript(error.to_string()))?;
        preview.apply_effects(&passive_effects);
        passive_effects.merge(
            preview
                .passive_effects(PassiveGameEvent::SetStarted)
                .map_err(|error| GameError::PowerScript(error.to_string()))?,
        );

        let player_mercenary_versions = settings
            .player_mercenaries
            .keys()
            .cloned()
            .map(|player_id| (player_id, 1))
            .collect();

        Ok(MatchEvent::GameStarted {
            settings,
            seed,
            initial_mana,
            power_deck_version: 1,
            player_mercenary_versions,
            draw_seed,
            passive_effects,
        })
    }

    pub fn from_started(
        players: &[PlayerId],
        settings: GameSettings,
        seed: i64,
        initial_mana: IndexMap<PlayerId, PlayerMana>,
        draw_seed: i64,
        registry: PowerCardRegistry,
    ) -> Result<Self, GameError> {
        let classic = fodinha_classic::Game::from_started_with_rules(
            players,
            Self::classic_settings(&settings),
            {
                let event = fodinha_classic::Game::start_match_event_with_seed(
                    players,
                    Self::classic_settings(&settings),
                    seed,
                )?;
                let fodinha_classic::MatchEvent::GameStarted { seed, .. } = event else {
                    unreachable!()
                };
                let mut deck = Card::shuffled_deck(seed, 0);
                let decks = players
                    .iter()
                    .map(|player| (player.clone(), vec![deck.remove(0)]))
                    .collect();
                let upcard = deck[0];
                NewSet {
                    dealing_mode: fodinha_classic::DealingMode::Increasing,
                    cards_count: 1,
                    shuffle: DeckShuffle { seed, sequence: 0 },
                    decks,
                    upcard,
                }
            },
            fodinha_classic::GameRules {
                life_loss_per_bid_difference: LIFE_LOSS_PER_BID_DIFFERENCE,
            },
        )?;
        let power_set = Self::new_power_set(
            players,
            seed,
            0,
            &settings.power_deck_id,
            &settings.player_mercenaries,
            initial_mana.clone(),
            &registry,
        )?;

        let mut game = Self {
            core: classic,
            stage: PowerGameStage::Bidding,
            power_decks: power_set.decks,
            mana: initial_mana,
            registry,
            power_deck_id: settings.power_deck_id,
            player_mercenaries: settings.player_mercenaries,
            power_seed: seed,
            draw_seed,
            next_power_shuffle_sequence: power_set.shuffle.sequence.wrapping_add(1),
            pending_set_resolution: None,
        };
        let initial_lifes = game.initial_lifes(players, settings.life_multiplier)?;
        game.core.apply_life_totals(&initial_lifes);

        Ok(game)
    }

    pub fn validate_bid(
        &self,
        player_id: &PlayerId,
        bid: usize,
    ) -> Result<MatchEvent, BiddingError> {
        self.core.validate_bid(player_id, bid)?;
        let passive_effects = self
            .passive_effects(PassiveGameEvent::BidPlaced {
                player_id: player_id.clone(),
                bid,
            })
            .map_err(|error| BiddingError::PowerScript(error.to_string()))?;

        Ok(MatchEvent::BidPlaced {
            player_id: player_id.clone(),
            bid,
            mana: self.mana_after_bid(player_id, bid),
            passive_effects,
        })
    }

    pub fn validate_turn(&self, turn: Turn) -> Result<MatchEvent, DealError> {
        if !matches!(self.stage, PowerGameStage::Dealing) {
            return Err(DealError::BiddingStageActive);
        }

        let event = self.core.validate_turn(turn)?;
        let fodinha_classic::MatchEvent::TurnPlayed { turn } = event else {
            unreachable!("validate_turn only emits TurnPlayed")
        };
        let mut passive_effects = self
            .passive_effects(PassiveGameEvent::TurnPlayed {
                player_id: turn.player_id.clone(),
                card: turn.card,
            })
            .map_err(|error| DealError::PowerScript(error.to_string()))?;

        let base_event = MatchEvent::TurnPlayed {
            turn: turn.clone(),
            passive_effects: passive_effects.clone(),
        };

        let mut preview = self.clone();
        let round_winner = match preview.apply_match_event(base_event) {
            AppliedGameChange::TurnPlayed {
                state:
                    DealState {
                        outcome: fodinha_classic::GameOutcome::RoundEnded { next, .. },
                        pile,
                    },
                ..
            } => pile
                .into_iter()
                .find(|turn| turn.player_id == next)
                .map(|turn| (next, turn.card)),
            _ => None,
        };

        if let Some((winner, card)) = round_winner {
            passive_effects.merge(
                preview
                    .passive_effects(PassiveGameEvent::RoundEnded { winner, card })
                    .map_err(|error| DealError::PowerScript(error.to_string()))?,
            );
        }

        Ok(MatchEvent::TurnPlayed {
            turn,
            passive_effects,
        })
    }

    pub fn validate_power_card(
        &self,
        player_id: &PlayerId,
        card_id: &CardId,
        target_player_id: Option<PlayerId>,
    ) -> Result<MatchEvent, PowerCardError> {
        if !matches!(self.stage, PowerGameStage::Power { .. }) {
            return Err(PowerCardError::PowerStageRequired);
        }

        if self.current_player().as_ref() != Some(player_id) {
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

        if !card.usable {
            return Err(PowerCardError::CardDisabled);
        }

        let definition = self
            .registry
            .power_card_definition(&self.power_deck_id, &card.id)?
            .ok_or(PowerCardError::InvalidPowerCard)?;

        if definition.card_type.needs_target() && target_player_id.is_none() {
            return Err(PowerCardError::TargetRequired);
        }

        if let Some(target_player_id) = target_player_id.as_ref()
            && !self.core.is_player_alive(target_player_id)
        {
            return Err(PowerCardError::InvalidTarget);
        }

        let output = self.run_power_script(&definition, player_id, target_player_id.clone())?;
        let mana_cost = output
            .mana_cost
            .unwrap_or_else(|| i64::try_from(definition.mana_cost).unwrap_or(i64::MAX));
        let owner_current_mana = output
            .mana
            .get(player_id)
            .map(|mana| mana.current)
            .or_else(|| self.mana.get(player_id).map(|mana| mana.current))
            .unwrap_or_default();

        if mana_cost > 0 && owner_current_mana < usize::try_from(mana_cost).unwrap_or(usize::MAX) {
            return Err(PowerCardError::NotEnoughMana);
        }

        let mut card = card;
        card.mana_cost = usize::try_from(mana_cost).unwrap_or(0);

        let mut effects = self.power_card_effects(player_id, mana_cost, output);
        effects.merge(
            self.passive_effects(PassiveGameEvent::PowerCardPlayed {
                player_id: player_id.clone(),
                card_id: definition.id.as_str().to_string(),
                target_player_id: target_player_id.clone(),
            })
            .map_err(PowerCardError::Script)?,
        );

        let finalizing_set = matches!(
            self.stage,
            PowerGameStage::Power {
                phase: PowerPhase::Second,
                ref pending_players,
            } if pending_players.len() == 1
        );
        let next_set = if finalizing_set {
            match &self.pending_set_resolution {
                Some(pending) => Some(pending.next_set.clone()),
                None => None,
            }
        } else {
            None
        };

        let mut set_ended_effects = PowerCardEffects::default();
        if finalizing_set {
            let mut set_preview = self.clone();
            if let Some(deck) = set_preview.power_decks.get_mut(player_id)
                && let Some(idx) = deck.iter().position(|held| held.id == card.id)
            {
                deck.remove(idx);
            }
            set_preview.apply_effects(&effects);
            let bids = set_preview.current_set_bids();
            let before = set_preview.core.get_lifes();
            let player_ids: Vec<_> = set_preview
                .core
                .get_player_snapshots()
                .keys()
                .cloned()
                .collect();
            set_preview.core.finalize_pending_set(next_set.as_ref());
            let lost_players = player_ids
                .into_iter()
                .filter_map(|player_id| {
                    let lives = before[&player_id];
                    let remaining_lives = set_preview
                        .core
                        .get_lifes()
                        .get(&player_id)
                        .copied()
                        .unwrap_or(lives);
                    (remaining_lives < lives).then_some((player_id, lives - remaining_lives))
                })
                .collect();
            set_ended_effects = set_preview
                .passive_effects(PassiveGameEvent::SetEnded { lost_players, bids })
                .map_err(PowerCardError::Script)?;
        }

        let mut preview = self.clone();
        preview.apply_match_event(MatchEvent::PowerCardPlayed {
            player_id: player_id.clone(),
            card: card.clone(),
            target_player_id: target_player_id.clone(),
            effects: effects.clone(),
            set_ended_effects: set_ended_effects.clone(),
            set_started_effects: PowerCardEffects::default(),
        });

        if matches!(preview.stage, PowerGameStage::Dealing) && !preview.is_finished() {
            effects.merge(
                preview
                    .passive_effects(PassiveGameEvent::RoundStart)
                    .map_err(PowerCardError::Script)?,
            );
        }

        let next_set_passive_effects = if finalizing_set && !preview.is_finished() {
            preview
                .passive_effects(PassiveGameEvent::SetStarted)
                .map_err(PowerCardError::Script)?
        } else {
            PowerCardEffects::default()
        };

        Ok(MatchEvent::PowerCardPlayed {
            player_id: player_id.clone(),
            card,
            target_player_id,
            effects,
            set_ended_effects,
            set_started_effects: next_set_passive_effects,
        })
    }

    pub fn validate_skip_power_phase(
        &self,
        player_id: &PlayerId,
    ) -> Result<MatchEvent, PowerCardError> {
        if !matches!(self.stage, PowerGameStage::Power { .. }) {
            return Err(PowerCardError::PowerStageRequired);
        }

        if self.current_player().as_ref() != Some(player_id) {
            return Err(PowerCardError::NotYourTurn);
        }

        if !self.core.is_player_alive(player_id) {
            return Err(PowerCardError::InvalidPlayer);
        }

        let finalizing_set = matches!(
            self.stage,
            PowerGameStage::Power {
                phase: PowerPhase::Second,
                ref pending_players,
            } if pending_players.len() == 1
        );
        let next_set = if finalizing_set {
            match &self.pending_set_resolution {
                Some(pending) => Some(pending.next_set.clone()),
                None => None,
            }
        } else {
            None
        };
        let mut effects = PowerCardEffects::default();
        let mut set_ended_effects = PowerCardEffects::default();

        if finalizing_set {
            let mut set_preview = self.clone();
            set_preview.apply_effects(&effects);
            let bids = set_preview.current_set_bids();
            let before = set_preview.core.get_lifes();
            let player_ids: Vec<_> = set_preview
                .core
                .get_player_snapshots()
                .keys()
                .cloned()
                .collect();
            set_preview.core.finalize_pending_set(next_set.as_ref());
            let lost_players = player_ids
                .into_iter()
                .filter_map(|player_id| {
                    let lives = before[&player_id];
                    let remaining_lives = set_preview
                        .core
                        .get_lifes()
                        .get(&player_id)
                        .copied()
                        .unwrap_or(lives);
                    (remaining_lives < lives).then_some((player_id, lives - remaining_lives))
                })
                .collect();
            set_ended_effects = set_preview
                .passive_effects(PassiveGameEvent::SetEnded { lost_players, bids })
                .map_err(PowerCardError::Script)?;
        }

        let mut preview = self.clone();
        preview.apply_match_event(MatchEvent::PowerPhaseSkipped {
            player_id: player_id.clone(),
            effects: effects.clone(),
            set_ended_effects: set_ended_effects.clone(),
            set_started_effects: PowerCardEffects::default(),
        });

        if matches!(preview.stage, PowerGameStage::Dealing) && !preview.is_finished() {
            effects.merge(
                preview
                    .passive_effects(PassiveGameEvent::RoundStart)
                    .map_err(PowerCardError::Script)?,
            );
        }

        let next_set_passive_effects = if finalizing_set && !preview.is_finished() {
            preview
                .passive_effects(PassiveGameEvent::SetStarted)
                .map_err(PowerCardError::Script)?
        } else {
            PowerCardEffects::default()
        };

        Ok(MatchEvent::PowerPhaseSkipped {
            player_id: player_id.clone(),
            effects,
            set_ended_effects,
            set_started_effects: next_set_passive_effects,
        })
    }

    pub fn apply_match_event(&mut self, event: MatchEvent) -> AppliedGameChange {
        match event {
            MatchEvent::BidPlaced {
                player_id,
                bid,
                mana,
                passive_effects,
            } => {
                match self
                    .core
                    .apply_match_event(fodinha_classic::MatchEvent::BidPlaced { player_id, bid })
                {
                    fodinha_classic::AppliedGameChange::BidPlaced {
                        player_id,
                        bid,
                        state,
                    } => {
                        let deck_reveals = passive_effects.deck_reveals.clone();
                        let mut mana = self.apply_mana_totals(&mana);
                        let (passive_mana, _) = self.apply_effects(&passive_effects);
                        self.set_stage_after_bid(&state);
                        mana.extend(passive_mana);

                        AppliedGameChange::BidPlaced {
                            player_id,
                            bid,
                            state,
                            mana,
                            deck_reveals,
                        }
                    }
                    _ => unreachable!("bid event applies as bid change"),
                }
            }
            MatchEvent::TurnPlayed {
                turn,
                passive_effects,
            } => {
                let next_set = self.core.next_set_for_turn(&turn);
                let next_power_set = next_set.as_ref().map(|set| {
                    let players: Vec<_> = set.decks.keys().cloned().collect();
                    self.new_power_set_for_game(&players)
                });
                let is_set_end = next_set.is_some();
                let state = self.core.apply_turn(turn, next_set.clone(), !is_set_end);

                let deck_reveals = passive_effects.deck_reveals.clone();
                let (passive_mana, passive_power_decks) = self.apply_effects(&passive_effects);
                let mana = if passive_mana.is_empty() {
                    None
                } else {
                    Some(passive_mana)
                };
                let mut power_decks = if passive_power_decks.is_empty() {
                    None
                } else {
                    Some(passive_power_decks.into_iter().collect::<IndexMap<_, _>>())
                };

                let mut mana = mana;
                if is_set_end {
                    self.merge_effects_into_pending_resolution(&passive_effects);
                    if let (Some(next_set), Some(next_power_set)) =
                        (next_set.clone(), next_power_set)
                    {
                        self.apply_power_set(&next_power_set);
                        power_decks = Some(dto_decks(&self.power_decks));
                        mana = Some(mana_to_dto(&self.mana));
                        self.pending_set_resolution = Some(PendingSetResolution {
                            next_set,
                            next_power_set,
                        });
                        self.stage = PowerGameStage::Power {
                            phase: PowerPhase::Second,
                            pending_players: self.power_phase_order(),
                        };
                    }
                } else {
                    self.stage = PowerGameStage::Dealing;
                }

                AppliedGameChange::TurnPlayed {
                    state,
                    lifes: None,
                    power_decks,
                    mana,
                    deck_reveals,
                }
            }
            MatchEvent::PowerCardPlayed {
                player_id,
                card,
                target_player_id,
                effects,
                set_ended_effects,
                set_started_effects,
            } => {
                let (next_set, next_power_set) = if matches!(
                    self.stage,
                    PowerGameStage::Power {
                        phase: PowerPhase::Second,
                        ref pending_players,
                    } if pending_players.len() == 1
                ) {
                    match &self.pending_set_resolution {
                        Some(pending) => (
                            Some(pending.next_set.clone()),
                            Some(pending.next_power_set.clone()),
                        ),
                        None => (None, None),
                    }
                } else {
                    (None, None)
                };
                if let Some(deck) = self.power_decks.get_mut(&player_id)
                    && let Some(idx) = deck.iter().position(|held| held.id == card.id)
                {
                    deck.remove(idx);
                }

                let (mana, power_decks) = self.apply_effects(&effects);
                let deck_reveals =
                    Self::merge_deck_reveals(&effects, &set_ended_effects, &set_started_effects);
                let completed_phase = self.advance_power_phase(&player_id);

                if next_set.is_some() {
                    self.merge_effects_into_pending_resolution(&effects);

                    let Some(next_set) = next_set else {
                        unreachable!("next_set should be present when finalizing a set")
                    };
                    let Some(next_power_set) = next_power_set else {
                        unreachable!("next_power_set should be present when finalizing a set")
                    };

                    let _ = self.core.finalize_pending_set(Some(&next_set));
                    let _ = self.apply_effects(&set_ended_effects);
                    if self.core.is_finished() {
                        return AppliedGameChange::PowerCardPlayed(PowerCardOutcome {
                            player_id,
                            card: card.to_dto(),
                            target_player_id,
                            lifes: self.core.get_lifes(),
                            mana: mana_to_dto(&self.mana),
                            decks: self
                                .core
                                .get_player_snapshots()
                                .into_iter()
                                .map(|(player_id, snapshot)| (player_id, snapshot.deck))
                                .collect(),
                            power_decks: self
                                .power_decks
                                .iter()
                                .map(|(player_id, deck)| {
                                    (
                                        player_id.clone(),
                                        deck.iter()
                                            .map(|card| self.to_hand_dto(player_id, card))
                                            .collect(),
                                    )
                                })
                                .collect(),
                            deck_reveals,
                            ended: true,
                            next_set: None,
                            next_power_set: None,
                            next_set_passive_effects: PowerCardEffects::default(),
                        });
                    }

                    self.apply_power_set(&next_power_set);

                    let _ = self.apply_effects(&set_started_effects);
                    self.stage = PowerGameStage::Bidding;

                    return AppliedGameChange::PowerCardPlayed(PowerCardOutcome {
                        player_id,
                        card: card.to_dto(),
                        target_player_id,
                        lifes: self.core.get_lifes(),
                        mana: mana_to_dto(&self.mana),
                        decks: self
                            .core
                            .get_player_snapshots()
                            .into_iter()
                            .map(|(player_id, snapshot)| (player_id, snapshot.deck))
                            .collect(),
                        power_decks: self
                            .power_decks
                            .iter()
                            .map(|(player_id, deck)| {
                                (
                                    player_id.clone(),
                                    deck.iter()
                                        .map(|card| self.to_hand_dto(player_id, card))
                                        .collect(),
                                )
                            })
                            .collect(),
                        deck_reveals,
                        ended: self.core.is_finished(),
                        next_set: Some(next_set),
                        next_power_set: Some(next_power_set),
                        next_set_passive_effects: set_started_effects,
                    });
                }

                if matches!(completed_phase, Some(PowerPhase::First)) {
                    self.stage = PowerGameStage::Dealing;
                }

                AppliedGameChange::PowerCardPlayed(PowerCardOutcome {
                    player_id,
                    card: card.to_dto(),
                    target_player_id,
                    lifes: self.core.get_lifes(),
                    mana,
                    decks: effects.decks,
                    power_decks,
                    deck_reveals,
                    ended: self.core.is_finished(),
                    next_set: None,
                    next_power_set: None,
                    next_set_passive_effects: PowerCardEffects::default(),
                })
            }
            MatchEvent::PowerPhaseSkipped {
                player_id,
                effects,
                set_ended_effects,
                set_started_effects,
            } => {
                let (next_set, next_power_set) = if matches!(
                    self.stage,
                    PowerGameStage::Power {
                        phase: PowerPhase::Second,
                        ref pending_players,
                    } if pending_players.len() == 1
                ) {
                    match &self.pending_set_resolution {
                        Some(pending) => (
                            Some(pending.next_set.clone()),
                            Some(pending.next_power_set.clone()),
                        ),
                        None => (None, None),
                    }
                } else {
                    (None, None)
                };
                let (mana, power_decks) = self.apply_effects(&effects);
                let deck_reveals =
                    Self::merge_deck_reveals(&effects, &set_ended_effects, &set_started_effects);
                let completed_phase = self.advance_power_phase(&player_id);

                if next_set.is_some() {
                    self.merge_effects_into_pending_resolution(&effects);

                    let Some(next_set) = next_set else {
                        unreachable!("next_set should be present when finalizing a set")
                    };
                    let Some(next_power_set) = next_power_set else {
                        unreachable!("next_power_set should be present when finalizing a set")
                    };

                    let _ = self.core.finalize_pending_set(Some(&next_set));
                    let _ = self.apply_effects(&set_ended_effects);
                    if self.core.is_finished() {
                        return AppliedGameChange::PowerPhaseSkipped(PowerPhaseSkipOutcome {
                            player_id,
                            lifes: self.core.get_lifes(),
                            changed_lifes: effects.lifes.clone(),
                            mana: mana_to_dto(&self.mana),
                            decks: self
                                .core
                                .get_player_snapshots()
                                .into_iter()
                                .map(|(player_id, snapshot)| (player_id, snapshot.deck))
                                .collect(),
                            power_decks: self
                                .power_decks
                                .iter()
                                .map(|(player_id, deck)| {
                                    (
                                        player_id.clone(),
                                        deck.iter()
                                            .map(|card| self.to_hand_dto(player_id, card))
                                            .collect(),
                                    )
                                })
                                .collect(),
                            deck_reveals,
                            ended: true,
                            next_set: None,
                            next_power_set: None,
                            next_set_passive_effects: PowerCardEffects::default(),
                        });
                    }

                    self.apply_power_set(&next_power_set);

                    let _ = self.apply_effects(&set_started_effects);
                    self.stage = PowerGameStage::Bidding;

                    return AppliedGameChange::PowerPhaseSkipped(PowerPhaseSkipOutcome {
                        player_id,
                        lifes: self.core.get_lifes(),
                        changed_lifes: effects.lifes.clone(),
                        mana: mana_to_dto(&self.mana),
                        decks: self
                            .core
                            .get_player_snapshots()
                            .into_iter()
                            .map(|(player_id, snapshot)| (player_id, snapshot.deck))
                            .collect(),
                        power_decks: self
                            .power_decks
                            .iter()
                            .map(|(player_id, deck)| {
                                (
                                    player_id.clone(),
                                    deck.iter()
                                        .map(|card| self.to_hand_dto(player_id, card))
                                        .collect(),
                                )
                            })
                            .collect(),
                        deck_reveals,
                        ended: self.core.is_finished(),
                        next_set: Some(next_set),
                        next_power_set: Some(next_power_set),
                        next_set_passive_effects: set_started_effects,
                    });
                }

                if matches!(completed_phase, Some(PowerPhase::First)) {
                    self.stage = PowerGameStage::Dealing;
                }

                AppliedGameChange::PowerPhaseSkipped(PowerPhaseSkipOutcome {
                    player_id,
                    lifes: self.core.get_lifes(),
                    changed_lifes: effects.lifes.clone(),
                    mana,
                    decks: effects.decks,
                    power_decks,
                    deck_reveals,
                    ended: self.core.is_finished(),
                    next_set: None,
                    next_power_set: None,
                    next_set_passive_effects: PowerCardEffects::default(),
                })
            }
            MatchEvent::GameStarted { .. } => unreachable!("GameStarted is applied by facade"),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.core.is_finished()
    }

    pub fn get_lifes(&self) -> HashMap<PlayerId, usize> {
        self.core.get_lifes()
    }

    pub fn current_player(&self) -> Option<PlayerId> {
        match &self.stage {
            PowerGameStage::Bidding | PowerGameStage::Dealing => self.core.current_player(),
            PowerGameStage::Power {
                pending_players, ..
            } => pending_players
                .iter()
                .find(|player_id| self.core.is_player_alive(player_id))
                .cloned(),
        }
    }

    pub fn get_stage_dto(&self) -> GameStageDto {
        match &self.stage {
            PowerGameStage::Bidding => GameStageDto::Bidding {
                possible_bids: self.core.get_possible_bids(),
            },
            PowerGameStage::Power { phase, .. } => GameStageDto::Power {
                phase: match phase {
                    PowerPhase::First => crate::services::PowerPhaseDto::First,
                    PowerPhase::Second => crate::services::PowerPhaseDto::Second,
                },
            },
            PowerGameStage::Dealing => GameStageDto::Dealing,
        }
    }

    pub fn get_game_info(&self, player_id: &PlayerId) -> GameInfoDto {
        let mut info = self.core.get_game_info(player_id);
        info.current_player = self
            .current_player()
            .map(|player_id| player_id.as_str().to_string())
            .unwrap_or_default();
        info.stage = self.get_stage_dto();
        for player in &mut info.info {
            player.mana = self.mana.get(&player.id).map(PlayerMana::to_dto);
        }
        info.power_cards = Some(
            self.power_decks
                .get(player_id)
                .map(|deck| {
                    deck.iter()
                        .map(|card| self.to_hand_dto(player_id, card))
                        .collect()
                })
                .unwrap_or_default(),
        );

        info
    }

    fn to_hand_dto(&self, player_id: &PlayerId, card: &PowerCard) -> PowerCardDto {
        let reason = if !card.usable {
            Some("disabled")
        } else if !matches!(self.stage, PowerGameStage::Power { .. }) {
            Some("not_power_phase")
        } else if self.current_player().as_ref() != Some(player_id) {
            Some("not_your_turn")
        } else if self
            .mana
            .get(player_id)
            .is_none_or(|mana| mana.current < card.mana_cost)
        {
            Some("insufficient_mana")
        } else {
            None
        };
        let mut dto = card.to_dto();
        dto.state = Some(PowerCardStateDto {
            ready: reason.is_none(),
            reason: reason.map(str::to_string),
        });
        dto
    }

    pub fn get_bidding_player(&self) -> PlayerId {
        self.core.get_bidding_player()
    }

    pub fn get_possible_bids(&self) -> Vec<usize> {
        self.core.get_possible_bids()
    }

    pub fn apply_start_effects(
        &mut self,
        effects: &PowerCardEffects,
    ) -> (
        HashMap<PlayerId, PlayerManaDto>,
        HashMap<PlayerId, Vec<PowerCardDto>>,
    ) {
        self.apply_effects(effects)
    }

    fn run_power_script(
        &self,
        definition: &PowerCardDefinition,
        owner_id: &PlayerId,
        target_player_id: Option<PlayerId>,
    ) -> Result<PowerScriptOutput, PowerCardError> {
        let players = self.script_players(Some((owner_id, &definition.id)));
        let draw_power_cards = self.power_card_drawer();

        Ok(super::power_lua::run_power_card_script(
            &definition.script,
            PowerScriptInput {
                card_id: definition.id.as_str().to_string(),
                mana_cost: definition.mana_cost,
                owner_id: owner_id.clone(),
                target_player_id,
                players,
                draw_power_cards,
                event: None,
                card_state: None,
                current_trump: self.core.current_trump(),
            },
        )?)
    }

    fn run_power_card_event(
        &self,
        definition: &PowerCardDefinition,
        owner_id: &PlayerId,
        card: &PowerCard,
        event: PassiveGameEvent,
    ) -> Result<PowerScriptOutput, PowerScriptError> {
        super::power_lua::run_power_card_script(
            &definition.script,
            PowerScriptInput {
                card_id: definition.id.as_str().to_string(),
                mana_cost: definition.mana_cost,
                owner_id: owner_id.clone(),
                target_player_id: None,
                players: self.script_players(None),
                draw_power_cards: self.power_card_drawer(),
                event: Some(event),
                card_state: Some(card.into()),
                current_trump: self.core.current_trump(),
            },
        )
    }

    fn passive_effects(
        &self,
        event: PassiveGameEvent,
    ) -> Result<PowerCardEffects, PowerScriptError> {
        let mut effects = PowerCardEffects::default();
        let mut preview = self.clone();

        for player_id in self.power_phase_order() {
            if !preview.core.is_player_alive(&player_id) {
                continue;
            }

            let Some(mercenary_id) = preview.player_mercenaries.get(&player_id) else {
                continue;
            };
            let Some(definition) = self.registry.mercenary_definition(mercenary_id) else {
                continue;
            };
            let output = super::power_lua::run_passive_script(
                &definition.passive_script,
                PassiveScriptInput {
                    mercenary_id: definition.id,
                    owner_id: player_id.clone(),
                    base_life: definition.base_life,
                    initial_mana: definition.initial_mana,
                    event: event.clone(),
                    players: preview.script_players(None),
                    draw_power_cards: preview.power_card_drawer(),
                    current_trump: preview.core.current_trump(),
                },
            )?;

            let output_effects = Self::script_output_effects(output);
            preview.apply_effects(&output_effects);
            effects.merge(output_effects);
        }

        // Card handlers are opt-in: the runtime selects only the handler for
        // `event`, and the first copy of a definition is sufficient because
        // set_usable updates every matching copy in that player's hand.
        for player_id in self.power_phase_order() {
            let mut definition_ids = Vec::new();
            if let Some(deck) = preview.power_decks.get(&player_id) {
                for card in deck {
                    if !definition_ids.contains(&card.id) {
                        definition_ids.push(card.id.clone());
                    }
                }
            }

            for card_id in definition_ids {
                let Some(card) = preview
                    .power_decks
                    .get(&player_id)
                    .and_then(|deck| deck.iter().find(|card| card.id == card_id))
                    .cloned()
                else {
                    continue;
                };
                let definition = preview
                    .registry
                    .power_card_definition(&preview.power_deck_id, &card.id)
                    .map_err(|error| PowerScriptError::Lua(mlua::Error::external(error)))?
                    .ok_or_else(|| {
                        PowerScriptError::Lua(mlua::Error::external(format!(
                            "missing power card definition {}",
                            card.id
                        )))
                    })?;
                if !definition
                    .event_handlers
                    .iter()
                    .any(|handler| handler == event.handler_name())
                {
                    continue;
                }
                let output =
                    preview.run_power_card_event(&definition, &player_id, &card, event.clone())?;
                let output_effects = Self::script_output_effects(output);
                preview.apply_effects(&output_effects);
                effects.merge(output_effects);
            }
        }

        Ok(effects)
    }

    fn current_set_bids(&self) -> HashMap<PlayerId, usize> {
        self.core
            .get_player_snapshots()
            .into_iter()
            .filter_map(|(player_id, player)| player.bid.map(|bid| (player_id, bid)))
            .collect()
    }

    fn script_players(
        &self,
        excluded_power_card: Option<(&PlayerId, &CardId)>,
    ) -> HashMap<PlayerId, ScriptPlayerState> {
        self.core
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
                                excluded_power_card.is_none_or(|(owner_id, card_id)| {
                                    &player_id != owner_id || &card.id != card_id
                                })
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
            .collect()
    }

    fn power_card_drawer(&self) -> DrawPowerCardsFn {
        let power_deck_id = self.power_deck_id.clone();
        let registry = self.registry.clone();
        let player_mercenaries = self.player_mercenaries.clone();
        let draw_seed = self.draw_seed;
        let next_power_shuffle_sequence = self.next_power_shuffle_sequence;
        let mut player_order = self.power_decks.keys().cloned().collect::<Vec<_>>();
        let mut missing_players = self
            .core
            .get_player_snapshots()
            .keys()
            .filter(|player_id| !player_order.contains(player_id))
            .cloned()
            .collect::<Vec<_>>();
        missing_players.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        player_order.extend(missing_players);

        let draw_offsets = Rc::new(RefCell::new(
            self.power_decks
                .iter()
                .map(|(player_id, deck)| (player_id.clone(), deck.len()))
                .collect::<HashMap<_, _>>(),
        ));

        Rc::new(move |player_id, count| {
            let Some(player_id) = player_order
                .iter()
                .find(|known_player_id| known_player_id.as_str() == player_id)
            else {
                return Err(format!("unknown player_id: {player_id}"));
            };

            let offset = draw_offsets
                .borrow()
                .get(player_id)
                .copied()
                .unwrap_or_default();
            let cards = registry
                .draw_power_cards_for_player(
                    &power_deck_id,
                    &player_mercenaries,
                    player_id,
                    count,
                    (draw_seed, next_power_shuffle_sequence),
                    offset,
                )
                .map_err(|error| error.to_string())?;

            draw_offsets
                .borrow_mut()
                .insert(player_id.clone(), offset.saturating_add(count));

            Ok(cards
                .into_iter()
                .map(|card| ScriptPowerCardState::from(&card))
                .collect())
        })
    }

    fn power_card_effects(
        &self,
        owner_id: &PlayerId,
        mana_cost: i64,
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

        if mana_cost < 0 {
            owner_mana.current = owner_mana
                .current
                .saturating_add(usize::try_from(mana_cost.unsigned_abs()).unwrap_or(usize::MAX))
                .min(owner_mana.max);
        } else {
            owner_mana.current = owner_mana
                .current
                .saturating_sub(usize::try_from(mana_cost).unwrap_or(usize::MAX));
        }

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
            deck_reveals: output.deck_reveals,
        }
    }

    fn script_output_effects(output: PowerScriptOutput) -> PowerCardEffects {
        let mana = output
            .mana
            .into_iter()
            .map(|(player_id, mana)| (player_id, PlayerMana::from(mana)))
            .collect::<HashMap<_, _>>();
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
            deck_reveals: output.deck_reveals,
        }
    }

    fn merge_deck_reveals(
        effects: &PowerCardEffects,
        set_ended_effects: &PowerCardEffects,
        set_started_effects: &PowerCardEffects,
    ) -> Vec<DeckReveal> {
        effects
            .deck_reveals
            .iter()
            .chain(&set_ended_effects.deck_reveals)
            .chain(&set_started_effects.deck_reveals)
            .cloned()
            .collect()
    }

    fn classic_settings(_settings: &GameSettings) -> fodinha_classic::GameSettings {
        fodinha_classic::GameSettings {
            lifes: DEFAULT_INITIAL_LIFES,
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

    fn merge_effects_into_pending_resolution(&mut self, effects: &PowerCardEffects) {
        let Some(pending) = self.pending_set_resolution.as_mut() else {
            return;
        };

        for (player_id, deck) in &effects.decks {
            pending
                .next_set
                .decks
                .insert(player_id.clone(), deck.clone());
        }

        for (player_id, deck) in &effects.power_decks {
            pending
                .next_power_set
                .decks
                .insert(player_id.clone(), deck.clone());
        }

        for (player_id, mana) in &effects.mana {
            pending
                .next_power_set
                .mana
                .insert(player_id.clone(), mana.clone());
        }
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

    fn apply_effects(
        &mut self,
        effects: &PowerCardEffects,
    ) -> (
        HashMap<PlayerId, PlayerManaDto>,
        HashMap<PlayerId, Vec<PowerCardDto>>,
    ) {
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
                    deck.iter()
                        .map(|card| self.to_hand_dto(player_id, card))
                        .collect(),
                )
            })
            .collect();

        (mana, power_decks)
    }

    fn set_stage_after_bid(&mut self, state: &BiddingState) {
        self.stage = match state {
            BiddingState::Active { .. } => PowerGameStage::Bidding,
            BiddingState::Ended { .. } => PowerGameStage::Power {
                phase: PowerPhase::First,
                pending_players: self.power_phase_order(),
            },
        };
    }

    fn advance_power_phase(&mut self, player_id: &PlayerId) -> Option<PowerPhase> {
        let alive_players = self
            .core
            .get_player_snapshots()
            .into_iter()
            .filter(|(_, player)| player.lifes > 0)
            .map(|(player_id, _)| player_id)
            .collect::<Vec<_>>();
        let PowerGameStage::Power {
            phase,
            pending_players,
        } = &mut self.stage
        else {
            return None;
        };

        pending_players.retain(|pending_player| {
            pending_player != player_id && alive_players.contains(pending_player)
        });

        if pending_players.is_empty() {
            let phase = *phase;
            self.stage = match phase {
                PowerPhase::First => PowerGameStage::Dealing,
                PowerPhase::Second => PowerGameStage::Power {
                    phase: PowerPhase::Second,
                    pending_players: Vec::new(),
                },
            };

            return Some(phase);
        }

        None
    }

    fn power_phase_order(&self) -> Vec<PlayerId> {
        self.core
            .get_round_order()
            .into_iter()
            .filter(|player_id| self.core.is_player_alive(player_id))
            .collect()
    }

    fn new_power_set_for_game(&self, players: &[PlayerId]) -> PowerSet {
        Self::new_power_set(
            players,
            self.power_seed,
            self.next_power_shuffle_sequence,
            &self.power_deck_id,
            &self.player_mercenaries,
            self.next_set_mana(players),
            &self.registry,
        )
        .expect("FodinhaPower card definitions are loaded before the game starts")
    }

    fn new_power_set(
        players: &[PlayerId],
        seed: i64,
        sequence: i64,
        power_deck_id: &DeckId,
        player_mercenaries: &HashMap<PlayerId, MercenaryId>,
        mana: IndexMap<PlayerId, PlayerMana>,
        registry: &PowerCardRegistry,
    ) -> Result<PowerSet, PowerCardDefinitionError> {
        let definition = registry.power_deck_definition(power_deck_id)?;
        let decks = if definition.is_partitioned() {
            Self::new_partitioned_power_decks(
                players,
                player_mercenaries,
                &definition,
                seed,
                sequence,
                registry,
            )?
        } else {
            Self::new_unpartitioned_power_decks(players)?
        };

        Ok(PowerSet {
            shuffle: DeckShuffle { seed, sequence },
            decks,
            mana,
        })
    }

    fn new_unpartitioned_power_decks(
        players: &[PlayerId],
    ) -> Result<IndexMap<PlayerId, Vec<PowerCard>>, PowerCardDefinitionError> {
        Ok(players
            .iter()
            .map(|player_id| (player_id.clone(), Vec::new()))
            .collect())
    }

    fn new_partitioned_power_decks(
        players: &[PlayerId],
        player_mercenaries: &HashMap<PlayerId, MercenaryId>,
        deck_definition: &PowerDeckDefinition,
        seed: i64,
        sequence: i64,
        registry: &PowerCardRegistry,
    ) -> Result<IndexMap<PlayerId, Vec<PowerCard>>, PowerCardDefinitionError> {
        let mut generic_deck = if deck_definition.generic_card_ids.is_empty() {
            Vec::new()
        } else {
            let generic_cards = registry
                .weighted_power_card_definitions_from_ids(&deck_definition.generic_card_ids)?;
            let mut generic_deck =
                (0..players.len().saturating_mul(GENERIC_POWER_CARDS_PER_PLAYER))
                    .map(|idx| generic_cards[idx % generic_cards.len()].to_card())
                    .collect::<Vec<_>>();
            shuffle_power_cards(&mut generic_deck, seed, sequence);

            generic_deck
        };

        let mut decks = IndexMap::new();

        for (player_idx, player_id) in players.iter().enumerate() {
            let mut cards = generic_deck
                .drain(..GENERIC_POWER_CARDS_PER_PLAYER.min(generic_deck.len()))
                .collect::<Vec<_>>();

            if let Some(mercenary_id) = player_mercenaries.get(player_id)
                && let Some(card_ids) = deck_definition.mercenary_card_ids.get(mercenary_id)
            {
                let mercenary_cards =
                    registry.weighted_power_card_definitions_from_ids(card_ids)?;
                let mut mercenary_deck = (0..MERCENARY_POWER_CARDS_PER_PLAYER)
                    .map(|idx| mercenary_cards[idx % mercenary_cards.len()].to_card())
                    .collect::<Vec<_>>();
                shuffle_power_cards(
                    &mut mercenary_deck,
                    seed,
                    sequence
                        .wrapping_mul(31)
                        .wrapping_add(player_idx as i64)
                        .wrapping_add(1),
                );
                cards.extend(mercenary_deck);
            }

            decks.insert(player_id.clone(), cards);
        }

        Ok(decks)
    }

    fn initial_mana(
        players: &[PlayerId],
        player_mercenaries: &HashMap<PlayerId, MercenaryId>,
        registry: &PowerCardRegistry,
    ) -> Result<IndexMap<PlayerId, PlayerMana>, PowerCardDefinitionError> {
        players
            .iter()
            .map(|player_id| {
                let mana = match player_mercenaries.get(player_id) {
                    Some(mercenary_id) => {
                        let definition =
                            registry.mercenary_definition(mercenary_id).ok_or_else(|| {
                                PowerCardDefinitionError::MissingMercenaryDefinition {
                                    mercenary_id: mercenary_id.to_string(),
                                }
                            })?;

                        PlayerMana::with_max(definition.initial_mana)
                    }
                    None => PlayerMana::initial(),
                };

                Ok((player_id.clone(), mana))
            })
            .collect()
    }

    fn next_set_mana(&self, players: &[PlayerId]) -> IndexMap<PlayerId, PlayerMana> {
        players
            .iter()
            .map(|player_id| {
                let mana = self
                    .mana
                    .get(player_id)
                    .cloned()
                    .unwrap_or_else(PlayerMana::initial);

                (player_id.clone(), mana)
            })
            .collect()
    }

    fn initial_lifes(
        &self,
        players: &[PlayerId],
        life_multiplier: f64,
    ) -> Result<HashMap<PlayerId, usize>, PowerCardDefinitionError> {
        players
            .iter()
            .map(|player_id| {
                let base_lifes = match self.player_mercenaries.get(player_id) {
                    Some(mercenary_id) => {
                        self.registry
                            .mercenary_definition(mercenary_id)
                            .ok_or_else(|| PowerCardDefinitionError::MissingMercenaryDefinition {
                                mercenary_id: mercenary_id.to_string(),
                            })?
                            .base_life
                    }
                    None => DEFAULT_INITIAL_LIFES,
                };
                let lifes = scaled_life_total(base_lifes, life_multiplier);

                Ok((player_id.clone(), lifes))
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

fn scaled_life_total(base_lifes: usize, life_multiplier: f64) -> usize {
    ((base_lifes as f64) * life_multiplier).round().max(1.0) as usize
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
    use std::sync::Arc;

    use crate::models::id::PlayerId;

    use super::*;

    const HEAL_10_SCRIPT: &str = r#"
return {
    type = PowerCardType.Instant,
    mana_cost = 2,
    quantity = 1,
    effect = function(game, card)
        game.add_lives(card.owner_id, 10)
    end,
}
"#;

    const STRIKE_10_SCRIPT: &str = r#"
return {
    type = PowerCardType.Targetable,
    mana_cost = 3,
    quantity = 1,
    effect = function(game, card)
        game.add_lives(card.target_player_id, -10)
    end,
}
"#;

    const NOOP_POWER_SCRIPT: &str = r#"
return {
    type = PowerCardType.Instant,
    mana_cost = 0,
    quantity = 1,
    effect = function(game, card)
    end,
}
"#;

    const BID_HEAL_PASSIVE_SCRIPT: &str = r#"
return {
    base_life = 50,
    initial_mana = 2,
    on_bid_placed = function(game, event, mercenary)
        if event.player_id == mercenary.owner_id then
            game.add_lives(mercenary.owner_id, 1)
        end
    end,
}
"#;

    const MATCH_STARTED_HEAL_PASSIVE_SCRIPT: &str = r#"
return {
    base_life = 50,
    initial_mana = 2,
    on_match_started = function(game, event, mercenary)
        game.add_lives(mercenary.owner_id, 2)
    end,
}
"#;

    const ROUND_START_HEAL_PASSIVE_SCRIPT: &str = r#"
return {
    base_life = 50,
    initial_mana = 2,
    on_round_start = function(game, event, mercenary)
        game.add_lives(mercenary.owner_id, 1)
    end,
}
"#;

    const ROUND_ENDED_HEAL_PASSIVE_SCRIPT: &str = r#"
return {
    base_life = 50,
    initial_mana = 2,
    on_round_ended = function(game, event, mercenary)
        game.add_lives(mercenary.owner_id, 1)
    end,
}
"#;

    const SET_ENDED_HIGHEST_BIDDER_MANA_PASSIVE_SCRIPT: &str = r#"
return {
    base_life = 50,
    initial_mana = 2,
    on_set_ended = function(game, event, mercenary)
        local highest_bid = -1
        local highest_bidder = nil
        local tie = false

        for player_id, bid in pairs(event.bids) do
            if bid > highest_bid then
                highest_bid = bid
                highest_bidder = player_id
                tie = false
            elseif bid == highest_bid then
                tie = true
            end
        end

        if not tie and highest_bidder == mercenary.owner_id then
            game:add_mana(mercenary.owner_id, 3)
        end
    end,
}
"#;

    fn test_registry() -> PowerCardRegistry {
        registry_with_power_card_definitions(test_power_card_definitions())
    }

    fn registry_with_power_card_definitions(
        definitions: Vec<PowerCardDefinitionInput>,
    ) -> PowerCardRegistry {
        let mut registry = PowerCardRegistry::default();
        registry
            .replace_power_card_definitions(test_deck_id(), definitions)
            .expect("valid test power card definitions");
        registry
    }

    fn test_power_card_definitions() -> Vec<PowerCardDefinitionInput> {
        vec![
            PowerCardDefinitionInput {
                id: card_id("heal_10"),
                name: "Heal 10".to_string(),
                description: "Restore 10 lives to yourself.".to_string(),
                mana_cost: 2,
                quantity: 1,
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
                quantity: 1,
                card_type: PowerCardType::Targetable,
                image_url: None,
                script: STRIKE_10_SCRIPT.to_string(),
                source: "test/strike_10.lua".to_string(),
            },
        ]
    }

    fn partitioned_power_card_definitions() -> Vec<PowerCardDefinitionInput> {
        (0..10)
            .map(|idx| partitioned_card_definition(format!("generic_{idx}")))
            .chain((0..5).map(|idx| partitioned_card_definition(format!("artemis_{idx}"))))
            .collect()
    }

    fn partitioned_card_definition(id: String) -> PowerCardDefinitionInput {
        PowerCardDefinitionInput {
            id: card_id(&id),
            name: id.clone(),
            description: "Test card".to_string(),
            mana_cost: 0,
            quantity: 1,
            card_type: PowerCardType::Instant,
            image_url: None,
            script: NOOP_POWER_SCRIPT.to_string(),
            source: format!("test/{id}.lua"),
        }
    }

    fn partitioned_power_deck_registry() -> PowerCardRegistry {
        let mut registry = PowerCardRegistry::default();
        registry
            .replace_power_card_registry(
                partitioned_power_card_definitions(),
                vec![PowerDeckDefinitionInput {
                    id: test_deck_id(),
                    generic_card_ids: (0..10)
                        .map(|idx| card_id(&format!("generic_{idx}")))
                        .collect(),
                    mercenary_card_ids: HashMap::from([(
                        mercenary_id("artemis"),
                        (0..5)
                            .map(|idx| card_id(&format!("artemis_{idx}")))
                            .collect(),
                    )]),
                }],
            )
            .expect("valid partitioned deck");
        registry
            .replace_mercenary_definitions(vec![MercenaryDefinitionInput {
                id: mercenary_id("artemis"),
                name: "Artemis".to_string(),
                base_life: 50,
                initial_mana: 2,
                passive_script: r#"
                    return {
                        base_life = 50,
                        initial_mana = 2,
                    }
                "#
                .to_string(),
                passive_source: "test/artemis_passive.lua".to_string(),
            }])
            .expect("valid partitioned mercenary");

        registry
    }

    fn draw_power_card_registry() -> (PowerCardRegistry, CardId) {
        let draw_id = card_id("draw_eight");
        let mut definitions = vec![PowerCardDefinitionInput {
            id: draw_id.clone(),
            name: "Draw Eight".to_string(),
            description: "Draws eight power cards.".to_string(),
            mana_cost: 0,
            quantity: 1,
            card_type: PowerCardType::Instant,
            image_url: None,
            script: r#"
                return {
                    type = PowerCardType.Instant,
                    mana_cost = 0,
                    quantity = 1,
                    effect = function(game, card)
                        game:draw_power_cards(card.owner_id, 8)
                    end,
                }
            "#
            .to_string(),
            source: "test/draw_eight.lua".to_string(),
        }];

        definitions.extend((0..24).map(|idx| PowerCardDefinitionInput {
            id: card_id(&format!("drawn_{idx}")),
            name: format!("Drawn {idx}"),
            description: "Can be drawn.".to_string(),
            mana_cost: 0,
            quantity: 1,
            card_type: PowerCardType::Instant,
            image_url: None,
            script: NOOP_POWER_SCRIPT.to_string(),
            source: format!("test/drawn_{idx}.lua"),
        }));

        let registry = registry_with_power_card_definitions(definitions);

        (registry, draw_id)
    }

    fn test_registry_with_mercenary_passives() -> PowerCardRegistry {
        test_registry_with_mercenary_passive(BID_HEAL_PASSIVE_SCRIPT)
    }

    fn test_registry_with_mercenary_passive(script: &str) -> PowerCardRegistry {
        let mut registry = test_registry();
        registry
            .replace_mercenary_definitions(vec![MercenaryDefinitionInput {
                id: mercenary_id("artemis"),
                name: "Artemis".to_string(),
                base_life: 50,
                initial_mana: 2,
                passive_script: script.to_string(),
                passive_source: "test/artemis_passive.lua".to_string(),
            }])
            .expect("valid mercenary passive");
        registry
    }

    fn new_test_game(players: &[PlayerId]) -> Game {
        Game::new_with_seed(players, test_settings(), 42, test_registry()).unwrap()
    }

    fn card_id(value: &str) -> CardId {
        CardId(Arc::from(value))
    }

    fn mercenary_id(value: &str) -> MercenaryId {
        MercenaryId(Arc::from(value))
    }

    fn test_deck_id() -> DeckId {
        DeckId(Arc::from("test_deck"))
    }

    fn test_settings() -> GameSettings {
        GameSettings {
            life_multiplier: 1.0,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::new(),
        }
    }

    fn test_players() -> [PlayerId; 2] {
        [PlayerId(Arc::from("P1")), PlayerId(Arc::from("P2"))]
    }

    fn enter_power_phase(game: &mut Game) {
        while matches!(game.stage, PowerGameStage::Bidding) {
            let player_id = game.current_player().expect("expected bidding player");
            let bid = game
                .get_possible_bids()
                .into_iter()
                .next()
                .expect("expected at least one bid");
            let event = game.validate_bid(&player_id, bid).unwrap();

            game.apply_match_event(event);
        }
    }

    fn finish_power_phase(game: &mut Game) {
        while matches!(game.stage, PowerGameStage::Power { .. }) {
            let player_id = game.current_player().expect("expected power player");
            let event = game.validate_skip_power_phase(&player_id).unwrap();

            game.apply_match_event(event);
        }
    }

    fn set_power_phase(game: &mut Game, players: &[PlayerId]) {
        game.stage = PowerGameStage::Power {
            phase: PowerPhase::First,
            pending_players: players.to_vec(),
        };
    }

    fn drawn_power_card_ids(
        registry: &PowerCardRegistry,
        players: &[PlayerId],
        player_id: &PlayerId,
        draw_id: &CardId,
        draw_seed: i64,
    ) -> Vec<CardId> {
        let mut game =
            Game::new_with_seeds(players, test_settings(), 42, draw_seed, registry.clone())
                .unwrap();
        set_power_phase(&mut game, players);
        game.power_decks.insert(
            player_id.clone(),
            vec![
                registry
                    .power_card_definition(&test_deck_id(), draw_id)
                    .unwrap()
                    .unwrap()
                    .to_card(),
            ],
        );
        let event = game.validate_power_card(player_id, draw_id, None).unwrap();
        let AppliedGameChange::PowerCardPlayed(outcome) = game.apply_match_event(event) else {
            panic!("expected power card outcome");
        };

        outcome.power_decks[player_id]
            .iter()
            .map(|card| card.id.clone())
            .collect()
    }

    fn validate_current_turn(game: &Game) -> MatchEvent {
        let player_id = game.core.current_player().expect("expected turn player");
        let card = game.core.get_player_snapshots()[&player_id].deck[0];

        game.validate_turn(Turn { player_id, card }).unwrap()
    }

    fn prepare_second_set_end_power_phase(game: &mut Game, highest_bidder: &PlayerId) {
        enter_power_phase(game);
        finish_power_phase(game);
        while matches!(game.stage, PowerGameStage::Dealing) {
            let event = validate_current_turn(game);
            game.apply_match_event(event);
        }
        finish_power_phase(game);

        while matches!(game.stage, PowerGameStage::Bidding) {
            let player_id = game.current_player().expect("expected bidding player");
            let bid = if &player_id == highest_bidder { 1 } else { 0 };
            assert!(game.get_possible_bids().contains(&bid));
            game.apply_match_event(game.validate_bid(&player_id, bid).unwrap());
        }
        finish_power_phase(game);
        while matches!(game.stage, PowerGameStage::Dealing) {
            let event = validate_current_turn(game);
            game.apply_match_event(event);
        }

        assert!(matches!(
            game.stage,
            PowerGameStage::Power {
                phase: PowerPhase::Second,
                ..
            }
        ));
    }

    fn registry_power_card(registry: &PowerCardRegistry, id: &str) -> PowerCard {
        registry
            .power_card_definition(&test_deck_id(), &card_id(id))
            .unwrap()
            .unwrap()
            .to_card()
    }

    #[test]
    fn loads_power_cards_from_runtime_registry() {
        let registry = test_registry();
        let definitions = registry.power_card_definitions(&test_deck_id()).unwrap();

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
    fn empty_power_deck_starts_without_power_cards() {
        let mut registry = PowerCardRegistry::default();
        let deck_id = DeckId(Arc::from("empty_deck"));
        registry
            .replace_power_card_registry(
                Vec::new(),
                vec![PowerDeckDefinitionInput {
                    id: deck_id.clone(),
                    generic_card_ids: Vec::new(),
                    mercenary_card_ids: HashMap::new(),
                }],
            )
            .expect("valid empty deck definition");
        let players = test_players();
        let settings = GameSettings {
            life_multiplier: 1.0,
            power_deck_id: deck_id,
            player_mercenaries: HashMap::new(),
        };

        let game = Game::new_with_seed(&players, settings, 42, registry).unwrap();
        assert_eq!(game.power_decks.len(), players.len());
        assert!(game.power_decks.values().all(Vec::is_empty));
    }

    #[test]
    fn game_starts_with_initial_mana_pool() {
        let registry = test_registry();
        let players = test_players();

        let MatchEvent::GameStarted { initial_mana, .. } =
            Game::start_match_event_with_seed(&players, test_settings(), 42, &registry).unwrap()
        else {
            panic!("expected game started event");
        };

        assert_eq!(initial_mana.len(), players.len());
        for player in players {
            assert_eq!(
                initial_mana.get(&player),
                Some(&PlayerMana {
                    current: INITIAL_MANA_POOL,
                    max: INITIAL_MANA_POOL,
                })
            );
        }
    }

    #[test]
    fn mercenary_defines_base_life_and_initial_mana() {
        let registry = test_registry_with_mercenary_passive(
            r#"
                return {
                    base_life = 123,
                    initial_mana = 7,
                }
            "#,
        );
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            life_multiplier: 1.0,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::from([
                (player1.clone(), mercenary_id("artemis")),
                (player2.clone(), mercenary_id("artemis")),
            ]),
        };
        let game = Game::new_with_seed(&players, settings, 42, registry).unwrap();

        assert_eq!(game.core.get_lifes().get(&player1), Some(&123));
        assert_eq!(game.core.get_lifes().get(&player2), Some(&123));
        assert_eq!(
            game.mana.get(&player1),
            Some(&PlayerMana { current: 7, max: 7 })
        );
        assert_eq!(
            game.mana.get(&player2),
            Some(&PlayerMana { current: 7, max: 7 })
        );
    }

    #[test]
    fn life_multiplier_scales_mercenary_base_life() {
        let registry = test_registry_with_mercenary_passive(
            r#"
                return {
                    base_life = 50,
                    initial_mana = 2,
                }
            "#,
        );
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            life_multiplier: 0.2,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::from([
                (player1.clone(), mercenary_id("artemis")),
                (player2.clone(), mercenary_id("artemis")),
            ]),
        };
        let game = Game::new_with_seed(&players, settings, 42, registry).unwrap();

        assert_eq!(game.core.get_lifes().get(&player1), Some(&10));
        assert_eq!(game.core.get_lifes().get(&player2), Some(&10));
    }

    #[test]
    fn game_started_event_persists_draw_seed() {
        let registry = test_registry();
        let players = test_players();

        let MatchEvent::GameStarted { draw_seed, .. } =
            Game::start_match_event_with_seeds(&players, test_settings(), 42, 77, &registry)
                .unwrap()
        else {
            panic!("expected game started event");
        };

        assert_eq!(draw_seed, 77);
    }

    #[test]
    fn card_quantity_controls_draw_pool_distribution() {
        let heavy_id = card_id("heavy");
        let light_id = card_id("light");
        let registry = registry_with_power_card_definitions(vec![
            PowerCardDefinitionInput {
                id: heavy_id.clone(),
                name: "Heavy".to_string(),
                description: "Appears more often.".to_string(),
                mana_cost: 1,
                quantity: 3,
                card_type: PowerCardType::Instant,
                image_url: None,
                script: r#"
                    return {
                        type = PowerCardType.Instant,
                        mana_cost = 1,
                        quantity = 3,
                        effect = function(game, card)
                        end,
                    }
                "#
                .to_string(),
                source: "test/heavy.lua".to_string(),
            },
            PowerCardDefinitionInput {
                id: light_id.clone(),
                name: "Light".to_string(),
                description: "Appears less often.".to_string(),
                mana_cost: 1,
                quantity: 1,
                card_type: PowerCardType::Instant,
                image_url: None,
                script: r#"
                    return {
                        type = PowerCardType.Instant,
                        mana_cost = 1,
                        quantity = 1,
                        effect = function(game, card)
                        end,
                    }
                "#
                .to_string(),
                source: "test/light.lua".to_string(),
            },
        ]);
        let player = PlayerId(Arc::from("P1"));
        let drawn = registry
            .draw_power_cards_for_player(&test_deck_id(), &HashMap::new(), &player, 4, (42, 0), 0)
            .unwrap();
        let heavy_count = drawn.iter().filter(|card| card.id == heavy_id).count();
        let light_count = drawn.iter().filter(|card| card.id == light_id).count();

        assert_eq!(drawn.len(), 4);
        assert_eq!(heavy_count, 3);
        assert_eq!(light_count, 1);
    }

    #[test]
    fn draw_power_cards_uses_persisted_draw_seed() {
        let (registry, draw_id) = draw_power_card_registry();
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2];

        let first = drawn_power_card_ids(&registry, &players, &player1, &draw_id, 77);
        let repeated = drawn_power_card_ids(&registry, &players, &player1, &draw_id, 77);
        let changed = drawn_power_card_ids(&registry, &players, &player1, &draw_id, 123_456_789);

        assert_eq!(first, repeated);
        assert_ne!(first, changed);
    }

    #[test]
    fn partitioned_deck_deals_generic_and_mercenary_cards() {
        let registry = partitioned_power_deck_registry();
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            life_multiplier: 1.0,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::from([
                (player1.clone(), mercenary_id("artemis")),
                (player2.clone(), mercenary_id("artemis")),
            ]),
        };

        let game = Game::new_with_seed(&players, settings, 42, registry).unwrap();

        for player in players {
            let cards = game.power_decks.get(&player).unwrap();

            assert_eq!(cards.len(), 2);
            assert!(
                cards
                    .iter()
                    .any(|card| card.id.as_str().starts_with("generic_"))
            );
            assert!(
                cards
                    .iter()
                    .any(|card| card.id.as_str().starts_with("artemis_"))
            );
        }
    }

    #[test]
    fn partitioned_deck_without_generic_cards_deals_mercenary_cards() {
        let mut registry = PowerCardRegistry::default();
        registry
            .replace_power_card_registry(
                test_power_card_definitions(),
                vec![PowerDeckDefinitionInput {
                    id: test_deck_id(),
                    generic_card_ids: Vec::new(),
                    mercenary_card_ids: HashMap::from([(
                        mercenary_id("gambler"),
                        vec![card_id("heal_10"), card_id("strike_10")],
                    )]),
                }],
            )
            .expect("valid Gambler-only deck");
        registry
            .replace_mercenary_definitions(vec![MercenaryDefinitionInput {
                id: mercenary_id("gambler"),
                name: "Gambler".to_string(),
                base_life: 50,
                initial_mana: 2,
                passive_script: r#"
                    return {
                        base_life = 50,
                        initial_mana = 2,
                    }
                "#
                .to_string(),
                passive_source: "test/gambler_passive.lua".to_string(),
            }])
            .expect("valid Gambler mercenary");
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            life_multiplier: 1.0,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::from([
                (player1.clone(), mercenary_id("gambler")),
                (player2.clone(), mercenary_id("gambler")),
            ]),
        };

        let game = Game::new_with_seed(&players, settings, 42, registry).unwrap();

        for player in players {
            let cards = game.power_decks.get(&player).unwrap();

            assert_eq!(cards.len(), 1);
            assert!(matches!(cards[0].id.as_str(), "heal_10" | "strike_10"));
        }
    }

    #[test]
    fn mercenary_passive_effect_is_persisted_on_bid_event() {
        let registry = test_registry_with_mercenary_passives();
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            life_multiplier: 1.0,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::from([(player1.clone(), mercenary_id("artemis"))]),
        };
        let mut game = Game::new_with_seed(&players, settings, 42, registry).unwrap();

        let event = game.validate_bid(&player1, 1).unwrap();
        let MatchEvent::BidPlaced {
            passive_effects, ..
        } = &event
        else {
            panic!("expected bid event");
        };

        assert_eq!(passive_effects.lifes.get(&player1), Some(&51));

        game.apply_match_event(event);

        assert_eq!(game.core.get_lifes().get(&player1), Some(&51));
    }

    #[test]
    fn mercenary_passive_effect_is_persisted_on_match_started_event() {
        let registry = test_registry_with_mercenary_passive(MATCH_STARTED_HEAL_PASSIVE_SCRIPT);
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            life_multiplier: 1.0,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::from([(player1.clone(), mercenary_id("artemis"))]),
        };

        let event =
            Game::start_match_event_with_seed(&players, settings.clone(), 42, &registry).unwrap();
        let MatchEvent::GameStarted {
            passive_effects, ..
        } = &event
        else {
            panic!("expected game started event");
        };

        assert_eq!(passive_effects.lifes.get(&player1), Some(&52));

        let game = Game::new_with_seed(&players, settings, 42, registry).unwrap();

        assert_eq!(game.core.get_lifes().get(&player1), Some(&52));
    }

    #[test]
    fn mercenary_passive_effect_is_persisted_on_round_start_event() {
        let registry = test_registry_with_mercenary_passive(ROUND_START_HEAL_PASSIVE_SCRIPT);
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            life_multiplier: 1.0,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::from([(player1.clone(), mercenary_id("artemis"))]),
        };
        let mut game = Game::new_with_seed(&players, settings, 42, registry).unwrap();

        enter_power_phase(&mut game);
        let first_power_player = game.current_player().expect("expected power player");
        game.apply_match_event(game.validate_skip_power_phase(&first_power_player).unwrap());
        let before = game.core.get_lifes()[&player1];
        let player_id = game.current_player().expect("expected final power player");
        let event = game.validate_skip_power_phase(&player_id).unwrap();
        let MatchEvent::PowerPhaseSkipped {
            effects: passive_effects,
            ..
        } = &event
        else {
            panic!("expected power phase skip event");
        };

        assert_eq!(passive_effects.lifes.get(&player1), Some(&(before + 1)));

        game.apply_match_event(event);

        assert_eq!(game.core.get_lifes().get(&player1), Some(&(before + 1)));
    }

    #[test]
    fn mercenary_passive_effect_is_persisted_on_round_ended_event() {
        let registry = test_registry_with_mercenary_passive(ROUND_ENDED_HEAL_PASSIVE_SCRIPT);
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            life_multiplier: 1.0,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::from([(player1.clone(), mercenary_id("artemis"))]),
        };
        let mut game = Game::new_with_seed(&players, settings, 42, registry).unwrap();

        enter_power_phase(&mut game);
        finish_power_phase(&mut game);
        let event = validate_current_turn(&game);
        game.apply_match_event(event);
        let event = validate_current_turn(&game);
        game.apply_match_event(event);

        finish_power_phase(&mut game);
        enter_power_phase(&mut game);
        finish_power_phase(&mut game);
        let event = validate_current_turn(&game);
        game.apply_match_event(event);
        let before = game.core.get_lifes()[&player1];
        let event = validate_current_turn(&game);
        let MatchEvent::TurnPlayed {
            passive_effects, ..
        } = &event
        else {
            panic!("expected turn event");
        };

        assert_eq!(passive_effects.lifes.get(&player1), Some(&(before + 1)));

        game.apply_match_event(event);

        assert_eq!(game.core.get_lifes().get(&player1), Some(&(before + 1)));
    }

    #[test]
    fn set_ended_passive_uses_completed_bids_on_skipped_power_phase() {
        let registry =
            test_registry_with_mercenary_passive(SET_ENDED_HIGHEST_BIDDER_MANA_PASSIVE_SCRIPT);
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            life_multiplier: 1.0,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::from([(player1.clone(), mercenary_id("artemis"))]),
        };
        let mut game = Game::new_with_seed(&players, settings, 42, registry).unwrap();
        game.mana.insert(
            player1.clone(),
            PlayerMana {
                current: 0,
                max: 100,
            },
        );

        prepare_second_set_end_power_phase(&mut game, &player1);
        loop {
            let should_skip = match &game.stage {
                PowerGameStage::Power {
                    pending_players, ..
                } => pending_players.len() > 1,
                _ => false,
            };
            if !should_skip {
                break;
            }
            let player_id = game.current_player().expect("expected power player");
            game.apply_match_event(game.validate_skip_power_phase(&player_id).unwrap());
        }

        let before = game.mana[&player1].current;
        let player_id = game.current_player().expect("expected final power player");
        let event = game.validate_skip_power_phase(&player_id).unwrap();
        let MatchEvent::PowerPhaseSkipped {
            set_ended_effects, ..
        } = &event
        else {
            panic!("expected power phase skip event");
        };

        assert_eq!(set_ended_effects.mana[&player1].current, before + 3);
        game.apply_match_event(event);
    }

    #[test]
    fn set_ended_passive_uses_completed_bids_on_power_card_resolution() {
        let registry =
            test_registry_with_mercenary_passive(SET_ENDED_HIGHEST_BIDDER_MANA_PASSIVE_SCRIPT);
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            life_multiplier: 1.0,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::from([(player1.clone(), mercenary_id("artemis"))]),
        };
        let mut game = Game::new_with_seed(&players, settings, 42, registry).unwrap();
        game.mana.insert(
            player1.clone(),
            PlayerMana {
                current: 0,
                max: 100,
            },
        );

        prepare_second_set_end_power_phase(&mut game, &player1);
        loop {
            let should_skip = match &game.stage {
                PowerGameStage::Power {
                    pending_players, ..
                } => pending_players.len() > 1,
                _ => false,
            };
            if !should_skip {
                break;
            }
            let player_id = game.current_player().expect("expected power player");
            game.apply_match_event(game.validate_skip_power_phase(&player_id).unwrap());
        }

        let before = game.mana[&player1].current;
        let player_id = game.current_player().expect("expected final power player");
        let card = registry_power_card(&game.registry, "heal_10");
        game.power_decks.insert(player_id.clone(), vec![card]);
        let event = game
            .validate_power_card(&player_id, &card_id("heal_10"), None)
            .unwrap();
        let MatchEvent::PowerCardPlayed {
            set_ended_effects, ..
        } = &event
        else {
            panic!("expected power card event");
        };

        assert_eq!(set_ended_effects.mana[&player1].current, before + 3);
        game.apply_match_event(event);
    }

    #[test]
    fn bid_mismatch_costs_ten_lives() {
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);

        game.apply_match_event(game.validate_bid(&player1, 1).unwrap());
        game.apply_match_event(game.validate_bid(&player2, 1).unwrap());
        finish_power_phase(&mut game);

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
        let mut game = new_test_game(&players);
        set_power_phase(&mut game, &players);

        game.power_decks.insert(
            player1.clone(),
            vec![
                game.registry
                    .power_card_definition(&test_deck_id(), &card_id("strike_10"))
                    .unwrap()
                    .unwrap()
                    .to_card(),
            ],
        );
        game.mana
            .insert(player1.clone(), PlayerMana { current: 3, max: 3 });

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
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);
        set_power_phase(&mut game, &players);

        game.power_decks.insert(
            player1.clone(),
            vec![
                game.registry
                    .power_card_definition(&test_deck_id(), &card_id("strike_10"))
                    .unwrap()
                    .unwrap()
                    .to_card(),
            ],
        );
        game.mana
            .insert(player1.clone(), PlayerMana { current: 5, max: 5 });

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
    fn power_card_script_can_reduce_mana_cost_before_deduction() {
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2];
        let discounted_id = card_id("discounted");

        let registry = registry_with_power_card_definitions(vec![PowerCardDefinitionInput {
            id: discounted_id.clone(),
            name: "Discounted".to_string(),
            description: "Costs less while resolving.".to_string(),
            mana_cost: 3,
            quantity: 1,
            card_type: PowerCardType::Instant,
            image_url: None,
            script: r#"
                    return {
                        type = PowerCardType.Instant,
                        mana_cost = 3,
                        quantity = 1,
                        effect = function(game, card)
                            card:add_mana_cost(-2)
                        end,
                    }
                "#
            .to_string(),
            source: "test/discounted.lua".to_string(),
        }]);
        let mut game =
            Game::new_with_seed(&players, test_settings(), 42, registry.clone()).unwrap();
        set_power_phase(&mut game, &players);
        game.power_decks.insert(
            player1.clone(),
            vec![
                registry
                    .power_card_definition(&test_deck_id(), &discounted_id)
                    .unwrap()
                    .unwrap()
                    .to_card(),
            ],
        );
        game.mana
            .insert(player1.clone(), PlayerMana { current: 1, max: 3 });

        let event = game
            .validate_power_card(&player1, &discounted_id, None)
            .unwrap();
        let AppliedGameChange::PowerCardPlayed(outcome) = game.apply_match_event(event) else {
            panic!("expected power card outcome");
        };

        assert_eq!(outcome.card.mana_cost, 1);
        assert_eq!(game.mana[&player1].current, 0);
    }

    #[test]
    fn negative_power_card_cost_regenerates_mana() {
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2];
        let refund_id = card_id("refund");

        let registry = registry_with_power_card_definitions(vec![PowerCardDefinitionInput {
            id: refund_id.clone(),
            name: "Refund".to_string(),
            description: "Regenerates mana while resolving.".to_string(),
            mana_cost: 2,
            quantity: 1,
            card_type: PowerCardType::Instant,
            image_url: None,
            script: r#"
                    return {
                        type = PowerCardType.Instant,
                        mana_cost = 2,
                        quantity = 1,
                        effect = function(game, card)
                            card:add_mana_cost(-4)
                        end,
                    }
                "#
            .to_string(),
            source: "test/refund.lua".to_string(),
        }]);
        let mut game =
            Game::new_with_seed(&players, test_settings(), 42, registry.clone()).unwrap();
        set_power_phase(&mut game, &players);
        game.power_decks.insert(
            player1.clone(),
            vec![
                registry
                    .power_card_definition(&test_deck_id(), &refund_id)
                    .unwrap()
                    .unwrap()
                    .to_card(),
            ],
        );
        game.mana
            .insert(player1.clone(), PlayerMana { current: 1, max: 5 });

        let event = game
            .validate_power_card(&player1, &refund_id, None)
            .unwrap();
        let AppliedGameChange::PowerCardPlayed(outcome) = game.apply_match_event(event) else {
            panic!("expected power card outcome");
        };

        assert_eq!(outcome.mana.get(&player1).unwrap().current, 3);
        assert_eq!(game.mana[&player1].current, 3);
    }

    #[test]
    fn power_card_script_can_increase_mana_cost_before_deduction() {
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2];
        let surcharge_id = card_id("surcharge");

        let registry = registry_with_power_card_definitions(vec![PowerCardDefinitionInput {
            id: surcharge_id.clone(),
            name: "Surcharge".to_string(),
            description: "Costs more while resolving.".to_string(),
            mana_cost: 2,
            quantity: 1,
            card_type: PowerCardType::Instant,
            image_url: None,
            script: r#"
                    return {
                        type = PowerCardType.Instant,
                        mana_cost = 2,
                        quantity = 1,
                        effect = function(game, card)
                            card:add_mana_cost(2)
                        end,
                    }
                "#
            .to_string(),
            source: "test/surcharge.lua".to_string(),
        }]);
        let mut game =
            Game::new_with_seed(&players, test_settings(), 42, registry.clone()).unwrap();
        set_power_phase(&mut game, &players);
        game.power_decks.insert(
            player1.clone(),
            vec![
                registry
                    .power_card_definition(&test_deck_id(), &surcharge_id)
                    .unwrap()
                    .unwrap()
                    .to_card(),
            ],
        );
        game.mana
            .insert(player1.clone(), PlayerMana { current: 4, max: 4 });

        let event = game
            .validate_power_card(&player1, &surcharge_id, None)
            .unwrap();
        let AppliedGameChange::PowerCardPlayed(outcome) = game.apply_match_event(event) else {
            panic!("expected power card outcome");
        };

        assert_eq!(outcome.card.mana_cost, 4);
        assert_eq!(game.mana[&player1].current, 0);
    }

    #[test]
    fn power_card_script_cannot_raise_mana_cost_past_available_mana() {
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2];
        let expensive_id = card_id("too_expensive");

        let registry = registry_with_power_card_definitions(vec![PowerCardDefinitionInput {
            id: expensive_id.clone(),
            name: "Too Expensive".to_string(),
            description: "Costs too much while resolving.".to_string(),
            mana_cost: 2,
            quantity: 1,
            card_type: PowerCardType::Instant,
            image_url: None,
            script: r#"
                    return {
                        type = PowerCardType.Instant,
                        mana_cost = 2,
                        quantity = 1,
                        effect = function(game, card)
                            card:add_mana_cost(3)
                        end,
                    }
                "#
            .to_string(),
            source: "test/too_expensive.lua".to_string(),
        }]);
        let mut game =
            Game::new_with_seed(&players, test_settings(), 42, registry.clone()).unwrap();
        set_power_phase(&mut game, &players);
        game.power_decks.insert(
            player1.clone(),
            vec![
                registry
                    .power_card_definition(&test_deck_id(), &expensive_id)
                    .unwrap()
                    .unwrap()
                    .to_card(),
            ],
        );
        game.mana
            .insert(player1.clone(), PlayerMana { current: 4, max: 4 });

        assert!(matches!(
            game.validate_power_card(&player1, &expensive_id, None),
            Err(PowerCardError::NotEnoughMana)
        ));
        assert_eq!(game.mana[&player1].current, 4);
    }

    #[test]
    fn power_card_script_can_draw_power_cards() {
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2];
        let draw_id = card_id("draw_two");

        let registry = registry_with_power_card_definitions(vec![
            PowerCardDefinitionInput {
                id: draw_id.clone(),
                name: "Draw Two".to_string(),
                description: "Draws two power cards.".to_string(),
                mana_cost: 0,
                quantity: 1,
                card_type: PowerCardType::Instant,
                image_url: None,
                script: r#"
                        return {
                            type = PowerCardType.Instant,
                            mana_cost = 0,
                            quantity = 1,
                            effect = function(game, card)
                                game:draw_power_cards(card.owner_id, 2)
                            end,
                        }
                    "#
                .to_string(),
                source: "test/draw_two.lua".to_string(),
            },
            PowerCardDefinitionInput {
                id: card_id("drawn_one"),
                name: "Drawn One".to_string(),
                description: "Can be drawn.".to_string(),
                mana_cost: 0,
                quantity: 1,
                card_type: PowerCardType::Instant,
                image_url: None,
                script: NOOP_POWER_SCRIPT.to_string(),
                source: "test/drawn_one.lua".to_string(),
            },
            PowerCardDefinitionInput {
                id: card_id("drawn_two"),
                name: "Drawn Two".to_string(),
                description: "Can be drawn.".to_string(),
                mana_cost: 0,
                quantity: 1,
                card_type: PowerCardType::Instant,
                image_url: None,
                script: NOOP_POWER_SCRIPT.to_string(),
                source: "test/drawn_two.lua".to_string(),
            },
        ]);
        let mut game =
            Game::new_with_seed(&players, test_settings(), 42, registry.clone()).unwrap();
        set_power_phase(&mut game, &players);
        game.power_decks.insert(
            player1.clone(),
            vec![
                registry
                    .power_card_definition(&test_deck_id(), &draw_id)
                    .unwrap()
                    .unwrap()
                    .to_card(),
            ],
        );

        let event = game.validate_power_card(&player1, &draw_id, None).unwrap();
        let AppliedGameChange::PowerCardPlayed(outcome) = game.apply_match_event(event) else {
            panic!("expected power card outcome");
        };

        assert_eq!(outcome.power_decks.get(&player1).unwrap().len(), 2);
        assert_eq!(game.power_decks[&player1].len(), 2);
    }

    #[test]
    fn power_card_requires_enough_mana() {
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);
        let card = registry_power_card(&game.registry, "strike_10");

        set_power_phase(&mut game, &players);
        game.power_decks.insert(player1.clone(), vec![card]);
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
        let custom_deck_id = DeckId(Arc::from("custom_deck"));

        let mut registry = PowerCardRegistry::default();
        registry
            .replace_power_card_registry(
                test_power_card_definitions(),
                vec![
                    PowerDeckDefinitionInput {
                        id: test_deck_id(),
                        generic_card_ids: vec![card_id("heal_10")],
                        mercenary_card_ids: HashMap::new(),
                    },
                    PowerDeckDefinitionInput {
                        id: custom_deck_id.clone(),
                        generic_card_ids: vec![card_id("strike_10")],
                        mercenary_card_ids: HashMap::new(),
                    },
                ],
            )
            .expect("valid custom deck definitions");

        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            life_multiplier: 1.0,
            power_deck_id: custom_deck_id,
            player_mercenaries: HashMap::new(),
        };
        let mut game = Game::new_with_seed(&players, settings, 42, registry).unwrap();
        set_power_phase(&mut game, &players);
        game.mana
            .insert(player1.clone(), PlayerMana { current: 3, max: 3 });

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
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);
        let card = registry_power_card(&game.registry, "strike_10");

        game.power_decks.insert(player1.clone(), vec![card]);

        assert!(matches!(
            game.validate_power_card(&player1, &card_id("strike_10"), Some(player2.clone())),
            Err(PowerCardError::PowerStageRequired)
        ));

        game.apply_match_event(game.validate_bid(&player1, 1).unwrap());
        game.apply_match_event(game.validate_bid(&player2, 1).unwrap());

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

        game.apply_match_event(game.validate_skip_power_phase(&player1).unwrap());
        game.apply_match_event(game.validate_skip_power_phase(&player2).unwrap());

        assert!(matches!(
            game.validate_power_card(&player1, &card_id("strike_10"), Some(player2)),
            Err(PowerCardError::PowerStageRequired)
        ));
    }

    #[test]
    fn applying_persisted_power_card_event_removes_card_and_can_end_game() {
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);
        let card = registry_power_card(&game.registry, "strike_10");

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
                    deck_reveals: Vec::new(),
                },
                set_ended_effects: PowerCardEffects::default(),
                set_started_effects: PowerCardEffects::default(),
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
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);

        game.apply_match_event(game.validate_bid(&player1, 1).unwrap());
        game.apply_match_event(game.validate_bid(&player2, 1).unwrap());
        finish_power_phase(&mut game);

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
        let MatchEvent::TurnPlayed { .. } = &event else {
            panic!("expected turn event at set end");
        };

        let AppliedGameChange::TurnPlayed {
            power_decks: Some(power_decks),
            ..
        } = game.apply_match_event(event)
        else {
            panic!("expected refreshed power decks");
        };

        assert_eq!(power_decks.len(), 2);
        assert_eq!(power_decks[&player1].len(), 1);
        assert_eq!(power_decks[&player2].len(), 1);
        assert_eq!(game.power_decks[&player1].len(), 1);
        assert_eq!(game.power_decks[&player2].len(), 1);
    }

    #[test]
    fn bidding_turn_regenerates_next_players_mana() {
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let mut game = new_test_game(&players);

        game.mana.insert(
            player2.clone(),
            PlayerMana {
                current: 1,
                max: INITIAL_MANA_POOL,
            },
        );

        let event = game.validate_bid(&player1, 1).unwrap();
        let MatchEvent::BidPlaced { mana, .. } = &event else {
            panic!("expected bid event");
        };

        assert_eq!(mana.get(&player2), Some(&PlayerMana { current: 2, max: 2 }));

        let AppliedGameChange::BidPlaced { mana, .. } = game.apply_match_event(event) else {
            panic!("expected bid change");
        };

        assert_eq!(
            mana.get(&player2),
            Some(&PlayerManaDto { current: 2, max: 2 })
        );
        assert_eq!(game.mana[&player2].current, 2);
    }

    #[test]
    fn next_set_increases_and_refills_mana_pool() {
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
        finish_power_phase(&mut game);

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
            Some(&PlayerManaDto { current: 1, max: 2 })
        );
        assert_eq!(
            mana.get(&player2),
            Some(&PlayerManaDto { current: 2, max: 2 })
        );
        assert_eq!(game.mana[&player1].current, 1);
        assert_eq!(game.mana[&player1].max, 2);
        assert_eq!(game.mana[&player2].current, 2);
        assert_eq!(game.mana[&player2].max, 2);
    }

    #[test]
    fn next_set_mana_pool_has_no_global_cap() {
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
                current: 4,
                max: 10
            })
        );
    }

    #[test]
    fn game_info_exposes_private_power_cards() {
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let players = [player1.clone(), player2.clone()];
        let game = new_test_game(&players);

        let info = game.get_game_info(&player1);

        assert_eq!(info.power_cards.unwrap().len(), 1);
    }
}
