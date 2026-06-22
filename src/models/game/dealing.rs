use std::collections::BinaryHeap;

use indexmap::IndexMap;

use super::game::{data::GameData, id::PlayerId, *};

#[derive(Debug)]
pub struct DealingStage {
    players: IndexMap<PlayerId, DealingPlayer>,
    pile: BinaryHeap<(u8, Turn)>,
    upcard: Card,
    data: GameData,
}

impl DealingStage {
    fn new(players: IndexMap<PlayerId, BiddingPlayer>, data: GameData) -> Self {
        let mut deck = Card::shuffled_deck();

        let players = players
            .into_iter()
            .map(|(id, p)| {
                let p = DealingPlayer {
                    bid: p.bid.expect("Players should all have bidded by now"),
                    rounds: 0,
                    deck: deck.drain(..data.cards_count).collect(),
                };

                (id, p)
            })
            .collect();

        Self {
            data,
            players,
            pile: BinaryHeap::new(),
            upcard: deck[0],
        }
    }

    pub fn deal(self, turn: Turn) -> Result<Game, DealError> {
        let current_dealer = self.data.peek_current();

        let player = self
            .players
            .get_mut(&turn.player_id)
            .ok_or(DealError::InvalidPlayer)?;

        if current_dealer.filter(|c| **c != turn.player_id).is_none() {
            return Err(DealError::NotYourTurn {
                expected: current_dealer.cloned(),
            });
        }

        if !player.deck.contains(&turn.card) {
            return Err(DealError::InvalidCard);
        }

        player.deck.retain(|&c| c != turn.card);

        //add card to the heap
        self.pile
            .push((turn.card.get_trump_value(self.upcard), turn));
        self.data.order.next();

        //finish set/game
        if self.players.iter().all(|(_, p)| p.deck.is_empty()) {
            let pile = self.award_points();
            self.remove_lifes();
            self.data.order.shift();

            let players_alive: Vec<_> = self.data.alive_players().collect();

            let lifes = self.data.get_lifes();

            let event = match players_alive.len() {
                0 | 1 => GameEvent::Ended { lifes },
                _ => {
                    self.start_new_set();

                    let (decks, upcard) = self.get_decks();

                    GameEvent::SetEnded {
                        lifes,
                        possible: self.data.get_possible_bids(),
                        next: self.data.get_current().clone(),
                        upcard,
                        decks,
                    }
                }
            };

            return Ok(DealState { event, pile });
        }

        //finish round
        if self.pile.len() == self.players.len() {
            let pile = self.award_points();

            let player_id = &pile[0].player_id;

            let idx = self
                .players
                .get_index_of(player_id)
                .expect("Player should be in the IndexMap");

            self.data.order.shift_to(idx);

            let event = GameEvent::RoundEnded {
                next: player_id.clone(),
                rounds: self.get_points(),
            };

            return Ok(DealState { event, pile });
        }

        let event = GameEvent::TurnPlayed {
            next: self.data.get_current().clone(),
        };

        Ok(DealState {
            pile: self.get_pile(),
            event,
        })
    }

    pub fn get_decks(&self) -> (IndexMap<PlayerId, Vec<Card>>, Card) {
        let decks = self
            .players
            .iter()
            .map(|(id, p)| (id.clone(), p.deck.clone()))
            .collect();

        (decks, self.upcard)
    }

    fn start_new_set(&mut self) {
        let (mode, count) = self
            .data
            .mode
            .get_next(self.data.cards_count, self.players.len());

        self.data.mode = mode;
        self.data.cards_count = count;

        let mut deck = Card::shuffled_deck();

        let n = self.data.cards_count;

        for (_, player) in self.players.iter_mut() {
            player.deck = deck.drain(..n).collect();
            player.bid = None;
        }

        self.upcard = deck[0];
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
        self.players
            .iter()
            .map(|(id, player)| (id.clone(), player.rounds))
            .collect()
    }

    fn get_pile(&self) -> Vec<Turn> {
        self.pile.iter().cloned().map(|(_, t)| t).collect()
    }

    pub fn get_info(&self, player_id: &PlayerId) -> GameInfoDto {
        let player = self
            .players
            .get(player_id)
            .expect("Player should exist here");

        let deck = Some(player.deck.clone());

        let info = self
            .players
            .iter()
            .map(|(id, p)| PlayerInfoDto {
                id: id.clone(),
                lifes: self.data.get_player_data(id).1.lifes,
                bid: Some(p.bid),
                rounds: Some(p.rounds),
            })
            .collect();

        let current_player = self.data.get_current();

        GameInfoDto {
            deck,
            upcard: Some(self.upcard),
            info,
            current_player: current_player.0.to_string(),
            stage: GameStageDto::Dealing,
        }
    }

    fn remove_lifes(&mut self) {
        let lost = self.players.iter().filter(|(_, p)| p.bid != p.rounds);

        for (id, _) in lost {
            self.data.remove_life(id);
        }
    }

    pub fn data(&self) -> &GameData {
        &self.data
    }
}
