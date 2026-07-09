mod api;
mod definitions;
pub mod lua_codegen;
mod runtime;

use std::{collections::HashMap, rc::Rc};

pub use definitions::{FODINHA_LUA_DEFINITIONS, MERCENARY_PASSIVE_TEMPLATE, POWER_CARD_TEMPLATE};
use power_lua_api::metadata;
pub use runtime::{
    parse_mercenary_passive_definition, parse_power_card_script_definition, run_passive_script,
    run_power_card_script,
    validate_mercenary_passive_script, validate_power_card_script,
};

pub fn lua_api_type_definitions() -> [power_lua_api::LuaTypeDefinition; 5] {
    api::userdata_type_definitions()
}

use crate::models::{
    Card,
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
}

#[derive(Clone)]
pub struct PowerScriptInput {
    pub card_id: String,
    pub mana_cost: usize,
    pub owner_id: PlayerId,
    pub target_player_id: Option<PlayerId>,
    pub players: HashMap<PlayerId, ScriptPlayerState>,
    pub draw_power_cards: DrawPowerCardsFn,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerCardScriptDefinition {
    pub mana_cost: usize,
    pub card_type: PowerCardType,
    pub quantity: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MercenaryPassiveDefinition {
    pub base_life: usize,
    pub initial_mana: usize,
}

pub type DrawPowerCardsFn = Rc<dyn Fn(&str, usize) -> Result<Vec<ScriptPowerCardState>, String>>;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PassiveGameEvent {
    MatchStarted,
    BidPlaced {
        player_id: PlayerId,
        bid: usize,
    },
    PowerCardPlayed {
        player_id: PlayerId,
        card_id: String,
        target_player_id: Option<PlayerId>,
    },
    RoundStart,
    TurnPlayed {
        player_id: PlayerId,
        card: Card,
    },
    RoundEnded,
    SetStarted,
    SetEnded,
}

impl PassiveGameEvent {
    pub(crate) fn handler_name(&self) -> &'static str {
        match self {
            Self::MatchStarted => metadata::ON_MATCH_STARTED,
            Self::BidPlaced { .. } => metadata::ON_BID_PLACED,
            Self::PowerCardPlayed { .. } => metadata::ON_POWER_CARD_PLAYED,
            Self::RoundStart => metadata::ON_ROUND_START,
            Self::TurnPlayed { .. } => metadata::ON_TURN_PLAYED,
            Self::RoundEnded => metadata::ON_ROUND_ENDED,
            Self::SetStarted => metadata::ON_SET_STARTED,
            Self::SetEnded => metadata::ON_SET_ENDED,
        }
    }

    pub(crate) fn event_type(&self) -> &'static str {
        match self {
            Self::MatchStarted => "match_started",
            Self::BidPlaced { .. } => "bid_placed",
            Self::PowerCardPlayed { .. } => "power_card_played",
            Self::RoundStart => "round_start",
            Self::TurnPlayed { .. } => "turn_played",
            Self::RoundEnded => "round_ended",
            Self::SetStarted => "set_started",
            Self::SetEnded => "set_ended",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerScriptOutput {
    pub lifes: HashMap<PlayerId, usize>,
    pub mana: HashMap<PlayerId, ScriptManaState>,
    pub cards: HashMap<PlayerId, Vec<Card>>,
    pub power_cards: HashMap<PlayerId, Vec<ScriptPowerCardState>>,
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
        let globals = lua.globals();
        let game = api::build_game_api(players, input.draw_power_cards.clone());
        let card = api::build_power_card(&input);
        let mercenary = api::build_mercenary(&passive_input(
            player,
            PassiveGameEvent::MatchStarted,
            input.players.clone(),
        ));

        globals.set("game", game).unwrap();
        globals.set("card", card).unwrap();
        globals.set("mercenary", mercenary).unwrap();

        for method in metadata::GAME_TYPE.methods {
            let source = format!("return type(game.{})", method.name);
            let lua_type: String = lua.load(source).eval().unwrap();
            assert_eq!(lua_type, "function", "{} should be callable", method.name);
        }

        for field in metadata::POWER_CARD_TYPE.fields {
            let source = format!("return card.{} ~= nil", field.name);
            let _: bool = lua.load(source).eval().unwrap();
        }

        for method in metadata::POWER_CARD_TYPE.methods {
            let source = format!("return type(card.{})", method.name);
            let lua_type: String = lua.load(source).eval().unwrap();
            assert_eq!(lua_type, "function", "{} should be callable", method.name);
        }

        for field in metadata::MERCENARY_TYPE.fields {
            let source = format!("return mercenary.{} ~= nil", field.name);
            let _: bool = lua.load(source).eval().unwrap();
        }

        let card_fields_ok: bool = lua
            .load(
                r#"
                local cards = game.get_cards("P1")
                return #cards == 0
                "#,
            )
            .eval()
            .unwrap();
        assert!(card_fields_ok);

        lua.load(power_lua_api::generate::render_definitions())
            .set_name("fodinha.d.lua")
            .exec()
            .unwrap();
        lua.load(power_lua_api::generate::render_power_card_template())
            .set_name("power-card-template.lua")
            .exec()
            .unwrap();
        lua.load(power_lua_api::generate::render_mercenary_passive_template())
            .set_name("mercenary-passive-template.lua")
            .exec()
            .unwrap();

        validate_power_card_script(
            power_lua_api::generate::render_power_card_template(),
            "template",
        )
        .unwrap();
        validate_mercenary_passive_script(
            power_lua_api::generate::render_mercenary_passive_template(),
            "template",
        )
        .unwrap();
    }

    #[test]
    fn generated_files_are_embedded() {
        assert!(FODINHA_LUA_DEFINITIONS.contains("---@class Game"));
        assert!(POWER_CARD_TEMPLATE.contains("PowerCardScript"));
        assert!(MERCENARY_PASSIVE_TEMPLATE.contains("MercenaryPassiveScript"));
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
        assert!(error.to_string().contains("type, mana_cost, quantity, and effect fields"));

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
