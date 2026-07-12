use std::{
    cell::{Cell, RefCell},
    cmp::Ordering,
    collections::HashMap,
    rc::Rc,
};

use lua_api_derive::LuaApiType;
use mlua_extras::mlua::{
    self, AnyUserData, FromLua, FromLuaMulti, IntoLuaMulti, Lua, Table, Value,
};
use mlua_extras::typed::{TypedFunction, TypedMultiValue};
use mlua_extras::{TypedUserData, typed_user_data_impl};

use crate::models::{Card, Rank, Suit, game::fodinha_power::PowerCardType, id::PlayerId};

use super::{
    DrawPowerCardsFn, PassiveGameEvent, PassiveScriptInput, PowerScriptInput, ScriptPlayerState,
    ScriptPowerCardState,
};

#[derive(Clone, TypedUserData, LuaApiType)]
pub struct LuaGame {
    #[field(skip)]
    players: Rc<RefCell<HashMap<String, ScriptPlayerState>>>,
    #[field(skip)]
    draw_power_cards: DrawPowerCardsFn,
    #[field(skip)]
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

    fn lives_for_player(&self, player_id: &str) -> mlua::Result<usize> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        Ok(player.lifes)
    }

    fn current_trump_as_lua(&self) -> &'static str {
        self.current_trump.lua_name()
    }

    fn adjust_lives(&self, player_id: &str, delta: i64) -> mlua::Result<usize> {
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

    fn set_lives_for_player(&self, player_id: &str, lifes: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id) else {
            return Err(unknown_player(player_id));
        };

        player.lifes = usize::try_from(lifes).unwrap_or(0);

        Ok(player.lifes)
    }

    fn bid_for_player(&self, player_id: &str) -> mlua::Result<Option<usize>> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        Ok(player.bid)
    }

    fn add_bids_for_player(&self, player_id: &str, bid_count: i64) -> mlua::Result<()> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id) else {
            return Err(unknown_player(player_id));
        };

        if let Some(bid) = player.bid.as_mut() {
            *bid += bid_count as usize;
        }

        Ok(())
    }

    fn rounds_for_player(&self, player_id: &str) -> mlua::Result<usize> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        Ok(player.rounds)
    }

    fn mana_for_player(&self, player_id: &str) -> mlua::Result<usize> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        Ok(player.mana.current)
    }

    fn max_mana_for_player(&self, player_id: &str) -> mlua::Result<usize> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        Ok(player.mana.max)
    }

    fn adjust_mana_for_player(&self, player_id: &str, delta: i64) -> mlua::Result<usize> {
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

    fn set_mana_for_player(&self, player_id: &str, mana: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id) else {
            return Err(unknown_player(player_id));
        };

        player.mana.current = usize::try_from(mana).unwrap_or(0).min(player.mana.max);

        Ok(player.mana.current)
    }

    fn set_max_mana_for_player(&self, player_id: &str, mana: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id) else {
            return Err(unknown_player(player_id));
        };

        player.mana.max = usize::try_from(mana).unwrap_or(0);
        player.mana.current = player.mana.current.min(player.mana.max);

        Ok(player.mana.max)
    }

    fn cards_for_player(&self, player_id: &str) -> mlua::Result<Vec<LuaCard>> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        Ok(player
            .cards
            .iter()
            .copied()
            .map(LuaCard::from_card)
            .collect())
    }

    fn switch_cards_for_players(
        &self,
        first_player_id: &str,
        first_card: &LuaCard,
        second_player_id: &str,
        second_card: &LuaCard,
    ) -> mlua::Result<bool> {
        let first_card = first_card.to_card();
        let second_card = second_card.to_card();

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

    fn power_cards_for_player(&self, player_id: &str) -> mlua::Result<Vec<LuaPowerCardState>> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id) else {
            return Err(unknown_player(player_id));
        };

        Ok(power_cards_to_lua_vec(
            &player.power_cards,
            Rc::clone(&self.players),
            player_id,
        ))
    }

    fn steal_power_card_between_players(
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

    fn draw_power_cards_for_player(
        &self,
        player_id: &str,
        count: i64,
    ) -> mlua::Result<Vec<LuaPowerCardState>> {
        let count = usize_count(count, "count")?;
        if !self.players.borrow().contains_key(player_id) {
            return Err(unknown_player(player_id));
        }

        if count == 0 {
            return Ok(Vec::new());
        }

        let drawn = (self.draw_power_cards)(player_id, count).map_err(mlua::Error::external)?;
        let result = power_cards_to_lua_vec(&drawn, Rc::clone(&self.players), player_id);

        self.players
            .borrow_mut()
            .get_mut(player_id)
            .expect("player was validated above")
            .power_cards
            .extend(drawn);

        Ok(result)
    }

    fn list_player_ids(&self) -> Vec<String> {
        self.players
            .borrow()
            .keys()
            .map(|player_id| player_id.to_owned())
            .collect()
    }
}

