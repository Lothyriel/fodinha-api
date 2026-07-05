use std::{
    cell::{Cell, RefCell},
    cmp::Ordering,
    collections::HashMap,
    rc::Rc,
};

use mlua::{HookTriggers, Lua, LuaOptions, StdLib, Value, VmState};

use crate::models::id::PlayerId;

const LUA_MEMORY_LIMIT_BYTES: usize = 256 * 1024;
const LUA_HOOK_INSTRUCTION_INTERVAL: u32 = 1_000;
const LUA_MAX_HOOK_TICKS: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPlayerState {
    pub lifes: usize,
    pub bid: Option<usize>,
    pub rounds: usize,
}

#[derive(Debug, Clone)]
pub struct PowerScriptInput {
    pub owner_id: PlayerId,
    pub target_player_id: Option<PlayerId>,
    pub players: HashMap<PlayerId, ScriptPlayerState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerScriptOutput {
    pub lifes: HashMap<PlayerId, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerCardMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
    pub requires_target: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PowerScriptError {
    #[error("lua error: {0}")]
    Lua(#[from] mlua::Error),
}

pub fn load_power_card_metadata(
    source: &str,
    path: &str,
) -> Result<PowerCardMetadata, mlua::Error> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH,
        LuaOptions::new(),
    )?;

    set_limits(&lua)?;

    let table: mlua::Table = lua.load(source).set_name(path).eval()?;
    let effect: Value = table.get("effect")?;

    if !matches!(effect, Value::Function(_)) {
        return Err(mlua::Error::external("card effect must be a function"));
    }

    let metadata = PowerCardMetadata {
        id: table.get("id")?,
        name: table.get("name")?,
        description: table.get("description")?,
        requires_target: table.get("requires_target")?,
    };

    if metadata.id.trim().is_empty() {
        return Err(mlua::Error::external("card id cannot be empty"));
    }

    if metadata.name.trim().is_empty() {
        return Err(mlua::Error::external("card name cannot be empty"));
    }

    Ok(metadata)
}

pub fn run_power_card_script(
    script: &str,
    input: PowerScriptInput,
) -> Result<PowerScriptOutput, PowerScriptError> {
    let players = Rc::new(RefCell::new(
        input
            .players
            .iter()
            .map(|(player_id, state)| (player_id.as_str().to_string(), state.clone()))
            .collect::<HashMap<_, _>>(),
    ));

    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH,
        LuaOptions::new(),
    )?;

    set_limits(&lua)?;

    let globals = lua.globals();
    let game = build_game_api(&lua, Rc::clone(&players))?;
    let card = build_card_table(&lua, &input)?;

    globals.set("game", game.clone())?;
    globals.set("card", card.clone())?;

    match lua.load(script).set_name("power_card").eval()? {
        Value::Table(table) => {
            let effect: mlua::Function = table.get("effect")?;
            effect.call::<()>((game, card))?;
        }
        _ => {}
    }

    let players = players.borrow();
    let lifes = input
        .players
        .iter()
        .filter_map(|(player_id, initial_state)| {
            let next = players.get(player_id.as_str())?;

            (next.lifes != initial_state.lifes).then(|| (player_id.clone(), next.lifes))
        })
        .collect();

