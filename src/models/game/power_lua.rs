use std::{
    cell::{Cell, RefCell},
    cmp::Ordering,
    collections::HashMap,
    rc::Rc,
};

use mlua::{HookTriggers, Lua, LuaOptions, StdLib, Value, VmState};

use crate::models::{Card, Rank, Suit, game::fodinha_power::PowerCardType, id::PlayerId};

const LUA_MEMORY_LIMIT_BYTES: usize = 256 * 1024;
const LUA_HOOK_INSTRUCTION_INTERVAL: u32 = 1_000;
const LUA_MAX_HOOK_TICKS: u32 = 100;

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

#[derive(Debug, Clone)]
pub struct PowerScriptInput {
    pub card_id: String,
    pub mana_cost: usize,
    pub owner_id: PlayerId,
    pub target_player_id: Option<PlayerId>,
    pub players: HashMap<PlayerId, ScriptPlayerState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerScriptOutput {
    pub lifes: HashMap<PlayerId, usize>,
    pub mana: HashMap<PlayerId, ScriptManaState>,
    pub cards: HashMap<PlayerId, Vec<Card>>,
    pub power_cards: HashMap<PlayerId, Vec<ScriptPowerCardState>>,
}

#[derive(Debug, thiserror::Error)]
pub enum PowerScriptError {
    #[error("lua error: {0}")]
    Lua(#[from] mlua::Error),
}

pub fn validate_power_card_script(source: &str, path: &str) -> Result<(), mlua::Error> {
    with_power_card_table(source, path, |table| validate_power_card_table(&table))
}

fn with_power_card_table<T>(
    source: &str,
    path: &str,
    f: impl FnOnce(mlua::Table) -> Result<T, mlua::Error>,
) -> Result<T, mlua::Error> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH,
        LuaOptions::new(),
    )?;

    set_limits(&lua)?;

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