#[typed_user_data_impl]
#[allow(dead_code)]
impl LuaGame {
    fn get_lives(&self, player_id: String) -> mlua::Result<usize> {
        self.lives_for_player(&player_id)
    }

    fn get_current_trump(&self) -> &'static str {
        self.current_trump_as_lua()
    }

    fn add_lives(&self, player_id: String, delta: i64) -> mlua::Result<usize> {
        self.adjust_lives(&player_id, delta)
    }

    fn set_lives(&self, player_id: String, lifes: i64) -> mlua::Result<usize> {
        self.set_lives_for_player(&player_id, lifes)
    }

    fn get_bid(&self, player_id: String) -> mlua::Result<Option<usize>> {
        self.bid_for_player(&player_id)
    }

    fn add_bids(&self, player_id: String, bid_count: i64) -> mlua::Result<()> {
        self.add_bids_for_player(&player_id, bid_count)
    }

    fn get_rounds(&self, player_id: String) -> mlua::Result<usize> {
        self.rounds_for_player(&player_id)
    }

    fn get_mana(&self, player_id: String) -> mlua::Result<usize> {
        self.mana_for_player(&player_id)
    }

    fn get_max_mana(&self, player_id: String) -> mlua::Result<usize> {
        self.max_mana_for_player(&player_id)
    }

    fn get_mana_pool(&self, player_id: String) -> mlua::Result<usize> {
        self.max_mana_for_player(&player_id)
    }

    fn add_mana(&self, player_id: String, delta: i64) -> mlua::Result<usize> {
        self.adjust_mana_for_player(&player_id, delta)
    }

    fn set_mana(&self, player_id: String, mana: i64) -> mlua::Result<usize> {
        self.set_mana_for_player(&player_id, mana)
    }

    fn set_max_mana(&self, player_id: String, mana: i64) -> mlua::Result<usize> {
        self.set_max_mana_for_player(&player_id, mana)
    }

    fn get_cards(&self, player_id: String) -> mlua::Result<Vec<LuaCard>> {
        self.cards_for_player(&player_id)
    }

    fn switch_cards(
        &self,
        first_player_id: String,
        first_card: LuaCard,
        second_player_id: String,
        second_card: LuaCard,
    ) -> mlua::Result<bool> {
        self.switch_cards_for_players(
            &first_player_id,
            &first_card,
            &second_player_id,
            &second_card,
        )
    }

    fn get_power_cards(&self, player_id: String) -> mlua::Result<Vec<LuaPowerCardState>> {
        self.power_cards_for_player(&player_id)
    }

    fn steal_power_card(
        &self,
        from_player_id: String,
        card_id: String,
        to_player_id: String,
    ) -> mlua::Result<bool> {
        self.steal_power_card_between_players(&from_player_id, &card_id, &to_player_id)
    }

    fn draw_power_cards(
        &self,
        player_id: String,
        count: i64,
    ) -> mlua::Result<Vec<LuaPowerCardState>> {
        self.draw_power_cards_for_player(&player_id, count)
    }

    fn player_ids(&self) -> Vec<String> {
        self.list_player_ids()
    }

    #[getter("get_lives")]
    fn get_lives_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<String, usize>> {
        game_function(lua, self, 1, |_, this, player_id: String| {
            this.lives_for_player(&player_id)
        })
    }

    #[getter("get_current_trump")]
    fn get_current_trump_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<(), String>> {
        game_function(lua, self, 0, |_, this, ()| {
            Ok(this.current_trump_as_lua().to_owned())
        })
    }

    #[getter("add_lives")]
    fn add_lives_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<(String, i64), usize>> {
        game_function(
            lua,
            self,
            2,
            |_, this, (player_id, delta): (String, i64)| this.adjust_lives(&player_id, delta),
        )
    }

    #[getter("set_lives")]
    fn set_lives_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<(String, i64), usize>> {
        game_function(
            lua,
            self,
            2,
            |_, this, (player_id, lifes): (String, i64)| {
                this.set_lives_for_player(&player_id, lifes)
            },
        )
    }

    #[getter("get_bid")]
    fn get_bid_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<String, Option<usize>>> {
        game_function(lua, self, 1, |_, this, player_id: String| {
            this.bid_for_player(&player_id)
        })
    }

    #[getter("add_bids")]
    fn add_bids_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<(String, i64), ()>> {
        game_function(
            lua,
            self,
            2,
            |_, this, (player_id, bid_count): (String, i64)| {
                this.add_bids_for_player(&player_id, bid_count)
            },
        )
    }

    #[getter("get_rounds")]
    fn get_rounds_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<String, usize>> {
        game_function(lua, self, 1, |_, this, player_id: String| {
            this.rounds_for_player(&player_id)
        })
    }

    #[getter("get_mana")]
    fn get_mana_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<String, usize>> {
        game_function(lua, self, 1, |_, this, player_id: String| {
            this.mana_for_player(&player_id)
        })
    }

    #[getter("get_max_mana")]
    fn get_max_mana_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<String, usize>> {
        game_function(lua, self, 1, |_, this, player_id: String| {
            this.max_mana_for_player(&player_id)
        })
    }

    #[getter("get_mana_pool")]
    fn get_mana_pool_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<String, usize>> {
        game_function(lua, self, 1, |_, this, player_id: String| {
            this.max_mana_for_player(&player_id)
        })
    }

    #[getter("add_mana")]
    fn add_mana_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<(String, i64), usize>> {
        game_function(
            lua,
            self,
            2,
            |_, this, (player_id, delta): (String, i64)| {
                this.adjust_mana_for_player(&player_id, delta)
            },
        )
    }

    #[getter("set_mana")]
    fn set_mana_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<(String, i64), usize>> {
        game_function(lua, self, 2, |_, this, (player_id, mana): (String, i64)| {
            this.set_mana_for_player(&player_id, mana)
        })
    }

    #[getter("set_max_mana")]
    fn set_max_mana_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<(String, i64), usize>> {
        game_function(lua, self, 2, |_, this, (player_id, mana): (String, i64)| {
            this.set_max_mana_for_player(&player_id, mana)
        })
    }

    #[getter("get_cards")]
    fn get_cards_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<String, Vec<LuaCard>>> {
        game_function(lua, self, 1, |_, this, player_id: String| {
            this.cards_for_player(&player_id)
        })
    }

    #[getter("switch_cards")]
    fn switch_cards_field(
        &self,
        lua: &Lua,
    ) -> mlua::Result<TypedFunction<(String, LuaCard, String, LuaCard), bool>> {
        game_function(
            lua,
            self,
            4,
            |_, this, args: (String, LuaCard, String, LuaCard)| {
                let (first_player_id, first_card, second_player_id, second_card) = args;
                this.switch_cards_for_players(
                    &first_player_id,
                    &first_card,
                    &second_player_id,
                    &second_card,
                )
            },
        )
    }

    #[getter("get_power_cards")]
    fn get_power_cards_field(
        &self,
        lua: &Lua,
    ) -> mlua::Result<TypedFunction<String, Vec<LuaPowerCardState>>> {
        game_function(lua, self, 1, |_, this, player_id: String| {
            this.power_cards_for_player(&player_id)
        })
    }

    #[getter("steal_power_card")]
    fn steal_power_card_field(
        &self,
        lua: &Lua,
    ) -> mlua::Result<TypedFunction<(String, String, String), bool>> {
        game_function(
            lua,
            self,
            3,
            |_, this, (from_player_id, card_id, to_player_id): (String, String, String)| {
                this.steal_power_card_between_players(&from_player_id, &card_id, &to_player_id)
            },
        )
    }

    #[getter("draw_power_cards")]
    fn draw_power_cards_field(
        &self,
        lua: &Lua,
    ) -> mlua::Result<TypedFunction<(String, i64), Vec<LuaPowerCardState>>> {
        game_function(
            lua,
            self,
            2,
            |_, this, (player_id, count): (String, i64)| {
                this.draw_power_cards_for_player(&player_id, count)
            },
        )
    }

    #[getter("player_ids")]
    fn player_ids_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<(), Vec<String>>> {
        game_function(lua, self, 0, |_, this, ()| Ok(this.player_ids()))
    }
}

