pub mod commands;
pub mod game;
pub mod id;
pub mod lobby;
pub mod util;

use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use strum_macros::{Display, EnumIter};

use id::PlayerId;
use util::DeterministicRng;

pub use game::{Game, GameOutcome, LobbyState};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, PartialOrd, Eq, Ord)]
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

    pub fn shuffled_deck(seed: i64, sequence: i64) -> Vec<Card> {
        let mut deck = Self::deck();
        let mut rng = DeterministicRng::new(seed, sequence);

        for i in (1..deck.len()).rev() {
            deck.swap(i, rng.next_index(i + 1));
        }

        deck
    }

    fn get_value(&self) -> u8 {
        let rank = self.rank as u8 * 10;
        let suit = self.suit as u8;
        rank + suit
    }

    pub(crate) fn get_trump_value(&self, upcard: Card) -> u8 {
        let card_value = self.get_value();

        if upcard.rank.get_next() == self.rank {
            card_value + 100
        } else {
            card_value
        }
    }

    pub(crate) fn is_trump(&self, upcard: Card) -> bool {
        upcard.rank.get_next() == self.rank
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

#[derive(thiserror::Error, Debug)]
pub enum GameError {
    #[error("Not enough players")]
    NotEnoughPlayers,
    #[error("Too many players")]
    TooManyPlayers,
    #[error("Invalid stage")]
    InvalidStage,
    #[error("Invalid deal | {0}")]
    InvalidDeal(#[from] DealError),
    #[error("Invalid bid | {0}")]
    InvalidBid(#[from] BiddingError),
}

#[derive(Debug, thiserror::Error)]
pub enum DealError {
    #[error("Bidding stage active")]
    BiddingStageActive,
    #[error("Expected {expected:?}")]
    NotYourTurn { expected: Option<PlayerId> },
    #[error("Invalid card")]
    InvalidCard,
    #[error("Invalid player")]
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

        let upcard = Card::new(Rank::Three, Suit::Clubs);
        let gold_trump_value = Card::new(Rank::Four, Suit::Golds).get_trump_value(upcard);

        assert!(gold_trump_value > upcard.get_value());
        assert!(gold_trump_value == 100);
    }
}
