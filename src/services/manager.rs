use std::{borrow::BorrowMut, collections::HashMap};

use indexmap::IndexMap;

use crate::{
    models::{
        commands::{Command, GameCommand, LobbyCommand, ServerMessage}, game::BiddingState, id::PlayerId, *
    }, services::{
        dispatcher::ManagerHandle, lobby::LobbiesManager, repositories::{game::TurnDto, get_mongo_client},
        InboundReceiver, LobbyError, ManagerError, OutboundSender
    }, AppSettings
};

use super::repositories::game::GamesRepository;

pub struct GameManager {
    lobby_manager: LobbiesManager,
    connections: HashMap<PlayerId, Connection>,
}

impl GameManager {
    pub fn new() -> Self {
        Self {
            lobby_manager: LobbiesManager::new(),
            connections: HashMap::new(),
        }
    }

    pub async fn start(self, settings: &AppSettings) -> ManagerHandle {
        let (tx, rx) = flume::unbounded();

        tokio::spawn(self.inbound_handler(rx));

        let db = get_mongo_client(&settings.mongo_conn_string)
            .await
            .expect("Expected to create mongo client")
            .database("oh_hell");

        let repo = GamesRepository::new(&db);

        ManagerHandle::new(repo, tx)
    }

    async fn inbound_handler(mut self, rx: InboundReceiver) {
        loop {
            let msg = match rx.recv_async().await {
                Ok(m) => m,
                Err(err) => {
                    tracing::error!("Error GameMessage recv: {err}");
                    panic!("{err}")
                }
            };

            if let Err(err) = self.handle(msg) {
                tracing::error!("Error GameMessage handle: {err}");
            }
        }
    }

    fn handle(&mut self, msg: Command) -> Result<(), ManagerError> {
        match msg {
            Command::Lobby(msg) => self.handle_lobby(msg),
            Command::Game(msg, id) => self.handle_game(msg, id),
        }
    }

    fn handle_game(&mut self, msg: GameCommand, id: PlayerId) -> Result<(), ManagerError> {
        let lobby = self.lobby_manager.get(id)?;

        match msg {
            GameCommand::PlayTurn { card } => todo!(),
            GameCommand::PutBid { bid } => todo!(),
        }
    }

    fn handle_lobby(&mut self, msg: LobbyCommand) -> Result<(), ManagerError> {
        match msg {
            LobbyCommand::CreateLobby(lobby_id, settings) => {
                self.lobby_manager.create_lobby(lobby_id, settings);
                Ok(())
            }
            LobbyCommand::JoinLobby {
                lobby_id,
                user_claims,
                respond,
            } => {
                let response = self.lobby_manager.join_lobby(&lobby_id, user_claims.clone());
                respond
                    .send(response)
                    .map_err(|_| ManagerError::ReceiverDisposed)?;

                self.broadcast(&lobby_id, ServerMessage::PlayerJoined(user_claims))
            }
            LobbyCommand::GetLobbies(respond) => {
                let lobbies = self.lobby_manager.get_lobbies_info();

        respond
            .send(lobbies)
            .map_err(|_| ManagerError::ReceiverDisposed)
            },
            LobbyCommand::StatusChange { ready } => todo!(),
        }
    }

    pub async fn play_turn(&self, card: Card, player_id: PlayerId) -> Result<(), LobbyError> {
        let (players, state) = {
            let game_id = manager
                .players_lobby
                .get(&player_id)
                .ok_or(LobbyError::WrongLobby)
                .cloned()?;

            let lobby = manager
                .lobbies
                .get_mut(&game_id)
                .ok_or(LobbyError::InvalidLobby)?;

            if !lobby.players.contains_key(&player_id) {
                return Err(LobbyError::WrongLobby);
            }

            let game = lobby.get_game()?;

            let turn = Turn {
                player_id: player_id.clone(),
                card,
            };

            let state = game
                .deal(turn)
                .map_err(|e| LobbyError::GameError(GameError::InvalidDeal(e)))?;

            let dto = TurnDto::new(&game_id, &player_id, set_id, round_id, card, i);
            self.games_repo.insert_turn(dto);

            match state.event {
                GameEvent::SetEnded {
                    lifes,
                    upcard,
                    decks,
                    next,
                    possible,
                } => todo!(),
                GameEvent::RoundEnded { next, rounds } => todo!(),
                GameEvent::Ended { lifes } => todo!(),
                GameEvent::TurnPlayed { next } => todo!(),
            }

            (lobby.get_players_id(), state)
        };

        let msg = ServerMessage::TurnPlayed { pile: state.pile };
        self.broadcast_msg(&players, &msg).await;

        let game_ended = matches!(state.event, GameEvent::Ended { lifes: _ });

        match state.event {
            GameEvent::SetEnded {
                lifes,
                upcard,
                decks,
                next,
                possible,
            } => {
                let msg = ServerMessage::SetEnded { lifes };
                self.broadcast_msg(&players, &msg).await;

                self.init_set(decks, next, upcard, possible).await;
            }
            GameEvent::RoundEnded { rounds, next } => {
                let msg = ServerMessage::RoundEnded(rounds);
                self.broadcast_msg(&players, &msg).await;

                let msg = ServerMessage::PlayerTurn { player_id: next };
                self.broadcast_msg(&players, &msg).await;
            }
            GameEvent::TurnPlayed { next } => {
                let msg = ServerMessage::PlayerTurn { player_id: next };
                self.broadcast_msg(&players, &msg).await;
            }
            GameEvent::Ended { lifes } => {
                let msg = ServerMessage::GameEnded { lifes };
                self.broadcast_msg(&players, &msg).await;
            }
        }

        if game_ended {
            let game_id = manager
                .players_lobby
                .get(&player_id)
                .ok_or(LobbyError::WrongLobby)
                .cloned()?;

            manager.lobbies.remove(&game_id);

            tracing::debug!("removing finished game from list");

            for p in &players {
                manager.players_lobby.remove(p);
            }
        }

        Ok(())
    }