#[derive(Clone, TypedUserData, LuaApiType)]
pub struct LuaPowerCard {
    #[field(readonly)]
    pub id: String,
    #[field(skip)]
    pub base_mana_cost: usize,
    #[field(skip)]
    mana_cost_delta: Rc<Cell<i64>>,
    #[field(readonly)]
    pub owner_id: String,
    #[field(readonly)]
    pub target_player_id: Option<String>,
}

impl LuaPowerCard {
    pub(crate) fn mana_cost(&self) -> i64 {
        adjusted_mana_cost(self.base_mana_cost, self.mana_cost_delta.get())
    }

    fn adjusted_mana_cost_value(&self, delta: i64) -> i64 {
        self.mana_cost_delta
            .set(self.mana_cost_delta.get().saturating_add(delta));

        self.mana_cost()
    }
}

#[typed_user_data_impl]
impl LuaPowerCard {
    fn add_mana_cost(&self, delta: i64) -> i64 {
        self.adjusted_mana_cost_value(delta)
    }

    #[getter("mana_cost")]
    fn mana_cost_field(&self) -> i64 {
        self.mana_cost()
    }

    #[getter("add_mana_cost")]
    fn add_mana_cost_field(&self, lua: &Lua) -> mlua::Result<TypedFunction<i64, i64>> {
        power_card_function(lua, self, 1, |_, this, delta: i64| {
            Ok(this.add_mana_cost(delta))
        })
    }
}

