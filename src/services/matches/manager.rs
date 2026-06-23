use std::collections::{HashMap, HashSet};

use tokio::sync::{mpsc, oneshot};

use crate::{
    infra::{AnonymousUserClaims, UserClaims},
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
    stats_projector: StatsProjectorHandle,
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
    ) -> Self {
        Self {
            registry: MatchRegistry::new(),
            repo,
            stats_repo,
            users_repo,
            stats_projector,
        }
    }

    #[cfg(test)]
    pub(crate) fn active_player_route_count(&self) -> usize {
        self.registry.player_route_count()
    }

    pub async fn create_lobby(
        &self,
        _player_id: PlayerId,
        settings: GameSettings,
    ) -> Result<CreateLobbyResponse, ManagerError> {
        let match_id = id::gen_matchid();
        let (tx, rx) = flume::unbounded();

        let actor = self.new_actor(match_id.clone());

        self.registry.mark_ready(match_id.clone(), tx.clone());
        tokio::spawn(actor.run(rx));

        let result = Self::request(&tx, |respond| MatchActorMessage::CreateMatch {
            settings,
            respond,
        })
        .await;

        if result.is_err() {
            self.registry.remove_match(&match_id);
        }

        result?;

        Ok(CreateLobbyResponse { lobby_id: match_id })
    }

    pub async fn join_lobby(
        &self,
        match_id: MatchId,
        user_claims: UserClaims,
    ) -> Result<LobbyInfo, ManagerError> {
        self.users_repo.upsert_user(&user_claims).await?;
        let player_id = user_claims.id();
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

        Ok(())
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
    ) {
        let Ok(sender) = self.sender_for_match(match_id).await else {
            return;
        };

        let _ = sender
            .send_async(MatchActorMessage::DisconnectPlayer {
                player_id,
                outbound_tx,
            })
            .await;
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
        let player_ids = players
            .keys()
            .map(|player_id| player_id.as_str().to_string())
            .collect::<Vec<_>>();
        let users = self.users_repo.users_by_id(&player_ids).await?;

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
            .users_repo
            .user(player_id.as_str())
            .await?
            .unwrap_or_else(|| fallback_user_claims(player_id)))
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
        let metadata = self
            .repo
            .active_metadata(match_id)
            .await?
            .ok_or(LobbyError::InvalidLobby)?;
        let metadata_status = metadata.status;
        let events = self.repo.load_events(match_id).await?;

        if events.is_empty() && metadata_status != MatchMetadataStatus::Waiting {
            return Err(LobbyError::InvalidLobby.into());
        }

        let mut actor = self.new_actor(match_id.clone());

        actor.restore_from_metadata(metadata);

        for event in events {
            actor.version = actor.version.max(event.sequence + 1);

            if let Err(e) = actor.replay_event(event.event) {
                self.registry.remove_match(match_id);

                return Err(e);
            }
        }

        if actor.is_finished() {
            self.registry.remove_match(match_id);

            if let Err(e) = self.repo.mark_metadata_finished(match_id).await {
                tracing::error!("Error marking stale finished match metadata: {e}");
            }

            self.stats_projector.notify_match_finished(match_id);

            return Err(LobbyError::InvalidLobby.into());
        }

        let (tx, rx): (MatchSender, MatchReceiver) = flume::unbounded();

        self.registry.mark_ready(match_id.clone(), tx.clone());
        tokio::spawn(actor.run(rx));

        Ok(tx)
    }

    fn new_actor(&self, match_id: MatchId) -> MatchActor {
        MatchActor::new(
            match_id,
            self.repo.clone(),
            self.stats_projector.clone(),
            self.registry.matches.clone(),
            self.registry.player_routes.clone(),
        )
    }
}
