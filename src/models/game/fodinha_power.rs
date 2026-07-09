use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{Arc, RwLock},
};

use indexmap::IndexMap;

use crate::{
    models::{
        BiddingError, Card, DealError, GameError, Turn,
        game::{
            BiddingState, DealState, DeckShuffle, NewSet, fodinha_classic,
            power_lua::{
                DrawPowerCardsFn, PassiveGameEvent, PassiveScriptInput, PowerScriptError,
                PowerScriptInput, PowerScriptOutput, ScriptManaState, ScriptPlayerState,
                ScriptPowerCardState,
            },
        },
        id::{CardId, DeckId, MercenaryId, PlayerId},
        util::DeterministicRng,
    },
    services::{GameInfoDto, GameStageDto, PlayerManaDto, PowerCardDto},
};

const LIFE_LOSS_PER_BID_DIFFERENCE: usize = 10;
const POWER_CARDS_PER_PLAYER: usize = 1;
const GENERIC_POWER_CARDS_PER_PLAYER: usize = 1;
const MERCENARY_POWER_CARDS_PER_PLAYER: usize = 1;
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
    stage: PowerGameStage,
    power_decks: IndexMap<PlayerId, Vec<PowerCard>>,
    mana: IndexMap<PlayerId, PlayerMana>,
    registry: PowerCardRegistry,
    power_deck_id: DeckId,
    player_mercenaries: HashMap<PlayerId, MercenaryId>,
    power_seed: i64,
    draw_seed: i64,
    next_power_shuffle_sequence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PowerGameStage {
    Bidding,
    Power {
        pending_players: Vec<PlayerId>,
    },
    Dealing,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GameSettings {
    pub lifes: usize,
    pub power_deck_id: DeckId,
    pub player_mercenaries: HashMap<PlayerId, MercenaryId>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
        set: NewSet,
        power_set: PowerSet,
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
        next_set: Option<NewSet>,
        next_power_set: Option<PowerSet>,
        passive_effects: PowerCardEffects,
        next_set_passive_effects: PowerCardEffects,
    },
    PowerCardPlayed {
        player_id: PlayerId,
        card: PowerCard,
        target_player_id: Option<PlayerId>,
        effects: PowerCardEffects,
    },
    PowerPhaseSkipped {
        player_id: PlayerId,
        effects: PowerCardEffects,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PowerCardEffects {
    pub lifes: HashMap<PlayerId, usize>,
    pub mana: HashMap<PlayerId, PlayerMana>,
    pub decks: HashMap<PlayerId, Vec<Card>>,
    pub power_decks: HashMap<PlayerId, Vec<PowerCard>>,
}

impl PowerCardEffects {
    fn merge(&mut self, other: Self) {
        self.lifes.extend(other.lifes);
        self.mana.extend(other.mana);
        self.decks.extend(other.decks);
        self.power_decks.extend(other.power_decks);
    }
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
        lifes: Option<HashMap<PlayerId, usize>>,
        power_decks: Option<IndexMap<PlayerId, Vec<PowerCardDto>>>,
        mana: Option<HashMap<PlayerId, PlayerManaDto>>,
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
    pub ended: bool,
}

#[derive(Debug, Clone)]
pub struct PowerPhaseSkipOutcome {
    pub player_id: PlayerId,
    pub lifes: HashMap<PlayerId, usize>,
    pub changed_lifes: HashMap<PlayerId, usize>,
    pub mana: HashMap<PlayerId, PlayerManaDto>,
    pub decks: HashMap<PlayerId, Vec<Card>>,
    pub power_decks: HashMap<PlayerId, Vec<PowerCardDto>>,
    pub ended: bool,
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
    pub generic_card_ids: Vec<CardId>,
    pub mercenary_card_ids: HashMap<MercenaryId, Vec<CardId>>,
}

#[derive(Debug, Clone)]
pub struct MercenaryDefinitionInput {
    pub id: MercenaryId,
    pub name: String,
    pub passive_script: String,
    pub passive_source: String,
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

#[derive(Debug, Clone)]
struct PowerDeckDefinition {
    card_ids: Vec<CardId>,
    generic_card_ids: Vec<CardId>,
    mercenary_card_ids: HashMap<MercenaryId, Vec<CardId>>,
}

impl PowerDeckDefinition {
    fn from_input(input: PowerDeckDefinitionInput) -> Self {
        Self {
            card_ids: input.card_ids,
            generic_card_ids: input.generic_card_ids,
            mercenary_card_ids: input.mercenary_card_ids,
        }
    }

    fn is_partitioned(&self) -> bool {
        !self.generic_card_ids.is_empty() || !self.mercenary_card_ids.is_empty()
    }

    fn all_card_ids(&self) -> Vec<CardId> {
        if !self.is_partitioned() {
            return self.card_ids.clone();
        }

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
    passive_script: String,
    passive_source: String,
}

impl MercenaryDefinition {
    fn from_input(input: MercenaryDefinitionInput) -> Result<Self, PowerCardDefinitionError> {
        super::power_lua::validate_mercenary_passive_script(
            &input.passive_script,
            &input.passive_source,
        )
        .map_err(|error| PowerCardDefinitionError::InvalidDefinition {
            path: input.passive_source.clone(),
            message: error.to_string(),
        })?;

        Ok(Self {
            id: input.id,
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
        let card_ids = definitions
            .iter()
            .map(|definition| definition.id.clone())
            .collect();

        self.replace_power_card_registry(
            definitions,
            vec![PowerDeckDefinitionInput {
                id: deck_id,
                card_ids,
                generic_card_ids: Vec::new(),
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
        let deck_card_ids = self.power_deck_definition(deck_id)?.all_card_ids();

        self.power_card_definitions_from_ids(&deck_card_ids)
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
            return self.power_card_definitions_from_ids(&deck_definition.card_ids);
        }

        let mut card_ids = deck_definition.generic_card_ids.clone();
        if let Some(mercenary_id) = player_mercenaries.get(player_id)
            && let Some(mercenary_card_ids) = deck_definition.mercenary_card_ids.get(mercenary_id)
        {
            card_ids.extend(mercenary_card_ids.iter().cloned());
        }

        self.power_card_definitions_from_ids(&card_ids)
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
                set,
                power_set,
                draw_seed,
                passive_effects,
            } => {
                let mut game =
                    Self::from_started(players, settings, set, power_set, draw_seed, registry)?;
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
            &settings.player_mercenaries,
            Self::initial_mana(players),
            registry,
        )?;
        let game = Self::from_started(
            players,
            settings.clone(),
            set.clone(),
            power_set.clone(),
            draw_seed,
            registry.clone(),
        )?;
        let mut preview = game.clone();
        let mut passive_effects = preview
            .passive_effects(PassiveGameEvent::MatchStarted)
            .unwrap_or_default();
        preview.apply_effects(&passive_effects);
        passive_effects.merge(
            preview
                .passive_effects(PassiveGameEvent::SetStarted)
                .unwrap_or_default(),
        );

        Ok(MatchEvent::GameStarted {
            settings,
            set,
            power_set,
            draw_seed,
            passive_effects,
        })
    }

    pub fn from_started(
        players: &[PlayerId],
        settings: GameSettings,
        set: NewSet,
        power_set: PowerSet,
        draw_seed: i64,
        registry: PowerCardRegistry,
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
            stage: PowerGameStage::Bidding,
            power_decks: power_set.decks,
            mana,
            registry,
            power_deck_id: settings.power_deck_id,
            player_mercenaries: settings.player_mercenaries,
            power_seed: power_set.shuffle.seed,
            draw_seed,
            next_power_shuffle_sequence: power_set.shuffle.sequence.wrapping_add(1),
        })
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
            .unwrap_or_default();

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
        let fodinha_classic::MatchEvent::TurnPlayed { turn, next_set } = event else {
            unreachable!("validate_turn only emits TurnPlayed")
        };
        let next_power_set = next_set.as_ref().map(|set| {
            let players: Vec<_> = set.decks.keys().cloned().collect();

            self.new_power_set_for_game(&players)
        });
        let mut passive_effects = self
            .passive_effects(PassiveGameEvent::TurnPlayed {
                player_id: turn.player_id.clone(),
                card: turn.card,
            })
            .unwrap_or_default();

        if next_set.is_some() {
            passive_effects.merge(
                self.passive_effects(PassiveGameEvent::SetEnded)
                    .unwrap_or_default(),
            );
        }

        let base_event = MatchEvent::TurnPlayed {
            turn: turn.clone(),
            next_set: next_set.clone(),
            next_power_set: next_power_set.clone(),
            passive_effects: passive_effects.clone(),
            next_set_passive_effects: PowerCardEffects::default(),
        };

        let mut preview = self.clone();
        let round_ended = matches!(
            preview.apply_match_event(base_event),
            AppliedGameChange::TurnPlayed {
                state: DealState {
                    outcome: fodinha_classic::GameOutcome::RoundEnded { .. },
                    ..
                },
                ..
            }
        );

        if round_ended {
            passive_effects.merge(
                preview
                    .passive_effects(PassiveGameEvent::RoundEnded)
                    .unwrap_or_default(),
            );
        }

        let next_set_passive_effects = if next_set.is_some() && !preview.is_finished() {
            preview
                .passive_effects(PassiveGameEvent::SetStarted)
                .unwrap_or_default()
        } else {
            PowerCardEffects::default()
        };

        Ok(MatchEvent::TurnPlayed {
            turn,
            next_set,
            next_power_set,
            passive_effects,
            next_set_passive_effects,
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
            .unwrap_or_default(),
        );

        let mut preview = self.clone();
        preview.apply_match_event(MatchEvent::PowerCardPlayed {
            player_id: player_id.clone(),
            card: card.clone(),
            target_player_id: target_player_id.clone(),
            effects: effects.clone(),
        });

        if matches!(preview.stage, PowerGameStage::Dealing) && !preview.is_finished() {
            effects.merge(
                preview
                    .passive_effects(PassiveGameEvent::RoundStart)
                    .unwrap_or_default(),
            );
        }

        Ok(MatchEvent::PowerCardPlayed {
            player_id: player_id.clone(),
            card,
            target_player_id,
            effects,
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

        let mut preview = self.clone();
        preview.apply_match_event(MatchEvent::PowerPhaseSkipped {
            player_id: player_id.clone(),
            effects: PowerCardEffects::default(),
        });

        let effects = if matches!(preview.stage, PowerGameStage::Dealing) && !preview.is_finished() {
            preview
                .passive_effects(PassiveGameEvent::RoundStart)
                .unwrap_or_default()
        } else {
            PowerCardEffects::default()
        };

        Ok(MatchEvent::PowerPhaseSkipped {
            player_id: player_id.clone(),
            effects,
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
                        let mut mana = self.apply_mana_totals(&mana);
                        let (passive_mana, _) = self.apply_effects(&passive_effects);
                        self.set_stage_after_bid(&state);
                        mana.extend(passive_mana);

                        AppliedGameChange::BidPlaced {
                            player_id,
                            bid,
                            state,
                            mana,
                        }
                    }
                    _ => unreachable!("bid event applies as bid change"),
                }
            }
            MatchEvent::TurnPlayed {
                turn,
                next_set,
                next_power_set,
                passive_effects,
                next_set_passive_effects,
            } => {
                let mut state = match self
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
                let (passive_mana, passive_power_decks) = self.apply_effects(&passive_effects);
                Self::sync_turn_outcome_lifes(&mut state.outcome, self.core.get_lifes());
                let (next_set_passive_mana, next_set_passive_power_decks) =
                    self.apply_effects(&next_set_passive_effects);
                let lifes = (!next_set_passive_effects.lifes.is_empty())
                    .then(|| next_set_passive_effects.lifes.clone());
                Self::merge_next_set_decks(&mut state.outcome, &next_set_passive_effects.decks);
                let mana = if passive_mana.is_empty() {
                    mana
                } else {
                    let mut mana = mana.unwrap_or_default();
                    mana.extend(passive_mana);
                    Some(mana)
                };
                let mana = if next_set_passive_mana.is_empty() {
                    mana
                } else {
                    let mut mana = mana.unwrap_or_default();
                    mana.extend(next_set_passive_mana);
                    Some(mana)
                };
                let power_decks = if passive_power_decks.is_empty() {
                    power_decks
                } else {
                    let mut power_decks = power_decks.unwrap_or_default();
                    power_decks.extend(passive_power_decks);
                    Some(power_decks)
                };
                let power_decks = if next_set_passive_power_decks.is_empty() {
                    power_decks
                } else {
                    let mut power_decks = power_decks.unwrap_or_default();
                    power_decks.extend(next_set_passive_power_decks);
                    Some(power_decks)
                };

                self.stage = if matches!(
                    &state.outcome,
                    fodinha_classic::GameOutcome::SetEnded { .. }
                ) {
                    PowerGameStage::Bidding
                } else {
                    PowerGameStage::Dealing
                };

                AppliedGameChange::TurnPlayed {
                    state,
                    lifes,
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

                let (mana, power_decks) = self.apply_effects(&effects);
                self.advance_power_phase(&player_id);

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
            MatchEvent::PowerPhaseSkipped { player_id, effects } => {
                let (mana, power_decks) = self.apply_effects(&effects);
                self.advance_power_phase(&player_id);

                AppliedGameChange::PowerPhaseSkipped(PowerPhaseSkipOutcome {
                    player_id,
                    lifes: self.core.get_lifes(),
                    changed_lifes: effects.lifes.clone(),
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

    pub fn get_lifes(&self) -> HashMap<PlayerId, usize> {
        self.core.get_lifes()
    }

    pub fn current_player(&self) -> Option<PlayerId> {
        match &self.stage {
            PowerGameStage::Bidding | PowerGameStage::Dealing => self.core.current_player(),
            PowerGameStage::Power { pending_players } => pending_players
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
            PowerGameStage::Power { .. } => GameStageDto::Power,
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
            },
        )?)
    }

    fn passive_effects(
        &self,
        event: PassiveGameEvent,
    ) -> Result<PowerCardEffects, PowerScriptError> {
        let mut effects = PowerCardEffects::default();

        for (player_id, mercenary_id) in &self.player_mercenaries {
            if !self.core.is_player_alive(player_id) {
                continue;
            }

            let Some(definition) = self.registry.mercenary_definition(mercenary_id) else {
                continue;
            };
            let output = super::power_lua::run_passive_script(
                &definition.passive_script,
                PassiveScriptInput {
                    mercenary_id: definition.id,
                    owner_id: player_id.clone(),
                    event: event.clone(),
                    players: self.script_players(None),
                    draw_power_cards: self.power_card_drawer(),
                },
            )?;

            effects.merge(Self::script_output_effects(output));
        }

        Ok(effects)
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
                    deck.iter().map(PowerCard::to_dto).collect(),
                )
            })
            .collect();

        (mana, power_decks)
    }

    fn set_stage_after_bid(&mut self, state: &BiddingState) {
        self.stage = match state {
            BiddingState::Active { .. } => PowerGameStage::Bidding,
            BiddingState::Ended { .. } => PowerGameStage::Power {
                pending_players: self.power_phase_order(),
            },
        };
    }

    fn advance_power_phase(&mut self, player_id: &PlayerId) {
        let alive_players = self
            .core
            .get_player_snapshots()
            .into_iter()
            .filter(|(_, player)| player.lifes > 0)
            .map(|(player_id, _)| player_id)
            .collect::<Vec<_>>();
        let PowerGameStage::Power { pending_players } = &mut self.stage else {
            return;
        };

        pending_players.retain(|pending_player| {
            pending_player != player_id && alive_players.contains(pending_player)
        });

        if pending_players.is_empty() {
            self.stage = PowerGameStage::Dealing;
        }
    }

    fn power_phase_order(&self) -> Vec<PlayerId> {
        self.core
            .get_round_order()
            .into_iter()
            .filter(|player_id| self.core.is_player_alive(player_id))
            .collect()
    }

    fn sync_turn_outcome_lifes(
        outcome: &mut fodinha_classic::GameOutcome,
        lifes: HashMap<PlayerId, usize>,
    ) {
        match outcome {
            fodinha_classic::GameOutcome::SetEnded {
                lifes: outcome_lifes,
                ..
            }
            | fodinha_classic::GameOutcome::Ended {
                lifes: outcome_lifes,
            } => *outcome_lifes = lifes,
            _ => {}
        }
    }

    fn merge_next_set_decks(
        outcome: &mut fodinha_classic::GameOutcome,
        decks: &HashMap<PlayerId, Vec<Card>>,
    ) {
        if decks.is_empty() {
            return;
        }

        if let fodinha_classic::GameOutcome::SetEnded {
            decks: outcome_decks,
            ..
        } = outcome
        {
            for (player_id, deck) in decks {
                outcome_decks.insert(player_id.clone(), deck.clone());
            }
        }
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
            Self::new_unpartitioned_power_decks(players, &definition, seed, sequence, registry)?
        };

        Ok(PowerSet {
            shuffle: DeckShuffle { seed, sequence },
            decks,
            mana,
        })
    }

    fn new_unpartitioned_power_decks(
        players: &[PlayerId],
        deck_definition: &PowerDeckDefinition,
        seed: i64,
        sequence: i64,
        registry: &PowerCardRegistry,
    ) -> Result<IndexMap<PlayerId, Vec<PowerCard>>, PowerCardDefinitionError> {
        let definitions = registry.power_card_definitions_from_ids(&deck_definition.card_ids)?;
        let needed_cards = players.len().saturating_mul(POWER_CARDS_PER_PLAYER);
        let mut deck = (0..needed_cards)
            .map(|idx| definitions[idx % definitions.len()].to_card())
            .collect::<Vec<_>>();

        shuffle_power_cards(&mut deck, seed, sequence);

        Ok(players
            .iter()
            .map(|player_id| {
                (
                    player_id.clone(),
                    deck.drain(..POWER_CARDS_PER_PLAYER.min(deck.len()))
                        .collect(),
                )
            })
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
        let generic_cards =
            registry.power_card_definitions_from_ids(&deck_definition.generic_card_ids)?;
        let mut generic_deck = (0..players.len().saturating_mul(GENERIC_POWER_CARDS_PER_PLAYER))
            .map(|idx| generic_cards[idx % generic_cards.len()].to_card())
            .collect::<Vec<_>>();
        shuffle_power_cards(&mut generic_deck, seed, sequence);

        let mut decks = IndexMap::new();

        for (player_idx, player_id) in players.iter().enumerate() {
            let mut cards = generic_deck
                .drain(..GENERIC_POWER_CARDS_PER_PLAYER.min(generic_deck.len()))
                .collect::<Vec<_>>();

            if let Some(mercenary_id) = player_mercenaries.get(player_id)
                && let Some(card_ids) = deck_definition.mercenary_card_ids.get(mercenary_id)
            {
                let mercenary_cards = registry.power_card_definitions_from_ids(card_ids)?;
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

    const NOOP_POWER_SCRIPT: &str = r#"
return {
    effect = function(game, card)
    end,
}
"#;

    const BID_HEAL_PASSIVE_SCRIPT: &str = r#"
return {
    on_bid_placed = function(game, event, mercenary)
        if event.player_id == mercenary.owner_id then
            game.add_lives(mercenary.owner_id, 1)
        end
    end,
}
"#;

    const MATCH_STARTED_HEAL_PASSIVE_SCRIPT: &str = r#"
return {
    on_match_started = function(game, event, mercenary)
        game.add_lives(mercenary.owner_id, 2)
    end,
}
"#;

    const ROUND_START_HEAL_PASSIVE_SCRIPT: &str = r#"
return {
    on_round_start = function(game, event, mercenary)
        game.add_lives(mercenary.owner_id, 1)
    end,
}
"#;

    const ROUND_ENDED_HEAL_PASSIVE_SCRIPT: &str = r#"
return {
    on_round_ended = function(game, event, mercenary)
        game.add_lives(mercenary.owner_id, 1)
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
                    card_ids: (0..10)
                        .map(|idx| card_id(&format!("generic_{idx}")))
                        .chain((0..5).map(|idx| card_id(&format!("artemis_{idx}"))))
                        .collect(),
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
    }

    fn draw_power_card_registry() -> (PowerCardRegistry, CardId) {
        let draw_id = card_id("draw_eight");
        let mut definitions = vec![PowerCardDefinitionInput {
            id: draw_id.clone(),
            name: "Draw Eight".to_string(),
            description: "Draws eight power cards.".to_string(),
            mana_cost: 0,
            card_type: PowerCardType::Instant,
            image_url: None,
            script: r#"
                return {
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
            lifes: DEFAULT_INITIAL_LIFES,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::new(),
        }
    }

    fn test_players() -> [PlayerId; 2] {
        [PlayerId(Arc::from("P1")), PlayerId(Arc::from("P2"))]
    }

    fn bid_current_player(game: &mut Game, bid: usize) {
        let player_id = game.current_player().expect("expected bidding player");
        let event = game.validate_bid(&player_id, bid).unwrap();

        game.apply_match_event(event);
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

    fn advance_to_dealing(game: &mut Game) {
        enter_power_phase(game);
        finish_power_phase(game);
    }

    fn set_power_phase(game: &mut Game, players: &[PlayerId]) {
        game.stage = PowerGameStage::Power {
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
    fn game_starts_with_initial_mana_pool() {
        let registry = test_registry();
        let players = test_players();

        let MatchEvent::GameStarted { power_set, .. } =
            Game::start_match_event_with_seed(&players, test_settings(), 42, &registry).unwrap()
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
            lifes: DEFAULT_INITIAL_LIFES,
            power_deck_id: test_deck_id(),
            player_mercenaries: HashMap::from([
                (player1.clone(), mercenary_id("artemis")),
                (player2.clone(), mercenary_id("artemis")),
            ]),
        };

        let MatchEvent::GameStarted { power_set, .. } =
            Game::start_match_event_with_seed(&players, settings, 42, &registry).unwrap()
        else {
            panic!("expected game started event");
        };

        for player in players {
            let cards = power_set.decks.get(&player).unwrap();

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
    fn mercenary_passive_effect_is_persisted_on_bid_event() {
        let registry = test_registry_with_mercenary_passives();
        let [player1, player2] = test_players();
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings {
            lifes: DEFAULT_INITIAL_LIFES,
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
            lifes: DEFAULT_INITIAL_LIFES,
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
            lifes: DEFAULT_INITIAL_LIFES,
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
            lifes: DEFAULT_INITIAL_LIFES,
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
            card_type: PowerCardType::Instant,
            image_url: None,
            script: r#"
                    return {
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
            card_type: PowerCardType::Instant,
            image_url: None,
            script: r#"
                    return {
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
            card_type: PowerCardType::Instant,
            image_url: None,
            script: r#"
                    return {
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
            card_type: PowerCardType::Instant,
            image_url: None,
            script: r#"
                    return {
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
                card_type: PowerCardType::Instant,
                image_url: None,
                script: r#"
                        return {
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
                        card_ids: vec![card_id("heal_10")],
                        generic_card_ids: Vec::new(),
                        mercenary_card_ids: HashMap::new(),
                    },
                    PowerDeckDefinitionInput {
                        id: custom_deck_id.clone(),
                        card_ids: vec![card_id("strike_10")],
                        generic_card_ids: Vec::new(),
                        mercenary_card_ids: HashMap::new(),
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
            Some(&PlayerManaDto { current: 3, max: 3 })
        );
        assert_eq!(
            mana.get(&player2),
            Some(&PlayerManaDto { current: 3, max: 3 })
        );
        assert_eq!(game.mana[&player1].current, 3);
        assert_eq!(game.mana[&player1].max, 3);
        assert_eq!(game.mana[&player2].current, 3);
        assert_eq!(game.mana[&player2].max, 3);
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
                current: 11,
                max: 11,
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

        assert_eq!(info.power_cards.unwrap().len(), POWER_CARDS_PER_PLAYER);
    }
}
