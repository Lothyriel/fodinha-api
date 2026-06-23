mod projection;
mod projector;

use std::collections::HashMap;

use crate::{
    infra::UserClaims,
    models::{Card, Rank, Suit},
};

pub(crate) use projection::project_match_stats;
pub use projector::{StatsProjector, StatsProjectorHandle};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MatchPlayerStats {
    pub match_id: String,
    pub player_id: String,
    pub games_played: i64,
    pub matches_won: i64,
    pub rounds_won: i64,
    pub trump_cards: i64,
    pub total_bid: i64,
    pub bid_count: i64,
    pub bids_hit: i64,
    pub bids_missed: i64,
    pub winning_cards: HashMap<String, i64>,
}

impl MatchPlayerStats {
    pub(crate) fn new(match_id: String, player_id: String) -> Self {
        Self {
            match_id,
            player_id,
            games_played: 0,
            matches_won: 0,
            rounds_won: 0,
            trump_cards: 0,
            total_bid: 0,
            bid_count: 0,
            bids_hit: 0,
            bids_missed: 0,
            winning_cards: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PlayerStats {
    pub player_id: String,
    pub games_played: i64,
    pub matches_won: i64,
    pub rounds_won: i64,
    pub trump_cards: i64,
    pub total_bid: i64,
    pub bid_count: i64,
    pub bids_hit: i64,
    pub bids_missed: i64,
    pub winning_cards: HashMap<String, i64>,
}

impl PlayerStats {
    pub(crate) fn new(player_id: String) -> Self {
        Self {
            player_id,
            ..Default::default()
        }
    }

    pub(crate) fn apply_match(&mut self, stats: &MatchPlayerStats) {
        self.games_played += stats.games_played;
        self.matches_won += stats.matches_won;
        self.rounds_won += stats.rounds_won;
        self.trump_cards += stats.trump_cards;
        self.total_bid += stats.total_bid;
        self.bid_count += stats.bid_count;
        self.bids_hit += stats.bids_hit;
        self.bids_missed += stats.bids_missed;

        for (card, wins) in &stats.winning_cards {
            *self.winning_cards.entry(card.clone()).or_default() += wins;
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlayerStatsResponse {
    pub player_id: String,
    pub player: Option<UserClaims>,
    pub games_played: i64,
    pub matches_won: i64,
    pub rounds_won: i64,
    pub trump_cards: i64,
    pub bid_count: i64,
    pub total_bid: i64,
    pub average_bid: f64,
    pub bids_hit: i64,
    pub bids_missed: i64,
    pub bid_accuracy: f64,
    pub win_rate: f64,
    pub favorite_card: Option<Card>,
    pub favorite_card_wins: i64,
}

impl PlayerStats {
    pub(crate) fn into_response(self, player: Option<UserClaims>) -> PlayerStatsResponse {
        let average_bid = ratio(self.total_bid, self.bid_count);
        let bid_accuracy = percent(self.bids_hit, self.bid_count);
        let win_rate = percent(self.matches_won, self.games_played);
        let (favorite_card, favorite_card_wins) = favorite_card(&self.winning_cards);

        PlayerStatsResponse {
            player_id: self.player_id,
            player,
            games_played: self.games_played,
            matches_won: self.matches_won,
            rounds_won: self.rounds_won,
            trump_cards: self.trump_cards,
            bid_count: self.bid_count,
            total_bid: self.total_bid,
            average_bid,
            bids_hit: self.bids_hit,
            bids_missed: self.bids_missed,
            bid_accuracy,
            win_rate,
            favorite_card,
            favorite_card_wins,
        }
    }
}

pub(crate) fn card_key(card: Card) -> String {
    format!("{:?}_{:?}", card.rank, card.suit)
}

fn favorite_card(winning_cards: &HashMap<String, i64>) -> (Option<Card>, i64) {
    winning_cards
        .iter()
        .filter_map(|(key, wins)| card_from_key(key).map(|card| (card, *wins)))
        .max_by_key(|(card, wins)| (*wins, *card))
        .map(|(card, wins)| (Some(card), wins))
        .unwrap_or((None, 0))
}

fn card_from_key(key: &str) -> Option<Card> {
    let (rank, suit) = key.split_once('_')?;

    Some(Card {
        rank: rank_from_key(rank)?,
        suit: suit_from_key(suit)?,
    })
}

fn rank_from_key(rank: &str) -> Option<Rank> {
    match rank {
        "Four" => Some(Rank::Four),
        "Five" => Some(Rank::Five),
        "Six" => Some(Rank::Six),
        "Seven" => Some(Rank::Seven),
        "Ten" => Some(Rank::Ten),
        "Eleven" => Some(Rank::Eleven),
        "Twelve" => Some(Rank::Twelve),
        "One" => Some(Rank::One),
        "Two" => Some(Rank::Two),
        "Three" => Some(Rank::Three),
        _ => None,
    }
}

fn suit_from_key(suit: &str) -> Option<Suit> {
    match suit {
        "Golds" => Some(Suit::Golds),
        "Swords" => Some(Suit::Swords),
        "Cups" => Some(Suit::Cups),
        "Clubs" => Some(Suit::Clubs),
        _ => None,
    }
}

fn ratio(value: i64, total: i64) -> f64 {
    match total {
        0 => 0.0,
        total => value as f64 / total as f64,
    }
}

fn percent(value: i64, total: i64) -> f64 {
    ratio(value, total) * 100.0
}
