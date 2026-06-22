mod bidding;
mod data;
mod dealing;

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::{
    models::message::GameMessage,
    services::{GameInfoDto, GameStageDto, PlayerInfoDto},
};

use super::{
    game::{
        bidding::BiddingStage, data::GameData, dealing::DealingStage, id::PlayerId,
        util::CyclicIterator,
    },
    *,
};

#[derive(Debug)]
pub enum Game {
    Dealing(DealingStage),
    Bidding(BiddingStage),
}

impl Game {
    pub fn data(&self) -> &GameData {
        match self {
            Game::Dealing(d) => d.data(),
            Game::Bidding(b) => b.data(),
        }
    }

    pub fn process(&mut self, player_id: PlayerId, msg: GameMessage) -> Result<(), GameError> {
        match self {
            Game::Dealing(d) => match msg {
                GameMessage::PlayTurn { card } => d.deal(Turn { player_id, card }),
                GameMessage::PutBid { bid } => Err(GameError::InvalidStage),
                GameMessage::PlayerStatusChange { ready } => Err(GameError::InvalidStage),
            },
            Game::Bidding(b) => match msg {
                GameMessage::PlayTurn { card } => Err(GameError::InvalidStage),
                GameMessage::PutBid { bid } => b.bid(player_id, bid),
                GameMessage::PlayerStatusChange { ready } => Err(GameError::InvalidStage),
            },
        }
    }
}

const MAX_AVAILABLE_CARDS: usize = 40 - 1;
const MAX_PLAYER_COUNT: usize = 13;

#[derive(Debug)]
pub struct GameSettings {
    cards_count: usize,
    lifes: usize,
    mode: DealingMode,
    pub max_players: usize,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            mode: DealingMode::Increasing,
            cards_count: 1,
            lifes: 5,
            max_players: MAX_PLAYER_COUNT,
        }
    }
}

impl Game {
    pub fn new_default(players: &[PlayerId]) -> Result<Self, GameError> {
        Self::new(players, Default::default())
    }

    pub fn new(players: &[PlayerId], settings: GameSettings) -> Result<Self, GameError> {
        Self::validate_game(players, &settings)?;

        let persistent = GameData::new(players, settings);

        let stage = BiddingStage::new(persistent);

        Ok(Game::Bidding(stage))
    }

    pub fn get_game_info(&self, player_id: &PlayerId) -> GameInfoDto {
        match self {
            Game::Dealing(d) => d.get_info(player_id),
            Game::Bidding(b) => b.get_info(),
        }
    }

    fn validate_game(players: &[PlayerId], settings: &GameSettings) -> Result<(), GameError> {
        if players.len() < 2 {
            return Err(GameError::NotEnoughPlayers);
        }

        if players.len() > settings.max_players {
            return Err(GameError::TooManyPlayers);
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct BiddingPlayer {
    bid: Option<usize>,
}

#[derive(Debug)]
pub struct DealingPlayer {
    deck: Vec<Card>,
    bid: usize,
    rounds: usize,
}

impl DealingPlayer {
    pub fn new(deck: Vec<Card>, bid: usize) -> Self {
        Self {
            deck,
            bid,
            rounds: 0,
        }
    }
}

#[derive(Debug)]
pub enum LobbyState {
    NotStarted(GameSettings),
    Playing(Game),
}

#[derive(Debug)]
pub enum GameEvent {
    SetEnded {
        lifes: HashMap<PlayerId, usize>,
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
                2..=MAX_PLAYER_COUNT => {
                    assert!(matches!(result, Ok(g) if g.data().players.len() == p))
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

        let settings = GameSettings {
            cards_count: 2,
            ..Default::default()
        };
        let data = GameData::new(&[player1.clone(), player2.clone()], settings);
        let game = BiddingStage::new(data);

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![0, 1, 2]);

        game.bid(&player1, 1).unwrap();

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![0, 2]);

        let settings = GameSettings {
            cards_count: 3,
            ..Default::default()
        };
        let data = GameData::new(&[player1.clone(), player2.clone()], settings);
        let game = BiddingStage::new(data);

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![0, 1, 2, 3]);

        game.bid(&player1, 3).unwrap();

        let possible = game.get_possible_bids();
        assert_eq!(possible, vec![1, 2, 3]);
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
