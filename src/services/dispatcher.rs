use std::{collections::HashMap, sync::Arc};

use dashmap::DashMap;
use indexmap::IndexMap;
use tokio::sync::{mpsc, oneshot};

use crate::{
    infra::UserClaims,
    models::{
        Card, Game, GameError, LobbyState, Turn,
        commands::{CreateLobbyResponse, GameCommand, GetLobbyDto, LobbyInfo, ServerMessage},
        game::{AppliedGameEvent, BiddingState, DomainEvent, GameEvent, GameSettings, NewSet},
        id::{self, LobbyId, PlayerId},
        lobby::{Lobby, PlayerStatus},
    },
    services::{LobbyError, ManagerError, repositories::game::GamesRepository},
};

pub type GameSender = flume::Sender<GameActorCommand>;
pub type PlayerSender = mpsc::Sender<ServerMessage>;
pub type PlayerReceiver = mpsc::Receiver<ServerMessage>;

type GameReceiver = flume::Receiver<GameActorCommand>;
pub type GameSenders = Arc<DashMap<LobbyId, GameSender>>;
type PlayerGames = Arc<DashMap<PlayerId, LobbyId>>;

#[derive(Clone)]
pub struct ManagerHandle {
    pub game_senders: GameSenders,
    player_games: PlayerGames,
    games_repo: GamesRepository,
}

pub struct PlayerConnectionContext {
    pub game_id: LobbyId,
    pub game_tx: GameSender,
    pub outbound_rx: PlayerReceiver,
}

impl ManagerHandle {
    pub fn new(games_repo: GamesRepository) -> Self {
        Self {
            game_senders: Arc::new(DashMap::new()),
            player_games: Arc::new(DashMap::new()),
            games_repo,
        }
    }

    pub async fn create_lobby(
        &self,
        _player_id: PlayerId,
        settings: GameSettings,
    ) -> Result<CreateLobbyResponse, ManagerError> {
        let lobby_id = id::gen_lobbyid();
        let (tx, rx) = flume::unbounded();

        let actor = GameActor::new(
            lobby_id.clone(),
            self.games_repo.clone(),
            self.game_senders.clone(),
            self.player_games.clone(),
        );

        self.game_senders.insert(lobby_id.clone(), tx.clone());
        tokio::spawn(actor.run(rx));

        let result = Self::request(&tx, |respond| GameActorCommand::CreateLobby {
            settings,
            respond,
        })
        .await;

        if result.is_err() {
            self.game_senders.remove(&lobby_id);
        }

        result?;

        Ok(CreateLobbyResponse { lobby_id })
    }

    pub async fn join_lobby(
        &self,
        lobby_id: LobbyId,
        user_claims: UserClaims,
    ) -> Result<LobbyInfo, ManagerError> {
        let sender = self.sender_for_game(&lobby_id)?;

        Self::request(&sender, |respond| GameActorCommand::JoinLobby {
            user_claims,
            respond,
        })
        .await
    }

    pub async fn get_lobbies(&self) -> Vec<GetLobbyDto> {
        let actors: Vec<_> = self
            .game_senders
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        let mut lobbies = Vec::new();

        for sender in actors {
            let response = Self::request(&sender, |respond| GameActorCommand::GetLobbySummary {
                respond,
            })
            .await;

            if let Ok(Some(lobby)) = response {
                lobbies.push(lobby);
            }
        }

        lobbies
    }

    pub async fn play_turn(&self, card: Card, player_id: PlayerId) -> Result<(), ManagerError> {
        let sender = self.sender_for_player(&player_id)?;

        Self::request(&sender, |respond| GameActorCommand::GameCommand {
            player_id,
            command: GameCommand::PlayTurn { card },
            respond,
        })
        .await
    }

    pub async fn bid(&self, bid: usize, player_id: PlayerId) -> Result<(), ManagerError> {
        let sender = self.sender_for_player(&player_id)?;

        Self::request(&sender, |respond| GameActorCommand::GameCommand {
            player_id,
            command: GameCommand::PutBid { bid },
            respond,
        })
        .await
    }

    pub async fn player_status_change(
        &self,
        player_id: PlayerId,
        ready: bool,
    ) -> Result<(), ManagerError> {
        let sender = self.sender_for_player(&player_id)?;

        Self::request(&sender, |respond| GameActorCommand::StatusChange {
            player_id,
            ready,
            respond,
        })
        .await
    }

