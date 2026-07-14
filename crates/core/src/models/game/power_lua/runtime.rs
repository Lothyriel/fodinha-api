use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
};

use mlua_extras::mlua::{self, HookTriggers, Lua, LuaOptions, StdLib, Value, VmState};

use crate::models::{
    Card, Rank, Suit,
    id::{MercenaryId, PlayerId},
};

use super::{
    MercenaryPassiveDefinition, PassiveGameEvent, PassiveScriptInput, PowerCardScriptDefinition,
    PowerScriptError, PowerScriptInput, PowerScriptOutput, ScriptManaState, ScriptPlayerState,
    ScriptPowerCardState, api,
};

const LUA_MEMORY_LIMIT_BYTES: usize = 256 * 1024;
const LUA_HOOK_INSTRUCTION_INTERVAL: u32 = 1_000;
const LUA_MAX_HOOK_TICKS: u32 = 100;

pub fn validate_power_card_script(source: &str, path: &str) -> Result<(), mlua::Error> {
    parse_power_card_script_definition(source, path).map(|_| ())
}

/// Parse a script and execute its effect and every handler it declares with the
/// same API/runtime used by a match. This is deliberately a smoke check: it
/// catches runtime failures without trying to model every possible game state.
pub fn validate_power_card_script_execution(source: &str, path: &str) -> Result<(), mlua::Error> {
    let definition = parse_power_card_script_definition(source, path)
        .map_err(|error| contextual_error(path, "definition", error))?;
    let owner = PlayerId(Arc::from("validator-owner"));
    let target = PlayerId(Arc::from("validator-target"));

    run_power_card_script(
        source,
        smoke_power_input(&owner, Some(&target), definition.mana_cost, None),
    )
    .map_err(|error| contextual_error(path, "effect", error))?;

    for handler in definition.event_handlers {
        let event = smoke_event_for_handler(&handler, &owner, &target)?;
        run_power_card_script(
            source,
            smoke_power_input(
                &owner,
                Some(&target),
                definition.mana_cost,
                Some((event, smoke_power_card_state())),
            ),
        )
        .map_err(|error| contextual_error(path, &handler, error))?;
    }

    Ok(())
}

pub fn validate_mercenary_passive_script(source: &str, path: &str) -> Result<(), mlua::Error> {
    parse_mercenary_passive_definition(source, path).map(|_| ())
}

pub fn validate_mercenary_passive_script_execution(
    source: &str,
    path: &str,
) -> Result<(), mlua::Error> {
    let definition = parse_mercenary_passive_definition(source, path)
        .map_err(|error| contextual_error(path, "definition", error))?;
    let owner = PlayerId(std::sync::Arc::from("validator-owner"));
    let target = PlayerId(std::sync::Arc::from("validator-target"));
    let _mercenary_id = MercenaryId(std::sync::Arc::from("validator-mercenary"));

    for handler in [
        "on_match_started",
        "on_bid_placed",
        "on_power_card_played",
        "on_round_start",
        "on_turn_played",
        "on_round_ended",
        "on_set_started",
        "on_set_ended",
    ] {
        if !handler_is_present(source, handler)? {
            continue;
        }
        let event = smoke_event_for_handler(handler, &owner, &target)?;
        run_passive_script(source, smoke_passive_input(&owner, definition, event))
            .map_err(|error| contextual_error(path, handler, error))?;
    }

    Ok(())
}

fn contextual_error(path: &str, context: &str, error: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::external(format!("{path} {context} failed: {error}"))
}

fn handler_is_present(source: &str, handler: &str) -> Result<bool, mlua::Error> {
    let lua = create_lua()?;
    let table: mlua::Table = lua.load(source).set_name("handler-check").eval()?;
    Ok(matches!(table.get::<Value>(handler)?, Value::Function(_)))
}

