use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use mlua::{HookTriggers, Lua, LuaOptions, StdLib, Value, VmState};
use power_lua_api::metadata;

use crate::models::id::PlayerId;

use super::{
    PassiveScriptInput, PowerScriptError, PowerScriptInput, PowerScriptOutput, ScriptPlayerState,
    api,
};

const LUA_MEMORY_LIMIT_BYTES: usize = 256 * 1024;
const LUA_HOOK_INSTRUCTION_INTERVAL: u32 = 1_000;
const LUA_MAX_HOOK_TICKS: u32 = 100;

pub fn validate_power_card_script(source: &str, path: &str) -> Result<(), mlua::Error> {
    with_script_table(source, path, |table| validate_power_card_table(&table))
}

pub fn validate_mercenary_passive_script(source: &str, path: &str) -> Result<(), mlua::Error> {
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

fn validate_power_card_table(table: &mlua::Table) -> Result<(), mlua::Error> {
    let effect: Value = table.get("effect")?;

    if !matches!(effect, Value::Function(_)) {
        return Err(mlua::Error::external("card effect must be a function"));
    }

    for pair in table.clone().pairs::<Value, Value>() {
        let (key, _) = pair?;
        let Value::String(key) = key else {
            return Err(mlua::Error::external(
                "card script table can only contain the effect field",
            ));
        };

        if key.to_str()? != "effect" {
            return Err(mlua::Error::external(
                "card script table can only contain the effect field",
            ));
        }
    }

    Ok(())
}

fn validate_passive_table(table: &mlua::Table) -> Result<(), mlua::Error> {
    let mut has_handler = false;

    for pair in table.clone().pairs::<Value, Value>() {
        let (key, value) = pair?;
        let Value::String(key) = key else {
            return Err(mlua::Error::external(
                "passive script table can only contain event handler fields",
            ));
        };
        let key = key.to_str()?;

        if !metadata::PASSIVE_EVENT_HANDLERS.contains(&key.as_ref()) {
            return Err(mlua::Error::external(format!(
                "unsupported passive event handler: {key}"
            )));
        }

        if !matches!(value, Value::Function(_)) {
            return Err(mlua::Error::external(format!(
                "passive event handler {key} must be a function"
            )));
        }

        has_handler = true;
    }

    if !has_handler {
        return Err(mlua::Error::external(
            "passive script must define at least one event handler",
        ));
    }

    Ok(())
}

pub fn run_power_card_script(
    script: &str,
    input: PowerScriptInput,
) -> Result<PowerScriptOutput, PowerScriptError> {
    let players = shared_players(&input.players);
    let lua = create_lua()?;
    let globals = lua.globals();
    let game = api::build_game_api(Rc::clone(&players));
    let card = api::build_power_card(&input);

    globals.set("game", game.clone())?;
    globals.set("card", card.clone())?;

    if let Value::Table(table) = lua.load(script).set_name("power_card").eval()? {
        let effect: mlua::Function = table.get("effect")?;
        effect.call::<()>((game, card))?;
    }

    Ok(output_from_players(&input.players, &players.borrow()))
}

pub fn run_passive_script(
    script: &str,
    input: PassiveScriptInput,
) -> Result<PowerScriptOutput, PowerScriptError> {
    let players = shared_players(&input.players);
    let lua = create_lua()?;
    let globals = lua.globals();
    let game = api::build_game_api(Rc::clone(&players));
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

    Ok(output_from_players(&input.players, &players.borrow()))
}

pub(crate) fn create_lua() -> Result<Lua, mlua::Error> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH,
        LuaOptions::new(),
    )?;

    set_limits(&lua)?;

    Ok(lua)
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