    pub async fn reconnect(&self, player_id: PlayerId) -> Result<(), ManagerError> {
        let sender = self.sender_for_player(&player_id)?;

        Self::request(&sender, |respond| GameActorCommand::Reconnect {
            player_id,
            respond,
        })
        .await
    }

    pub async fn connect_player(
        &self,
        player_id: PlayerId,
    ) -> Result<PlayerConnectionContext, ManagerError> {
        let game_id = self
            .player_games
            .get(&player_id)
            .map(|entry| entry.value().clone())
            .ok_or(LobbyError::PlayerNotInLobby)?;
        let game_tx = self.sender_for_game(&game_id)?;
        let (outbound_tx, outbound_rx) = mpsc::channel(128);

        Self::request(&game_tx, |respond| GameActorCommand::ConnectPlayer {
            player_id,
            outbound_tx,
            respond,
        })
        .await?;

        Ok(PlayerConnectionContext {
            game_id,
            game_tx,
            outbound_rx,
        })
    }

    async fn request<T>(
        sender: &GameSender,
        build: impl FnOnce(oneshot::Sender<Result<T, ManagerError>>) -> GameActorCommand,
    ) -> Result<T, ManagerError> {
        let (tx, rx) = oneshot::channel();

        sender
            .send_async(build(tx))
            .await
            .map_err(|_| ManagerError::ReceiverDisposed)?;

        rx.await.map_err(|_| ManagerError::ReceiverDisposed)?
    }

    fn sender_for_game(&self, lobby_id: &LobbyId) -> Result<GameSender, ManagerError> {
        self.game_senders
            .get(lobby_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| LobbyError::InvalidLobby.into())
    }

    fn sender_for_player(&self, player_id: &PlayerId) -> Result<GameSender, ManagerError> {
        let lobby_id = self
            .player_games
            .get(player_id)
            .map(|entry| entry.value().clone())
            .ok_or(LobbyError::PlayerNotInLobby)?;

        self.sender_for_game(&lobby_id)
    }
}

pub enum GameActorCommand {
    ConnectPlayer {
        player_id: PlayerId,
        outbound_tx: PlayerSender,
        respond: oneshot::Sender<Result<(), ManagerError>>,
    },
    DisconnectPlayer {
        player_id: PlayerId,
    },
    CreateLobby {
        settings: GameSettings,
        respond: oneshot::Sender<Result<(), ManagerError>>,
    },
    JoinLobby {
        user_claims: UserClaims,
        respond: oneshot::Sender<Result<LobbyInfo, ManagerError>>,
    },
    StatusChange {
        player_id: PlayerId,
        ready: bool,
        respond: oneshot::Sender<Result<(), ManagerError>>,
    },
    GameCommand {
        player_id: PlayerId,
        command: GameCommand,
        respond: oneshot::Sender<Result<(), ManagerError>>,
    },
    Reconnect {
        player_id: PlayerId,
        respond: oneshot::Sender<Result<(), ManagerError>>,
    },
    GetLobbySummary {
        respond: oneshot::Sender<Result<Option<GetLobbyDto>, ManagerError>>,
    },
}

struct GameActor {
    lobby_id: LobbyId,
    lobby: Option<Lobby>,
    players: HashMap<PlayerId, PlayerSender>,
    version: usize,
    repo: GamesRepository,
    game_senders: GameSenders,
    player_games: PlayerGames,
}

enum AppliedEvent {
    None,
    PlayerJoined {
        user_claims: UserClaims,
    },
    PlayerStatusChanged {
        player_id: PlayerId,
        ready: bool,
    },
    GameStarted {
        set: NewSet,
        next: PlayerId,
        possible_bids: Vec<usize>,
    },
    Game(AppliedGameEvent),
}

impl GameActor {
    fn new(
        lobby_id: LobbyId,
        repo: GamesRepository,
        game_senders: GameSenders,
        player_games: PlayerGames,
    ) -> Self {
        Self {
            lobby_id,
            lobby: None,
            players: HashMap::new(),
            version: 0,
            repo,
            game_senders,
            player_games,
        }
    }

    async fn run(mut self, rx: GameReceiver) {
        while let Ok(command) = rx.recv_async().await {
            let should_continue = self.handle(command).await;

            if !should_continue {
                break;
            }
        }
    }

