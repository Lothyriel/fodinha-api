use std::collections::{BinaryHeap, HashMap};

use indexmap::IndexMap;

use crate::services::{GameInfoDto, GameStageDto, PlayerInfoDto};

use crate::models::{
    BiddingError, Card, DealError, GameError, Rank, Turn, id::PlayerId, util::CyclicIterator,
};

const INITIAL_CARDS_COUNT: usize = 1;
const MAX_AVAILABLE_CARDS: usize = 40 - 1;
pub const MAX_PLAYER_COUNT: usize = 13;

#[derive(Debug, Clone)]
pub struct Game {
    players: IndexMap<PlayerId, Player>,
    pile: BinaryHeap<(u8, Turn)>,
    dealing_mode: DealingMode,
    bidding_iter: CyclicIterator,
    round_iter: CyclicIterator,
    cards_count: usize,
    upcard: Card,
    seed: i64,
    next_shuffle_sequence: i64,
    rules: GameRules,
}

#[derive(Debug, Clone)]
struct Player {
    lifes: usize,
    deck: Vec<Card>,
    bid: Option<usize>,
    rounds: usize,
}

impl Player {
    fn new(deck: Vec<Card>, lifes: usize) -> Self {
        Self {
            lifes,
            deck,
            bid: None,
            rounds: 0,
        }
    }