fn smoke_players(owner: &PlayerId, target: &PlayerId) -> HashMap<PlayerId, ScriptPlayerState> {
    let card = Card::new(Rank::One, Suit::Clubs);
    let power_card = ScriptPowerCardState {
        id: "validator-power-card".to_string(),
        name: "Validator card".to_string(),
        description: "Smoke test card".to_string(),
        mana_cost: 1,
        card_type: crate::models::game::fodinha_power::PowerCardType::Instant,
        image_url: None,
        usable: true,
    };
    HashMap::from([
        (
            owner.clone(),
            ScriptPlayerState {
                lifes: 50,
                bid: Some(1),
                rounds: 1,
                mana: ScriptManaState {
                    current: 5,
                    max: 10,
                },
                cards: vec![card],
                power_cards: vec![power_card.clone()],
            },
        ),
        (
            target.clone(),
            ScriptPlayerState {
                lifes: 45,
                bid: Some(2),
                rounds: 2,
                mana: ScriptManaState {
                    current: 4,
                    max: 10,
                },
                cards: vec![Card::new(Rank::Two, Suit::Golds)],
                power_cards: vec![power_card],
            },
        ),
    ])
}

fn smoke_draw(_: &str, count: usize) -> Result<Vec<ScriptPowerCardState>, String> {
    Ok((0..count)
        .map(|index| ScriptPowerCardState {
            id: format!("drawn-{index}"),
            name: "Drawn card".to_string(),
            description: String::new(),
            mana_cost: 1,
            card_type: crate::models::game::fodinha_power::PowerCardType::Instant,
            image_url: None,
            usable: true,
        })
        .collect())
}

fn smoke_power_card_state() -> ScriptPowerCardState {
    ScriptPowerCardState {
        id: "validator-power-card".to_string(),
        name: "Validator card".to_string(),
        description: "Smoke test card".to_string(),
        mana_cost: 1,
        card_type: crate::models::game::fodinha_power::PowerCardType::Instant,
        image_url: None,
        usable: true,
    }
}

fn smoke_power_input(
    owner: &PlayerId,
    target: Option<&PlayerId>,
    mana_cost: usize,
    event: Option<(PassiveGameEvent, ScriptPowerCardState)>,
) -> PowerScriptInput {
    let (event, card_state) =
        event.map_or((None, None), |(event, state)| (Some(event), Some(state)));
    PowerScriptInput {
        card_id: "validator-card".to_string(),
        mana_cost,
        owner_id: owner.clone(),
        target_player_id: target.cloned(),
        players: smoke_players(owner, target.unwrap()),
        draw_power_cards: Rc::new(smoke_draw),
        event,
        card_state,
        current_trump: Rank::Four,
    }
}

fn smoke_passive_input(
    owner: &PlayerId,
    definition: MercenaryPassiveDefinition,
    event: PassiveGameEvent,
) -> PassiveScriptInput {
    let target = PlayerId(std::sync::Arc::from("validator-target"));
    PassiveScriptInput {
        mercenary_id: MercenaryId(std::sync::Arc::from("validator-mercenary")),
        owner_id: owner.clone(),
        base_life: definition.base_life,
        initial_mana: definition.initial_mana,
        event,
        players: smoke_players(owner, &target),
        draw_power_cards: Rc::new(smoke_draw),
        current_trump: Rank::Four,
    }
}

fn smoke_event_for_handler(
    handler: &str,
    owner: &PlayerId,
    target: &PlayerId,
) -> Result<PassiveGameEvent, mlua::Error> {
    let card = Card::new(Rank::One, Suit::Clubs);
    Ok(match handler {
        "on_match_started" => PassiveGameEvent::MatchStarted,
        "on_bid_placed" => PassiveGameEvent::BidPlaced {
            player_id: owner.clone(),
            bid: 1,
        },
        "on_power_card_played" => PassiveGameEvent::PowerCardPlayed {
            player_id: owner.clone(),
            card_id: "validator-card".into(),
            target_player_id: Some(target.clone()),
        },
        "on_round_start" => PassiveGameEvent::RoundStart,
        "on_turn_played" => PassiveGameEvent::TurnPlayed {
            player_id: owner.clone(),
            card,
        },
        "on_round_ended" => PassiveGameEvent::RoundEnded {
            winner: owner.clone(),
            card,
        },
        "on_set_started" => PassiveGameEvent::SetStarted,
        "on_set_ended" => PassiveGameEvent::SetEnded {
            lost_players: HashMap::from([(target.clone(), 1)]),
            bids: HashMap::from([(owner.clone(), 1), (target.clone(), 2)]),
        },
        _ => {
            return Err(mlua::Error::external(format!(
                "unsupported handler {handler}"
            )));
        }
    })
}

