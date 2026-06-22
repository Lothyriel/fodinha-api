use indexmap::IndexMap;

use super::{data::GameData, id::PlayerId, *};

#[derive(Debug)]
pub struct BiddingStage {
    players: IndexMap<PlayerId, BiddingPlayer>,
    data: GameData,
}

impl BiddingStage {
    pub fn new(data: GameData) -> Self {
        let players = data
            .players
            .iter()
            .map(|(id, _)| {
                let player = BiddingPlayer { bid: None };
                (id.clone(), player)
            })
            .collect();

        Self { players, data }
    }

    pub fn bid(self, player_id: &PlayerId, bid: usize) -> Result<Game, BiddingError> {
        if !self.validate_bid(bid) {
            return Err(BiddingError::BidOutOfRange);
        }

        let current_bidder = self.data.peek_current();

        if current_bidder.filter(|c| *c == player_id).is_none() {
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

        self.data.order.next();

        match self.data.peek_current() {
            Some(next) => BiddingState::Active {
                next: next.clone(),
                possible_bids: self.get_possible_bids(),
            },
            None => {
                self.data.order.shift();
                let next = self.data.get_current().clone();
                BiddingState::Ended { next }
            }
        }
    }

    pub fn get_info(&self) -> GameInfoDto {
        let info = self
            .players
            .iter()
            .map(|(id, p)| PlayerInfoDto {
                id: id.clone(),
                lifes: self.data.get_player_data(id).1.lifes,
                bid: p.bid,
                rounds: None,
            })
            .collect();

        let current_player = self.data().get_current();

        GameInfoDto {
            deck: None,
            upcard: None,
            info,
            current_player: current_player.0.to_string(),
            stage: GameStageDto::Bidding {
                possible_bids: self.get_possible_bids(),
            },
        }
    }

    pub fn get_possible_bids(&self) -> Vec<usize> {
        let last = self.data.order.peek_next().is_none();

        if last {
            (0..=self.data.cards_count)
                .filter(|&i| !self.makes_perfect_bidding_round(i, last))
                .collect()
        } else {
            (0..=self.data.cards_count).collect()
        }
    }

    pub fn data(&self) -> &GameData {
        &self.data
    }

    fn validate_bid(&mut self, bid: usize) -> bool {
        let last = self.data.order.peek_next().is_none();

        bid <= self.data.cards_count && !self.makes_perfect_bidding_round(bid, last)
    }

    fn makes_perfect_bidding_round(&self, bid: usize, last: bool) -> bool {
        let current_bidding: usize = self
            .players
            .iter()
            .map(|(_, p)| p.bid.unwrap_or_default())
            .sum();

        last && bid + current_bidding == self.data.cards_count
    }
}
