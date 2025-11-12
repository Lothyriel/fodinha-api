use std::collections::{BinaryHeap, HashMap};

use indexmap::IndexMap;

use crate::services::{GameInfoDto, GameStageDto, PlayerInfoDto, manager::PlayerId};

use super::{
    BiddingError, BiddingState, Card, DealState, DealingMode, GameError, GameEvent, Player, Turn,
    TurnError, iter::CyclicIterator,
};

#[derive(Debug)]
pub struct Game {
    players: IndexMap<PlayerId, Player>,
    pile: BinaryHeap<(u16, Turn)>,
    dealing_mode: DealingMode,
    bidding_iter: CyclicIterator,
    round_iter: CyclicIterator,
    cards_count: usize,
    upcard: Card,
}

#[derive(PartialEq, Debug, Eq)]
enum GameStage {
    Dealing,
    Bidding,
}

const MAX_AVAILABLE_CARDS: usize = 40 - 1;
pub const MAX_PLAYER_COUNT: usize = 13;

impl Game {
    pub fn new_default(players: &[PlayerId]) -> Result<Self, GameError> {
        Self::new(players, 1)
    }

    pub fn new(player_names: &[PlayerId], initial_cards_count: usize) -> Result<Self, GameError> {
        Self::validate_game(player_names)?;

        let (players, upcard) = Self::init_players(player_names, initial_cards_count);

        Ok(Self {
            players,
            pile: BinaryHeap::new(),
            dealing_mode: DealingMode::Increasing,
            cards_count: initial_cards_count,
            bidding_iter: CyclicIterator::new(player_names.len()),
            round_iter: CyclicIterator::new(player_names.len()),
            upcard,
        })
    }

    pub fn deal(&mut self, turn: Turn) -> Result<DealState, TurnError> {
        if self.get_stage() == GameStage::Bidding {
            return Err(TurnError::BiddingStageActive);
        }

        let current_dealer = self.peek_current_dealer();

        let player = self
            .players
            .get_mut(&turn.player_id)
            .ok_or(TurnError::InvalidPlayer)?;

        if current_dealer.as_ref() != Some(&turn.player_id) {
            return Err(TurnError::NotYourTurn {
                expected: current_dealer,
            });
        }

        if !player.deck.contains(&turn.card) {
            return Err(TurnError::NotYourCard);
        }

        player.deck.retain(|&c| c != turn.card);

        //add card to the heap
        self.pile.push((self.get_card_value(turn.card), turn));
        self.round_iter.next();

        //finish set/game
        if self.alive_players().all(|(_, p)| p.deck.is_empty()) {
            let pile = self.award_points();
            self.remove_lifes();
            self.round_iter.shift();

            let players_alive: Vec<_> = self.alive_players().collect();

            let event = match players_alive.len() {
                0 | 1 => GameEvent::Ended {
                    lifes: self.get_lifes(),
                },
                _ => {
                    self.start_new_set();

                    let (decks, upcard) = self.get_decks();

                    GameEvent::SetEnded {
                        lifes: self.get_lifes(),
                        possible: self.get_possible_bids(),
                        next: self.get_bidding_player(),
                        upcard,
                        decks,
                    }
                }
            };

            return Ok(DealState { event, pile });
        }

        //finish round
        if self.pile.len() == self.alive_players().count() {
            let pile = self.award_points();

            let player_id = &pile[0].player_id;

            let idx = self
                .players
                .get_index_of(player_id)
                .expect("Player should be in the IndexMap");

            self.round_iter.shift_to(idx);

            let event = GameEvent::RoundEnded {
                next: player_id.clone(),
                rounds: self.get_points(),
            };

            return Ok(DealState { event, pile });
        }

        let event = GameEvent::TurnPlayed {
            next: self.peek_current_dealer().expect("Should have a dealer"),
        };

        Ok(DealState {
            pile: self.get_pile(),
            event,
        })
    }

