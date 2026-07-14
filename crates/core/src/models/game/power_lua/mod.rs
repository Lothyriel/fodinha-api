mod api;
pub mod lua_codegen;
mod runtime;

use std::{collections::HashMap, rc::Rc};

use lua_api_derive::{LuaApiEvent, LuaApiScript};
use mlua_extras::mlua;
pub use runtime::{
    parse_mercenary_passive_definition, parse_power_card_script_definition, run_passive_script,
    run_power_card_script, validate_mercenary_passive_script,
    validate_mercenary_passive_script_execution, validate_power_card_script,
    validate_power_card_script_execution,
};

use crate::models::{
    Card, Rank,
    game::fodinha_power::PowerCardType,
    id::{MercenaryId, PlayerId},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPlayerState {
    pub lifes: usize,
    pub bid: Option<usize>,
    pub rounds: usize,
    pub mana: ScriptManaState,
    pub cards: Vec<Card>,
    pub power_cards: Vec<ScriptPowerCardState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptManaState {
    pub current: usize,
    pub max: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPowerCardState {
    pub id: String,
    pub name: String,
    pub description: String,
    pub mana_cost: usize,
    pub card_type: PowerCardType,
    pub image_url: Option<String>,
    pub usable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeckReveal {
    pub caster_id: String,
    pub target_player_id: String,
    pub cards: Vec<Card>,
}

#[derive(Clone)]
pub struct PowerScriptInput {
    pub card_id: String,
    pub mana_cost: usize,
    pub owner_id: PlayerId,
    pub target_player_id: Option<PlayerId>,
    pub players: HashMap<PlayerId, ScriptPlayerState>,
    pub draw_power_cards: DrawPowerCardsFn,
    pub event: Option<PassiveGameEvent>,
    pub card_state: Option<ScriptPowerCardState>,
    pub current_trump: Rank,
}

impl std::fmt::Debug for PowerScriptInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PowerScriptInput")
            .field("card_id", &self.card_id)
            .field("mana_cost", &self.mana_cost)
            .field("owner_id", &self.owner_id)
            .field("target_player_id", &self.target_player_id)
            .field("players", &self.players)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct PassiveScriptInput {
    pub mercenary_id: MercenaryId,
    pub owner_id: PlayerId,
    pub base_life: usize,
    pub initial_mana: usize,
    pub event: PassiveGameEvent,
    pub players: HashMap<PlayerId, ScriptPlayerState>,
    pub draw_power_cards: DrawPowerCardsFn,
    pub current_trump: Rank,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerCardScriptDefinition {
    pub mana_cost: usize,
    pub card_type: PowerCardType,
    pub quantity: usize,
    pub event_handlers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MercenaryPassiveDefinition {
    pub base_life: usize,
    pub initial_mana: usize,
}

pub type DrawPowerCardsFn = Rc<dyn Fn(&str, usize) -> Result<Vec<ScriptPowerCardState>, String>>;

#[allow(dead_code)]
#[derive(LuaApiScript)]
#[lua_api_script(description = "A power card script definition.")]
struct PowerCardScript {
    #[lua_api_field(name = "type")]
    card_type: PowerCardType,
    #[description("Base mana cost of the card.")]
    mana_cost: usize,
    #[description("Number of copies added to a deck.")]
    quantity: usize,
    #[description("Runs when the card is played.")]
    effect: fn(api::LuaGame, api::LuaPowerCard),
}

#[allow(dead_code)]
#[derive(LuaApiScript)]
#[lua_api_script(description = "A mercenary passive script definition.")]
struct MercenaryPassiveScript {
    #[description("Base life total for the mercenary.")]
    base_life: usize,
    #[description("Initial mana pool size for the mercenary.")]
    initial_mana: usize,
    #[description("Runs when a match starts.")]
    on_match_started: fn(api::LuaGame, PassiveGameEvent, api::LuaMercenary),
    #[description("Runs after a bid is placed.")]
    on_bid_placed: fn(api::LuaGame, PassiveGameEvent, api::LuaMercenary),
    #[description("Runs when a power card is played.")]
    on_power_card_played: fn(api::LuaGame, PassiveGameEvent, api::LuaMercenary),
    #[description("Runs when a round starts.")]
    on_round_start: fn(api::LuaGame, PassiveGameEvent, api::LuaMercenary),
    #[description("Runs after a normal card is played.")]
    on_turn_played: fn(api::LuaGame, PassiveGameEvent, api::LuaMercenary),
    #[description("Runs when a round ends.")]
    on_round_ended: fn(api::LuaGame, PassiveGameEvent, api::LuaMercenary),
    #[description("Runs when a set starts.")]
    on_set_started: fn(api::LuaGame, PassiveGameEvent, api::LuaMercenary),
    #[description("Runs when a set ends.")]
    on_set_ended: fn(api::LuaGame, PassiveGameEvent, api::LuaMercenary),
}

impl std::fmt::Debug for PassiveScriptInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassiveScriptInput")
            .field("mercenary_id", &self.mercenary_id)
            .field("owner_id", &self.owner_id)
            .field("event", &self.event)
            .field("players", &self.players)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, LuaApiEvent)]
pub enum PassiveGameEvent {
    #[lua_api_event(description = "Passive event emitted when a match starts.")]
    MatchStarted,
    #[lua_api_event(description = "Passive event emitted after a bid is placed.")]
    BidPlaced { player_id: PlayerId, bid: usize },
    #[lua_api_event(description = "Passive event emitted after a power card is played.")]
    PowerCardPlayed {
        player_id: PlayerId,
        card_id: String,
        target_player_id: Option<PlayerId>,
    },
    #[lua_api_event(description = "Passive event emitted when a round starts.")]
    RoundStart,
    #[lua_api_event(description = "Passive event emitted after a normal card is played.")]
    TurnPlayed { player_id: PlayerId, card: Card },
    #[lua_api_event(description = "Passive event emitted when a round ends.")]
    RoundEnded { winner: PlayerId, card: Card },
    #[lua_api_event(description = "Passive event emitted when a set starts.")]
    SetStarted,
    #[lua_api_event(description = "Passive event emitted when a set ends.")]
    SetEnded {
        lost_players: HashMap<PlayerId, usize>,
        bids: HashMap<PlayerId, usize>,
    },
}

impl PassiveGameEvent {
    pub(crate) fn handler_name(&self) -> &'static str {
        match self {
            Self::MatchStarted => lua_codegen::passive_handler_name("match_started"),
            Self::BidPlaced { .. } => lua_codegen::passive_handler_name("bid_placed"),
            Self::PowerCardPlayed { .. } => lua_codegen::passive_handler_name("power_card_played"),
            Self::RoundStart => lua_codegen::passive_handler_name("round_start"),
            Self::TurnPlayed { .. } => lua_codegen::passive_handler_name("turn_played"),
            Self::RoundEnded { .. } => lua_codegen::passive_handler_name("round_ended"),
            Self::SetStarted => lua_codegen::passive_handler_name("set_started"),
            Self::SetEnded { .. } => lua_codegen::passive_handler_name("set_ended"),
        }
    }

    pub(crate) fn event_type(&self) -> &'static str {
        match self {
            Self::MatchStarted => "match_started",
            Self::BidPlaced { .. } => "bid_placed",
            Self::PowerCardPlayed { .. } => "power_card_played",
            Self::RoundStart => "round_start",
            Self::TurnPlayed { .. } => "turn_played",
            Self::RoundEnded { .. } => "round_ended",
            Self::SetStarted => "set_started",
            Self::SetEnded { .. } => "set_ended",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerScriptOutput {
    pub lifes: HashMap<PlayerId, usize>,
    pub mana: HashMap<PlayerId, ScriptManaState>,
    pub cards: HashMap<PlayerId, Vec<Card>>,
    pub power_cards: HashMap<PlayerId, Vec<ScriptPowerCardState>>,
    pub deck_reveals: Vec<DeckReveal>,
    pub mana_cost: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
pub enum PowerScriptError {
    #[error("lua error: {0}")]
    Lua(#[from] mlua::Error),
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

    use crate::models::{Card, Rank, Suit, id::PlayerId};

    use super::*;

    fn script_player(lifes: usize) -> ScriptPlayerState {
        ScriptPlayerState {
            lifes,
            bid: None,
            rounds: 0,
            mana: ScriptManaState {
                current: 5,
                max: 10,
            },
            cards: Vec::new(),
            power_cards: Vec::new(),
        }
    }

    fn no_power_card_draws() -> DrawPowerCardsFn {
        Rc::new(|_, _| Err("power card drawing is not available".to_string()))
    }

    fn script_input(
        owner_id: PlayerId,
        target_player_id: Option<PlayerId>,
        players: HashMap<PlayerId, ScriptPlayerState>,
    ) -> PowerScriptInput {
        PowerScriptInput {
            card_id: "test_card".to_string(),
            mana_cost: 2,
            owner_id,
            target_player_id,
            players,
            draw_power_cards: no_power_card_draws(),
            event: None,
            card_state: None,
            current_trump: Rank::Four,
        }
    }

    fn passive_input(
        owner_id: PlayerId,
        event: PassiveGameEvent,
        players: HashMap<PlayerId, ScriptPlayerState>,
    ) -> PassiveScriptInput {
        PassiveScriptInput {
            mercenary_id: crate::models::id::MercenaryId(Arc::from("artemis")),
            owner_id,
            base_life: 50,
            initial_mana: 2,
            event,
            players,
            draw_power_cards: no_power_card_draws(),
            current_trump: Rank::Four,
        }
    }

    #[test]
    fn documented_api_names_are_bound_and_generated_lua_is_valid() {
        let player = PlayerId(Arc::from("P1"));
        let input = script_input(
            player.clone(),
            None,
            HashMap::from([(player.clone(), script_player(50))]),
        );
        let players = Rc::new(RefCell::new(
            input
                .players
                .iter()
                .map(|(player_id, state)| (player_id.as_str().to_string(), state.clone()))
                .collect::<HashMap<_, _>>(),
        ));
        let lua = runtime::create_lua().unwrap();
        let game = api::build_game_api(
            Rc::clone(&players),
            input.draw_power_cards.clone(),
            Rc::new(RefCell::new(Vec::new())),
            input.current_trump,
        );
        let card = api::build_power_card(&input);
        let mercenary = api::build_mercenary(&passive_input(
            player,
            PassiveGameEvent::MatchStarted,
            input.players.clone(),
        ));

        for name in [
            "get_lives",
            "get_current_trump",
            "add_lives",
            "set_lives",
            "get_bid",
            "add_bids",
            "get_rounds",
            "get_mana",
            "get_max_mana",
            "get_mana_pool",
            "add_mana",
            "set_mana",
            "set_max_mana",
            "get_cards",
            "reveal_deck",
            "switch_cards",
            "get_power_cards",
            "steal_power_card",
            "draw_power_cards",
            "player_ids",
        ] {
            let source = format!("return function(game) return type(game.{name}) end");
            let check: mlua::Function = lua.load(source).eval().unwrap();
            let lua_type: String = check.call(game.clone()).unwrap();
            assert_eq!(lua_type, "function", "{name} should be callable");
        }

        for name in ["id", "mana_cost", "owner_id", "target_player_id"] {
            let source = format!("return function(card) return card.{name} ~= nil end");
            let check: mlua::Function = lua.load(source).eval().unwrap();
            let _: bool = check.call(card.clone()).unwrap();
        }

        let name = "add_mana_cost";
        let source = format!("return function(card) return type(card.{name}) end");
        let check: mlua::Function = lua.load(source).eval().unwrap();
        let lua_type: String = check.call(card.clone()).unwrap();
        assert_eq!(lua_type, "function", "{name} should be callable");

        let card_state = api::LuaPowerCardState::with_context(
            &ScriptPowerCardState {
                id: "state_card".to_string(),
                name: "State Card".to_string(),
                description: "A test card".to_string(),
                mana_cost: 1,
                card_type: PowerCardType::Instant,
                image_url: None,
                usable: true,
            },
            Rc::clone(&players),
            "P1",
        );
        let check: mlua::Function = lua
            .load("return function(card_state) return type(card_state.set_usable) end")
            .eval()
            .unwrap();
        let lua_type: String = check.call(card_state).unwrap();
        assert_eq!(lua_type, "function", "set_usable should be callable");

        for name in ["id", "owner_id", "base_life", "initial_mana"] {
            let source = format!("return function(mercenary) return mercenary.{name} ~= nil end");
            let check: mlua::Function = lua.load(source).eval().unwrap();
            let _: bool = check.call(mercenary.clone()).unwrap();
        }

        let card_fields_ok: bool = lua
            .load(
                r#"return function(game)
                local cards = game.get_cards("P1")
                return #cards == 0
            end"#,
            )
            .eval::<mlua::Function>()
            .unwrap()
            .call(game)
            .unwrap();
        assert!(card_fields_ok);

        lua.load(lua_codegen::render_definitions())
            .set_name("fodinha.d.lua")
            .exec()
            .unwrap();
        lua.load(super::lua_codegen::render_power_card_template())
            .set_name("power-card-template.lua")
            .exec()
            .unwrap();
        lua.load(super::lua_codegen::render_mercenary_passive_template())
            .set_name("mercenary-passive-template.lua")
            .exec()
            .unwrap();

        validate_power_card_script(super::lua_codegen::render_power_card_template(), "template")
            .unwrap();
        validate_mercenary_passive_script(
            super::lua_codegen::render_mercenary_passive_template(),
            "template",
        )
        .unwrap();
    }

    #[test]
    fn typed_definitions_are_generated() {
        let definitions = lua_codegen::render_definitions();
        assert!(definitions.starts_with("---@meta\n\n"));
        assert!(definitions.contains("---@class Game"));
        assert!(
            definitions
                .contains("---@field get_lives fun(self: Game, player_id: PlayerId): integer")
        );
        assert!(
            definitions
                .contains("---@field get_cards fun(self: Game, player_id: PlayerId): Card[]")
        );
        assert!(definitions.contains(
            "---@field reveal_deck fun(self: Game, caster_id: PlayerId, target_player_id: PlayerId)"
        ));
        assert!(definitions.contains(
            "---@field get_power_cards fun(self: Game, player_id: PlayerId): PowerCardState[]"
        ));
        assert!(definitions.contains("---@field get_current_trump fun(self: Game): Rank"));
        assert!(
            definitions
                .contains("---@field add_mana_cost fun(self: PowerCard, delta: integer): integer")
        );
        assert!(
            definitions.contains("---@field set_usable fun(self: PowerCardState, usable: boolean)")
        );
        assert!(definitions.contains("---@field rank Rank"));
        assert!(definitions.contains("---@field suit Suit"));
        assert!(definitions.contains("---@field type PowerCardType"));
        assert!(definitions.contains("---@field type \"bid_placed\""));
        assert!(definitions.contains("---@field bids table<PlayerId, integer>"));
        assert!(definitions.contains(
            "---@field on_match_started? fun(game: Game, event: MatchStartedEvent, mercenary: Mercenary)"
        ));
        assert!(!definitions.contains("_CLASS_"));
        assert!(!definitions.contains("LuaCard"));
        assert!(!definitions.contains("game = nil"));
        assert!(!definitions.contains("card = nil"));
        assert!(!definitions.contains("mercenary = nil"));
        assert!(!definitions.contains("event = nil"));
        assert!(lua_codegen::render_power_card_template().contains("PowerCardScript"));
        assert!(
            lua_codegen::render_mercenary_passive_template().contains("MercenaryPassiveScript")
        );
    }

    #[test]
    fn passive_script_rejects_unknown_handlers() {
        let source = r#"
            return {
                base_life = 50,
                initial_mana = 2,
                on_unknown = function(game, event, mercenary)
                end,
            }
        "#;

        let error = validate_mercenary_passive_script(source, "passive.lua").unwrap_err();

        assert!(
            error
                .to_string()
                .contains("unsupported passive event handler")
        );
    }

    #[test]
    fn passive_script_can_react_to_bid_events() {
        let player = PlayerId(Arc::from("P1"));
        let output = run_passive_script(
            r#"
            return {
                base_life = 50,
                initial_mana = 2,
                on_bid_placed = function(game, event, mercenary)
                    if event.player_id == mercenary.owner_id then
                        game.add_lives(mercenary.owner_id, event.bid)
                    end
                end,
            }
            "#,
            passive_input(
                player.clone(),
                PassiveGameEvent::BidPlaced {
                    player_id: player.clone(),
                    bid: 2,
                },
                HashMap::from([(player.clone(), script_player(50))]),
            ),
        )
        .unwrap();

        assert_eq!(output.lifes.get(&player), Some(&52));
    }

    #[test]
    fn passive_script_can_react_to_round_start_events() {
        let player = PlayerId(Arc::from("P1"));
        let output = run_passive_script(
            r#"
            return {
                base_life = 50,
                initial_mana = 2,
                on_round_start = function(game, event, mercenary)
                    game.add_lives(mercenary.owner_id, 3)
                end,
            }
            "#,
            passive_input(
                player.clone(),
                PassiveGameEvent::RoundStart,
                HashMap::from([(player.clone(), script_player(50))]),
            ),
        )
        .unwrap();

        assert_eq!(output.lifes.get(&player), Some(&53));
    }

    #[test]
    fn set_ended_event_exposes_lost_players_and_current_lives() {
        let player = PlayerId(Arc::from("P1"));
        let lost_player = PlayerId(Arc::from("P2"));
        let output = run_passive_script(
            r#"
            return {
                base_life = 50,
                initial_mana = 2,
                on_set_ended = function(game, event, mercenary)
                    assert(event.lost_players["P2"] == 10)
                    assert(event.bids["P1"] == 1)
                    assert(event.bids["P2"] == 2)
                    assert(game.get_lives("P2") == 0)
                    game.add_lives(mercenary.owner_id, 1)
                end,
            }
            "#,
            passive_input(
                player.clone(),
                PassiveGameEvent::SetEnded {
                    lost_players: HashMap::from([(lost_player.clone(), 10)]),
                    bids: HashMap::from([(player.clone(), 1), (lost_player.clone(), 2)]),
                },
                HashMap::from([
                    (player.clone(), script_player(50)),
                    (lost_player, script_player(0)),
                ]),
            ),
        )
        .unwrap();

        assert_eq!(output.lifes.get(&player), Some(&51));
    }

    #[test]
    fn round_ended_event_exposes_winner_and_card() {
        let player = PlayerId(Arc::from("P1"));
        let output = run_passive_script(
            r#"
            return {
                base_life = 50,
                initial_mana = 2,
                on_round_ended = function(game, event, mercenary)
                    assert(event.winner == "P1")
                    assert(event.card.rank == "Three")
                    assert(event.card.suit == "Clubs")
                    assert(game.get_current_trump() == "Four")
                end,
            }
            "#,
            passive_input(
                player.clone(),
                PassiveGameEvent::RoundEnded {
                    winner: player.clone(),
                    card: Card::new(Rank::Three, Suit::Clubs),
                },
                HashMap::from([(player, script_player(50))]),
            ),
        )
        .unwrap();

        assert!(output.lifes.is_empty());
    }

    #[test]
    fn power_card_script_definition_is_extracted_from_lua() {
        let definition = parse_power_card_script_definition(
            r#"
                return {
                    type = PowerCardType.Interactive,
                    mana_cost = 7,
                    quantity = 4,
                    effect = function(game, card)
                    end,
                }
            "#,
            "power.lua",
        )
        .unwrap();

        assert_eq!(definition.card_type, PowerCardType::Interactive);
        assert_eq!(definition.mana_cost, 7);
        assert_eq!(definition.quantity, 4);
    }

    #[test]
    fn mercenary_passive_definition_is_extracted_from_lua() {
        let definition = parse_mercenary_passive_definition(
            r#"
                return {
                    base_life = 120,
                    initial_mana = 9,
                }
            "#,
            "mercenary.lua",
        )
        .unwrap();

        assert_eq!(definition.base_life, 120);
        assert_eq!(definition.initial_mana, 9);
    }

    #[test]
    fn script_can_change_lives_through_limited_api() {
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let output = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Targetable,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    game.add_lives(card.target_player_id, -10)
                end,
            }
            "#,
            script_input(
                player1.clone(),
                Some(player2.clone()),
                HashMap::from([
                    (player1, script_player(50)),
                    (player2.clone(), script_player(50)),
                ]),
            ),
        )
        .unwrap();

        assert_eq!(output.lifes.get(&player2), Some(&40));
    }

    #[test]
    fn script_can_call_game_methods_with_colon_syntax() {
        let player = PlayerId(Arc::from("P1"));
        let output = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Instant,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    game:add_lives(card.owner_id, 5)
                end,
            }
            "#,
            script_input(
                player.clone(),
                None,
                HashMap::from([(player.clone(), script_player(50))]),
            ),
        )
        .unwrap();

        assert_eq!(output.lifes.get(&player), Some(&55));
    }

    #[test]
    fn script_can_reveal_target_deck_to_caster() {
        let caster = PlayerId(Arc::from("P1"));
        let target = PlayerId(Arc::from("P2"));
        let target_card = Card::new(Rank::Two, Suit::Golds);
        let mut target_state = script_player(50);
        target_state.cards = vec![target_card];

        let output = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Targetable,
                mana_cost = 0,
                quantity = 1,
                effect = function(game, card)
                    game:reveal_deck(card.owner_id, card.target_player_id)
                end,
            }
            "#,
            script_input(
                caster.clone(),
                Some(target.clone()),
                HashMap::from([(caster, script_player(50)), (target, target_state)]),
            ),
        )
        .unwrap();

        assert_eq!(
            output.deck_reveals,
            vec![DeckReveal {
                caster_id: "P1".to_string(),
                target_player_id: "P2".to_string(),
                cards: vec![target_card],
            }]
        );
        assert!(output.cards.is_empty());
    }

    #[test]
    fn card_script_rejects_metadata_fields() {
        let player = PlayerId(Arc::from("P1"));
        let source = r#"
            return {
                name = "Heal 10",
                type = PowerCardType.Instant,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    game.add_lives(card.owner_id, 10)
                end,
            }
        "#;

        let error = validate_power_card_script(source, "test.lua").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("type, mana_cost, quantity, and effect fields")
        );

        let output = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Instant,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    game.add_lives(card.owner_id, 10)
                end,
            }
            "#,
            script_input(
                player.clone(),
                None,
                HashMap::from([(player.clone(), script_player(50))]),
            ),
        )
        .unwrap();

        assert_eq!(output.lifes.get(&player), Some(&60));
    }

    #[test]
    fn script_can_set_lives_and_add_large_deltas() {
        let player = PlayerId(Arc::from("P1"));
        let output = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Instant,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    game.add_lives(card.owner_id, 1000)
                    game.set_lives(card.owner_id, 777)
                end,
            }
            "#,
            script_input(
                player.clone(),
                None,
                HashMap::from([(player.clone(), script_player(50))]),
            ),
        )
        .unwrap();

        assert_eq!(output.lifes.get(&player), Some(&777));
    }

    #[test]
    fn script_can_read_and_change_mana() {
        let player = PlayerId(Arc::from("P1"));
        let output = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Instant,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    local mana = game.get_mana(card.owner_id)
                    game.set_mana(card.owner_id, mana - 3)
                    game.add_mana(card.owner_id, 1)
                end,
            }
            "#,
            script_input(
                player.clone(),
                None,
                HashMap::from([(player.clone(), script_player(50))]),
            ),
        )
        .unwrap();

        assert_eq!(
            output.mana.get(&player),
            Some(&ScriptManaState {
                current: 3,
                max: 10
            })
        );
    }

    #[test]
    fn script_can_adjust_current_card_mana_cost() {
        let player = PlayerId(Arc::from("P1"));
        let output = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Instant,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    card:add_mana_cost(3)
                    card.add_mana_cost(-1)
                end,
            }
            "#,
            script_input(
                player.clone(),
                None,
                HashMap::from([(player.clone(), script_player(50))]),
            ),
        )
        .unwrap();

        assert_eq!(output.mana_cost, Some(4));
    }

    #[test]
    fn current_card_mana_cost_can_be_negative() {
        let player = PlayerId(Arc::from("P1"));
        let output = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Instant,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    card:add_mana_cost(-5)
                    if card.mana_cost ~= -3 then
                        error("expected negative mana cost")
                    end
                end,
            }
            "#,
            script_input(
                player.clone(),
                None,
                HashMap::from([(player.clone(), script_player(50))]),
            ),
        )
        .unwrap();

        assert_eq!(output.mana_cost, Some(-3));
    }

    #[test]
    fn script_can_draw_power_cards() {
        let player = PlayerId(Arc::from("P1"));
        let mut input = script_input(
            player.clone(),
            None,
            HashMap::from([(player.clone(), script_player(50))]),
        );
        input.draw_power_cards = Rc::new(|player_id, count| {
            Ok((0..count)
                .map(|idx| ScriptPowerCardState {
                    id: format!("{player_id}_drawn_{idx}"),
                    name: format!("Drawn {idx}"),
                    description: "Drawn by test".to_string(),
                    mana_cost: idx,
                    card_type: PowerCardType::Instant,
                    image_url: None,
                    usable: true,
                })
                .collect())
        });

        let output = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Instant,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    local drawn = game.draw_power_cards(card.owner_id, 2)
                    game.add_mana(card.owner_id, #drawn)
                end,
            }
            "#,
            input,
        )
        .unwrap();

        let power_cards = output.power_cards.get(&player).unwrap();
        assert_eq!(power_cards.len(), 2);
        assert_eq!(power_cards[0].id, "P1_drawn_0");
        assert_eq!(output.mana.get(&player).unwrap().current, 7);
    }

    #[test]
    fn script_can_switch_visible_normal_cards() {
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let first_card = Card::new(Rank::Four, Suit::Golds);
        let second_card = Card::new(Rank::Three, Suit::Clubs);
        let mut first_state = script_player(50);
        let mut second_state = script_player(50);
        first_state.cards = vec![first_card];
        second_state.cards = vec![second_card];
        let output = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Targetable,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    local mine = game.get_cards(card.owner_id)[1]
                    local theirs = game.get_cards(card.target_player_id)[1]
                    game.switch_cards(card.owner_id, mine, card.target_player_id, theirs)
                end,
            }
            "#,
            script_input(
                player1.clone(),
                Some(player2.clone()),
                HashMap::from([
                    (player1.clone(), first_state),
                    (player2.clone(), second_state),
                ]),
            ),
        )
        .unwrap();

        assert_eq!(output.cards.get(&player1), Some(&vec![second_card]));
        assert_eq!(output.cards.get(&player2), Some(&vec![first_card]));
    }

    #[test]
    fn script_can_steal_power_cards() {
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let first_state = script_player(50);
        let mut second_state = script_player(50);
        second_state.power_cards = vec![ScriptPowerCardState {
            id: "stolen".to_string(),
            name: "Stolen".to_string(),
            description: "Take it.".to_string(),
            mana_cost: 1,
            card_type: PowerCardType::Instant,
            image_url: None,
            usable: true,
        }];
        let output = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Targetable,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    local cards = game.get_power_cards(card.target_player_id)
                    game.steal_power_card(card.target_player_id, cards[1].id, card.owner_id)
                end,
            }
            "#,
            script_input(
                player1.clone(),
                Some(player2.clone()),
                HashMap::from([
                    (player1.clone(), first_state),
                    (player2.clone(), second_state),
                ]),
            ),
        )
        .unwrap();

        assert_eq!(output.power_cards.get(&player1).unwrap()[0].id, "stolen");
        assert!(output.power_cards.get(&player2).unwrap().is_empty());
    }

    #[test]
    fn script_cannot_open_unsafe_standard_libraries() {
        let player = PlayerId(Arc::from("P1"));
        let error = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Instant,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    os.execute('true')
                end,
            }
            "#,
            script_input(
                player.clone(),
                None,
                HashMap::from([(player, script_player(50))]),
            ),
        )
        .unwrap_err();

        assert!(error.to_string().contains("os"));
    }

    #[test]
    fn script_instruction_limit_stops_infinite_loop() {
        let player = PlayerId(Arc::from("P1"));
        let error = run_power_card_script(
            r#"
            return {
                type = PowerCardType.Instant,
                mana_cost = 2,
                quantity = 1,
                effect = function(game, card)
                    while true do end
                end,
            }
            "#,
            script_input(
                player.clone(),
                None,
                HashMap::from([(player, script_player(50))]),
            ),
        )
        .unwrap_err();

        assert!(error.to_string().contains("instruction limit"));
    }
}
