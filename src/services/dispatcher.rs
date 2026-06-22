use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use dashmap::DashMap;
use indexmap::IndexMap;
use tokio::sync::{Mutex, mpsc, oneshot};

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

pub type MatchSender = flume::Sender<MatchActorMessage>;
pub type PlayerSender = mpsc::Sender<ServerMessage>;
pub type PlayerReceiver = mpsc::Receiver<ServerMessage>;

type MatchReceiver = flume::Receiver<MatchActorMessage>;
pub type MatchSenders = Arc<DashMap<LobbyId, MatchSender>>;
type PlayerGames = Arc<DashMap<PlayerId, LobbyId>>;

#[derive(Clone)]
pub struct ManagerHandle {
    pub match_senders: MatchSenders,
    player_games: PlayerGames,
    games_repo: GamesRepository,
    actor_load_lock: Arc<Mutex<()>>,
}

pub struct PlayerConnectionContext {
    pub match_id: LobbyId,
    pub match_tx: MatchSender,
    pub outbound_tx: PlayerSender,
    pub outbound_rx: PlayerReceiver,
}

impl ManagerHandle {
    pub fn new(games_repo: GamesRepository) -> Self {
        Self {
            match_senders: Arc::new(DashMap::new()),
            player_games: Arc::new(DashMap::new()),
            games_repo,
            actor_load_lock: Arc::new(Mutex::new(())),
        }
    }

    #[cfg(test)]
    pub(crate) fn active_player_route_count(&self) -> usize {
        self.player_games.len()
    }

    pub async fn create_lobby(
        &self,
        _player_id: PlayerId,
        settings: GameSettings,
    ) -> Result<CreateLobbyResponse, ManagerError> {
        let lobby_id = id::gen_lobbyid();
        let (tx, rx) = flume::unbounded();

        let actor = MatchActor::new(
            lobby_id.clone(),
            self.games_repo.clone(),
            self.match_senders.clone(),
            self.player_games.clone(),
        );

        self.match_senders.insert(lobby_id.clone(), tx.clone());
        tokio::spawn(actor.run(rx));

        let result = Self::request(&tx, |respond| MatchActorMessage::CreateLobby {
            settings,
            respond,
        })
        .await;

        if result.is_err() {
            self.match_senders.remove(&lobby_id);
        }

        result?;

        Ok(CreateLobbyResponse { lobby_id })
    }

    pub async fn join_lobby(
        &self,
        lobby_id: LobbyId,
        user_claims: UserClaims,
    ) -> Result<LobbyInfo, ManagerError> {
        let sender = self.sender_for_match(&lobby_id).await?;

        Self::request(&sender, |respond| MatchActorMessage::JoinLobby {
            user_claims,
            respond,
        })
        .await
    }

