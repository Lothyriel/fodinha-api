use std::{
    cell::{Cell, RefCell},
    cmp::Ordering,
    collections::HashMap,
    marker::PhantomData,
    rc::Rc,
    sync::Arc,
};

use lua_api_derive::{LuaApiType, lua_api_impl};
use mlua_extras::mlua::{
    self, AnyUserData, FromLua, FromLuaMulti, IntoLua, IntoLuaMulti, Lua, Table, Value,
};
use mlua_extras::typed::{Type, Typed};
use mlua_extras::{TypedUserData, typed_user_data_impl};

use crate::models::{Card, Rank, Suit, game::fodinha_power::PowerCardType, id::PlayerId};

use super::{
    DrawPowerCardsFn, PassiveGameEvent, PassiveScriptInput, PowerScriptInput, ScriptPlayerState,
    ScriptPowerCardState,
};

pub(crate) trait LuaApiFunctionSignature {
    fn type_definition() -> Type;
}

pub(crate) struct LuaBoundFunction<S> {
    function: mlua::Function,
    signature: PhantomData<S>,
}

impl<S> LuaBoundFunction<S> {
    fn new(function: mlua::Function) -> Self {
        Self {
            function,
            signature: PhantomData,
        }
    }
}

impl<S> IntoLua for LuaBoundFunction<S> {
    fn into_lua(self, _lua: &Lua) -> mlua::Result<Value> {
        Ok(Value::Function(self.function))
    }
}

impl<S: LuaApiFunctionSignature> Typed for LuaBoundFunction<S> {
    fn ty() -> Type {
        S::type_definition()
    }
}

#[derive(Clone, TypedUserData, LuaApiType)]
pub struct LuaGame {
    #[field(skip)]
    players: Rc<RefCell<HashMap<String, ScriptPlayerState>>>,
    #[field(skip)]
    draw_power_cards: DrawPowerCardsFn,
    #[field(skip)]
    deck_reveals: Rc<RefCell<Vec<super::DeckReveal>>>,
    #[field(skip)]
    current_trump: Rank,
}

#[lua_api_impl]
impl LuaGame {
    pub(crate) fn new(
        players: Rc<RefCell<HashMap<String, ScriptPlayerState>>>,
        draw_power_cards: DrawPowerCardsFn,
        deck_reveals: Rc<RefCell<Vec<super::DeckReveal>>>,
        current_trump: Rank,
    ) -> Self {
        Self {
            players,
            draw_power_cards,
            deck_reveals,
            current_trump,
        }
    }

