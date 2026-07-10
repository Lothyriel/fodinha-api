use std::{
    cell::{Cell, RefCell},
    cmp::Ordering,
    collections::HashMap,
    rc::Rc,
};

use mlua::{IntoLuaMulti, Lua, Table, UserData, UserDataFields, UserDataMethods, Value};
use power_lua_api::{LuaTypeDefinition, metadata};

use crate::models::{Card, Rank, Suit, game::fodinha_power::PowerCardType, id::PlayerId};

use super::{
    DrawPowerCardsFn, PassiveGameEvent, PassiveScriptInput, PowerScriptInput, ScriptPlayerState,
    ScriptPowerCardState,
};

pub trait LuaApiType {
    const DEFINITION: LuaTypeDefinition;
}

#[derive(Clone)]
pub struct LuaGame {
    players: Rc<RefCell<HashMap<String, ScriptPlayerState>>>,
    draw_power_cards: DrawPowerCardsFn,
    current_trump: Rank,
}

impl LuaGame {
    pub(crate) fn new(
        players: Rc<RefCell<HashMap<String, ScriptPlayerState>>>,
        draw_power_cards: DrawPowerCardsFn,
        current_trump: Rank,
    ) -> Self {
        Self {
            players,
            draw_power_cards,
            current_trump,
        }
    }