    async fn handle(&mut self, command: GameActorCommand) -> bool {
        match command {
            GameActorCommand::ConnectPlayer {
                player_id,
                outbound_tx,
                respond,
            } => {
                respond_once(respond, self.handle_connect_player(player_id, outbound_tx));
            }
            GameActorCommand::DisconnectPlayer { player_id } => {
                self.players.remove(&player_id);
            }
            GameActorCommand::CreateLobby { settings, respond } => {
                respond_once(respond, self.handle_create_lobby(settings).await);
            }
            GameActorCommand::JoinLobby {
                user_claims,
                respond,
            } => {
                respond_once(respond, self.handle_join_lobby(user_claims).await);
            }
            GameActorCommand::StatusChange {
                player_id,
                ready,
                respond,
            } => {
                respond_once(respond, self.handle_status_change(player_id, ready).await);
            }
            GameActorCommand::GameCommand {
                player_id,
                command,
                respond,
            } => {
                let result = self.handle_game_command(player_id, command).await;
                let should_continue = !matches!(&result, Ok(ActorResult::Stop));
                respond_once(respond, result.map(|_| ()));
                return should_continue;
            }
            GameActorCommand::Reconnect { player_id, respond } => {
                respond_once(respond, self.handle_reconnect(player_id).await);
            }
            GameActorCommand::GetLobbySummary { respond } => {
                respond_once(respond, self.handle_get_lobby_summary());
            }
        }

        true
    }

    async fn handle_create_lobby(&mut self, settings: GameSettings) -> Result<(), ManagerError> {
        if self.lobby.is_some() {
            return Ok(());
        }

        self.persist_apply(DomainEvent::LobbyCreated { settings })
            .await?;

        Ok(())
    }

    fn handle_connect_player(
        &mut self,
        player_id: PlayerId,
        outbound_tx: PlayerSender,
    ) -> Result<(), ManagerError> {
        let lobby = self.lobby()?;

        if !lobby.players.contains_key(&player_id) {
            return Err(LobbyError::WrongLobby.into());
        }

        self.players.insert(player_id, outbound_tx);

        Ok(())
    }

    async fn handle_join_lobby(
        &mut self,
        user_claims: UserClaims,
    ) -> Result<LobbyInfo, ManagerError> {
        let player_id = user_claims.id();

        if let Some(lobby) = self.lobby.as_ref() {
            if lobby.players.contains_key(&player_id) {
                self.player_games
                    .insert(player_id.clone(), self.lobby_id.clone());
                return Ok(lobby.get_info(&player_id));
            }

            match &lobby.state {
                LobbyState::NotStarted(settings) => {
                    if lobby.players.len() == settings.max_players {
                        return Err(LobbyError::GameError(GameError::TooManyPlayers).into());
                    }
                }
                LobbyState::Playing(_) => return Err(LobbyError::GameAlreadyStarted.into()),
            }
        } else {
            return Err(LobbyError::InvalidLobby.into());
        }

        let applied = self
            .persist_apply(DomainEvent::PlayerJoined {
                user_claims: user_claims.clone(),
            })
            .await?;

        if let AppliedEvent::PlayerJoined { user_claims } = applied {
            self.broadcast(ServerMessage::PlayerJoined(user_claims))
                .await;
        }

        Ok(self.lobby()?.get_info(&player_id))
    }

    async fn handle_status_change(
        &mut self,
        player_id: PlayerId,
        ready: bool,
    ) -> Result<(), ManagerError> {
        {
            let lobby = self.lobby()?;

            if !lobby.players.contains_key(&player_id) {
                return Err(LobbyError::WrongLobby.into());
            }

            if matches!(lobby.state, LobbyState::Playing(_)) {
                return Err(LobbyError::GameAlreadyStarted.into());
            }
        }

        let applied = self
            .persist_apply(DomainEvent::PlayerStatusChanged {
                player_id: player_id.clone(),
                ready,
            })
            .await?;

        if let AppliedEvent::PlayerStatusChanged { player_id, ready } = applied {
            let msg = ServerMessage::PlayerStatusChange { player_id, ready };
            self.broadcast(msg).await;
        }

        if let Some((players, settings)) = self.start_game_data()? {
            let event = Game::start_event(&players, settings)
                .map_err(|e| ManagerError::Lobby(LobbyError::GameError(e)))?;
            let applied = self.persist_apply(event).await?;

            if let AppliedEvent::GameStarted {
                set,
                next,
                possible_bids,
            } = applied
            {
                self.init_set(set.decks, set.upcard, next, possible_bids)
                    .await;
            }
        }

        Ok(())
    }