    pub async fn get_lobbies(&self) -> Vec<GetLobbyDto> {
        let mut lobby_ids: HashSet<_> = self
            .match_senders
            .iter()
            .map(|entry| entry.key().clone())
            .collect();

        match self.games_repo.waiting_game_ids().await {
            Ok(waiting) => lobby_ids.extend(waiting),
            Err(e) => tracing::error!("Error loading waiting game metadata: {e}"),
        }

        let mut lobbies = Vec::new();

        for lobby_id in lobby_ids {
            let sender = match self.sender_for_match(&lobby_id).await {
                Ok(sender) => sender,
                Err(e) => {
                    tracing::error!("Error loading match actor for {lobby_id:?}: {e}");
                    continue;
                }
            };

            let response = Self::request(&sender, |respond| MatchActorMessage::GetLobbySummary {
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
        let sender = self.sender_for_player(&player_id).await?;

        Self::request(&sender, |respond| MatchActorMessage::GameCommand {
            player_id,
            command: GameCommand::PlayTurn { card },
            respond,
        })
        .await
    }

    pub async fn bid(&self, bid: usize, player_id: PlayerId) -> Result<(), ManagerError> {
        let sender = self.sender_for_player(&player_id).await?;

        Self::request(&sender, |respond| MatchActorMessage::GameCommand {
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
        let sender = self.sender_for_player(&player_id).await?;

        Self::request(&sender, |respond| MatchActorMessage::StatusChange {
            player_id,
            ready,
            respond,
        })
        .await
    }

    pub async fn connect_player(
        &self,
        player_id: PlayerId,
    ) -> Result<PlayerConnectionContext, ManagerError> {
        let match_id = self.match_id_for_player(&player_id).await?;
        let match_tx = self.sender_for_match(&match_id).await?;
        let (outbound_tx, outbound_rx) = mpsc::channel(128);

        Self::request(&match_tx, |respond| MatchActorMessage::ConnectPlayer {
            player_id,
            outbound_tx: outbound_tx.clone(),
            respond,
        })
        .await?;

        Ok(PlayerConnectionContext {
            match_id,
            match_tx,
            outbound_tx,
            outbound_rx,
        })
    }

    async fn request<T>(
        sender: &MatchSender,
        build: impl FnOnce(oneshot::Sender<Result<T, ManagerError>>) -> MatchActorMessage,
    ) -> Result<T, ManagerError> {
        let (tx, rx) = oneshot::channel();

        sender
            .send_async(build(tx))
            .await
            .map_err(|_| ManagerError::ReceiverDisposed)?;

        rx.await.map_err(|_| ManagerError::ReceiverDisposed)?
    }

    async fn sender_for_match(&self, lobby_id: &LobbyId) -> Result<MatchSender, ManagerError> {
        if let Some(sender) = self.active_sender_for_match(lobby_id) {
            return Ok(sender);
        }

        self.load_match_actor(lobby_id).await
    }

    async fn sender_for_player(&self, player_id: &PlayerId) -> Result<MatchSender, ManagerError> {
        let lobby_id = self.match_id_for_player(player_id).await?;

        self.sender_for_match(&lobby_id).await
    }

    async fn match_id_for_player(&self, player_id: &PlayerId) -> Result<LobbyId, ManagerError> {
        if let Some(lobby_id) = self
            .player_games
            .get(player_id)
            .map(|entry| entry.value().clone())
        {
            return Ok(lobby_id);
        }

        self.games_repo
            .active_metadata_for_player(player_id)
            .await?
            .map(|metadata| metadata.lobby_id())
            .ok_or_else(|| LobbyError::PlayerNotInLobby.into())
    }

    async fn load_match_actor(&self, lobby_id: &LobbyId) -> Result<MatchSender, ManagerError> {
        let _guard = self.actor_load_lock.lock().await;

        if let Some(sender) = self.active_sender_for_match(lobby_id) {
            return Ok(sender);
        }

        if self.games_repo.active_metadata(lobby_id).await?.is_none() {
            return Err(LobbyError::InvalidLobby.into());
        }

        let events = self.games_repo.load_events(lobby_id).await?;

        if events.is_empty() {
            return Err(LobbyError::InvalidLobby.into());
        }

        let mut actor = MatchActor::new(
            lobby_id.clone(),
            self.games_repo.clone(),
            self.match_senders.clone(),
            self.player_games.clone(),
        );

        for event in events {
            actor.version = actor.version.max(event.sequence + 1);

            if let Err(e) = actor.apply_event(event.event) {
                actor.stop_game();

                return Err(e);
            }
        }

        if actor.is_finished() {
            actor.stop_game();

            if let Err(e) = self.games_repo.mark_metadata_finished(lobby_id).await {
                tracing::error!("Error marking stale finished game metadata: {e}");
            }

            return Err(LobbyError::InvalidLobby.into());
        }

        let (tx, rx) = flume::unbounded();

        self.match_senders.insert(lobby_id.clone(), tx.clone());
        tokio::spawn(actor.run(rx));

        Ok(tx)
    }

    fn active_sender_for_match(&self, lobby_id: &LobbyId) -> Option<MatchSender> {
        self.match_senders
            .get(lobby_id)
            .map(|entry| entry.value().clone())
    }
}

pub enum MatchActorMessage {
    ConnectPlayer {
        player_id: PlayerId,
        outbound_tx: PlayerSender,
        respond: oneshot::Sender<Result<(), ManagerError>>,
    },
    DisconnectPlayer {
        player_id: PlayerId,
        outbound_tx: PlayerSender,
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
    GetLobbySummary {
        respond: oneshot::Sender<Result<Option<GetLobbyDto>, ManagerError>>,
    },
}

struct MatchActor {
    lobby_id: LobbyId,
    lobby: Option<Lobby>,
    players: HashMap<PlayerId, PlayerSender>,
    version: usize,
    repo: GamesRepository,
    match_senders: MatchSenders,
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

impl MatchActor {
    fn new(
        lobby_id: LobbyId,
        repo: GamesRepository,
        match_senders: MatchSenders,
        player_games: PlayerGames,
    ) -> Self {
        Self {
            lobby_id,
            lobby: None,
            players: HashMap::new(),
            version: 0,
            repo,
            match_senders,
            player_games,
        }
    }

    async fn run(mut self, rx: MatchReceiver) {
        while let Ok(command) = rx.recv_async().await {
            let should_continue = self.handle(command).await;

            if self.is_finished() {
                self.stop_game();
                break;
            }

            if !should_continue {
                break;
            }
        }
    }

    async fn handle(&mut self, command: MatchActorMessage) -> bool {
        match command {
            MatchActorMessage::ConnectPlayer {
                player_id,
                outbound_tx,
                respond,
            } => {
                respond_once(
                    respond,
                    self.handle_connect_player(player_id, outbound_tx).await,
                );
            }
            MatchActorMessage::DisconnectPlayer {
                player_id,
                outbound_tx,
            } => {
                if self
                    .players
                    .get(&player_id)
                    .is_some_and(|current| current.same_channel(&outbound_tx))
                {
                    self.players.remove(&player_id);
                }
            }
            MatchActorMessage::CreateLobby { settings, respond } => {
                respond_once(respond, self.handle_create_lobby(settings).await);
            }
            MatchActorMessage::JoinLobby {
                user_claims,
                respond,
            } => {
                respond_once(respond, self.handle_join_lobby(user_claims).await);
            }
            MatchActorMessage::StatusChange {
                player_id,
                ready,
                respond,
            } => {
                respond_once(respond, self.handle_status_change(player_id, ready).await);
            }
            MatchActorMessage::GameCommand {
                player_id,
                command,
                respond,
            } => {
                let result = self.handle_game_command(player_id, command).await;
                let should_continue = !matches!(&result, Ok(ActorResult::Stop));
                respond_once(respond, result.map(|_| ()));
                return should_continue;
            }
            MatchActorMessage::GetLobbySummary { respond } => {
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

    async fn handle_connect_player(
        &mut self,
        player_id: PlayerId,
        outbound_tx: PlayerSender,
    ) -> Result<(), ManagerError> {
        let snapshot = {
            let lobby = self.lobby()?;

            if !lobby.players.contains_key(&player_id) {
                return Err(LobbyError::WrongLobby.into());
            }

            lobby.get_info(&player_id)
        };

        self.players.insert(player_id.clone(), outbound_tx);
        self.send_to_player(&player_id, ServerMessage::Snapshot(snapshot))
            .await;

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
                    Ok(ActorResult::Stop)
                } else {
                    Ok(ActorResult::Continue)
                }
            }
            _ => unreachable!("game command must apply a game event"),
        }
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

        let applied = self.apply_event(event.clone())?;

        if let Err(e) = self.project_metadata(&event).await {
            tracing::error!("Error projecting game metadata: {e}");
        }

        Ok(applied)
    }

    async fn project_metadata(&self, event: &DomainEvent) -> mongodb::error::Result<()> {
        match event {
            DomainEvent::LobbyCreated { .. } => self.repo.create_metadata(&self.lobby_id).await,
            DomainEvent::PlayerJoined { user_claims } => {
                let player_id = user_claims.id();

                self.repo
                    .add_metadata_player(&self.lobby_id, &player_id)
                    .await
            }
            DomainEvent::GameStarted { .. } => {
                self.repo.mark_metadata_playing(&self.lobby_id).await
            }
            DomainEvent::TurnPlayed { .. } if self.is_finished() => {
                self.repo.mark_metadata_finished(&self.lobby_id).await
            }
            _ => Ok(()),
        }
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

    fn stop_game(&mut self) {
        self.match_senders.remove(&self.lobby_id);

        if let Some(lobby) = self.lobby.as_ref() {
            for player_id in lobby.players.keys() {
                self.player_games.remove(player_id);
            }
        }

        self.players.clear();
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

    fn is_finished(&self) -> bool {
        matches!(
            self.lobby.as_ref().map(|lobby| &lobby.state),
            Some(LobbyState::Playing(game)) if game.is_finished()
        )
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
