use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use mlua::{HookTriggers, Lua, LuaOptions, StdLib, Value, VmState};
use power_lua_api::metadata;

use crate::models::id::PlayerId;

use super::{
    MercenaryPassiveDefinition, PassiveScriptInput, PowerCardScriptDefinition, PowerScriptError,
    PowerScriptInput, PowerScriptOutput, ScriptPlayerState,
    api,
};

const LUA_MEMORY_LIMIT_BYTES: usize = 256 * 1024;
const LUA_HOOK_INSTRUCTION_INTERVAL: u32 = 1_000;
const LUA_MAX_HOOK_TICKS: u32 = 100;

pub fn validate_power_card_script(source: &str, path: &str) -> Result<(), mlua::Error> {
    parse_power_card_script_definition(source, path).map(|_| ())
}

pub fn validate_mercenary_passive_script(source: &str, path: &str) -> Result<(), mlua::Error> {
    parse_mercenary_passive_definition(source, path).map(|_| ())
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

fn validate_power_card_table(table: &mlua::Table) -> Result<PowerCardScriptDefinition, mlua::Error> {
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
            key if metadata::PASSIVE_EVENT_HANDLERS.contains(&key) => {
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

        if !metadata::PASSIVE_EVENT_HANDLERS.contains(&key.as_ref()) {
            return Err(mlua::Error::external(format!(
                "unsupported passive event handler: {key}"
            )));
        }

        if !matches!(value, Value::Function(_)) {
            return Err(mlua::Error::external(
                format!("passive event handler {key} must be a function"),
            ));
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
    let lua = create_lua()?;
    let globals = lua.globals();
    let game = api::build_game_api(Rc::clone(&players), input.draw_power_cards.clone());
    let card = api::build_power_card(&input);

    globals.set("game", game.clone())?;
    globals.set("card", card.clone())?;

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
        Some(card.mana_cost()),
    ))
}

pub fn run_passive_script(
    script: &str,
    input: PassiveScriptInput,
) -> Result<PowerScriptOutput, PowerScriptError> {
    let players = shared_players(&input.players);
    let lua = create_lua()?;
    let globals = lua.globals();
    let game = api::build_game_api(Rc::clone(&players), input.draw_power_cards.clone());
    let event = api::build_event_table(&lua, &input.event)?;
    let mercenary = api::build_mercenary(&input);

    globals.set("game", game.clone())?;
    globals.set("event", event.clone())?;
    globals.set("mercenary", mercenary.clone())?;

    if let Value::Table(table) = lua.load(script).set_name("mercenary_passive").eval()? {
        let handler: Value = table.get(input.event.handler_name())?;

        if let Value::Function(handler) = handler {
            handler.call::<()>((game, event, mercenary))?;
        }
    }

    Ok(output_from_players(&input.players, &players.borrow(), None))
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
    enum_table.set("Instant", metadata::POWER_CARD_TYPE_VALUES[0])?;
    enum_table.set("Targetable", metadata::POWER_CARD_TYPE_VALUES[1])?;
    enum_table.set("Interactive", metadata::POWER_CARD_TYPE_VALUES[2])?;
    lua.globals().set("PowerCardType", enum_table)?;

    Ok(())
}

fn power_card_type_field(table: &mlua::Table, name: &str) -> Result<crate::models::game::fodinha_power::PowerCardType, mlua::Error> {
    let value: Value = table.get(name)?;
    let Value::String(value) = value else {
        return Err(mlua::Error::external(format!("{name} must be a PowerCardType value")));
    };

    value
        .to_str()?
        .parse()
        .map_err(mlua::Error::external)
}

fn non_negative_integer_field(table: &mlua::Table, name: &str) -> Result<usize, mlua::Error> {
    let value: Value = table.get(name)?;

    match value {
        Value::Integer(value) => usize::try_from(value)
            .map_err(|_| mlua::Error::external(format!("{name} must be a non-negative integer"))),
        Value::Number(value) if value.fract() == 0.0 && value >= 0.0 => {
            usize::try_from(value as i64)
                .map_err(|_| mlua::Error::external(format!("{name} must be a non-negative integer")))
        }
        _ => Err(mlua::Error::external(format!("{name} must be a non-negative integer"))),
    }
}

fn positive_integer_field(table: &mlua::Table, name: &str) -> Result<usize, mlua::Error> {
    let value = non_negative_integer_field(table, name)?;

    if value == 0 {
        return Err(mlua::Error::external(format!("{name} must be greater than zero")));
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