pub fn parse_power_card_script_definition(
    source: &str,
    path: &str,
) -> Result<PowerCardScriptDefinition, mlua::Error> {
    with_script_table(source, path, |table| validate_power_card_table(&table))
}

pub fn parse_mercenary_passive_definition(
    source: &str,
    path: &str,
) -> Result<MercenaryPassiveDefinition, mlua::Error> {
    with_script_table(source, path, |table| validate_passive_table(&table))
}

fn with_script_table<T>(
    source: &str,
    path: &str,
    f: impl FnOnce(mlua::Table) -> Result<T, mlua::Error>,
) -> Result<T, mlua::Error> {
    let lua = create_lua()?;
    let table = lua.load(source).set_name(path).eval()?;

    f(table)
}

fn validate_power_card_table(
    table: &mlua::Table,
) -> Result<PowerCardScriptDefinition, mlua::Error> {
    let effect: Value = table.get("effect")?;
    let card_type = power_card_type_field(table, "type")?;
    let mana_cost = non_negative_integer_field(table, "mana_cost")?;
    let quantity = positive_integer_field(table, "quantity")?;

    if !matches!(effect, Value::Function(_)) {
        return Err(mlua::Error::external("card effect must be a function"));
    }

    let mut event_handlers = Vec::new();
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, _) = pair?;
        let Value::String(key) = key else {
            return Err(mlua::Error::external(
                "card script table can only contain type, mana_cost, quantity, and effect fields",
            ));
        };

        match key.to_str()?.as_ref() {
            "effect" | "type" | "mana_cost" | "quantity" => {}
            key if super::lua_codegen::is_passive_handler(key) => {
                if !matches!(table.get::<Value>(key)?, Value::Function(_)) {
                    return Err(mlua::Error::external(format!(
                        "power card event handler {key} must be a function"
                    )));
                }
                event_handlers.push(key.to_string());
            }
            _ => {
                return Err(mlua::Error::external(
                    "card script table can only contain type, mana_cost, quantity, and effect fields; supported event handler fields are also allowed",
                ));
            }
        }
    }

    Ok(PowerCardScriptDefinition {
        mana_cost,
        card_type,
        quantity,
        event_handlers,
    })
}

fn validate_passive_table(table: &mlua::Table) -> Result<MercenaryPassiveDefinition, mlua::Error> {
    let base_life = positive_integer_field(table, "base_life")?;
    let initial_mana = non_negative_integer_field(table, "initial_mana")?;

    for pair in table.clone().pairs::<Value, Value>() {
        let (key, value) = pair?;
        let Value::String(key) = key else {
            return Err(mlua::Error::external(
                "passive script table can only contain metadata and event handler fields",
            ));
        };
        let key = key.to_str()?;

        if matches!(key.as_ref(), "base_life" | "initial_mana") {
            continue;
        }

        if !super::lua_codegen::is_passive_handler(key.as_ref()) {
            return Err(mlua::Error::external(format!(
                "unsupported passive event handler: {key}"
            )));
        }

        if !matches!(value, Value::Function(_)) {
            return Err(mlua::Error::external(format!(
                "passive event handler {key} must be a function"
            )));
        }
    }

    Ok(MercenaryPassiveDefinition {
        base_life,
        initial_mana,
    })
}