    fn is_alive(&self) -> bool {
        self.lifes > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GameSettings {
    pub lifes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameRules {
    pub life_loss_per_bid_difference: usize,
}

impl Default for GameRules {
    fn default() -> Self {
        Self {
            life_loss_per_bid_difference: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSnapshot {
    pub lifes: usize,
    pub deck: Vec<Card>,
    pub bid: Option<usize>,
    pub rounds: usize,
}

pub const DEFAULT_INITIAL_LIFES: usize = 5;
pub const MIN_INITIAL_LIFES: usize = 1;
pub const MAX_INITIAL_LIFES: usize = 10;

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            lifes: DEFAULT_INITIAL_LIFES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DealingMode {
    Increasing,
    Decreasing,
}

impl DealingMode {
    pub fn get_next(&self, cards: usize, players: usize) -> (DealingMode, usize) {
        match self {
            DealingMode::Increasing => {
                if cards + 1 < MAX_AVAILABLE_CARDS / players {
                    (DealingMode::Increasing, cards + 1)
                } else {
                    (DealingMode::Decreasing, cards - 1)
                }
            }
            DealingMode::Decreasing => {
                if cards - 1 == 0 {
                    (DealingMode::Increasing, cards + 1)
                } else {
                    (DealingMode::Decreasing, cards - 1)
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NewSet {
    pub dealing_mode: DealingMode,
    pub cards_count: usize,
    pub shuffle: DeckShuffle,
    pub decks: IndexMap<PlayerId, Vec<Card>>,
    pub upcard: Card,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeckShuffle {
    pub seed: i64,
    pub sequence: i64,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug)]
#[serde(tag = "type", content = "data")]
pub enum GameCommand {
    PlayTurn { card: Card },
    PutBid { bid: usize },
}

impl GameCommand {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::PlayTurn { .. } => "game.fodinha_classic.play_turn",
            Self::PutBid { .. } => "game.fodinha_classic.put_bid",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MatchEvent {
    GameStarted {
        settings: GameSettings,
        seed: i64,
    },
    BidPlaced {
        player_id: PlayerId,
        bid: usize,
    },
    TurnPlayed {
        turn: Turn,
    },
}

#[derive(Debug, Clone)]
pub enum GameOutcome {
    SetPending {
        next: PlayerId,
    },
    SetEnded {
        lifes: HashMap<PlayerId, usize>,
        upcard: Card,
        decks: IndexMap<PlayerId, Vec<Card>>,
        next: PlayerId,
        possible: Vec<usize>,
    },
    RoundEnded {
        next: PlayerId,
        rounds: HashMap<PlayerId, usize>,
    },
    Ended {
        lifes: HashMap<PlayerId, usize>,
    },
    TurnPlayed {
        next: PlayerId,
    },
}

#[derive(Debug, Clone)]
pub struct DealState {
    pub outcome: GameOutcome,
    pub pile: Vec<Turn>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum BiddingState {
    Active {
        next: PlayerId,
        possible_bids: Vec<usize>,
    },
    Ended {
        next: PlayerId,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum GameStage {
    Bidding,
    Dealing,
}

#[derive(Debug, Clone)]
pub enum AppliedGameChange {
    BidPlaced {
        player_id: PlayerId,
        bid: usize,
        state: BiddingState,
    },
    TurnPlayed(DealState),
}

impl Game {
    pub fn new_default(players: &[PlayerId]) -> Result<Self, GameError> {
        Self::new(players, Default::default())
    }

    pub fn new(players: &[PlayerId], settings: GameSettings) -> Result<Self, GameError> {
        Self::new_with_seed(players, settings, rand::random())
    }

    pub fn new_with_seed(
        players: &[PlayerId],
        settings: GameSettings,
        seed: i64,
    ) -> Result<Self, GameError> {
        let event = Self::start_match_event_with_seed(players, settings, seed)?;

        match event {
            MatchEvent::GameStarted { settings, seed } => {
                Self::from_started_with_seed(players, settings, seed)
            }
            _ => unreachable!("start_match_event only emits GameStarted"),
        }
    }

    pub fn start_match_event(
        players: &[PlayerId],
        settings: GameSettings,
    ) -> Result<MatchEvent, GameError> {
        Self::start_match_event_with_seed(players, settings, rand::random())
    }

    pub fn start_match_event_with_seed(
        players: &[PlayerId],
        settings: GameSettings,
        seed: i64,
    ) -> Result<MatchEvent, GameError> {
        Self::validate_game(players, &settings)?;

        Ok(MatchEvent::GameStarted { settings, seed })
    }

    pub fn from_started(
        players: &[PlayerId],
        settings: GameSettings,
        set: NewSet,
    ) -> Result<Self, GameError> {
        Self::from_started_with_rules(players, settings, set, GameRules::default())
    }

    pub fn from_started_with_seed(
        players: &[PlayerId],
        settings: GameSettings,
        seed: i64,
    ) -> Result<Self, GameError> {
        let set = Self::new_set(
            players,
            DealingMode::Increasing,
            INITIAL_CARDS_COUNT,
            seed,
            0,
        );
        Self::from_started(players, settings, set)
    }

    pub fn from_started_with_rules(
        players: &[PlayerId],
        settings: GameSettings,
        set: NewSet,
        rules: GameRules,
    ) -> Result<Self, GameError> {
        Self::validate_game(players, &settings)?;

        let game_players = players
            .iter()
            .map(|id| {
                let deck = set.decks.get(id).cloned().unwrap_or_default();
                (id.clone(), Player::new(deck, settings.lifes))
            })
            .collect();

        Ok(Self {
            players: game_players,
            pile: BinaryHeap::new(),
            dealing_mode: set.dealing_mode,
            bidding_iter: CyclicIterator::new(players.len()),
            round_iter: CyclicIterator::new(players.len()),
            cards_count: set.cards_count,
            upcard: set.upcard,
            seed: set.shuffle.seed,
            next_shuffle_sequence: set.shuffle.sequence.wrapping_add(1),
            rules,
        })
    }

    pub fn validate_bid(
        &self,
        player_id: &PlayerId,
        bid: usize,
    ) -> Result<MatchEvent, BiddingError> {
        if self.get_stage() == GameStage::Dealing {
            return Err(BiddingError::DealingStageActive);
        }

        if !self.is_valid_bid(bid) {
            return Err(BiddingError::BidOutOfRange);
        }

        let current_bidder = self.peek_current_bidder();

        if current_bidder.as_ref() != Some(player_id) {
            return Err(BiddingError::NotYourTurn);
        }

        let player = self
            .players
            .get(player_id)
            .ok_or(BiddingError::InvalidPlayer)?;

        if player.bid.is_some() {
            return Err(BiddingError::AlreadyBidded);
        }

        Ok(MatchEvent::BidPlaced {
            player_id: player_id.clone(),
            bid,
        })
    }

    pub fn validate_turn(&self, turn: Turn) -> Result<MatchEvent, DealError> {
        if self.get_stage() == GameStage::Bidding {
            return Err(DealError::BiddingStageActive);
        }

        let current_dealer = self.peek_current_dealer();

        let player = self
            .players
            .get(&turn.player_id)
            .ok_or(DealError::InvalidPlayer)?;

        if current_dealer.as_ref() != Some(&turn.player_id) {
            return Err(DealError::NotYourTurn {
                expected: current_dealer,
            });
        }

        if !player.deck.contains(&turn.card) {
            return Err(DealError::InvalidCard);
        }

        Ok(MatchEvent::TurnPlayed { turn })
    }

    pub fn apply_match_event(&mut self, event: MatchEvent) -> AppliedGameChange {
        match event {
            MatchEvent::BidPlaced { player_id, bid } => {
                let state = self.apply_bid(&player_id, bid);

                AppliedGameChange::BidPlaced {
                    player_id,
                    bid,
                    state,
                }
            }
            MatchEvent::TurnPlayed { turn } => {
                let next_set = self.next_set_after_turn(&turn);
                AppliedGameChange::TurnPlayed(self.apply_turn(turn, next_set, true))
            }
            _ => unreachable!("only game play events can be applied to Game"),
        }
    }

    pub fn bid(&mut self, player_id: &PlayerId, bid: usize) -> Result<BiddingState, BiddingError> {
        let event = self.validate_bid(player_id, bid)?;

        match self.apply_match_event(event) {
            AppliedGameChange::BidPlaced { state, .. } => Ok(state),
            _ => unreachable!("bid emits bid event"),
        }
    }

    pub fn deal(&mut self, turn: Turn) -> Result<DealState, DealError> {
        let event = self.validate_turn(turn)?;

        match self.apply_match_event(event) {
            AppliedGameChange::TurnPlayed(state) => Ok(state),
            _ => unreachable!("deal emits turn event"),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.alive_players().count() <= 1
    }

    pub(crate) fn finalize_pending_set(&mut self, next_set: Option<&NewSet>) -> GameOutcome {
        self.remove_lifes();

        if self.is_finished() {
            return GameOutcome::Ended {
                lifes: self.get_lifes(),
            };
        }

        if let Some(set) = next_set {
            self.apply_new_set(set);

            return GameOutcome::SetEnded {
                lifes: self.get_lifes(),
                upcard: set.upcard,
                decks: set.decks.clone(),
                next: self.get_bidding_player(),
                possible: self.get_possible_bids(),
            };
        }

        GameOutcome::SetEnded {
            lifes: self.get_lifes(),
            upcard: self.upcard,
            decks: IndexMap::new(),
            next: self.get_bidding_player(),
            possible: self.get_possible_bids(),
        }
    }

    pub fn get_game_info(&self, player_id: &PlayerId) -> GameInfoDto {
        let player = self
            .players
            .get(player_id)
            .expect("Player should exist here");

        let info = self
            .players
            .iter()
            .map(|(id, p)| PlayerInfoDto {
                id: id.clone(),
                lifes: p.lifes,
                bid: p.bid,
                rounds: Some(p.rounds),
                mana: None,
            })
            .collect();

        let current_player = match self.get_stage() {
            GameStage::Bidding => self.peek_current_bidder(),
            GameStage::Dealing => self.peek_current_dealer(),
        }
        .expect("Should contain an active player");

        GameInfoDto {
            deck: Some(player.deck.clone()),
            power_cards: None,
            upcard: Some(self.upcard),
            info,
            current_player: current_player.0.to_string(),
            stage: self.get_stage_dto(),
        }
    }

    pub fn get_decks(&self) -> (IndexMap<PlayerId, Vec<Card>>, Card) {
        let decks = self
            .alive_players()
            .map(|(id, p)| (id.clone(), p.deck.clone()))
            .collect();

        (decks, self.upcard)
    }

    pub fn get_player_snapshots(&self) -> IndexMap<PlayerId, PlayerSnapshot> {
        self.players
            .iter()
            .map(|(id, player)| {
                (
                    id.clone(),
                    PlayerSnapshot {
                        lifes: player.lifes,
                        deck: player.deck.clone(),
                        bid: player.bid,
                        rounds: player.rounds,
                    },
                )
            })
            .collect()
    }

    pub fn apply_life_totals(&mut self, lifes: &HashMap<PlayerId, usize>) {
        let eliminated: Vec<_> = lifes
            .iter()
            .filter_map(|(id, life)| {
                let player = self.players.get_mut(id)?;

                if !player.is_alive() {
                    return None;
                }

                player.lifes = *life;

                (player.lifes == 0).then(|| id.clone())
            })
            .collect();

        for id in eliminated {
            let Some(idx) = self.players.get_index_of(&id) else {
                continue;
            };

            self.round_iter.remove(idx);
            self.bidding_iter.remove(idx);
        }
    }

    pub fn apply_decks(&mut self, decks: &HashMap<PlayerId, Vec<Card>>) {
        for (id, deck) in decks {
            if let Some(player) = self.players.get_mut(id)
                && player.is_alive()
            {
                player.deck = deck.clone();
            }
        }
    }

    pub fn current_player(&self) -> Option<PlayerId> {
        match self.get_stage() {
            GameStage::Bidding => self.peek_current_bidder(),
            GameStage::Dealing => self.peek_current_dealer(),
        }
    }

    pub fn get_stage_dto(&self) -> GameStageDto {
        match self.get_stage() {
            GameStage::Bidding => GameStageDto::Bidding {
                possible_bids: self.get_possible_bids(),
            },
            GameStage::Dealing => GameStageDto::Dealing,
        }
    }

    pub fn is_bidding_stage(&self) -> bool {
        self.get_stage() == GameStage::Bidding
    }

    pub fn is_player_alive(&self, player_id: &PlayerId) -> bool {
        self.players
            .get(player_id)
            .is_some_and(|player| player.is_alive())
    }

    pub fn current_trump(&self) -> Rank {
        self.upcard.rank.get_next()
    }

    pub fn get_bidding_player(&self) -> PlayerId {
        self.peek_current_bidder()
            .expect("Should contain a bidding player")
    }

    pub fn get_possible_bids(&self) -> Vec<usize> {
        let last = self.bidding_iter.peek_next().is_none();

        if last {
            (0..=self.cards_count)
                .filter(|&i| !self.makes_perfect_bidding_round(i, last))
                .collect()
        } else {
            (0..=self.cards_count).collect()
        }
    }

    pub fn get_round_order(&self) -> Vec<PlayerId> {
        let mut round_iter = self.round_iter.clone();

        round_iter
            .by_ref()
            .map(|idx| self.get_player(idx))
            .collect()
    }

    fn apply_bid(&mut self, player_id: &PlayerId, bid: usize) -> BiddingState {
        let player = self
            .players
            .get_mut(player_id)
            .expect("validated bid player should exist");

        player.bid = Some(bid);

        self.bidding_iter.next();

        match self.peek_current_bidder() {
            Some(next) => BiddingState::Active {
                next,
                possible_bids: self.get_possible_bids(),
            },
            None => {
                self.bidding_iter.shift();
                let next = self
                    .peek_current_dealer()
                    .expect("Should contain a dealing player");
                BiddingState::Ended { next }
            }
        }
    }

    pub(crate) fn apply_turn(
        &mut self,
        turn: Turn,
        next_set: Option<NewSet>,
        resolve_set_end: bool,
    ) -> DealState {
        let player = self
            .players
            .get_mut(&turn.player_id)
            .expect("validated turn player should exist");

        player.deck.retain(|&c| c != turn.card);

        self.pile
            .push((turn.card.get_trump_value(self.upcard), turn));
        self.round_iter.next();

        if self.alive_players().all(|(_, p)| p.deck.is_empty()) {
            let pile = self.award_points();
            self.remove_lifes();
            self.round_iter.shift();

            let lifes = self.get_lifes();
            let players_alive = self.alive_players().count();

            let outcome = match (players_alive, next_set) {
                (0 | 1, _) => GameOutcome::Ended { lifes },
                (_, Some(set)) if resolve_set_end => {
                    self.apply_new_set(&set);

                    GameOutcome::SetEnded {
                        lifes,
                        upcard: set.upcard,
                        decks: set.decks,
                        next: self.get_bidding_player(),
                        possible: self.get_possible_bids(),
                    }
                }
                (_, Some(_)) | (_, None) => GameOutcome::SetPending {
                    next: self.get_bidding_player(),
                },
            };

            return DealState { outcome, pile };
        }

        if self.pile.len() == self.alive_players().count() {
            let pile = self.award_points();
            let player_id = &pile[0].player_id;
            let idx = self
                .players
                .get_index_of(player_id)
                .expect("Player should be in the IndexMap");

            self.round_iter.shift_to(idx);

            let outcome = GameOutcome::RoundEnded {
                next: player_id.clone(),
                rounds: self.get_points(),
            };

            return DealState { outcome, pile };
        }

        DealState {
            pile: self.get_pile(),
            outcome: GameOutcome::TurnPlayed {
                next: self
                    .peek_current_dealer()
                    .expect("Should contain a dealing player"),
            },
        }
    }

    fn next_set_after_turn(&self, turn: &Turn) -> Option<NewSet> {
        let mut next = self.clone();
        let state = next.apply_turn(turn.clone(), None, true);

        match state.outcome {
            GameOutcome::SetEnded { .. } | GameOutcome::SetPending { .. } => {
                let (mode, count) = next
                    .dealing_mode
                    .get_next(next.cards_count, next.alive_players().count());
                let players: Vec<_> = next.alive_players().map(|(id, _)| id.clone()).collect();

                Some(next.new_set_for_game(&players, mode, count))
            }
            _ => None,
        }
    }

    pub(crate) fn next_set_for_turn(&self, turn: &Turn) -> Option<NewSet> {
        self.next_set_after_turn(turn)
    }

    fn apply_new_set(&mut self, set: &NewSet) {
        self.dealing_mode = set.dealing_mode;
        self.cards_count = set.cards_count;
        self.upcard = set.upcard;
        self.seed = set.shuffle.seed;
        self.next_shuffle_sequence = set.shuffle.sequence.wrapping_add(1);

        for (id, player) in self.players.iter_mut() {
            player.bid = None;
            if player.is_alive() {
                player.deck = set.decks.get(id).cloned().unwrap_or_default();
            }
        }
    }

    fn new_set_for_game(
        &self,
        players: &[PlayerId],
        mode: DealingMode,
        cards_count: usize,
    ) -> NewSet {
        Self::new_set(
            players,
            mode,
            cards_count,
            self.seed,
            self.next_shuffle_sequence,
        )
    }

    fn new_set(
        players: &[PlayerId],
        mode: DealingMode,
        cards_count: usize,
        seed: i64,
        sequence: i64,
    ) -> NewSet {
        let mut deck = Card::shuffled_deck(seed, sequence);
        let decks = players
            .iter()
            .map(|p| (p.clone(), deck.drain(..cards_count).collect()))
            .collect();

        NewSet {
            dealing_mode: mode,
            cards_count,
            shuffle: DeckShuffle { seed, sequence },
            decks,
            upcard: deck[0],
        }
    }

    fn get_stage(&self) -> GameStage {
        match self.alive_players().any(|(_, p)| p.bid.is_none()) {
            true => GameStage::Bidding,
            false => GameStage::Dealing,
        }
    }

    fn is_valid_bid(&self, bid: usize) -> bool {
        let last = self.bidding_iter.peek_next().is_none();

        bid <= self.cards_count && !self.makes_perfect_bidding_round(bid, last)
    }

    fn makes_perfect_bidding_round(&self, bid: usize, last: bool) -> bool {
        let current_bidding: usize = self
            .alive_players()
            .map(|(_, p)| p.bid.unwrap_or_default())
            .sum();

        last && bid + current_bidding == self.cards_count
    }

    fn award_points(&mut self) -> Vec<Turn> {
        let pile = self.get_pile();

        let (_, winner) = self.pile.pop().expect("Should contain a turn");

        self.pile.clear();

        let player = self
            .players
            .get_mut(&winner.player_id)
            .expect("This player should exist here");

        player.rounds += 1;

        pile
    }

    fn remove_lifes(&mut self) {
        let lost: Vec<_> = self
            .alive_players()
            .filter(|(_, p)| p.bid != Some(p.rounds))
            .map(|(id, _)| id.clone())
            .collect();

        for id in lost {
            let (idx, _, player) = self
                .players
                .get_full_mut(&id)
                .expect("Player should exist here");

            let bid = player.bid.expect("should have bid here");
            let diff = player.rounds.abs_diff(bid);

            let life_loss = diff.saturating_mul(self.rules.life_loss_per_bid_difference);
            player.lifes = player.lifes.saturating_sub(life_loss);

            if player.lifes == 0 {
                self.round_iter.remove(idx);
                self.bidding_iter.remove(idx);
            }
        }

        for (_, player) in self.players.iter_mut() {
            if player.is_alive() {
                player.rounds = 0;
            }
        }
    }

    fn get_pile(&self) -> Vec<Turn> {
        self.pile.iter().cloned().map(|(_, t)| t).collect()
    }

    fn get_points(&self) -> HashMap<PlayerId, usize> {
        self.alive_players()
            .map(|(id, player)| (id.clone(), player.rounds))
            .collect()
    }

    pub fn get_lifes(&self) -> HashMap<PlayerId, usize> {
        self.players
            .iter()
            .map(|(id, player)| (id.clone(), player.lifes))
            .collect()
    }

    fn peek_current_dealer(&self) -> Option<PlayerId> {
        self.round_iter.peek().map(|i| self.get_player(i))
    }

    fn peek_current_bidder(&self) -> Option<PlayerId> {
        self.bidding_iter.peek().map(|i| self.get_player(i))
    }

    fn get_player(&self, idx: usize) -> PlayerId {
        match self.players.get_index(idx) {
            Some((id, _)) => id.clone(),
            None => {
                let msg = format!("InvalidGameState: invalid player index: {idx}");
                tracing::error!(msg);
                panic!("{msg}");
            }
        }
    }

    fn validate_game(players: &[PlayerId], _settings: &GameSettings) -> Result<(), GameError> {
        if players.len() < 2 {
            return Err(GameError::NotEnoughPlayers);
        }

        if players.len() > MAX_PLAYER_COUNT {
            return Err(GameError::TooManyPlayers);
        }

        Ok(())
    }

    fn alive_players(&self) -> impl Iterator<Item = (&PlayerId, &Player)> {
        self.players.iter().filter(|(_, p)| p.is_alive())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_game() {
        let player1 = PlayerId("P1".into());
        let player2 = PlayerId("P2".into());

        let mut game = Game::new_default(&[player1.clone(), player2.clone()]).unwrap();
        assert!(game.pile.is_empty());

        let state = game.bid(&player1, 1).unwrap();
        assert!(
            matches!(state, BiddingState::Active { next, possible_bids: _ } if next == player2)
        );

        let state = game.bid(&player2, 1).unwrap();
        assert!(matches!(state, BiddingState::Ended { next } if next == player1));

        let first_played_card = game.players[&player1].deck[0];
        let first_turn = Turn {
            player_id: player1.clone(),
            card: first_played_card,
        };

        game.deal(first_turn).unwrap();

        assert!(game.pile.len() == 1);
        assert!(game.pile.peek().map(|(_, t)| t.card) == Some(first_played_card));

        let second_played_card = game.players[&player2].deck[0];
        let second_turn = Turn {
            player_id: player2.clone(),
            card: second_played_card,
        };

        let state = game.deal(second_turn).unwrap();

        assert!(matches!(
            state.outcome,
            GameOutcome::SetEnded {
                lifes: _,
                upcard: _,
                decks: _,
                next: _,
                possible: _,
            }
        ));

        assert!(state.pile.len() == 2);

        let winners_count = game.players.iter().filter(|(_, p)| p.lifes == 5).count();

        assert!(winners_count == 1);

        let next_bidder = game.get_bidding_player();
        let other = if next_bidder == player1 {
            player2.clone()
        } else {
            player1.clone()
        };

        let state = game.bid(&next_bidder, 2).unwrap();
        assert!(matches!(state, BiddingState::Active { next, possible_bids: _ } if next == other));

        let state = game.bid(&other, 2).unwrap();
        assert!(matches!(state, BiddingState::Ended { next: _ }));
    }

    #[test]
    fn test_seeded_decks_are_reproducible() {
        let player1 = PlayerId("P1".into());
        let player2 = PlayerId("P2".into());
        let players = [player1.clone(), player2.clone()];
        let settings = GameSettings::default();
        let seed = 42;

        let first = Game::new_with_seed(&players, settings.clone(), seed).unwrap();
        let second = Game::new_with_seed(&players, settings, seed).unwrap();

        assert_eq!(first.upcard, second.upcard);
        assert_eq!(first.players[&player1].deck, second.players[&player1].deck);
        assert_eq!(first.players[&player2].deck, second.players[&player2].deck);
        assert_eq!(first.seed, seed);
        assert_eq!(first.next_shuffle_sequence, 1);
    }

    #[test]
    fn test_invalid_bid() {
        let player1 = PlayerId("P1".into());
        let player2 = PlayerId("P2".into());

        let mut game = Game::new_default(&[player1.clone(), player2.clone()]).unwrap();

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![0, 1]);

        let state = game.bid(&player1, 1).unwrap();
        assert!(
            matches!(state, BiddingState::Active { next, possible_bids: _ } if next == player2)
        );

        let result = game.bid(&player2, 0);
        assert_eq!(result, Err(BiddingError::BidOutOfRange));

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![1]);
    }

    #[test]
    fn test_game_max_players() {
        for p in 0..MAX_PLAYER_COUNT + 3 {
            let players: Vec<_> = (0..p).map(|i| PlayerId(i.to_string().into())).collect();
            let result = Game::new_default(&players);

            match p {
                2..=MAX_PLAYER_COUNT => {
                    assert!(matches!(result, Ok(g) if g.players.len() == p))
                }
                0..=1 => {
                    assert!(matches!(result, Err(e) if matches!(e, GameError::NotEnoughPlayers)))
                }
                _ => assert!(matches!(result, Err(e) if matches!(e, GameError::TooManyPlayers))),
            }
        }
    }

    #[test]
    fn test_possible_bid() {
        let player1 = PlayerId("P1".into());
        let player2 = PlayerId("P2".into());

        let mut game = game_with_cards_count(&[player1.clone(), player2.clone()], 2);

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![0, 1, 2]);

        game.bid(&player1, 1).unwrap();

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![0, 2]);

        let mut game = game_with_cards_count(&[player1.clone(), player2], 3);

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![0, 1, 2, 3]);

        game.bid(&player1, 3).unwrap();

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![1, 2, 3]);
    }

    fn game_with_cards_count(players: &[PlayerId], cards_count: usize) -> Game {
        let set = Game::new_set(players, DealingMode::Increasing, cards_count, 1, 0);

        Game::from_started(players, GameSettings::default(), set).unwrap()
    }

    #[test]
    fn next_set_clears_bids_for_eliminated_players() {
        let player1 = PlayerId("P1".into());
        let player2 = PlayerId("P2".into());
        let mut game = game_with_cards_count(&[player1.clone(), player2.clone()], 1);

        game.players.get_mut(&player1).unwrap().lifes = 0;
        game.players.get_mut(&player1).unwrap().bid = Some(1);
        game.players.get_mut(&player2).unwrap().bid = Some(0);

        let next_set = game.new_set_for_game(&[player2.clone()], DealingMode::Increasing, 2);
        game.apply_new_set(&next_set);

        assert_eq!(game.players[&player1].bid, None);
        assert_eq!(game.players[&player2].bid, None);
    }

    #[test]
    fn test_card_mode() {
        assert_eq!(
            DealingMode::Increasing.get_next(1, 4),
            (DealingMode::Increasing, 2)
        );

        assert_eq!(
            DealingMode::Decreasing.get_next(1, 4),
            (DealingMode::Increasing, 2)
        );

        assert_eq!(
            DealingMode::Increasing.get_next(2, 4),
            (DealingMode::Increasing, 3)
        );

        assert_eq!(
            DealingMode::Increasing.get_next(7, 5),
            (DealingMode::Decreasing, 6)
        );
    }
}