    if let Value::Table(table) = lua.load(script).set_name("power_card").eval()? {
        let effect: mlua::Function = table.get("effect")?;
        effect.call::<()>((game, card))?;
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

    let mana = input
        .players
        .iter()
        .filter_map(|(player_id, initial_state)| {
            let next = players.get(player_id.as_str())?;

            (next.mana != initial_state.mana).then(|| (player_id.clone(), next.mana.clone()))
        })
        .collect();

    let cards = input
        .players
        .iter()
        .filter_map(|(player_id, initial_state)| {
            let next = players.get(player_id.as_str())?;

            (next.cards != initial_state.cards).then(|| (player_id.clone(), next.cards.clone()))
        })
        .collect();

    let power_cards = input
        .players
        .iter()
        .filter_map(|(player_id, initial_state)| {
            let next = players.get(player_id.as_str())?;

            (next.power_cards != initial_state.power_cards)
                .then(|| (player_id.clone(), next.power_cards.clone()))
        })
        .collect();

    Ok(PowerScriptOutput {
        lifes,
        mana,
        cards,
        power_cards,
    })
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

    let add_bids_players = Rc::clone(&players);
    game.set(
        "add_bids",
        lua.create_function(move |_, (player_id, bid_count): (String, i64)| {
            let mut players = add_bids_players.borrow_mut();

            let Some(player) = players.get_mut(&player_id) else {
                return Err(mlua::Error::external(format!(
                    "unknown player_id: {player_id}"
                )));
            };

            if let Some(bid) = player.bid.as_mut() {
                *bid += bid_count as usize;
            };

            Ok(())
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

    let get_mana_players = Rc::clone(&players);
    game.set(
        "get_mana",
        lua.create_function(move |_, player_id: String| {
            let players = get_mana_players.borrow();
            let Some(player) = players.get(&player_id) else {
                return Err(mlua::Error::external(format!(
                    "unknown player_id: {player_id}"
                )));
            };

            Ok(player.mana.current)
        })?,
    )?;

    let get_max_mana_players = Rc::clone(&players);
    let get_max_mana = lua.create_function(move |_, player_id: String| {
        let players = get_max_mana_players.borrow();
        let Some(player) = players.get(&player_id) else {
            return Err(mlua::Error::external(format!(
                "unknown player_id: {player_id}"
            )));
        };

        Ok(player.mana.max)
    })?;
    game.set("get_max_mana", get_max_mana.clone())?;
    game.set("get_mana_pool", get_max_mana)?;

    let add_mana_players = Rc::clone(&players);
    game.set(
        "add_mana",
        lua.create_function(move |_, (player_id, delta): (String, i64)| {
            let mut players = add_mana_players.borrow_mut();
            let Some(player) = players.get_mut(&player_id) else {
                return Err(mlua::Error::external(format!(
                    "unknown player_id: {player_id}"
                )));
            };

            player.mana.current = match delta.cmp(&0) {
                Ordering::Less => player
                    .mana
                    .current
                    .saturating_sub(delta.unsigned_abs() as usize),
                Ordering::Equal => player.mana.current,
                Ordering::Greater => player
                    .mana
                    .current
                    .saturating_add(delta as usize)
                    .min(player.mana.max),
            };

            Ok(player.mana.current)
        })?,
    )?;

    let set_mana_players = Rc::clone(&players);
    game.set(
        "set_mana",
        lua.create_function(move |_, (player_id, mana): (String, i64)| {
            let mut players = set_mana_players.borrow_mut();
            let Some(player) = players.get_mut(&player_id) else {
                return Err(mlua::Error::external(format!(
                    "unknown player_id: {player_id}"
                )));
            };

            player.mana.current = usize::try_from(mana).unwrap_or(0).min(player.mana.max);

            Ok(player.mana.current)
        })?,
    )?;

    let set_max_mana_players = Rc::clone(&players);
    game.set(
        "set_max_mana",
        lua.create_function(move |_, (player_id, mana): (String, i64)| {
            let mut players = set_max_mana_players.borrow_mut();
            let Some(player) = players.get_mut(&player_id) else {
                return Err(mlua::Error::external(format!(
                    "unknown player_id: {player_id}"
                )));
            };

            player.mana.max = usize::try_from(mana).unwrap_or(0);
            player.mana.current = player.mana.current.min(player.mana.max);

            Ok(player.mana.max)
        })?,
    )?;

    let get_cards_players = Rc::clone(&players);
    game.set(
        "get_cards",
        lua.create_function(move |lua, player_id: String| {
            let players = get_cards_players.borrow();
            let Some(player) = players.get(&player_id) else {
                return Err(mlua::Error::external(format!(
                    "unknown player_id: {player_id}"
                )));
            };

            cards_to_lua_table(lua, &player.cards)
        })?,
    )?;

    let switch_cards_players = Rc::clone(&players);
    game.set(
        "switch_cards",
        lua.create_function(
            move |_,
                  (first_player_id, first_card, second_player_id, second_card): (
                String,
                mlua::Table,
                String,
                mlua::Table,
            )| {
                let first_card = card_from_lua_table(&first_card)?;
                let second_card = card_from_lua_table(&second_card)?;

                if first_player_id == second_player_id {
                    return Ok(false);
                }

                let mut players = switch_cards_players.borrow_mut();
                let first_idx = players
                    .get(&first_player_id)
                    .and_then(|player| player.cards.iter().position(|card| card == &first_card))
                    .ok_or_else(|| {
                        mlua::Error::external(format!(
                            "card not found for player_id: {first_player_id}"
                        ))
                    })?;
                let second_idx = players
                    .get(&second_player_id)
                    .and_then(|player| player.cards.iter().position(|card| card == &second_card))
                    .ok_or_else(|| {
                        mlua::Error::external(format!(
                            "card not found for player_id: {second_player_id}"
                        ))
                    })?;

                players
                    .get_mut(&first_player_id)
                    .expect("player was validated above")
                    .cards[first_idx] = second_card;
                players
                    .get_mut(&second_player_id)
                    .expect("player was validated above")
                    .cards[second_idx] = first_card;

                Ok(true)
            },
        )?,
    )?;

    let get_power_cards_players = Rc::clone(&players);
    game.set(
        "get_power_cards",
        lua.create_function(move |lua, player_id: String| {
            let players = get_power_cards_players.borrow();
            let Some(player) = players.get(&player_id) else {
                return Err(mlua::Error::external(format!(
                    "unknown player_id: {player_id}"
                )));
            };

            power_cards_to_lua_table(lua, &player.power_cards)
        })?,
    )?;

    let steal_power_card_players = Rc::clone(&players);
    game.set(
        "steal_power_card",
        lua.create_function(
            move |_, (from_player_id, card_id, to_player_id): (String, String, String)| {
                if from_player_id == to_player_id {
                    return Ok(false);
                }

                let mut players = steal_power_card_players.borrow_mut();
                let card_idx = players
                    .get(&from_player_id)
                    .and_then(|player| {
                        player
                            .power_cards
                            .iter()
                            .position(|card| card.id == card_id)
                    })
                    .ok_or_else(|| {
                        mlua::Error::external(format!(
                            "power card not found for player_id: {from_player_id}"
                        ))
                    })?;

                if !players.contains_key(&to_player_id) {
                    return Err(mlua::Error::external(format!(
                        "unknown player_id: {to_player_id}"
                    )));
                }

                let card = players
                    .get_mut(&from_player_id)
                    .expect("player was validated above")
                    .power_cards
                    .remove(card_idx);
                players
                    .get_mut(&to_player_id)
                    .expect("player was validated above")
                    .power_cards
                    .push(card);

                Ok(true)
            },
        )?,
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

    card.set("id", input.card_id.as_str())?;
    card.set("mana_cost", input.mana_cost)?;
    card.set("owner_id", input.owner_id.as_str())?;
    card.set(
        "target_player_id",
        input.target_player_id.as_ref().map(PlayerId::as_str),
    )?;

    Ok(card)
}

fn cards_to_lua_table(lua: &Lua, cards: &[Card]) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;

    for (idx, card) in cards.iter().enumerate() {
        table.set(idx + 1, card_to_lua_table(lua, *card)?)?;
    }

    Ok(table)
}

fn card_to_lua_table(lua: &Lua, card: Card) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;

    table.set("rank", rank_to_str(card.rank))?;
    table.set("suit", suit_to_str(card.suit))?;

    Ok(table)
}

fn card_from_lua_table(table: &mlua::Table) -> mlua::Result<Card> {
    let rank = parse_rank(&table.get::<String>("rank")?)?;
    let suit = parse_suit(&table.get::<String>("suit")?)?;

    Ok(Card { rank, suit })
}

fn power_cards_to_lua_table(
    lua: &Lua,
    cards: &[ScriptPowerCardState],
) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;

    for (idx, card) in cards.iter().enumerate() {
        let card_table = lua.create_table()?;
        card_table.set("id", card.id.as_str())?;
        card_table.set("name", card.name.as_str())?;
        card_table.set("description", card.description.as_str())?;
        card_table.set("mana_cost", card.mana_cost)?;
        card_table.set("type", card.card_type.as_str())?;
        card_table.set("image_url", card.image_url.as_deref())?;
        table.set(idx + 1, card_table)?;
    }

    Ok(table)
}

fn rank_to_str(rank: Rank) -> &'static str {
    match rank {
        Rank::Four => "Four",
        Rank::Five => "Five",
        Rank::Six => "Six",
        Rank::Seven => "Seven",
        Rank::Ten => "Ten",
        Rank::Eleven => "Eleven",
        Rank::Twelve => "Twelve",
        Rank::One => "One",
        Rank::Two => "Two",
        Rank::Three => "Three",
    }
}

fn parse_rank(value: &str) -> mlua::Result<Rank> {
    match value {
        "Four" => Ok(Rank::Four),
        "Five" => Ok(Rank::Five),
        "Six" => Ok(Rank::Six),
        "Seven" => Ok(Rank::Seven),
        "Ten" => Ok(Rank::Ten),
        "Eleven" => Ok(Rank::Eleven),
        "Twelve" => Ok(Rank::Twelve),
        "One" => Ok(Rank::One),
        "Two" => Ok(Rank::Two),
        "Three" => Ok(Rank::Three),
        _ => Err(mlua::Error::external(format!("invalid rank: {value}"))),
    }
}

fn suit_to_str(suit: Suit) -> &'static str {
    match suit {
        Suit::Golds => "Golds",
        Suit::Swords => "Swords",
        Suit::Cups => "Cups",
        Suit::Clubs => "Clubs",
    }
}

fn parse_suit(value: &str) -> mlua::Result<Suit> {
    match value {
        "Golds" => Ok(Suit::Golds),
        "Swords" => Ok(Suit::Swords),
        "Cups" => Ok(Suit::Cups),
        "Clubs" => Ok(Suit::Clubs),
        _ => Err(mlua::Error::external(format!("invalid suit: {value}"))),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

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
        }
    }

    #[test]
    fn script_can_change_lives_through_limited_api() {
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let output = run_power_card_script(
            r#"
            return {
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
    fn card_script_rejects_metadata_fields() {
        let player = PlayerId(Arc::from("P1"));
        let source = r#"
            return {
                name = "Heal 10",
                effect = function(game, card)
                    game.add_lives(card.owner_id, 10)
                end,
            }
        "#;

        let error = validate_power_card_script(source, "test.lua").unwrap_err();
        assert!(error.to_string().contains("only contain the effect field"));

        let output = run_power_card_script(
            r#"
            return {
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