    Ok(PowerScriptOutput { lifes })
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

fn build_game_api(
    lua: &Lua,
    players: Rc<RefCell<HashMap<String, ScriptPlayerState>>>,
) -> mlua::Result<mlua::Table> {
    let game = lua.create_table()?;

    let get_lives_players = Rc::clone(&players);
    game.set(
        "get_lives",
        lua.create_function(move |_, player_id: String| {
            let players = get_lives_players.borrow();
            let Some(player) = players.get(&player_id) else {
                return Err(mlua::Error::external(format!(
                    "unknown player_id: {player_id}"
                )));
            };

            Ok(player.lifes)
        })?,
    )?;

    let add_lives_players = Rc::clone(&players);
    game.set(
        "add_lives",
        lua.create_function(move |_, (player_id, delta): (String, i64)| {
            let mut players = add_lives_players.borrow_mut();
            let Some(player) = players.get_mut(&player_id) else {
                return Err(mlua::Error::external(format!(
                    "unknown player_id: {player_id}"
                )));
            };

            player.lifes = match delta.cmp(&0) {
                Ordering::Less => player.lifes.saturating_sub(delta.unsigned_abs() as usize),
                Ordering::Equal => player.lifes,
                Ordering::Greater => player.lifes.saturating_add(delta as usize),
            };

            Ok(player.lifes)
        })?,
    )?;

    let set_lives_players = Rc::clone(&players);
    game.set(
        "set_lives",
        lua.create_function(move |_, (player_id, lifes): (String, i64)| {
            let mut players = set_lives_players.borrow_mut();
            let Some(player) = players.get_mut(&player_id) else {
                return Err(mlua::Error::external(format!(
                    "unknown player_id: {player_id}"
                )));
            };

            player.lifes = usize::try_from(lifes).unwrap_or(0);

            Ok(player.lifes)
        })?,
    )?;

    let get_bid_players = Rc::clone(&players);
    game.set(
        "get_bid",
        lua.create_function(move |_, player_id: String| {
            let players = get_bid_players.borrow();
            let Some(player) = players.get(&player_id) else {
                return Err(mlua::Error::external(format!(
                    "unknown player_id: {player_id}"
                )));
            };

            Ok(player.bid)
        })?,
    )?;

    let get_rounds_players = Rc::clone(&players);
    game.set(
        "get_rounds",
        lua.create_function(move |_, player_id: String| {
            let players = get_rounds_players.borrow();
            let Some(player) = players.get(&player_id) else {
                return Err(mlua::Error::external(format!(
                    "unknown player_id: {player_id}"
                )));
            };

            Ok(player.rounds)
        })?,
    )?;

    let player_ids_players = Rc::clone(&players);
    game.set(
        "player_ids",
        lua.create_function(move |lua, ()| {
            let ids = lua.create_table()?;

            for (idx, player_id) in player_ids_players.borrow().keys().enumerate() {
                ids.set(idx + 1, player_id.as_str())?;
            }

            Ok(ids)
        })?,
    )?;

    Ok(game)
}

fn build_card_table(lua: &Lua, input: &PowerScriptInput) -> mlua::Result<mlua::Table> {
    let card = lua.create_table()?;

    card.set("owner_id", input.owner_id.as_str())?;
    card.set(
        "target_player_id",
        input.target_player_id.as_ref().map(PlayerId::as_str),
    )?;

    Ok(card)
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use crate::models::id::PlayerId;

    use super::*;

    #[test]
    fn script_can_change_lives_through_limited_api() {
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let output = run_power_card_script(
            "game.add_lives(card.target_player_id, -10)",
            PowerScriptInput {
                owner_id: player1.clone(),
                target_player_id: Some(player2.clone()),
                players: HashMap::from([
                    (
                        player1,
                        ScriptPlayerState {
                            lifes: 50,
                            bid: None,
                            rounds: 0,
                        },
                    ),
                    (
                        player2.clone(),
                        ScriptPlayerState {
                            lifes: 50,
                            bid: None,
                            rounds: 0,
                        },
                    ),
                ]),
            },
        )
        .unwrap();

        assert_eq!(output.lifes.get(&player2), Some(&40));
    }

    #[test]
    fn card_file_metadata_and_effect_can_be_loaded() {
        let player = PlayerId(Arc::from("P1"));
        let source = r#"
            return {
                id = "heal_10",
                name = "Heal 10",
                description = "Restore 10 lives to yourself.",
                requires_target = false,
                effect = function(game, card)
                    game.add_lives(card.owner_id, 10)
                end,
            }
        "#;

        let metadata = load_power_card_metadata(source, "test.lua").unwrap();
        let output = run_power_card_script(
            source,
            PowerScriptInput {
                owner_id: player.clone(),
                target_player_id: None,
                players: HashMap::from([(
                    player.clone(),
                    ScriptPlayerState {
                        lifes: 50,
                        bid: None,
                        rounds: 0,
                    },
                )]),
            },
        )
        .unwrap();

        assert_eq!(metadata.id, "heal_10");
        assert_eq!(output.lifes.get(&player), Some(&60));
    }

    #[test]
    fn script_can_set_lives_and_add_large_deltas() {
        let player = PlayerId(Arc::from("P1"));
        let output = run_power_card_script(
            "game.add_lives(card.owner_id, 1000); game.set_lives(card.owner_id, 777)",
            PowerScriptInput {
                owner_id: player.clone(),
                target_player_id: None,
                players: HashMap::from([(
                    player.clone(),
                    ScriptPlayerState {
                        lifes: 50,
                        bid: None,
                        rounds: 0,
                    },
                )]),
            },
        )
        .unwrap();

        assert_eq!(output.lifes.get(&player), Some(&777));
    }

    #[test]
    fn script_cannot_open_unsafe_standard_libraries() {
        let player = PlayerId(Arc::from("P1"));
        let error = run_power_card_script(
            "os.execute('true')",
            PowerScriptInput {
                owner_id: player.clone(),
                target_player_id: None,
                players: HashMap::from([(
                    player,
                    ScriptPlayerState {
                        lifes: 50,
                        bid: None,
                        rounds: 0,
                    },
                )]),
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("os"));
    }

    #[test]
    fn script_instruction_limit_stops_infinite_loop() {
        let player = PlayerId(Arc::from("P1"));
        let error = run_power_card_script(
            "while true do end",
            PowerScriptInput {
                owner_id: player.clone(),
                target_player_id: None,
                players: HashMap::from([(
                    player,
                    ScriptPlayerState {
                        lifes: 50,
                        bid: None,
                        rounds: 0,
                    },
                )]),
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("instruction limit"));
    }
}