    fn get_lives(&self, player_id: &str) -> mlua::Result<usize> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        Ok(player.lifes)
    }

    fn get_current_trump(&self) -> &'static str {
        rank_to_str(self.current_trump)
    }

    fn add_lives(&self, player_id: &str, delta: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id) else {
            return Err(unknown_player(player_id));
        };

        player.lifes = match delta.cmp(&0) {
            Ordering::Less => player.lifes.saturating_sub(delta.unsigned_abs() as usize),
            Ordering::Equal => player.lifes,
            Ordering::Greater => player.lifes.saturating_add(delta as usize),
        };

        Ok(player.lifes)
    }

    fn set_lives(&self, player_id: &str, lifes: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id) else {
            return Err(unknown_player(player_id));
        };

        player.lifes = usize::try_from(lifes).unwrap_or(0);

        Ok(player.lifes)
    }

    fn get_bid(&self, player_id: &str) -> mlua::Result<Option<usize>> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        Ok(player.bid)
    }

    fn add_bids(&self, player_id: &str, bid_count: i64) -> mlua::Result<()> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id) else {
            return Err(unknown_player(player_id));
        };

        if let Some(bid) = player.bid.as_mut() {
            *bid += bid_count as usize;
        }

        Ok(())
    }

    fn get_rounds(&self, player_id: &str) -> mlua::Result<usize> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        Ok(player.rounds)
    }

    fn get_mana(&self, player_id: &str) -> mlua::Result<usize> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        Ok(player.mana.current)
    }

    fn get_max_mana(&self, player_id: &str) -> mlua::Result<usize> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        Ok(player.mana.max)
    }

    fn add_mana(&self, player_id: &str, delta: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id) else {
            return Err(unknown_player(player_id));
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
    }

    fn set_mana(&self, player_id: &str, mana: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id) else {
            return Err(unknown_player(player_id));
        };

        player.mana.current = usize::try_from(mana).unwrap_or(0).min(player.mana.max);

        Ok(player.mana.current)
    }

    fn set_max_mana(&self, player_id: &str, mana: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id) else {
            return Err(unknown_player(player_id));
        };

        player.mana.max = usize::try_from(mana).unwrap_or(0);
        player.mana.current = player.mana.current.min(player.mana.max);

        Ok(player.mana.max)
    }

    fn get_cards(&self, lua: &Lua, player_id: &str) -> mlua::Result<Table> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        cards_to_lua_table(lua, &player.cards)
    }

    fn switch_cards(
        &self,
        first_player_id: &str,
        first_card: &Value,
        second_player_id: &str,
        second_card: &Value,
    ) -> mlua::Result<bool> {
        let first_card = card_from_lua_value(first_card)?;
        let second_card = card_from_lua_value(second_card)?;

        if first_player_id == second_player_id {
            return Ok(false);
        }

        let mut players = self.players.borrow_mut();
        let first_idx = players
            .get(first_player_id)
            .and_then(|player| player.cards.iter().position(|card| card == &first_card))
            .ok_or_else(|| {
                mlua::Error::external(format!("card not found for player_id: {first_player_id}"))
            })?;
        let second_idx = players
            .get(second_player_id)
            .and_then(|player| player.cards.iter().position(|card| card == &second_card))
            .ok_or_else(|| {
                mlua::Error::external(format!("card not found for player_id: {second_player_id}"))
            })?;

        players
            .get_mut(first_player_id)
            .expect("player was validated above")
            .cards[first_idx] = second_card;
        players
            .get_mut(second_player_id)
            .expect("player was validated above")
            .cards[second_idx] = first_card;

        Ok(true)
    }

    fn get_power_cards(&self, lua: &Lua, player_id: &str) -> mlua::Result<Table> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        power_cards_to_lua_table(lua, &player.power_cards, Rc::clone(&self.players), player_id)
    }

    fn steal_power_card(
        &self,
        from_player_id: &str,
        card_id: &str,
        to_player_id: &str,
    ) -> mlua::Result<bool> {
        if from_player_id == to_player_id {
            return Ok(false);
        }

        let mut players = self.players.borrow_mut();
        let card_idx = players
            .get(from_player_id)
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

        if !players.contains_key(to_player_id) {
            return Err(unknown_player(to_player_id));
        }

        let card = players
            .get_mut(from_player_id)
            .expect("player was validated above")
            .power_cards
            .remove(card_idx);
        players
            .get_mut(to_player_id)
            .expect("player was validated above")
            .power_cards
            .push(card);

        Ok(true)
    }

    fn draw_power_cards(&self, lua: &Lua, player_id: &str, count: i64) -> mlua::Result<Table> {
        let count = usize_count(count, "count")?;
        if !self.players.borrow().contains_key(player_id) {
            return Err(unknown_player(player_id));
        }

        if count == 0 {
            return power_cards_to_lua_table(lua, &[], Rc::clone(&self.players), player_id);
        }

        let drawn = (self.draw_power_cards)(player_id, count).map_err(mlua::Error::external)?;
        let table = power_cards_to_lua_table(lua, &drawn, Rc::clone(&self.players), player_id)?;

        self.players
            .borrow_mut()
            .get_mut(player_id)
            .expect("player was validated above")
            .power_cards
            .extend(drawn);

        Ok(table)
    }

    fn player_ids(&self, lua: &Lua) -> mlua::Result<Table> {
        let ids = lua.create_table()?;

        for (idx, player_id) in self.players.borrow().keys().enumerate() {
            ids.set(idx + 1, player_id.as_str())?;
        }

        Ok(ids)
    }
}