    pub fn bid(&mut self, player_id: &PlayerId, bid: usize) -> Result<BiddingState, BiddingError> {
        if self.get_stage() == GameStage::Dealing {
            return Err(BiddingError::DealingStageActive);
        }

        if !self.validate_bid(bid) {
            return Err(BiddingError::BidOutOfRange);
        }

        let current_bidder = self.peek_current_bidder();

        if current_bidder.filter(|c| c == player_id).is_none() {
            return Err(BiddingError::NotYourTurn);
        }

        let player = self
            .players
            .get_mut(player_id)
            .ok_or(BiddingError::InvalidPlayer)?;

        if player.bid.is_some() {
            return Err(BiddingError::AlreadyBidded);
        }

        player.bid = Some(bid);

        self.bidding_iter.next();

        let state = match self.peek_current_bidder() {
            Some(next) => BiddingState::Active {
                next,
                possible_bids: self.get_possible_bids(),
            },
            None => {
                self.bidding_iter.shift();
                let next = self.peek_current_dealer().expect("Should have a dealer");
                BiddingState::Ended { next }
            }
        };

        Ok(state)
    }

    pub fn get_bidding_player(&self) -> PlayerId {
        let idx = match self.bidding_iter.peek() {
            Some(i) => i,
            None => {
                let msg = "InvalidGameState getting bid player";
                tracing::error!(msg);
                panic!("{msg}");
            }
        };

        self.get_player(idx)
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

    pub fn get_decks(&self) -> (IndexMap<PlayerId, Vec<Card>>, Card) {
        let decks = self
            .alive_players()
            .map(|(id, p)| (id.clone(), p.deck.clone()))
            .collect();

        (decks, self.upcard)
    }

    pub fn get_game_info(&self, player_id: &PlayerId) -> GameInfoDto {
        let player = self
            .players
            .get(player_id)
            .expect("Player should exist here");

        let deck = player.deck.clone();

        let info = self
            .alive_players()
            .map(|(id, p)| PlayerInfoDto {
                id: id.clone(),
                lifes: p.lifes,
                bid: p.bid,
                rounds: p.rounds,
            })
            .collect();

        let current_player = match self.get_stage() {
            GameStage::Dealing => self.peek_current_dealer(),
            GameStage::Bidding => self.peek_current_bidder(),
        }
        .expect("Should contain an active player");

        GameInfoDto {
            deck,
            upcard: self.upcard,
            info,
            current_player: current_player.0.to_string(),
            stage: match self.get_stage() {
                GameStage::Dealing => GameStageDto::Dealing,
                GameStage::Bidding => GameStageDto::Bidding {
                    possible_bids: self.get_possible_bids(),
                },
            },
        }
    }

    fn get_pile(&self) -> Vec<Turn> {
        self.pile.iter().cloned().map(|(_, t)| t).collect()
    }

    fn validate_bid(&mut self, bid: usize) -> bool {
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

    fn get_stage(&self) -> GameStage {
        match self.alive_players().any(|(_, p)| p.bid.is_none()) {
            true => GameStage::Bidding,
            false => GameStage::Dealing,
        }
    }

    fn start_new_set(&mut self) {
        let (mode, count) = Self::get_new_dealing_mode(
            self.dealing_mode,
            self.cards_count,
            self.alive_players().count(),
        );

        self.dealing_mode = mode;
        self.cards_count = count;

        let mut deck = Card::shuffled_deck();

        let n = self.cards_count;

        for (_, player) in self.alive_players_mut() {
            player.deck = deck.drain(..n).collect();
            player.bid = None;
        }

        self.upcard = deck[0];
    }

    fn get_new_dealing_mode(
        mode: DealingMode,
        count: usize,
        player_count: usize,
    ) -> (DealingMode, usize) {
        match mode {
            DealingMode::Increasing => {
                if count + 1 < MAX_AVAILABLE_CARDS / player_count {
                    (DealingMode::Increasing, count + 1)
                } else {
                    (DealingMode::Decreasing, count - 1)
                }
            }
            DealingMode::Decreasing => {
                if count - 1 == 0 {
                    (DealingMode::Increasing, count + 1)
                } else {
                    (DealingMode::Decreasing, count - 1)
                }
            }
        }
    }

    fn init_players(players: &[PlayerId], cards: usize) -> (IndexMap<PlayerId, Player>, Card) {
        let mut deck = Card::shuffled_deck();

        let decks = players
            .iter()
            .map(|p| (p.clone(), Player::new(deck.drain(..cards).collect())))
            .collect();

        (decks, deck[0])
    }

    fn remove_lifes(&mut self) {
        let lost = self
            .alive_players_mut()
            .filter(|(_, p)| p.bid != Some(p.rounds));

        for (_, player) in lost {
            player.decrease_life();
        }

        for (_, p) in self.alive_players_mut() {
            p.rounds = 0;
        }

        let unalive_players = self
            .players
            .iter()
            .enumerate()
            .filter(|(_, (_, p))| p.lifes == 0);

        for (idx, _) in unalive_players {
            self.round_iter.remove(idx);
            self.bidding_iter.remove(idx);
        }
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

    fn get_points(&self) -> HashMap<PlayerId, usize> {
        self.alive_players()
            .map(|(id, player)| (id.clone(), player.rounds))
            .collect()
    }

    fn get_lifes(&self) -> HashMap<PlayerId, usize> {
        self.players
            .iter()
            .map(|(id, player)| (id.clone(), player.lifes))
            .collect()
    }

    fn get_card_value(&self, card: Card) -> u16 {
        let card_value = card.get_value() as u16;

        if self.upcard.rank.get_next() == card.rank {
            card_value + 100
        } else {
            card_value
        }
    }

    fn peek_current_dealer(&self) -> Option<PlayerId> {
        self.round_iter.peek().map(|i| self.get_player(i))
    }

    fn get_player(&self, idx: usize) -> PlayerId {
        match self.players.get_index(idx) {
            Some(p) => p.0.clone(),
            None => {
                let msg = format!("InvalidGameState: invalid player index: {idx}");
                tracing::error!(msg);
                panic!("{msg}");
            }
        }
    }

    fn peek_current_bidder(&self) -> Option<PlayerId> {
        self.bidding_iter.peek().map(|i| self.get_player(i))
    }

    fn validate_game(players: &[PlayerId]) -> Result<(), GameError> {
        if players.len() < 2 {
            return Err(GameError::NotEnoughPlayers);
        }

        if players.len() > MAX_PLAYER_COUNT {
            return Err(GameError::TooManyPlayers);
        }

        Ok(())
    }

    fn alive_players(&self) -> impl Iterator<Item = (&PlayerId, &Player)> {
        self.players.iter().filter(|(_, p)| p.lifes > 0)
    }

    fn alive_players_mut(&mut self) -> impl Iterator<Item = (&PlayerId, &mut Player)> {
        self.players.iter_mut().filter(|(_, p)| p.is_alive())
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
            state.event,
            GameEvent::SetEnded {
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

        let state = game.bid(&player2, 2).unwrap();
        assert!(
            matches!(state, BiddingState::Active { next, possible_bids: _ } if next == player1)
        );

        let state = game.bid(&player1, 2).unwrap();
        assert!(matches!(state, BiddingState::Ended { next } if next == player2));
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
                2..=MAX_PLAYER_COUNT => assert!(matches!(result, Ok(g) if g.players.len() == p)),
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

        let mut game = Game::new(&[player1.clone(), player2.clone()], 2).unwrap();

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![0, 1, 2]);

        game.bid(&player1, 1).unwrap();

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![0, 2]);

        let mut game = Game::new(&[player1.clone(), player2], 3).unwrap();

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![0, 1, 2, 3]);

        game.bid(&player1, 3).unwrap();

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![1, 2, 3]);
    }

    #[test]
    fn test_card_mode() {
        assert_eq!(
            Game::get_new_dealing_mode(DealingMode::Increasing, 1, 4),
            (DealingMode::Increasing, 2)
        );

        assert_eq!(
            Game::get_new_dealing_mode(DealingMode::Decreasing, 1, 4),
            (DealingMode::Increasing, 2)
        );

        assert_eq!(
            Game::get_new_dealing_mode(DealingMode::Increasing, 2, 4),
            (DealingMode::Increasing, 3)
        );

        assert_eq!(
            Game::get_new_dealing_mode(DealingMode::Increasing, 7, 5),
            (DealingMode::Decreasing, 6)
        );
    }
}