pub fn run_power_card_script(
    script: &str,
    input: PowerScriptInput,
) -> Result<PowerScriptOutput, PowerScriptError> {
    let players = shared_players(&input.players);
    let deck_reveals = Rc::new(std::cell::RefCell::new(Vec::new()));
    let lua = create_lua()?;
    let game = api::build_game_api(
        Rc::clone(&players),
        input.draw_power_cards.clone(),
        Rc::clone(&deck_reveals),
        input.current_trump,
    );
    let card = api::build_power_card(&input);

    if let Value::Table(table) = lua.load(script).set_name("power_card").eval()? {
        if let Some(event) = input.event.as_ref() {
            let handler: Value = table.get(event.handler_name())?;
            if let Value::Function(handler) = handler {
                let state = input.card_state.as_ref().ok_or_else(|| {
                    mlua::Error::external("power card event handler requires card state")
                })?;
                let card_state = api::LuaPowerCardState::with_context(
                    state,
                    Rc::clone(&players),
                    input.owner_id.as_str(),
                );
                let event_table = api::build_event_table(&lua, event)?;
                handler.call::<()>((game, event_table, card_state))?;
            }
        } else {
            let effect: mlua::Function = table.get("effect")?;
            effect.call::<()>((game, card.clone()))?;
        }
    }

    Ok(output_from_players(
        &input.players,
        &players.borrow(),
        &deck_reveals.borrow(),
        Some(card.mana_cost()),
    ))
}

pub fn run_passive_script(
    script: &str,
    input: PassiveScriptInput,
) -> Result<PowerScriptOutput, PowerScriptError> {
    let players = shared_players(&input.players);
    let deck_reveals = Rc::new(std::cell::RefCell::new(Vec::new()));
    let lua = create_lua()?;
    let game = api::build_game_api(
        Rc::clone(&players),
        input.draw_power_cards.clone(),
        Rc::clone(&deck_reveals),
        input.current_trump,
    );
    let event = api::build_event_table(&lua, &input.event)?;
    let mercenary = api::build_mercenary(&input);

    if let Value::Table(table) = lua.load(script).set_name("mercenary_passive").eval()? {
        let handler: Value = table.get(input.event.handler_name())?;

        if let Value::Function(handler) = handler {
            handler.call::<()>((game, event, mercenary))?;
        }
    }

    Ok(output_from_players(
        &input.players,
        &players.borrow(),
        &deck_reveals.borrow(),
        None,
    ))
}

pub(crate) fn create_lua() -> Result<Lua, mlua::Error> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH,
        LuaOptions::new(),
    )?;

    register_power_card_type_enum(&lua)?;
    set_limits(&lua)?;

    Ok(lua)
}

fn register_power_card_type_enum(lua: &Lua) -> Result<(), mlua::Error> {
    let enum_table = lua.create_table()?;
    let definition = super::lua_codegen::enum_definition("PowerCardType");
    for variant in definition.variants {
        enum_table.set(variant.name, variant.value)?;
    }
    lua.globals().set(definition.name, enum_table)?;

    Ok(())
}

fn power_card_type_field(
    table: &mlua::Table,
    name: &str,
) -> Result<crate::models::game::fodinha_power::PowerCardType, mlua::Error> {
    let value: Value = table.get(name)?;
    let Value::String(value) = value else {
        return Err(mlua::Error::external(format!(
            "{name} must be a PowerCardType value"
        )));
    };

    value.to_str()?.parse().map_err(mlua::Error::external)
}

fn non_negative_integer_field(table: &mlua::Table, name: &str) -> Result<usize, mlua::Error> {
    let value: Value = table.get(name)?;

    match value {
        Value::Integer(value) => usize::try_from(value)
            .map_err(|_| mlua::Error::external(format!("{name} must be a non-negative integer"))),
        Value::Number(value) if value.fract() == 0.0 && value >= 0.0 => {
            usize::try_from(value as i64).map_err(|_| {
                mlua::Error::external(format!("{name} must be a non-negative integer"))
            })
        }
        _ => Err(mlua::Error::external(format!(
            "{name} must be a non-negative integer"
        ))),
    }
}

fn positive_integer_field(table: &mlua::Table, name: &str) -> Result<usize, mlua::Error> {
    let value = non_negative_integer_field(table, name)?;

    if value == 0 {
        return Err(mlua::Error::external(format!(
            "{name} must be greater than zero"
        )));
    }

    Ok(value)
}