    #[lua_api_method]
    fn get_lives(&self, player_id: PlayerId) -> mlua::Result<usize> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        Ok(player.lifes)
    }

    #[lua_api_method]
    fn get_current_trump(&self) -> Rank {
        self.current_trump
    }

    #[lua_api_method]
    fn add_lives(&self, player_id: PlayerId, delta: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        player.lifes = match delta.cmp(&0) {
            Ordering::Less => player.lifes.saturating_sub(delta.unsigned_abs() as usize),
            Ordering::Equal => player.lifes,
            Ordering::Greater => player.lifes.saturating_add(delta as usize),
        };

        Ok(player.lifes)
    }

    #[lua_api_method]
    fn set_lives(&self, player_id: PlayerId, lifes: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        player.lifes = usize::try_from(lifes).unwrap_or(0);

        Ok(player.lifes)
    }

    #[lua_api_method]
    fn get_bid(&self, player_id: PlayerId) -> mlua::Result<Option<usize>> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        Ok(player.bid)
    }

    #[lua_api_method]
    fn add_bids(&self, player_id: PlayerId, bid_count: i64) -> mlua::Result<()> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        if let Some(bid) = player.bid.as_mut() {
            *bid += bid_count as usize;
        }

        Ok(())
    }

    #[lua_api_method]
    fn get_rounds(&self, player_id: PlayerId) -> mlua::Result<usize> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        Ok(player.rounds)
    }

    #[lua_api_method]
    fn get_mana(&self, player_id: PlayerId) -> mlua::Result<usize> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        Ok(player.mana.current)
    }

    #[lua_api_method]
    fn get_max_mana(&self, player_id: PlayerId) -> mlua::Result<usize> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        Ok(player.mana.max)
    }

    #[lua_api_method]
    fn get_mana_pool(&self, player_id: PlayerId) -> mlua::Result<usize> {
        self.get_max_mana(player_id)
    }

    #[lua_api_method]
    fn add_mana(&self, player_id: PlayerId, delta: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
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

    #[lua_api_method]
    fn set_mana(&self, player_id: PlayerId, mana: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        player.mana.current = usize::try_from(mana).unwrap_or(0).min(player.mana.max);

        Ok(player.mana.current)
    }

    #[lua_api_method]
    fn set_max_mana(&self, player_id: PlayerId, mana: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        player.mana.max = usize::try_from(mana).unwrap_or(0);
        player.mana.current = player.mana.current.min(player.mana.max);

        Ok(player.mana.max)
    }

    /// Adjusts a player's maximum mana capacity. Increasing the maximum does
    /// not refill current mana; decreasing it clamps current mana to the new
    /// maximum. Returns the updated maximum.
    #[lua_api_method]
    fn add_max_mana(&self, player_id: PlayerId, delta: i64) -> mlua::Result<usize> {
        let mut players = self.players.borrow_mut();
        let Some(player) = players.get_mut(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        player.mana.max = match delta.cmp(&0) {
            Ordering::Less => player
                .mana
                .max
                .saturating_sub(delta.unsigned_abs() as usize),
            Ordering::Equal => player.mana.max,
            Ordering::Greater => player.mana.max.saturating_add(delta as usize),
        };
        player.mana.current = player.mana.current.min(player.mana.max);

        Ok(player.mana.max)
    }

    #[lua_api_method]
    fn get_cards(&self, player_id: PlayerId) -> mlua::Result<Vec<LuaCard>> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        Ok(player
            .cards
            .iter()
            .copied()
            .map(LuaCard::from_card)
            .collect())
    }

    #[lua_api_method]
    fn reveal_deck(&self, caster_id: PlayerId, target_player_id: PlayerId) -> mlua::Result<()> {
        let cards = {
            let players = self.players.borrow();
            if !players.contains_key(caster_id.as_str()) {
                return Err(unknown_player(caster_id.as_str()));
            }

            let Some(target) = players.get(target_player_id.as_str()) else {
                return Err(unknown_player(target_player_id.as_str()));
            };

            target.cards.clone()
        };

        self.deck_reveals.borrow_mut().push(super::DeckReveal {
            caster_id: caster_id.as_str().to_string(),
            target_player_id: target_player_id.as_str().to_string(),
            cards,
        });

        Ok(())
    }

    #[lua_api_method]
    fn switch_cards(
        &self,
        first_player_id: PlayerId,
        first_card: LuaCard,
        second_player_id: PlayerId,
        second_card: LuaCard,
    ) -> mlua::Result<bool> {
        let first_card = first_card.to_card();
        let second_card = second_card.to_card();

        if first_player_id == second_player_id {
            return Ok(false);
        }

        let mut players = self.players.borrow_mut();
        let first_idx = players
            .get(first_player_id.as_str())
            .and_then(|player| player.cards.iter().position(|card| card == &first_card))
            .ok_or_else(|| {
                mlua::Error::external(format!(
                    "card not found for player_id: {}",
                    first_player_id.as_str()
                ))
            })?;
        let second_idx = players
            .get(second_player_id.as_str())
            .and_then(|player| player.cards.iter().position(|card| card == &second_card))
            .ok_or_else(|| {
                mlua::Error::external(format!(
                    "card not found for player_id: {}",
                    second_player_id.as_str()
                ))
            })?;

        players
            .get_mut(first_player_id.as_str())
            .expect("player was validated above")
            .cards[first_idx] = second_card;
        players
            .get_mut(second_player_id.as_str())
            .expect("player was validated above")
            .cards[second_idx] = first_card;

        Ok(true)
    }

    #[lua_api_method]
    fn get_power_cards(&self, player_id: PlayerId) -> mlua::Result<Vec<LuaPowerCardState>> {
        let players = self.players.borrow();
        let Some(player) = players.get(player_id.as_str()) else {
            return Err(unknown_player(player_id.as_str()));
        };

        Ok(power_cards_to_lua_vec(
            &player.power_cards,
            Rc::clone(&self.players),
            player_id.as_str(),
        ))
    }

    #[lua_api_method]
    fn steal_power_card(
        &self,
        from_player_id: PlayerId,
        card_id: String,
        to_player_id: PlayerId,
    ) -> mlua::Result<bool> {
        if from_player_id == to_player_id {
            return Ok(false);
        }

        let mut players = self.players.borrow_mut();
        let card_idx = players
            .get(from_player_id.as_str())
            .and_then(|player| {
                player
                    .power_cards
                    .iter()
                    .position(|card| card.id == card_id)
            })
            .ok_or_else(|| {
                mlua::Error::external(format!(
                    "power card not found for player_id: {}",
                    from_player_id.as_str()
                ))
            })?;

        if !players.contains_key(to_player_id.as_str()) {
            return Err(unknown_player(to_player_id.as_str()));
        }

        let card = players
            .get_mut(from_player_id.as_str())
            .expect("player was validated above")
            .power_cards
            .remove(card_idx);
        players
            .get_mut(to_player_id.as_str())
            .expect("player was validated above")
            .power_cards
            .push(card);

        Ok(true)
    }

    #[lua_api_method]
    fn draw_power_cards(
        &self,
        player_id: PlayerId,
        count: i64,
    ) -> mlua::Result<Vec<LuaPowerCardState>> {
        let count = usize_count(count, "count")?;
        if !self.players.borrow().contains_key(player_id.as_str()) {
            return Err(unknown_player(player_id.as_str()));
        }

        if count == 0 {
            return Ok(Vec::new());
        }

        let drawn =
            (self.draw_power_cards)(player_id.as_str(), count).map_err(mlua::Error::external)?;
        let result = power_cards_to_lua_vec(&drawn, Rc::clone(&self.players), player_id.as_str());

        self.players
            .borrow_mut()
            .get_mut(player_id.as_str())
            .expect("player was validated above")
            .power_cards
            .extend(drawn);

        Ok(result)
    }

    #[lua_api_method]
    fn player_ids(&self) -> Vec<PlayerId> {
        self.players
            .borrow()
            .keys()
            .map(|player_id| PlayerId(Arc::from(player_id.as_str())))
            .collect()
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
    pub owner_id: PlayerId,
    #[field(readonly)]
    pub targets: Vec<PlayerId>,
}

impl LuaPowerCard {
    pub(crate) fn mana_cost(&self) -> i64 {
        adjusted_mana_cost(self.base_mana_cost, self.mana_cost_delta.get())
    }
}

#[lua_api_impl]
impl LuaPowerCard {
    #[lua_api_method]
    fn add_mana_cost(&self, delta: i64) -> i64 {
        self.mana_cost_delta
            .set(self.mana_cost_delta.get().saturating_add(delta));

        self.mana_cost()
    }

    #[getter("mana_cost")]
    fn mana_cost_field(&self) -> i64 {
        self.mana_cost()
    }
}

#[derive(Clone, TypedUserData, LuaApiType)]
pub struct LuaMercenary {
    #[field(readonly)]
    pub id: String,
    #[field(readonly)]
    pub owner_id: PlayerId,
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
    deck_reveals: Rc<RefCell<Vec<super::DeckReveal>>>,
    current_trump: Rank,
) -> LuaGame {
    LuaGame::new(players, draw_power_cards, deck_reveals, current_trump)
}

pub(crate) fn build_power_card(input: &PowerScriptInput) -> LuaPowerCard {
    LuaPowerCard {
        id: input.card_id.clone(),
        base_mana_cost: input.mana_cost,
        mana_cost_delta: Rc::new(Cell::new(0)),
        owner_id: input.owner_id.clone(),
        targets: input.targets.clone(),
    }
}

pub(crate) fn build_mercenary(input: &PassiveScriptInput) -> LuaMercenary {
    LuaMercenary {
        id: input.mercenary_id.as_str().to_string(),
        owner_id: input.owner_id.clone(),
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
        PassiveGameEvent::SetEnded { lost_players, bids } => {
            let lost_players = lost_players
                .iter()
                .map(|(player_id, lives)| (player_id.as_str(), *lives))
                .collect::<std::collections::HashMap<_, _>>();
            table.set("lost_players", lost_players)?;
            let bids = bids
                .iter()
                .map(|(player_id, bid)| (player_id.as_str(), *bid))
                .collect::<std::collections::HashMap<_, _>>();
            table.set("bids", bids)?;
        }
        PassiveGameEvent::BidPlaced { player_id, bid } => {
            table.set("player_id", player_id.as_str())?;
            table.set("bid", *bid)?;
        }
        PassiveGameEvent::PowerCardPlayed {
            player_id,
            card_id,
            targets,
        } => {
            table.set("player_id", player_id.as_str())?;
            table.set("card_id", card_id.as_str())?;
            table.set(
                "targets",
                targets.iter().map(PlayerId::as_str).collect::<Vec<_>>(),
            )?;
        }
        PassiveGameEvent::TurnPlayed { player_id, card } => {
            table.set("player_id", player_id.as_str())?;
            table.set("card", LuaCard::from_card(*card))?;
        }
    }

    Ok(table)
}

pub(crate) fn bind_lua_function<T, A, R, S, F>(
    lua: &Lua,
    this: &T,
    expected_args: usize,
    callback: F,
) -> mlua::Result<LuaBoundFunction<S>>
where
    T: Clone + 'static,
    A: FromLuaMulti,
    R: IntoLuaMulti,
    S: LuaApiFunctionSignature,
    F: Fn(&Lua, &T, A) -> mlua::Result<R> + 'static,
{
    let callback = Rc::new(callback);

    let this = this.clone();
    let function = lua.create_function(move |lua, args: mlua::Variadic<Value>| {
        let args = args_for_count(args, expected_args)?;
        let args = A::from_lua_multi(mlua::MultiValue::from_vec(args), lua)?;
        callback(lua, &this, args)
    })?;
    Ok(LuaBoundFunction::new(function))
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