impl UserData for LuaGame {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(metadata::GET_LIVES, |_, this, player_id: String| {
            this.get_lives(&player_id)
        });
        methods.add_method(metadata::GET_CURRENT_TRUMP, |_, this, ()| {
            Ok(this.get_current_trump())
        });
        methods.add_method(
            metadata::ADD_LIVES,
            |_, this, (player_id, delta): (String, i64)| this.add_lives(&player_id, delta),
        );
        methods.add_method(
            metadata::SET_LIVES,
            |_, this, (player_id, lifes): (String, i64)| this.set_lives(&player_id, lifes),
        );
        methods.add_method(metadata::GET_BID, |_, this, player_id: String| {
            this.get_bid(&player_id)
        });
        methods.add_method(
            metadata::ADD_BIDS,
            |_, this, (player_id, bid_count): (String, i64)| this.add_bids(&player_id, bid_count),
        );
        methods.add_method(metadata::GET_ROUNDS, |_, this, player_id: String| {
            this.get_rounds(&player_id)
        });
        methods.add_method(metadata::GET_MANA, |_, this, player_id: String| {
            this.get_mana(&player_id)
        });
        methods.add_method(metadata::GET_MAX_MANA, |_, this, player_id: String| {
            this.get_max_mana(&player_id)
        });
        methods.add_method(metadata::GET_MANA_POOL, |_, this, player_id: String| {
            this.get_max_mana(&player_id)
        });
        methods.add_method(
            metadata::ADD_MANA,
            |_, this, (player_id, delta): (String, i64)| this.add_mana(&player_id, delta),
        );
        methods.add_method(
            metadata::SET_MANA,
            |_, this, (player_id, mana): (String, i64)| this.set_mana(&player_id, mana),
        );
        methods.add_method(
            metadata::SET_MAX_MANA,
            |_, this, (player_id, mana): (String, i64)| this.set_max_mana(&player_id, mana),
        );
        methods.add_method(metadata::GET_CARDS, |lua, this, player_id: String| {
            this.get_cards(lua, &player_id)
        });
        methods.add_method(
            metadata::SWITCH_CARDS,
            |_,
             this,
             (first_player_id, first_card, second_player_id, second_card): (
                String,
                Value,
                String,
                Value,
            )| {
                this.switch_cards(
                    &first_player_id,
                    &first_card,
                    &second_player_id,
                    &second_card,
                )
            },
        );
        methods.add_method(metadata::GET_POWER_CARDS, |lua, this, player_id: String| {
            this.get_power_cards(lua, &player_id)
        });
        methods.add_method(
            metadata::STEAL_POWER_CARD,
            |_, this, (from_player_id, card_id, to_player_id): (String, String, String)| {
                this.steal_power_card(&from_player_id, &card_id, &to_player_id)
            },
        );
        methods.add_method(
            metadata::DRAW_POWER_CARDS,
            |lua, this, (player_id, count): (String, i64)| {
                this.draw_power_cards(lua, &player_id, count)
            },
        );
        methods.add_method(metadata::PLAYER_IDS, |lua, this, ()| this.player_ids(lua));
    }

    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        add_game_function_field(fields, metadata::GET_LIVES, 1, |_, this, args| {
            this.get_lives(&string_arg(&args, 0, "player_id")?)
        });
        add_game_function_field(fields, metadata::GET_CURRENT_TRUMP, 0, |_, this, _| {
            Ok(this.get_current_trump())
        });
        add_game_function_field(fields, metadata::ADD_LIVES, 2, |_, this, args| {
            this.add_lives(
                &string_arg(&args, 0, "player_id")?,
                i64_arg(&args, 1, "delta")?,
            )
        });
        add_game_function_field(fields, metadata::SET_LIVES, 2, |_, this, args| {
            this.set_lives(
                &string_arg(&args, 0, "player_id")?,
                i64_arg(&args, 1, "lifes")?,
            )
        });
        add_game_function_field(fields, metadata::GET_BID, 1, |_, this, args| {
            this.get_bid(&string_arg(&args, 0, "player_id")?)
        });
        add_game_function_field(fields, metadata::ADD_BIDS, 2, |_, this, args| {
            this.add_bids(
                &string_arg(&args, 0, "player_id")?,
                i64_arg(&args, 1, "bid_count")?,
            )
        });
        add_game_function_field(fields, metadata::GET_ROUNDS, 1, |_, this, args| {
            this.get_rounds(&string_arg(&args, 0, "player_id")?)
        });
        add_game_function_field(fields, metadata::GET_MANA, 1, |_, this, args| {
            this.get_mana(&string_arg(&args, 0, "player_id")?)
        });
        add_game_function_field(fields, metadata::GET_MAX_MANA, 1, |_, this, args| {
            this.get_max_mana(&string_arg(&args, 0, "player_id")?)
        });
        add_game_function_field(fields, metadata::GET_MANA_POOL, 1, |_, this, args| {
            this.get_max_mana(&string_arg(&args, 0, "player_id")?)
        });
        add_game_function_field(fields, metadata::ADD_MANA, 2, |_, this, args| {
            this.add_mana(
                &string_arg(&args, 0, "player_id")?,
                i64_arg(&args, 1, "delta")?,
            )
        });
        add_game_function_field(fields, metadata::SET_MANA, 2, |_, this, args| {
            this.set_mana(
                &string_arg(&args, 0, "player_id")?,
                i64_arg(&args, 1, "mana")?,
            )
        });
        add_game_function_field(fields, metadata::SET_MAX_MANA, 2, |_, this, args| {
            this.set_max_mana(
                &string_arg(&args, 0, "player_id")?,
                i64_arg(&args, 1, "mana")?,
            )
        });
        add_game_function_field(fields, metadata::GET_CARDS, 1, |lua, this, args| {
            this.get_cards(lua, &string_arg(&args, 0, "player_id")?)
        });
        add_game_function_field(fields, metadata::SWITCH_CARDS, 4, |_, this, args| {
            this.switch_cards(
                &string_arg(&args, 0, "first_player_id")?,
                &args[1],
                &string_arg(&args, 2, "second_player_id")?,
                &args[3],
            )
        });
        add_game_function_field(fields, metadata::GET_POWER_CARDS, 1, |lua, this, args| {
            this.get_power_cards(lua, &string_arg(&args, 0, "player_id")?)
        });
        add_game_function_field(fields, metadata::STEAL_POWER_CARD, 3, |_, this, args| {
            this.steal_power_card(
                &string_arg(&args, 0, "from_player_id")?,
                &string_arg(&args, 1, "card_id")?,
                &string_arg(&args, 2, "to_player_id")?,
            )
        });
        add_game_function_field(fields, metadata::DRAW_POWER_CARDS, 2, |lua, this, args| {
            this.draw_power_cards(
                lua,
                &string_arg(&args, 0, "player_id")?,
                i64_arg(&args, 1, "count")?,
            )
        });
        add_game_function_field(fields, metadata::PLAYER_IDS, 0, |lua, this, _| {
            this.player_ids(lua)
        });
    }
}