    async fn handle_game_command(
        &mut self,
        player_id: PlayerId,
        command: GameCommand,
    ) -> Result<ActorResult, ManagerError> {
        let event = {
            let lobby = self.lobby()?;

            if !lobby.players.contains_key(&player_id) {
                return Err(LobbyError::WrongLobby.into());
            }

            let game = match &lobby.state {
                LobbyState::NotStarted(_) => return Err(LobbyError::GameNotStarted.into()),
                LobbyState::Playing(game) => game,
            };

            match command {
                GameCommand::PlayTurn { card } => game
                    .validate_turn(Turn {
                        player_id: player_id.clone(),
                        card,
                    })
                    .map_err(ManagerError::Deal)?,
                GameCommand::PutBid { bid } => game
                    .validate_bid(&player_id, bid)
                    .map_err(ManagerError::Bid)?,
            }
        };

        let players = self.player_ids()?;
        let applied = self.persist_apply(event).await?;

        match applied {
            AppliedEvent::Game(AppliedGameEvent::BidPlaced {
                player_id,
                bid,
                state,
            }) => {
                self.broadcast_bid(player_id, bid, state).await;
                Ok(ActorResult::Continue)
            }
            AppliedEvent::Game(AppliedGameEvent::TurnPlayed(state)) => {
                let ended = self.broadcast_turn(state).await;

                if ended {
                    self.stop_game(&players);
                    Ok(ActorResult::Stop)
                } else {
                    Ok(ActorResult::Continue)
                }
            }
            _ => unreachable!("game command must apply a game event"),
        }
    }

    async fn handle_reconnect(&mut self, player_id: PlayerId) -> Result<(), ManagerError> {
        let info = {
            let lobby = self.lobby()?;

            if !lobby.players.contains_key(&player_id) {
                return Err(LobbyError::WrongLobby.into());
            }

            match &lobby.state {
                LobbyState::NotStarted(_) => return Err(LobbyError::GameNotStarted.into()),
                LobbyState::Playing(game) => game.get_game_info(&player_id),
            }
        };

        self.send_to_player(&player_id, ServerMessage::Reconnect(info))
            .await;

        Ok(())
    }

    fn handle_get_lobby_summary(&self) -> Result<Option<GetLobbyDto>, ManagerError> {
        let Some(lobby) = self.lobby.as_ref() else {
            return Ok(None);
        };

        if matches!(lobby.state, LobbyState::Playing(_)) {
            return Ok(None);
        }

        Ok(Some(GetLobbyDto {
            id: self.lobby_id.clone(),
            player_count: lobby.players.len(),
        }))
    }

    async fn persist_apply(&mut self, event: DomainEvent) -> Result<AppliedEvent, ManagerError> {
        self.repo
            .append_event(&self.lobby_id, self.version, event.clone())
            .await?;
        self.version += 1;

        self.apply_event(event)
    }

    fn apply_event(&mut self, event: DomainEvent) -> Result<AppliedEvent, ManagerError> {
        match event {
            DomainEvent::LobbyCreated { settings } => {
                self.lobby = Some(Lobby::new(settings));

                Ok(AppliedEvent::None)
            }
            DomainEvent::PlayerJoined { user_claims } => {
                let player_id = user_claims.id();
                let lobby = self.lobby_mut()?;

                lobby.players.insert(
                    player_id.clone(),
                    PlayerStatus {
                        ready: false,
                        player: user_claims.clone(),
                    },
                );

                self.player_games.insert(player_id, self.lobby_id.clone());

                Ok(AppliedEvent::PlayerJoined { user_claims })
            }
            DomainEvent::PlayerStatusChanged { player_id, ready } => {
                let lobby = self.lobby_mut()?;
                let player = lobby
                    .players
                    .get_mut(&player_id)
                    .ok_or(LobbyError::WrongLobby)?;

                player.ready = ready;

                Ok(AppliedEvent::PlayerStatusChanged { player_id, ready })
            }
            DomainEvent::GameStarted { settings, set } => {
                let lobby = self.lobby_mut()?;
                let players = lobby.get_players_id();
                let game = Game::from_started(&players, settings, set.clone())
                    .map_err(|e| ManagerError::Lobby(LobbyError::GameError(e)))?;

                lobby.state = LobbyState::Playing(game);

                let game = match &lobby.state {
                    LobbyState::Playing(game) => game,
                    LobbyState::NotStarted(_) => unreachable!("game was just started"),
                };

                Ok(AppliedEvent::GameStarted {
                    set,
                    next: game.get_bidding_player(),
                    possible_bids: game.get_possible_bids(),
                })
            }
            event @ (DomainEvent::BidPlaced { .. } | DomainEvent::TurnPlayed { .. }) => {
                let lobby = self.lobby_mut()?;
                let game = match &mut lobby.state {
                    LobbyState::NotStarted(_) => return Err(LobbyError::GameNotStarted.into()),
                    LobbyState::Playing(game) => game,
                };

                Ok(AppliedEvent::Game(game.apply_domain_event(event)))
            }
        }
    }

