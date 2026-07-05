use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
    time::Instant,
};

use chrono::Utc;
use dashmap::DashMap;
use tokio::sync::{mpsc, oneshot};

use crate::{
    infra::{AnonymousUserClaims, UserClaims, telemetry},
    models::{
        Card,
        commands::{
            CreateLobbyResponse, GameCommand, GetLobbyDto, LobbyInfo, MatchSnapshot, PlayerStatus,
            PlayingMatchSnapshot, ServerMessage,
        },
        game::GameSettings,
        id::{self, MatchId, PlayerId},
        lobby::{
            LobbyInfoInternal, LobbyPlayerStatus, MatchSnapshotInternal,
            PlayingMatchSnapshotInternal,
        },
    },
    services::{
        LobbyError, ManagerError,
        matches::{
            MatchActor, MatchActorMessage, MatchReceiver, MatchRegistry, MatchSender,
            OutboundMessage, PlayerReceiver, PlayerSender, SenderLookup,
        },
        repositories::matches::{MatchMetadataStatus, MatchesRepository},
        repositories::stats::StatsRepository,
        repositories::users::UsersRepository,
        stats::{PlayerStatsResponse, StatsProjectorHandle},
    },
};

#[derive(Clone)]
pub struct ManagerHandle {
    pub(crate) registry: MatchRegistry,
    repo: MatchesRepository,
    stats_repo: StatsRepository,
    users_repo: UsersRepository,
    user_cache: Arc<DashMap<PlayerId, UserClaims>>,
    stats_projector: StatsProjectorHandle,
    waiting_lobby_timeout: Duration,
}

fn fallback_user_claims(player_id: &PlayerId) -> UserClaims {
    UserClaims::Anonymous(AnonymousUserClaims {
        id: player_id.clone(),
        data: serde_json::json!({ "nickname": player_id.as_str() }),
    })
}

pub struct PlayerConnectionContext {
    pub match_id: MatchId,
    pub outbound_tx: PlayerSender,
    pub outbound_rx: PlayerReceiver,
}

impl ManagerHandle {
    pub fn new(
        repo: MatchesRepository,
        stats_repo: StatsRepository,
        users_repo: UsersRepository,
        stats_projector: StatsProjectorHandle,
        waiting_lobby_timeout: Duration,
    ) -> Self {
        Self {
            registry: MatchRegistry::new(),
            repo,
            stats_repo,
            users_repo,
            user_cache: Arc::new(DashMap::new()),
            stats_projector,
            waiting_lobby_timeout,
        }
    }

    #[cfg(test)]
    pub(crate) fn active_player_route_count(&self) -> usize {
        self.registry.player_route_count()
    }

    pub async fn create_lobby(
        &self,
        player_id: PlayerId,
        settings: GameSettings,
    ) -> Result<CreateLobbyResponse, ManagerError> {
        let started = Instant::now();
        let match_id = id::gen_matchid();
        let (tx, rx) = flume::unbounded();

        let actor = self.new_actor(match_id.clone());

        self.registry.mark_ready(match_id.clone(), tx.clone());
        tokio::spawn(actor.run(rx));

        let result = Self::request(&tx, |respond| MatchActorMessage::CreateMatch {
            creator_id: player_id,
            settings,
            respond,
        })
        .await;
        telemetry::record_actor_start("new", started.elapsed(), result.is_ok());

        if result.is_err() {
            self.registry.remove_match(&match_id);
        }

        result?;

        Ok(CreateLobbyResponse { lobby_id: match_id })
    }

    pub async fn join_lobby(
        &self,
        match_id: MatchId,
        player_id: PlayerId,
    ) -> Result<LobbyInfo, ManagerError> {
        let sender = self.sender_for_match(&match_id).await?;

        let info = Self::request(&sender, |respond| MatchActorMessage::JoinLobby {
            player_id,
            respond,
        })
        .await?;

        self.hydrate_lobby_info(info).await
    }