impl LuaApiType for LuaGame {
    const DEFINITION: LuaTypeDefinition = metadata::GAME_TYPE;
}

#[derive(Clone)]
pub struct LuaPowerCard {
    pub id: String,
    pub base_mana_cost: usize,
    mana_cost_delta: Rc<Cell<i64>>,
    pub owner_id: String,
    pub target_player_id: Option<String>,
}

impl LuaPowerCard {
    pub(crate) fn mana_cost(&self) -> i64 {
        adjusted_mana_cost(self.base_mana_cost, self.mana_cost_delta.get())
    }

    fn add_mana_cost(&self, delta: i64) -> i64 {
        self.mana_cost_delta
            .set(self.mana_cost_delta.get().saturating_add(delta));

        self.mana_cost()
    }
}

impl UserData for LuaPowerCard {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(metadata::ADD_MANA_COST, |_, this, delta: i64| {
            Ok(this.add_mana_cost(delta))
        });
    }

    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("id", |_, this| Ok(this.id.clone()));
        fields.add_field_method_get("mana_cost", |_, this| Ok(this.mana_cost()));
        fields.add_field_method_get("owner_id", |_, this| Ok(this.owner_id.clone()));
        fields.add_field_method_get("target_player_id", |_, this| {
            Ok(this.target_player_id.clone())
        });
        add_power_card_function_field(fields, metadata::ADD_MANA_COST, 1, |_, this, args| {
            Ok(this.add_mana_cost(i64_arg(&args, 0, "delta")?))
        });
    }
}

