use std::collections::HashMap;
use std::time::Duration;

use rand::RngExt;

use crate::models::{
    Card,
    commands::{ClientCommand, MatchSnapshot, ServerMessage},
    game::{GameCommand, fodinha_classic},
    id::PlayerId,
};

use super::ws::{
    ClientError, WebSocket, WsClient, err, validate_player_bidded, validate_player_status_change,
    validate_round_ended, validate_set_start, validate_turn_played,
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
    GameEnded { lifes: HashMap<PlayerId, usize> },
}

pub struct GameSession {
    pub players: HashMap<PlayerId, WebSocket>,
    pub decks: HashMap<PlayerId, Vec<Card>>,
    pub turn_delay: TurnDelay,
    pub bid_delay: TurnDelay,
}

impl GameSession {
    pub fn new(
        players: HashMap<PlayerId, WebSocket>,
        turn_delay: TurnDelay,
        bid_delay: TurnDelay,
    ) -> Self {
        Self {
            players,
            decks: HashMap::new(),
            turn_delay,
            bid_delay,
        }
    }

    pub async fn init(&mut self) -> Result<(), ClientError> {
        for stream in self.players.values_mut() {
            let snapshot = WsClient::get_snapshot(stream).await?;

            if !matches!(snapshot, MatchSnapshot::Waiting(_)) {
                return Err(err!("Expected Waiting snapshot on init, got {snapshot:?}"));
            }
        }

        Ok(())
    }

    pub async fn ready(&mut self) -> Result<(), ClientError> {
        let msg = ClientCommand::PlayerStatusChange { ready: true };

        for stream in self.players.values_mut() {
            WsClient::send_msg(stream, msg.clone()).await?;
        }

        for _ in 0..self.players.len() {
            for stream in self.players.values_mut() {
                WsClient::assert_msg(stream, validate_player_status_change).await?;
            }
        }

        for stream in self.players.values_mut() {
            let snapshot = WsClient::get_snapshot(stream).await?;

            if !matches!(snapshot, MatchSnapshot::Playing(_)) {
                return Err(err!(
                    "Expected playing snapshot after game start, got {snapshot:?}"
                ));
            }
        }

        Ok(())
    }

    pub async fn get_decks(&mut self) -> Result<(), ClientError> {
        for stream in self.players.values_mut() {
            WsClient::assert_msg(stream, validate_set_start).await?;
        }

        for (player_id, stream) in self.players.iter_mut() {
            let deck = WsClient::get_deck(stream).await?;
            self.decks.insert(player_id.clone(), deck);
        }

        Ok(())
    }

    pub async fn play_set(&mut self) -> Result<(), ClientError> {
        let rounds_count = self
            .decks
            .values()
            .next()
            .ok_or_else(|| err!("No deck data"))?
            .len();

        self.bidding(rounds_count).await?;

        if self.bid_delay.random_ms() > 0 {
            tokio::time::sleep(Duration::from_millis(self.bid_delay.random_ms())).await;
        }

        for i in 0..rounds_count {
            self.play_round(i == rounds_count - 1).await?;
        }

        self.decks.clear();
        Ok(())
    }

    async fn bidding(&mut self, bid_amount: usize) -> Result<(), ClientError> {
        for _ in 0..self.players.len() {
            self.bid_turn(bid_amount).await?;
        }

        Ok(())
    }

    async fn bid_turn(&mut self, bid: usize) -> Result<(), ClientError> {
        let next = {
            let stream = self
                .players
                .values_mut()
                .next()
                .ok_or_else(|| err!("No players"))?;
            WsClient::get_next_bidding_player(stream).await?
        };

        for stream in self.players.values_mut().skip(1) {
            WsClient::get_next_bidding_player(stream).await?;
        }

        tokio::time::sleep(Duration::from_millis(self.bid_delay.random_ms())).await;

        let stream = self
            .players
            .get_mut(&next)
            .ok_or_else(|| err!("Player {next:?} not found"))?;
        WsClient::send_msg(
            stream,
            ClientCommand::GameCommand(GameCommand::FodinhaClassic(
                fodinha_classic::GameCommand::PutBid { bid },
            )),
        )
        .await?;

        for stream in self.players.values_mut() {
            WsClient::assert_msg(stream, validate_player_bidded).await?;
        }

        Ok(())
    }

    async fn play_round(&mut self, is_last_round: bool) -> Result<(), ClientError> {
        for _ in 0..self.players.len() {
            self.play_turn().await?;
        }

        if !is_last_round {
            for stream in self.players.values_mut() {
                WsClient::assert_msg(stream, validate_round_ended).await?;
            }
        }

        Ok(())
    }

    async fn play_turn(&mut self) -> Result<(), ClientError> {
        let next = {
            let stream = self
                .players
                .values_mut()
                .next()
                .ok_or_else(|| err!("No players"))?;
            WsClient::get_next_turn_player(stream).await?
        };

        for stream in self.players.values_mut().skip(1) {
            WsClient::get_next_turn_player(stream).await?;
        }

        tokio::time::sleep(Duration::from_millis(self.turn_delay.random_ms())).await;

        let card = self
            .decks
            .get_mut(&next)
            .ok_or_else(|| err!("No deck for player {next:?}"))?
            .pop()
            .ok_or_else(|| err!("Deck empty for player {next:?}"))?;

        let stream = self
            .players
            .get_mut(&next)
            .ok_or_else(|| err!("Player {next:?} not found"))?;
        WsClient::send_msg(
            stream,
            ClientCommand::GameCommand(GameCommand::FodinhaClassic(
                fodinha_classic::GameCommand::PlayTurn { card },
            )),
        )
        .await?;

        for stream in self.players.values_mut() {
            WsClient::assert_msg(stream, validate_turn_played).await?;
        }

        Ok(())
    }

    pub async fn recv_set_or_game_ended(
        stream: &mut WebSocket,
    ) -> Result<GameOutcome, ClientError> {
        let msg = WsClient::recv_msg(stream).await?;

        match msg {
            ServerMessage::SetEnded { lifes: _ } => {
                Err(err!("Unexpected SetEnded during outcome detection"))
            }
            ServerMessage::GameEnded { lifes } => Ok(GameOutcome::GameEnded { lifes }),
            other => Err(err!("Expected GameEnded, got {other:?}")),
        }
    }

    pub async fn run_until_end(mut self) -> Result<GameOutcome, ClientError> {
        self.init().await?;
        self.ready().await?;
        self.get_decks().await?;
        self.play_set().await?;

        loop {
            let mut set_lifes: Option<HashMap<PlayerId, usize>> = None;

            for stream in self.players.values_mut() {
                match WsClient::recv_msg(stream).await {
                    Ok(ServerMessage::SetEnded { lifes }) => {
                        set_lifes = Some(lifes);
                    }
                    Ok(ServerMessage::GameEnded { lifes }) => {
                        return Ok(GameOutcome::GameEnded { lifes });
                    }
                    Ok(other) => {
                        return Err(err!("Expected Set or Game end, got {other:?}"));
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }

            if let Some(lifes) = set_lifes {
                self.decks.clear();

                self.players
                    .retain(|player_id, _| lifes.get(player_id).copied().unwrap_or_default() > 0);

                if self.players.len() <= 1 {
                    continue;
                }

                self.get_decks().await?;
                self.play_set().await?;
            } else {
                return Err(err!("No SetEnded outcome found"));
            }
        }
    }
}