#[derive(Clone, TypedUserData, LuaApiType)]
pub struct LuaMercenary {
    #[field(readonly)]
    pub id: String,
    #[field(readonly)]
    pub owner_id: String,
    #[field(readonly)]
    pub base_life: usize,
    #[field(readonly)]
    pub initial_mana: usize,
}

#[derive(Clone, Copy, TypedUserData, LuaApiType)]
pub struct LuaCard {
    #[field(readonly)]
    pub rank: Rank,
    #[field(readonly)]
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

impl FromLua for LuaCard {
    fn from_lua(value: Value, lua: &Lua) -> mlua::Result<Self> {
        let userdata = AnyUserData::from_lua(value, lua)?;
        Ok(*userdata.borrow::<Self>()?)
    }
}

#[derive(Clone, TypedUserData, LuaApiType)]
pub struct LuaPowerCardState {
    #[field(readonly)]
    pub id: String,
    #[field(readonly)]
    pub name: String,
    #[field(readonly)]
    pub description: String,
    #[field(readonly)]
    pub mana_cost: usize,
    #[field(rename = "type", readonly)]
    pub card_type: PowerCardType,
    #[field(readonly)]
    pub image_url: Option<String>,
    #[field(skip)]
    pub usable: bool,
    #[field(skip)]
    players: Rc<RefCell<HashMap<String, ScriptPlayerState>>>,
    #[field(skip)]
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