impl LuaApiType for LuaPowerCard {
    const DEFINITION: LuaTypeDefinition = metadata::POWER_CARD_TYPE;
}

#[derive(Clone)]
pub struct LuaMercenary {
    pub id: String,
    pub owner_id: String,
    pub base_life: usize,
    pub initial_mana: usize,
}

impl UserData for LuaMercenary {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("id", |_, this| Ok(this.id.clone()));
        fields.add_field_method_get("owner_id", |_, this| Ok(this.owner_id.clone()));
        fields.add_field_method_get("base_life", |_, this| Ok(this.base_life));
        fields.add_field_method_get("initial_mana", |_, this| Ok(this.initial_mana));
    }
}

impl LuaApiType for LuaMercenary {
    const DEFINITION: LuaTypeDefinition = metadata::MERCENARY_TYPE;
}

#[derive(Clone, Copy)]
pub struct LuaCard {
    pub rank: Rank,
    pub suit: Suit,
}

impl LuaCard {
    fn from_card(card: Card) -> Self {
        Self {
            rank: card.rank,
            suit: card.suit,
        }
    }

    fn to_card(self) -> Card {
        Card {
            rank: self.rank,
            suit: self.suit,
        }
    }
}

impl UserData for LuaCard {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("rank", |_, this| Ok(rank_to_str(this.rank)));
        fields.add_field_method_get("suit", |_, this| Ok(suit_to_str(this.suit)));
    }
}

impl LuaApiType for LuaCard {
    const DEFINITION: LuaTypeDefinition = metadata::CARD_TYPE;
}

#[derive(Clone)]
pub struct LuaPowerCardState {
    pub id: String,
    pub name: String,
    pub description: String,
    pub mana_cost: usize,
    pub card_type: PowerCardType,
    pub image_url: Option<String>,
    pub usable: bool,
    players: Rc<RefCell<HashMap<String, ScriptPlayerState>>>,
    owner_id: String,
}

impl From<&ScriptPowerCardState> for LuaPowerCardState {
    fn from(card: &ScriptPowerCardState) -> Self {
        Self {
            id: card.id.clone(),
            name: card.name.clone(),
            description: card.description.clone(),
            mana_cost: card.mana_cost,
            card_type: card.card_type,
            image_url: card.image_url.clone(),
            usable: card.usable,
            players: Rc::new(RefCell::new(HashMap::new())),
            owner_id: String::new(),
        }
    }
}

impl UserData for LuaPowerCardState {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("id", |_, this| Ok(this.id.clone()));
        fields.add_field_method_get("name", |_, this| Ok(this.name.clone()));
        fields.add_field_method_get("description", |_, this| Ok(this.description.clone()));
        fields.add_field_method_get("mana_cost", |_, this| Ok(this.mana_cost));
        fields.add_field_method_get("type", |_, this| Ok(this.card_type.as_str()));
        fields.add_field_method_get("image_url", |_, this| Ok(this.image_url.clone()));
        fields.add_field_method_get("usable", |_, this| Ok(this.usable));
        fields.add_field_method_set("usable", |_, this, usable: bool| {
            this.set_usable(usable);
            Ok(())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(metadata::SET_USABLE, |_, this, usable: bool| {
            this.set_usable(usable);
            Ok(())
        });
    }
}

impl LuaPowerCardState {
    pub(crate) fn with_context(
        card: &ScriptPowerCardState,
        players: Rc<RefCell<HashMap<String, ScriptPlayerState>>>,
        owner_id: &str,
    ) -> Self {
        Self {
            id: card.id.clone(),
            name: card.name.clone(),
            description: card.description.clone(),
            mana_cost: card.mana_cost,
            card_type: card.card_type,
            image_url: card.image_url.clone(),
            usable: card.usable,
            players,
            owner_id: owner_id.to_string(),
        }
    }