    pub async fn bid(&self, bid: usize, player_id: PlayerId) -> Result<(), LobbyError> {
        let (players, state) = {
            let lobby_id = {
                manager
                    .players_lobby
                    .get(&player_id)
                    .ok_or(LobbyError::WrongLobby)
                    .cloned()?
            };

            let lobby = manager
                .lobbies
                .get_mut(&lobby_id)
                .ok_or(LobbyError::InvalidLobby)?;

            let game = lobby.get_game()?;

            let state = game
                .bid(&player_id, bid)
                .map_err(|e| LobbyError::GameError(GameError::InvalidBid(e)))?;

            (lobby.get_players_id(), state)
        };

        let msg = ServerMessage::PlayerBidded { player_id, bid };
        self.broadcast_msg(&players, &msg).await;

        let msg = match state {
            BiddingState::Active {
                possible_bids,
                next,
            } => ServerMessage::PlayerBiddingTurn {
                player_id: next,
                possible_bids,
            },
            BiddingState::Ended { next } => ServerMessage::PlayerTurn { player_id: next },
        };

        self.broadcast(&players, msg);

        Ok(())
    }

    pub async fn store_player_connection(
        &self,
        player_id: PlayerId,
        sender: Connection,
    ) -> Result<(), ManagerError> {
        let (tx, rx) = flume::unbounded();

        let mut manager = self.inner.connections.lock().await;

        manager.insert(player_id, sender);

        Ok(())
    }

    pub async fn player_status_change(
        &self,
        player_id: PlayerId,
        ready: bool,
    ) -> Result<(), LobbyError> {
            let lobby = self.lobby_manager.get(player_id)?;

            let players = match lobby.state.borrow_mut() {
                LobbyState::NotStarted(_) => lobby.players,
                LobbyState::Playing(_) => return Err(LobbyError::GameAlreadyStarted),
            };

            players.get_mut(&player_id).expect("Player should be in the lobby").ready = ready;

            let should_start = players.len() == lobby.players.len();

            let lobby_players = lobby.get_players_id();

            let set_info = if should_start {
                let game = Game::new_default(&lobby_players)?;

                let (decks, upcard) = game.get_decks();

                let first = game.get_bidding_player();

                let possible = game.get_possible_bids();

                lobby.state = LobbyState::Playing(game);

                Some((decks, first, upcard, possible))
            } else {
                None
            };


        let msg = ServerMessage::PlayerStatusChange { player_id, ready };
        self.broadcast_msg(&players, &msg).await;

        if let Some((decks, first, upcard, possible_bids)) = set_info {
            self.init_set(decks, first, upcard, possible_bids).await;
        }

        Ok(())
    }

    async fn init_set(
        &self,
        decks: IndexMap<PlayerId, Vec<Card>>,
        next: PlayerId,
        upcard: Card,
        possible_bids: Vec<usize>,
    ) {
        let msg = ServerMessage::SetStart { upcard };
        self.broadcast(&, &msg).await;

        for (p, deck) in decks {
            let msg = ServerMessage::PlayerDeck(deck);

            self.unicast_msg(&p, &msg).await;
        }

        let msg = ServerMessage::PlayerBiddingTurn {
            player_id: next,
            possible_bids,
        };

        self.broadcast_msg(&players, &msg).await;
    }

    fn broadcast(&self, lobby_id: &LobbyId, msg: ServerMessage) -> Result<(), ManagerError> {
        let players = self
            .lobby_manager
            .get_players(lobby_id)
            .ok_or(LobbyError::InvalidLobby)?;

        for p in players {
            match self.connections.get(p) {
                Some(c) => send_msg(msg.clone(), p, c),
                None => {
                    tracing::error!("Connection {p:?} not found in lobby {lobby_id:?}");
                }
            }
        }

        Ok(())
    }
}

fn send_msg(msg: ServerMessage, player: &PlayerId, connection: &Connection) {
    let send = connection
        .send(msg)
        .map_err(|e| ManagerError::PlayerDisconnected(e.to_string()));

    if let Err(e) = send {
        tracing::error!("Error sending msg to: {player:?} | {e}");
    }
}

type Connection = OutboundSender;