    fn update_usable(&self, usable: bool) {
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

#[typed_user_data_impl]
impl LuaPowerCardState {
    #[getter("usable")]
    fn usable(&self) -> bool {
        self.usable
    }

    #[setter("usable")]
    fn set_usable_field(&self, usable: bool) {
        self.set_usable(usable);
    }

    #[method]
    fn set_usable(&self, usable: bool) {
        self.update_usable(usable);
    }
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

fn game_function<A, R, F>(
    lua: &Lua,
    this: &LuaGame,
    expected_args: usize,
    callback: F,
) -> mlua::Result<TypedFunction<A, R>>
where
    A: FromLuaMulti + TypedMultiValue,
    R: IntoLuaMulti + TypedMultiValue,
    F: Fn(&Lua, &LuaGame, A) -> mlua::Result<R> + 'static,
{
    let callback = Rc::new(callback);

    let this = this.clone();
    let function = lua.create_function(move |lua, args: mlua::Variadic<Value>| {
        let args = args_for_count(args, expected_args)?;
        let args = A::from_lua_multi(mlua::MultiValue::from_vec(args), lua)?;
        callback(lua, &this, args)
    })?;
    TypedFunction::<A, R>::from_lua(Value::Function(function), lua)
}

fn power_card_function<A, R, F>(
    lua: &Lua,
    this: &LuaPowerCard,
    expected_args: usize,
    callback: F,
) -> mlua::Result<TypedFunction<A, R>>
where
    A: FromLuaMulti + TypedMultiValue,
    R: IntoLuaMulti + TypedMultiValue,
    F: Fn(&Lua, &LuaPowerCard, A) -> mlua::Result<R> + 'static,
{
    let callback = Rc::new(callback);

    let this = this.clone();
    let function = lua.create_function(move |lua, args: mlua::Variadic<Value>| {
        let args = args_for_count(args, expected_args)?;
        let args = A::from_lua_multi(mlua::MultiValue::from_vec(args), lua)?;
        callback(lua, &this, args)
    })?;
    TypedFunction::<A, R>::from_lua(Value::Function(function), lua)
}

fn args_for_count(args: mlua::Variadic<Value>, expected_args: usize) -> mlua::Result<Vec<Value>> {
    let mut args = args.into_iter().collect::<Vec<_>>();

    if args
        .first()
        .is_some_and(|value| matches!(value, Value::UserData(_)))
    {
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

fn usize_count(value: i64, name: &str) -> mlua::Result<usize> {
    usize::try_from(value)
        .map_err(|_| mlua::Error::external(format!("{name} must be a non-negative integer")))
}

fn adjusted_mana_cost(base: usize, delta: i64) -> i64 {
    i64::try_from(base)
        .unwrap_or(i64::MAX)
        .saturating_add(delta)
}

fn power_cards_to_lua_vec(
    cards: &[ScriptPowerCardState],
    players: Rc<RefCell<HashMap<String, ScriptPlayerState>>>,
    owner_id: &str,
) -> Vec<LuaPowerCardState> {
    cards
        .iter()
        .map(|card| LuaPowerCardState::with_context(card, Rc::clone(&players), owner_id))
        .collect()
}

fn unknown_player(player_id: &str) -> mlua::Error {
    mlua::Error::external(format!("unknown player_id: {player_id}"))
}