    async fn broadcast_bid(&self, player_id: PlayerId, bid: usize, state: BiddingState) {
        let msg = ServerMessage::PlayerBidded {
            player_id: player_id.clone(),
            bid,
        };
        self.broadcast(msg).await;

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

        self.broadcast(msg).await;
    }

    async fn broadcast_turn(&self, state: crate::models::game::DealState) -> bool {
        let msg = ServerMessage::TurnPlayed { pile: state.pile };
        self.broadcast(msg).await;

        match state.event {
            GameEvent::SetEnded {
                lifes,
                upcard,
                decks,
                next,
                possible,
            } => {
                let msg = ServerMessage::SetEnded { lifes };
                self.broadcast(msg).await;

                self.init_set(decks, upcard, next, possible).await;

                false
            }
            GameEvent::RoundEnded { rounds, next } => {
                let msg = ServerMessage::RoundEnded(rounds);
                self.broadcast(msg).await;

                let msg = ServerMessage::PlayerTurn { player_id: next };
                self.broadcast(msg).await;

                false
            }
            GameEvent::TurnPlayed { next } => {
                let msg = ServerMessage::PlayerTurn { player_id: next };
                self.broadcast(msg).await;

                false
            }
            GameEvent::Ended { lifes } => {
                let msg = ServerMessage::GameEnded { lifes };
                self.broadcast(msg).await;

                true
            }
        }
    }

    async fn init_set(
        &self,
        decks: IndexMap<PlayerId, Vec<Card>>,
        upcard: Card,
        next: PlayerId,
        possible_bids: Vec<usize>,
    ) {
        self.broadcast(ServerMessage::SetStart { upcard }).await;

        for (player, deck) in decks {
            self.send_to_player(&player, ServerMessage::PlayerDeck(deck))
                .await;
        }

        self.broadcast(ServerMessage::PlayerBiddingTurn {
            player_id: next,
            possible_bids,
        })
        .await;
    }

    async fn send_to_player(&self, player_id: &PlayerId, msg: ServerMessage) {
        let Some(sender) = self.players.get(player_id).cloned() else {
            tracing::debug!("No websocket connection for player {player_id:?}");
            return;
        };

        if let Err(e) = sender.send(msg).await {
            tracing::error!("Error enqueueing message to {player_id:?}: {e}");
        }
    }

    async fn broadcast(&self, msg: ServerMessage) {
        let players: Vec<_> = self
            .players
            .iter()
            .map(|(player_id, sender)| (player_id.clone(), sender.clone()))
            .collect();

        for (player_id, sender) in players {
            if let Err(e) = sender.send(msg.clone()).await {
                tracing::error!("Error enqueueing message to {player_id:?}: {e}");
            }
        }
    }

    fn stop_game(&mut self, players: &[PlayerId]) {
        self.game_senders.remove(&self.lobby_id);

        for player in players {
            self.player_games.remove(player);
        }
    }

    fn start_game_data(&self) -> Result<Option<(Vec<PlayerId>, GameSettings)>, ManagerError> {
        let lobby = self.lobby()?;
        let settings = match &lobby.state {
            LobbyState::NotStarted(settings) => settings,
            LobbyState::Playing(_) => return Ok(None),
        };

        if lobby.players.len() < 2 || !lobby.players.values().all(|p| p.ready) {
            return Ok(None);
        }

        Ok(Some((lobby.get_players_id(), settings.clone())))
    }

    fn player_ids(&self) -> Result<Vec<PlayerId>, ManagerError> {
        Ok(self.lobby()?.get_players_id())
    }

    fn lobby(&self) -> Result<&Lobby, ManagerError> {
        self.lobby
            .as_ref()
            .ok_or_else(|| LobbyError::InvalidLobby.into())
    }

    fn lobby_mut(&mut self) -> Result<&mut Lobby, ManagerError> {
        self.lobby
            .as_mut()
            .ok_or_else(|| LobbyError::InvalidLobby.into())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ActorResult {
    Continue,
    Stop,
}

fn respond_once<T>(
    respond: oneshot::Sender<Result<T, ManagerError>>,
    result: Result<T, ManagerError>,
) {
    let _ = respond.send(result);
}
