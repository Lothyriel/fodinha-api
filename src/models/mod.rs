mod game;
pub mod iter;

use std::collections::{HashMap, HashSet};

pub use game::{Game, MAX_PLAYER_COUNT};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use rand::seq::SliceRandom;
use strum_macros::{Display, EnumIter};

use crate::services::manager::PlayerId;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Turn {
    pub player_id: PlayerId,
    pub card: Card,
}

impl PartialOrd for Turn {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Turn {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.card.cmp(&other.card)
    }
}

#[derive(Debug)]
pub struct Player {
    lifes: usize,
    deck: Vec<Card>,
    bid: Option<usize>,
    rounds: usize,
}

impl Player {
    pub fn new(deck: Vec<Card>) -> Self {
        Self {
            lifes: 5,
            deck,
            bid: None,
            rounds: 0,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.lifes != 0
    }

    pub fn decrease_life(&mut self) {
        self.lifes -= 1;
    }
}

#[derive(
    Debug, serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, PartialOrd, Eq, Ord,
)]
pub struct Card {
    pub rank: Rank,
    pub suit: Suit,
}

impl Card {
    pub fn new(rank: Rank, suit: Suit) -> Self {
        Self { rank, suit }
    }

    pub fn deck() -> Vec<Card> {
        Rank::iter()
            .flat_map(|rank| Suit::iter().map(move |suit| Card { suit, rank }))
            .collect()
    }

    pub fn shuffled_deck() -> Vec<Card> {
        let mut deck = Self::deck();

        deck.shuffle(&mut rand::thread_rng());

        deck
    }

    fn get_value(&self) -> u8 {
        let rank = self.rank as u8 * 10;
        let suit = self.suit as u8;
        rank + suit
    }
}

#[derive(Debug, Serialize, Deserialize, EnumIter, Clone, Copy, PartialEq, PartialOrd, Eq, Ord)]
pub enum Rank {
    Four,
    Five,
    Six,
    Seven,

    Ten,
    Eleven,
    Twelve,

    One,
    Two,
    Three,
}

impl Rank {
    fn get_next(&self) -> Rank {
        match self {
            Rank::Four => Rank::Five,
            Rank::Five => Rank::Six,
            Rank::Six => Rank::Seven,
            Rank::Seven => Rank::Ten,
            Rank::Ten => Rank::Eleven,
            Rank::Eleven => Rank::Twelve,
            Rank::Twelve => Rank::One,
            Rank::One => Rank::Two,
            Rank::Two => Rank::Three,
            Rank::Three => Rank::Four,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, EnumIter, Clone, Copy, PartialEq, PartialOrd, Eq, Ord)]
pub enum Suit {
    Golds,
    Swords,
    Cups,
    Clubs,
}

#[derive(Debug)]
pub enum LobbyState {
    NotStarted(HashSet<PlayerId>),
    Playing(Game),
}

#[derive(Debug)]
pub enum GameEvent {
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

pub struct DealState {
    pub event: GameEvent,
    pub pile: Vec<Turn>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BiddingState {
    Active {
        next: PlayerId,
        possible_bids: Vec<usize>,
    },
    Ended {
        next: PlayerId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DealingMode {
    Increasing,
    Decreasing,
}

#[derive(thiserror::Error, Debug)]
pub enum GameError {
    #[error("Not enough players")]
    NotEnoughPlayers,
    #[error("Too many players")]
    TooManyPlayers,
    #[error("Invalid turn | {0}")]
    InvalidTurn(#[from] TurnError),
    #[error("Invalid bid | {0}")]
    InvalidBid(#[from] BiddingError),
}

#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    #[error("BiddingStageActive")]
    BiddingStageActive,
    #[error("Expected {expected:?}")]
    NotYourTurn { expected: Option<PlayerId> },
    #[error("NotYourCard")]
    NotYourCard,
    #[error("InvalidPlayer")]
    InvalidPlayer,
}

#[derive(Debug, thiserror::Error, Display, PartialEq, Eq)]
pub enum BiddingError {
    InvalidPlayer,
    AlreadyBidded,
    DealingStageActive,
    NotYourTurn,
    BidOutOfRange,
}

#[cfg(test)]
mod tests {
    use crate::models::{Card, Rank, Suit};

    #[test]
    fn test_rank() {
        let a = Card::new(Rank::Six, Suit::Clubs);
        let b = Card::new(Rank::Seven, Suit::Golds);

        assert!(a < b);
    }

    #[test]
    fn test_rank_2() {
        let a = Card::new(Rank::Twelve, Suit::Clubs);
        let b = Card::new(Rank::Three, Suit::Golds);

        assert!(a < b);
    }

    #[test]
    fn test_suit() {
        let a = Card::new(Rank::Six, Suit::Clubs);
        let b = Card::new(Rank::Six, Suit::Golds);

        assert!(a > b);
    }

    #[test]
    fn test_value() {
        assert!(Card::new(Rank::Four, Suit::Golds).get_value() == 0);
        assert!(Card::new(Rank::Four, Suit::Clubs).get_value() == 3);
        assert!(Card::new(Rank::Six, Suit::Golds).get_value() == 20);
        assert!(Card::new(Rank::Six, Suit::Clubs).get_value() == 23);
        assert!(Card::new(Rank::Twelve, Suit::Clubs).get_value() == 63);
        assert!(Card::new(Rank::One, Suit::Clubs).get_value() == 73);
        assert!(Card::new(Rank::Three, Suit::Golds).get_value() == 90);
        assert!(Card::new(Rank::Three, Suit::Clubs).get_value() == 93);
    }
}