    fn set_usable(&self, usable: bool) {
        if self.owner_id.is_empty() {
            return;
        }
        if let Some(player) = self.players.borrow_mut().get_mut(&self.owner_id) {
            for card in &mut player.power_cards {
                if card.id == self.id {
                    card.usable = usable;
                }
            }
        }
    }
}

impl LuaApiType for LuaPowerCardState {
    const DEFINITION: LuaTypeDefinition = metadata::POWER_CARD_STATE_TYPE;
}

pub(crate) fn userdata_type_definitions() -> [LuaTypeDefinition; 5] {
    [
        <LuaCard as LuaApiType>::DEFINITION,
        <LuaPowerCard as LuaApiType>::DEFINITION,
        <LuaMercenary as LuaApiType>::DEFINITION,
        <LuaPowerCardState as LuaApiType>::DEFINITION,
        <LuaGame as LuaApiType>::DEFINITION,
    ]
}

pub(crate) fn build_game_api(
    players: Rc<RefCell<HashMap<String, ScriptPlayerState>>>,
    draw_power_cards: DrawPowerCardsFn,
    current_trump: Rank,
) -> LuaGame {
    LuaGame::new(players, draw_power_cards, current_trump)
}

pub(crate) fn build_power_card(input: &PowerScriptInput) -> LuaPowerCard {
    LuaPowerCard {
        id: input.card_id.clone(),
        base_mana_cost: input.mana_cost,
        mana_cost_delta: Rc::new(Cell::new(0)),
        owner_id: input.owner_id.as_str().to_string(),
        target_player_id: input
            .target_player_id
            .as_ref()
            .map(PlayerId::as_str)
            .map(ToString::to_string),
    }
}

pub(crate) fn build_mercenary(input: &PassiveScriptInput) -> LuaMercenary {
    LuaMercenary {
        id: input.mercenary_id.as_str().to_string(),
        owner_id: input.owner_id.as_str().to_string(),
        base_life: input.base_life,
        initial_mana: input.initial_mana,
    }
}

pub(crate) fn build_event_table(lua: &Lua, event: &PassiveGameEvent) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set("type", event.event_type())?;

    match event {
        PassiveGameEvent::MatchStarted
        | PassiveGameEvent::RoundStart
        | PassiveGameEvent::SetStarted => {}
        PassiveGameEvent::RoundEnded { winner, card } => {
            table.set("winner", winner.as_str())?;
            table.set("card", LuaCard::from_card(*card))?;
        }
        PassiveGameEvent::SetEnded { lost_players } => {
            let lost_players = lost_players
                .iter()
                .map(|(player_id, lives)| (player_id.as_str(), *lives))
                .collect::<std::collections::HashMap<_, _>>();
            table.set("lost_players", lost_players)?;
        }
        PassiveGameEvent::BidPlaced { player_id, bid } => {
            table.set("player_id", player_id.as_str())?;
            table.set("bid", *bid)?;
        }
        PassiveGameEvent::PowerCardPlayed {
            player_id,
            card_id,
            target_player_id,
        } => {
            table.set("player_id", player_id.as_str())?;
            table.set("card_id", card_id.as_str())?;
            table.set(
                "target_player_id",
                target_player_id.as_ref().map(PlayerId::as_str),
            )?;
        }
        PassiveGameEvent::TurnPlayed { player_id, card } => {
            table.set("player_id", player_id.as_str())?;
            table.set("card", LuaCard::from_card(*card))?;
        }
    }

    Ok(table)
}

fn add_game_function_field<F, R>(
    fields: &mut F,
    name: &'static str,
    expected_args: usize,
    callback: impl Fn(&Lua, &LuaGame, Vec<Value>) -> mlua::Result<R> + 'static,
) where
    F: UserDataFields<LuaGame>,
    R: IntoLuaMulti + 'static,
{
    let callback = Rc::new(callback);

    fields.add_field_method_get(name, move |lua, this| {
        let this = this.clone();
        let callback = Rc::clone(&callback);

        lua.create_function(move |lua, args: mlua::Variadic<Value>| {
            callback(lua, &this, args_for_count(args, expected_args)?)
        })
    });
}