fn shared_players(
    players: &HashMap<PlayerId, ScriptPlayerState>,
) -> Rc<RefCell<HashMap<String, ScriptPlayerState>>> {
    Rc::new(RefCell::new(
        players
            .iter()
            .map(|(player_id, state)| (player_id.as_str().to_string(), state.clone()))
            .collect::<HashMap<_, _>>(),
    ))
}

fn output_from_players(
    initial: &HashMap<PlayerId, ScriptPlayerState>,
    players: &HashMap<String, ScriptPlayerState>,
    deck_reveals: &[super::DeckReveal],
    mana_cost: Option<i64>,
) -> PowerScriptOutput {
    let lifes = initial
        .iter()
        .filter_map(|(player_id, initial_state)| {
            let next = players.get(player_id.as_str())?;

            (next.lifes != initial_state.lifes).then(|| (player_id.clone(), next.lifes))
        })
        .collect();

    let mana = initial
        .iter()
        .filter_map(|(player_id, initial_state)| {
            let next = players.get(player_id.as_str())?;

            (next.mana != initial_state.mana).then(|| (player_id.clone(), next.mana.clone()))
        })
        .collect();

    let cards = initial
        .iter()
        .filter_map(|(player_id, initial_state)| {
            let next = players.get(player_id.as_str())?;

            (next.cards != initial_state.cards).then(|| (player_id.clone(), next.cards.clone()))
        })
        .collect();

    let power_cards = initial
        .iter()
        .filter_map(|(player_id, initial_state)| {
            let next = players.get(player_id.as_str())?;

            (next.power_cards != initial_state.power_cards)
                .then(|| (player_id.clone(), next.power_cards.clone()))
        })
        .collect();

    PowerScriptOutput {
        lifes,
        mana,
        cards,
        power_cards,
        deck_reveals: deck_reveals.to_vec(),
        mana_cost,
    }
}

fn set_limits(lua: &Lua) -> Result<(), mlua::Error> {
    lua.set_memory_limit(LUA_MEMORY_LIMIT_BYTES)?;

    let ticks = Rc::new(Cell::new(0_u32));

    lua.set_hook(
        HookTriggers::new().every_nth_instruction(LUA_HOOK_INSTRUCTION_INTERVAL),
        move |_, _| {
            let next = ticks.get().saturating_add(1);
            ticks.set(next);

            if next > LUA_MAX_HOOK_TICKS {
                return Err(mlua::Error::RuntimeError(
                    "lua instruction limit exceeded".to_string(),
                ));
            }

            Ok(VmState::Continue)
        },
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_validation_accepts_generated_templates() {
        validate_power_card_script_execution(
            super::super::lua_codegen::render_power_card_template(),
            "card.lua",
        )
        .unwrap();
        validate_mercenary_passive_script_execution(
            super::super::lua_codegen::render_mercenary_passive_template(),
            "mercenary.lua",
        )
        .unwrap();
    }

    #[test]
    fn execution_validation_reports_effect_and_handler_failures() {
        let effect = r#"return { type = PowerCardType.Instant, mana_cost = 1, quantity = 1,
            effect = function() error('effect boom') end }"#;
        assert!(
            validate_power_card_script_execution(effect, "card.lua")
                .unwrap_err()
                .to_string()
                .contains("effect")
        );

        let handler = r#"return { base_life = 10, initial_mana = 1,
            on_round_start = function() error('handler boom') end }"#;
        assert!(
            validate_mercenary_passive_script_execution(handler, "mercenary.lua")
                .unwrap_err()
                .to_string()
                .contains("on_round_start")
        );
    }

    #[test]
    fn execution_validation_enforces_instruction_limit() {
        let source = r#"return { type = PowerCardType.Instant, mana_cost = 1, quantity = 1,
            effect = function() while true do end end }"#;
        assert!(
            validate_power_card_script_execution(source, "loop.lua")
                .unwrap_err()
                .to_string()
                .contains("instruction limit")
        );
    }
}