    pub async fn get_lobbies(&self) -> Vec<GetLobbyDto> {
        let mut match_ids: HashSet<_> = self
            .registry
            .matches
            .iter()
            .map(|entry| entry.key().clone())
            .collect();

        match self.repo.waiting_match_ids().await {
            Ok(waiting) => match_ids.extend(waiting),
            Err(e) => tracing::error!("Error loading waiting match metadata: {e}"),
        }

        let mut lobbies = Vec::new();

        for match_id in match_ids {
            let sender = match self.sender_for_match(&match_id).await {
                Ok(sender) => sender,
                Err(e) => {
                    tracing::error!("Error loading match actor for {match_id:?}: {e}");
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

    pub async fn leaderboard(&self, limit: i64) -> Result<Vec<PlayerStatsResponse>, ManagerError> {
        let stats = self.stats_repo.leaderboard(limit).await?;
        let player_ids = stats
            .iter()
            .map(|stats| stats.player_id.clone())
            .collect::<Vec<_>>();
        let users = self.users_repo.users_by_id(&player_ids).await?;
        let stats = stats
            .into_iter()
            .map(|stats| {
                let player = users.get(&stats.player_id).cloned();

                stats.into_response(player)
            })
            .collect();

        Ok(stats)
    }

    pub async fn player_stats(
        &self,
        player_id: &PlayerId,
    ) -> Result<Option<PlayerStatsResponse>, ManagerError> {
        let Some(stats) = self.stats_repo.player_stats(player_id).await? else {
            return Ok(None);
        };
        let user = self.users_repo.user(player_id.as_str()).await?;

        Ok(Some(stats.into_response(user)))
    }

    pub async fn upsert_user(&self, user: &UserClaims) -> Result<(), ManagerError> {
        self.users_repo.upsert_user(user).await?;
        self.cache_user(user.clone());

        Ok(())
    }

    pub async fn user(&self, player_id: &PlayerId) -> Result<Option<UserClaims>, ManagerError> {
        Ok(self.users_repo.user(player_id.as_str()).await?)
    }

    pub async fn store_refresh_token(
        &self,
        player_id: &PlayerId,
        token: &str,
        expires_at: i64,
    ) -> Result<(), ManagerError> {
        self.users_repo
            .store_refresh_token(player_id.as_str(), token, expires_at)
            .await?;

        Ok(())
    }

    pub async fn refresh_player_id(&self, token: &str) -> Result<Option<PlayerId>, ManagerError> {
        let Some(session) = self.users_repo.refresh_session(token).await? else {
            return Ok(None);
        };

        if session.expires_at <= Utc::now().timestamp() {
            return Ok(None);
        }

        Ok(Some(PlayerId(session.player_id.into())))
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
            outbound_tx,
            outbound_rx,
        })
    }

    pub async fn hydrate_outbound_message(
        &self,
        msg: OutboundMessage,
    ) -> Result<ServerMessage, ManagerError> {
        match msg {
            OutboundMessage::PlayerTurn { player_id } => {
                Ok(ServerMessage::PlayerTurn { player_id })
            }
            OutboundMessage::TurnPlayed { pile } => Ok(ServerMessage::TurnPlayed { pile }),
            OutboundMessage::PlayerBidded { player_id, bid } => {
                Ok(ServerMessage::PlayerBidded { player_id, bid })
            }
            OutboundMessage::PlayerBiddingTurn {
                player_id,
                possible_bids,
            } => Ok(ServerMessage::PlayerBiddingTurn {
                player_id,
                possible_bids,
            }),
            OutboundMessage::PlayerStatusChange { player_id, ready } => {
                Ok(ServerMessage::PlayerStatusChange { player_id, ready })
            }
            OutboundMessage::RoundEnded(rounds) => Ok(ServerMessage::RoundEnded(rounds)),
            OutboundMessage::PlayerDeck(deck) => Ok(ServerMessage::PlayerDeck(deck)),
            OutboundMessage::SetStart { upcard } => Ok(ServerMessage::SetStart { upcard }),
            OutboundMessage::SetEnded { lifes } => Ok(ServerMessage::SetEnded { lifes }),
            OutboundMessage::GameEnded { lifes } => Ok(ServerMessage::GameEnded { lifes }),
            OutboundMessage::PlayerJoined(player_id) => {
                let player = self.user_or_fallback(&player_id).await?;

                Ok(ServerMessage::PlayerJoined(player))
            }
            OutboundMessage::PlayerLeft { player_id } => {
                Ok(ServerMessage::PlayerLeft { player_id })
            }
            OutboundMessage::Snapshot(snapshot) => Ok(ServerMessage::Snapshot(
                self.hydrate_snapshot(snapshot).await?,
            )),
        }
    }

    pub async fn disconnect_player(
        &self,
        match_id: &MatchId,
        player_id: PlayerId,
        outbound_tx: PlayerSender,
        shutting_down: bool,
    ) {
        let Ok(sender) = self.sender_for_match(match_id).await else {
            return;
        };

        let message = MatchActorMessage::DisconnectPlayer {
            player_id,
            outbound_tx,
            shutting_down,
        };
        let kind = message.kind();
        let started = Instant::now();
        let result = sender.send_async(message).await;
        if result.is_ok() {
            telemetry::inc_actor_queue_depth();
        }
        telemetry::record_actor_message(kind, started.elapsed());

        if let Err(e) = result {
            tracing::warn!("Error enqueueing actor disconnect message: {e}");
        }
    }

    async fn hydrate_lobby_info(&self, info: LobbyInfoInternal) -> Result<LobbyInfo, ManagerError> {
        match info {
            LobbyInfoInternal::NotStarted(players) => {
                Ok(LobbyInfo::NotStarted(self.hydrate_players(players).await?))
            }
            LobbyInfoInternal::Playing(game) => Ok(LobbyInfo::Playing(game)),
        }
    }

    async fn hydrate_snapshot(
        &self,
        snapshot: MatchSnapshotInternal,
    ) -> Result<MatchSnapshot, ManagerError> {
        match snapshot {
            MatchSnapshotInternal::Waiting(players) => {
                Ok(MatchSnapshot::Waiting(self.hydrate_players(players).await?))
            }
            MatchSnapshotInternal::Playing(PlayingMatchSnapshotInternal { players, game }) => {
                Ok(MatchSnapshot::Playing(PlayingMatchSnapshot {
                    players: self.hydrate_players(players).await?,
                    game,
                }))
            }
        }
    }

    async fn hydrate_players(
        &self,
        players: HashMap<PlayerId, LobbyPlayerStatus>,
    ) -> Result<HashMap<PlayerId, PlayerStatus>, ManagerError> {
        let mut users = HashMap::new();
        let mut missing = Vec::new();

        for player_id in players.keys() {
            match self.user_cache.get(player_id) {
                Some(user) => {
                    users.insert(player_id.as_str().to_string(), user.clone());
                }
                None => missing.push(player_id.as_str().to_string()),
            }
        }

        let loaded_users = self.users_repo.users_by_id(&missing).await?;

        for user in loaded_users.values() {
            self.cache_user(user.clone());
        }

        users.extend(loaded_users);

        let players = players
            .into_iter()
            .map(|(player_id, status)| {
                let player = users
                    .get(player_id.as_str())
                    .cloned()
                    .unwrap_or_else(|| fallback_user_claims(&player_id));

                (
                    player_id,
                    PlayerStatus {
                        ready: status.ready,
                        player,
                    },
                )
            })
            .collect();

        Ok(players)
    }

    async fn user_or_fallback(&self, player_id: &PlayerId) -> Result<UserClaims, ManagerError> {
        Ok(self
            .cached_user(player_id)
            .await?
            .unwrap_or_else(|| fallback_user_claims(player_id)))
    }

    async fn cached_user(&self, player_id: &PlayerId) -> Result<Option<UserClaims>, ManagerError> {
        if let Some(user) = self.user_cache.get(player_id) {
            return Ok(Some(user.clone()));
        }

        let user = self.users_repo.user(player_id.as_str()).await?;

        if let Some(user) = &user {
            self.cache_user(user.clone());
        }

        Ok(user)
    }

    fn cache_user(&self, user: UserClaims) {
        self.user_cache.insert(user.id(), user);
    }

    async fn request<T>(
        sender: &MatchSender,
        build: impl FnOnce(oneshot::Sender<Result<T, ManagerError>>) -> MatchActorMessage,
    ) -> Result<T, ManagerError> {
        let (tx, rx) = oneshot::channel();
        let message = build(tx);
        let kind = message.kind();
        let started = Instant::now();

        sender
            .send_async(message)
            .await
            .map_err(|_| ManagerError::ReceiverDisposed)?;

        telemetry::inc_actor_queue_depth();

        let result = rx.await.map_err(|_| ManagerError::ReceiverDisposed)?;
        telemetry::record_actor_message(kind, started.elapsed());

        result
    }

    async fn sender_for_player(&self, player_id: &PlayerId) -> Result<MatchSender, ManagerError> {
        let match_id = self.match_id_for_player(player_id).await?;

        self.sender_for_match(&match_id).await
    }

    async fn match_id_for_player(&self, player_id: &PlayerId) -> Result<MatchId, ManagerError> {
        if let Some(match_id) = self.registry.match_for_player(player_id) {
            return Ok(match_id);
        }

        self.repo
            .active_metadata_for_player(player_id)
            .await?
            .map(|metadata| metadata.match_id())
            .ok_or_else(|| LobbyError::PlayerNotInLobby.into())
    }

    async fn sender_for_match(&self, match_id: &MatchId) -> Result<MatchSender, ManagerError> {
        match self.registry.sender_or_mark_loading(match_id).await? {
            SenderLookup::Ready(sender) => Ok(sender),
            SenderLookup::Load(loading) => {
                let result = self.load_match_actor(match_id).await;
                self.registry.finish_loading(match_id, &loading, &result);
                result
            }
        }
    }

    async fn load_match_actor(&self, match_id: &MatchId) -> Result<MatchSender, ManagerError> {
        let started = Instant::now();
        let metadata = self
            .repo
            .active_metadata(match_id)
            .await?
            .ok_or(LobbyError::InvalidLobby)?;

        if metadata.is_waiting_stale(self.waiting_lobby_timeout) {
            if let Err(e) = self.repo.delete_metadata(match_id).await {
                tracing::error!("Error deleting stale waiting match metadata: {e}");
            }

            telemetry::record_actor_start("load", started.elapsed(), false);
            return Err(LobbyError::InvalidLobby.into());
        }

        let metadata_status = metadata.status;
        let events = self.repo.load_events(match_id).await?;

        if events.is_empty() && metadata_status != MatchMetadataStatus::Waiting {
            telemetry::record_actor_start("load", started.elapsed(), false);
            return Err(LobbyError::InvalidLobby.into());
        }

        let mut actor = self.new_actor(match_id.clone());

        actor.restore_from_metadata(metadata);

        for event in events {
            actor.version = actor.version.max(event.sequence + 1);

            if let Err(e) = actor.replay_event(event.event) {
                self.registry.remove_match(match_id);
                telemetry::record_actor_start("load", started.elapsed(), false);

                return Err(e);
            }
        }

        if actor.is_finished() {
            self.registry.remove_match(match_id);

            if let Err(e) = self.repo.mark_metadata_finished(match_id).await {
                tracing::error!("Error marking stale finished match metadata: {e}");
            }

            self.stats_projector.notify_match_finished(match_id);
            telemetry::record_actor_start("load", started.elapsed(), false);

            return Err(LobbyError::InvalidLobby.into());
        }

        let (tx, rx): (MatchSender, MatchReceiver) = flume::unbounded();

        self.registry.mark_ready(match_id.clone(), tx.clone());
        tokio::spawn(actor.run(rx));
        telemetry::record_actor_start("load", started.elapsed(), true);

        Ok(tx)
    }

    fn new_actor(&self, match_id: MatchId) -> MatchActor {
        MatchActor::new(
            match_id,
            self.repo.clone(),
            self.stats_projector.clone(),
            self.registry.matches.clone(),
            self.registry.player_routes.clone(),
            self.waiting_lobby_timeout,
        )
    }
}