fn add_power_card_function_field<F, R>(
    fields: &mut F,
    name: &'static str,
    expected_args: usize,
    callback: impl Fn(&Lua, &LuaPowerCard, Vec<Value>) -> mlua::Result<R> + 'static,
) where
    F: UserDataFields<LuaPowerCard>,
    R: IntoLuaMulti + 'static,
{
    let callback = Rc::new(callback);

    fields.add_field_method_get(name, move |lua, this| {
        let this = this.clone();
        let callback = Rc::clone(&callback);

        lua.create_function(move |lua, args: mlua::Variadic<Value>| {
            callback(lua, &this, args_for_count(args, expected_args)?)
        })
    });
}

fn args_for_count(args: mlua::Variadic<Value>, expected_args: usize) -> mlua::Result<Vec<Value>> {
    let mut args = args.into_iter().collect::<Vec<_>>();

    if args.len() == expected_args + 1 {
        args.remove(0);
    }

    if args.len() != expected_args {
        return Err(mlua::Error::external(format!(
            "expected {expected_args} arguments, got {}",
            args.len()
        )));
    }

    Ok(args)
}

fn string_arg(args: &[Value], index: usize, name: &str) -> mlua::Result<String> {
    match args.get(index) {
        Some(Value::String(value)) => Ok(value.to_str()?.to_string()),
        Some(value) => Err(mlua::Error::external(format!(
            "{name} must be a string, got {}",
            value.type_name()
        ))),
        None => Err(mlua::Error::external(format!("missing argument: {name}"))),
    }
}

fn i64_arg(args: &[Value], index: usize, name: &str) -> mlua::Result<i64> {
    match args.get(index) {
        Some(Value::Integer(value)) => Ok(*value),
        Some(Value::Number(value)) => Ok(*value as i64),
        Some(value) => Err(mlua::Error::external(format!(
            "{name} must be an integer, got {}",
            value.type_name()
        ))),
        None => Err(mlua::Error::external(format!("missing argument: {name}"))),
    }
}

fn usize_count(value: i64, name: &str) -> mlua::Result<usize> {
    usize::try_from(value)
        .map_err(|_| mlua::Error::external(format!("{name} must be a non-negative integer")))
}

fn adjusted_mana_cost(base: usize, delta: i64) -> i64 {
    i64::try_from(base)
        .unwrap_or(i64::MAX)
        .saturating_add(delta)
}

fn cards_to_lua_table(lua: &Lua, cards: &[Card]) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    for (idx, card) in cards.iter().enumerate() {
        table.set(idx + 1, LuaCard::from_card(*card))?;
    }

    Ok(table)
}

fn card_from_lua_value(value: &Value) -> mlua::Result<Card> {
    match value {
        Value::UserData(userdata) => Ok(userdata.borrow::<LuaCard>()?.to_card()),
        value => Err(mlua::Error::external(format!(
            "card must be a Card userdata, got {}",
            value.type_name()
        ))),
    }
}

fn power_cards_to_lua_table(
    lua: &Lua,
    cards: &[ScriptPowerCardState],
    players: Rc<RefCell<HashMap<String, ScriptPlayerState>>>,
    owner_id: &str,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    for (idx, card) in cards.iter().enumerate() {
        table.set(
            idx + 1,
            LuaPowerCardState::with_context(card, Rc::clone(&players), owner_id),
        )?;
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

fn suit_to_str(suit: Suit) -> &'static str {
    match suit {
        Suit::Golds => "Golds",
        Suit::Swords => "Swords",
        Suit::Cups => "Cups",
        Suit::Clubs => "Clubs",
    }
}

fn unknown_player(player_id: &str) -> mlua::Error {
    mlua::Error::external(format!("unknown player_id: {player_id}"))
}
