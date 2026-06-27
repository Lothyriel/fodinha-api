use std::collections::HashMap;
use std::time::Duration;

use rand::RngExt;

use crate::models::{
    Card,
    commands::{ClientCommand, MatchSnapshot, ServerMessage},
    id::PlayerId,
};

use super::{
    http::HttpClient,
    ws::{
        WebSocket, WsClient, validate_player_bidded, validate_player_status_change,
        validate_round_ended, validate_set_start, validate_turn_played,
    },
};

pub struct TurnDelay {
    pub min_ms: u64,
    pub max_ms: u64,
}

impl TurnDelay {
    pub fn random_ms(&self) -> u64 {
        let mut rng = rand::rng();
        rng.random_range(self.min_ms..=self.max_ms)
    }
}

impl Default for TurnDelay {
    fn default() -> Self {
        Self {
            min_ms: 5_000,
            max_ms: 20_000,
        }
    }
}

pub enum GameOutcome {
    SetEnded { lifes: HashMap<PlayerId, usize> },
    GameEnded { lifes: HashMap<PlayerId, usize> },
    Error(String),
}

pub struct GameSession {
    pub players: HashMap<PlayerId, WebSocket>,
    pub decks: HashMap<PlayerId, Vec<Card>>,
    pub http: HttpClient,
    pub ws: WsClient,
    pub turn_delay: TurnDelay,
    pub bid_delay: TurnDelay,
}

impl GameSession {
    pub fn new(
        players: HashMap<PlayerId, WebSocket>,
        http: HttpClient,
        ws: WsClient,
        turn_delay: TurnDelay,
        bid_delay: TurnDelay,
    ) -> Self {
        Self {
            players,
            decks: HashMap::new(),
            http,
            ws,
            turn_delay,
            bid_delay,
        }
    }

    pub async fn init(&mut self) {
        for stream in self.players.values_mut() {
            let snapshot = WsClient::get_snapshot(stream).await;
            assert!(
                matches!(snapshot, MatchSnapshot::Waiting(_)),
                "Expected Waiting snapshot on init"
            );
        }
    }

    pub async fn ready(&mut self) {
        let msg = ClientCommand::PlayerStatusChange { ready: true };

        for stream in self.players.values_mut() {
            WsClient::send_msg(stream, msg).await;
        }

        for _ in 0..self.players.len() {
            for stream in self.players.values_mut() {
                WsClient::assert_msg(stream, validate_player_status_change).await;
            }
        }

        for stream in self.players.values_mut() {
            let snapshot = WsClient::get_snapshot(stream).await;
            assert!(
                matches!(snapshot, MatchSnapshot::Playing(_)),
                "Expected playing snapshot after game start"
            );
        }
    }

    pub async fn get_decks(&mut self) {
        for stream in self.players.values_mut() {
            WsClient::assert_msg(stream, validate_set_start).await;
        }

        for (player_id, stream) in self.players.iter_mut() {
            let deck = WsClient::get_deck(stream).await;
            self.decks.insert(player_id.clone(), deck);
        }
    }

    pub async fn play_set(&mut self) {
        let rounds_count = self.decks.values().next().unwrap().len();
        self.bidding(rounds_count).await;

        if self.bid_delay.random_ms() > 0 {
            tokio::time::sleep(Duration::from_millis(self.bid_delay.random_ms())).await;
        }

        for i in 0..rounds_count {
            self.play_round(i == rounds_count - 1).await;
        }

        self.decks.clear();
    }

    async fn bidding(&mut self, bid_amount: usize) {
        for _ in 0..self.players.len() {
            self.bid_turn(bid_amount).await;
        }
    }

    async fn bid_turn(&mut self, bid: usize) {
        let next = {
            let stream = self.players.values_mut().next().unwrap();
            WsClient::get_next_bidding_player(stream).await
        };

        for stream in self.players.values_mut().skip(1) {
            WsClient::get_next_bidding_player(stream).await;
        }

        tokio::time::sleep(Duration::from_millis(self.bid_delay.random_ms())).await;

        let stream = self.players.get_mut(&next).unwrap();
        WsClient::send_msg(stream, ClientCommand::PutBid { bid }).await;

        for stream in self.players.values_mut() {
            WsClient::assert_msg(stream, validate_player_bidded).await;
        }
    }

    async fn play_round(&mut self, is_last_round: bool) {
        for _ in 0..self.players.len() {
            self.play_turn().await;
        }

        if !is_last_round {
            for stream in self.players.values_mut() {
                WsClient::assert_msg(stream, validate_round_ended).await;
            }
        }
    }

    async fn play_turn(&mut self) {
        let next = {
            let stream = self.players.values_mut().next().unwrap();
            WsClient::get_next_turn_player(stream).await
        };

        for stream in self.players.values_mut().skip(1) {
            WsClient::get_next_turn_player(stream).await;
        }

        tokio::time::sleep(Duration::from_millis(self.turn_delay.random_ms())).await;

        let card = self.decks.get_mut(&next).unwrap().pop().unwrap();

        let stream = self.players.get_mut(&next).unwrap();
        WsClient::send_msg(stream, ClientCommand::PlayTurn { card }).await;

        for stream in self.players.values_mut() {
            WsClient::assert_msg(stream, validate_turn_played).await;
        }
    }

    pub async fn recv_set_or_game_ended(stream: &mut WebSocket) -> GameOutcome {
        match WsClient::recv_msg(stream).await {
            ServerMessage::SetEnded { lifes } => GameOutcome::SetEnded { lifes },
            ServerMessage::GameEnded { lifes } => GameOutcome::GameEnded { lifes },
            msg => GameOutcome::Error(format!("Expected Set or Game end | {msg:?}")),
        }
    }

    pub async fn run_until_end(mut self) -> GameOutcome {
        self.init().await;
        self.ready().await;
        self.get_decks().await;
        self.play_set().await;

        loop {
            let mut set_outcome = None;

            for stream in self.players.values_mut() {
                let result = Self::recv_set_or_game_ended(stream).await;

                match result {
                    GameOutcome::SetEnded { lifes } => {
                        set_outcome = Some(lifes);
                    }
                    GameOutcome::GameEnded { lifes } => {
                        return GameOutcome::GameEnded { lifes };
                    }
                    GameOutcome::Error(e) => {
                        return GameOutcome::Error(e);
                    }
                }
            }

            if let Some(lifes) = set_outcome {
                self.decks.clear();

                self.players
                    .retain(|player_id, _| lifes.get(player_id).copied().unwrap_or_default() > 0);

                if self.players.len() <= 1 {
                    continue;
                }

                self.get_decks().await;
                self.play_set().await;
            } else {
                break;
            }
        }

        GameOutcome::Error("Game ended without GameEnded message".into())
    }
}
