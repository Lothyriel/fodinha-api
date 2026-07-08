use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
    time::Instant,
};

use chrono::Utc;
use dashmap::DashMap;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use crate::{
    infra::{AnonymousUserClaims, UserClaims, telemetry},
    models::{
        Card,
        commands::{
            CreateLobbyResponse, GetLobbyDto, LobbyInfo, MatchSnapshot, PlayerStatus,
            PlayingMatchSnapshot, ServerMessage,
        },
        game::{GameCommand, GameSettings, GameType, fodinha_classic},
        id::{self, MatchId, MercenaryId, PlayerId},
        lobby::{
            LobbyInfoInternal, LobbyPlayerStatus, MatchSnapshotInternal,
            PlayingMatchSnapshotInternal,
        },
    },
    services::{
        LobbyError, ManagerError,
        card_definitions::{
            CardDefinitionAssetResponse, CardDefinitionError, CardDefinitionResponse,
            CardDefinitionsService, CreateCardDefinitionAssetInput,
            CreateCardDefinitionFromAssetInput, CreateCardDefinitionInput, CreatePowerDeckInput,
            PowerDeckResponse, UpdateCardDefinitionInput,
        },
        matches::{
            MatchActor, MatchActorContext, MatchActorMessage, MatchReceiver, MatchRegistry,
            MatchSender, OutboundMessage, PlayerReceiver, PlayerSender, SenderLookup,
        },
        mercenaries::{
            MercenariesService, MercenaryError, MercenaryResponse, UpsertMercenaryInput,
        },
        repositories::matches::{MatchMetadataDto, MatchMetadataStatus, MatchesRepository},
        repositories::stats::StatsRepository,
        repositories::users::UsersRepository,
        stats::{PlayerStatsResponse, StatsProjectorHandle},
        tasks::TaskTracker,
    },
};

const BACKGROUND_TASK_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct ManagerHandle {
    pub(crate) registry: MatchRegistry,
    repo: MatchesRepository,
    stats_repo: StatsRepository,
    users_repo: UsersRepository,
    card_definitions: CardDefinitionsService,
    mercenaries: MercenariesService,
    user_cache: Arc<DashMap<PlayerId, UserClaims>>,
    stats_projector: StatsProjectorHandle,
    background: ManagerBackground,
    waiting_lobby_timeout: Duration,
    empty_playing_timeout: Duration,
}

#[derive(Clone, Default)]
struct ManagerBackground {
    actor_tasks: TaskTracker,
    deferred_tasks: TaskTracker,
    janitor_task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

fn fallback_user_claims(player_id: &PlayerId) -> UserClaims {
    UserClaims::Anonymous(AnonymousUserClaims {
        id: player_id.clone(),
        data: serde_json::json!({ "nickname": player_id.as_str() }),
        role: Default::default(),
    })
}

pub struct PlayerConnectionContext {
    pub match_id: MatchId,
    pub game_type: GameType,
    pub outbound_tx: PlayerSender,
    pub outbound_rx: PlayerReceiver,
}

impl ManagerHandle {
    pub fn new(
        repo: MatchesRepository,
        stats_repo: StatsRepository,
        users_repo: UsersRepository,
        card_definitions: CardDefinitionsService,
        mercenaries: MercenariesService,
        stats_projector: StatsProjectorHandle,
        (waiting_lobby_timeout, empty_playing_timeout): (Duration, Duration),
    ) -> Self {
        Self {
            registry: MatchRegistry::new(),
            repo,
            stats_repo,
            users_repo,
            card_definitions,
            mercenaries,
            user_cache: Arc::new(DashMap::new()),
            stats_projector,
            background: ManagerBackground::default(),
            waiting_lobby_timeout,
            empty_playing_timeout,
        }
    }

    pub async fn shutdown(&self) {
        self.abort_janitor();
        self.registry.matches.clear();
        self.registry.player_routes.clear();

        self.background
            .actor_tasks
            .shutdown(BACKGROUND_TASK_SHUTDOWN_TIMEOUT)
            .await;
        self.background
            .deferred_tasks
            .shutdown(BACKGROUND_TASK_SHUTDOWN_TIMEOUT)
            .await;
        self.stats_projector
            .shutdown(BACKGROUND_TASK_SHUTDOWN_TIMEOUT)
            .await;
    }

    pub fn abort_background_tasks(&self) {
        self.abort_janitor();
        self.registry.matches.clear();
        self.registry.player_routes.clear();
        self.background.actor_tasks.abort_all();
        self.background.deferred_tasks.abort_all();
        self.stats_projector.abort();
    }

    fn abort_janitor(&self) {
        if let Some(handle) = self
            .background
            .janitor_task
            .lock()
            .expect("janitor task lock poisoned")
            .take()
        {
            handle.abort();
        }
    }

    pub(crate) fn start_abandoned_match_janitor(
        &self,
        empty_playing_timeout: Duration,
        scan_interval: Duration,
    ) {
        let repo = self.repo.clone();
        let registry = self.registry.clone();

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(scan_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                interval.tick().await;

                if let Err(e) =
                    abandon_stale_playing_matches(&repo, &registry, empty_playing_timeout).await
                {
                    tracing::error!("Error abandoning stale playing matches: {e}");
                }
            }
        });

        if let Some(previous) = self
            .background
            .janitor_task
            .lock()
            .expect("janitor task lock poisoned")
            .replace(handle)
        {
            previous.abort();
        }
    }

    pub async fn create_lobby(
        &self,
        player_id: PlayerId,
        settings: GameSettings,
    ) -> Result<CreateLobbyResponse, ManagerError> {
        self.validate_lobby_settings(&settings).await?;

        let started = Instant::now();
        let match_id = id::gen_matchid();
        let game_type = settings.game_type();
        let (tx, rx) = flume::unbounded();

        let actor = self.new_actor(match_id.clone(), game_type);

        self.registry
            .mark_ready(match_id.clone(), tx.clone(), game_type);
        self.background.actor_tasks.spawn(actor.run(rx));

        let result = Self::request(&tx, game_type, |respond| MatchActorMessage::CreateMatch {
            creator_id: player_id,
            settings,
            respond,
        })
        .await;
        telemetry::record_actor_start("new", Some(game_type), started.elapsed(), result.is_ok());

        if result.is_err() {
            self.registry.remove_match(&match_id);
        }

        result?;

        Ok(CreateLobbyResponse {
            lobby_id: match_id,
            game_type,
        })
    }

    async fn validate_lobby_settings(&self, settings: &GameSettings) -> Result<(), ManagerError> {
        if let GameSettings::FodinhaPower(settings) = settings
            && !self
                .card_definitions
                .power_deck_exists(&settings.power_deck_id)
                .await?
        {
            return Err(LobbyError::InvalidSettings(format!(
                "power deck `{}` does not exist",
                settings.power_deck_id
            ))
            .into());
        }

        Ok(())
    }

    pub async fn join_lobby(
        &self,
        match_id: MatchId,
        player_id: PlayerId,
    ) -> Result<LobbyInfo, ManagerError> {
        let context = self.sender_for_match(&match_id).await?;

        let info = Self::request(&context.sender, context.game_type, |respond| {
            MatchActorMessage::JoinLobby { player_id, respond }
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
            let context = match self.sender_for_match(&match_id).await {
                Ok(context) => context,
                Err(e) => {
                    tracing::error!("Error loading match actor for {match_id:?}: {e}");
                    continue;
                }
            };

            let response = Self::request(&context.sender, context.game_type, |respond| {
                MatchActorMessage::GetLobbySummary { respond }
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

    pub async fn create_card_definition(
        &self,
        creator_id: PlayerId,
        input: CreateCardDefinitionInput,
    ) -> Result<CardDefinitionResponse, CardDefinitionError> {
        self.card_definitions.create_card(creator_id, input).await
    }

    pub async fn create_card_definition_asset(
        &self,
        input: CreateCardDefinitionAssetInput,
    ) -> Result<CardDefinitionAssetResponse, CardDefinitionError> {
        self.card_definitions.create_card_asset(input).await
    }

    pub async fn create_card_definition_from_asset(
        &self,
        creator_id: PlayerId,
        input: CreateCardDefinitionFromAssetInput,
    ) -> Result<CardDefinitionResponse, CardDefinitionError> {
        self.card_definitions
            .create_card_from_asset(creator_id, input)
            .await
    }

    pub async fn update_card_definition(
        &self,
        editor_id: PlayerId,
        card_id: crate::models::id::CardId,
        input: UpdateCardDefinitionInput,
    ) -> Result<CardDefinitionResponse, CardDefinitionError> {
        self.card_definitions
            .update_card(editor_id, card_id, input)
            .await
    }

    pub async fn card_definitions(
        &self,
    ) -> Result<Vec<CardDefinitionResponse>, CardDefinitionError> {
        self.card_definitions.list_cards().await
    }

    pub async fn create_power_deck(
        &self,
        creator_id: PlayerId,
        input: CreatePowerDeckInput,
    ) -> Result<PowerDeckResponse, CardDefinitionError> {
        self.card_definitions.create_deck(creator_id, input).await
    }

    pub async fn power_decks(
        &self,
        viewer_id: &PlayerId,
    ) -> Result<Vec<PowerDeckResponse>, CardDefinitionError> {
        self.card_definitions.list_decks(viewer_id).await
    }

    pub async fn mercenaries(&self) -> Result<Vec<MercenaryResponse>, MercenaryError> {
        self.mercenaries.list_mercenaries().await
    }

    pub async fn create_mercenary(
        &self,
        creator_id: PlayerId,
        input: UpsertMercenaryInput,
    ) -> Result<MercenaryResponse, MercenaryError> {
        self.mercenaries.create_mercenary(creator_id, input).await
    }

    pub async fn update_mercenary(
        &self,
        editor_id: PlayerId,
        mercenary_id: crate::models::id::MercenaryId,
        input: UpsertMercenaryInput,
    ) -> Result<MercenaryResponse, MercenaryError> {
        self.mercenaries
            .update_mercenary(editor_id, mercenary_id, input)
            .await
    }

    pub async fn upsert_user(&self, user: &UserClaims) -> Result<UserClaims, ManagerError> {
        let user = self.users_repo.upsert_user(user).await?;
        self.cache_user(user.clone());

        Ok(user)
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
        self.game_command(
            GameCommand::FodinhaClassic(fodinha_classic::GameCommand::PlayTurn { card }),
            player_id,
        )
        .await
    }

    pub async fn bid(&self, bid: usize, player_id: PlayerId) -> Result<(), ManagerError> {
        self.game_command(
            GameCommand::FodinhaClassic(fodinha_classic::GameCommand::PutBid { bid }),
            player_id,
        )
        .await
    }

    pub async fn game_command(
        &self,
        command: GameCommand,
        player_id: PlayerId,
    ) -> Result<(), ManagerError> {
        let context = self.sender_for_player(&player_id).await?;
        let command = command.into_typed(context.game_type)?;

        Self::request(&context.sender, context.game_type, |respond| {
            MatchActorMessage::GameCommand {
                player_id,
                command,
                respond,
            }
        })
        .await
    }

    pub async fn player_status_change(
        &self,
        player_id: PlayerId,
        ready: bool,
    ) -> Result<(), ManagerError> {
        let context = self.sender_for_player(&player_id).await?;

        Self::request(&context.sender, context.game_type, |respond| {
            MatchActorMessage::StatusChange {
                player_id,
                ready,
                respond,
            }
        })
        .await
    }

    pub async fn select_mercenary(
        &self,
        player_id: PlayerId,
        mercenary_id: MercenaryId,
    ) -> Result<(), ManagerError> {
        let context = self.sender_for_player(&player_id).await?;

        Self::request(&context.sender, context.game_type, |respond| {
            MatchActorMessage::SelectMercenary {
                player_id,
                mercenary_id,
                respond,
            }
        })
        .await
    }

    pub async fn connect_player(
        &self,
        player_id: PlayerId,
    ) -> Result<PlayerConnectionContext, ManagerError> {
        let match_id = self.match_id_for_player(&player_id).await?;
        let context = self.sender_for_match(&match_id).await?;
        let (outbound_tx, outbound_rx) = mpsc::channel(128);

        Self::request(&context.sender, context.game_type, |respond| {
            MatchActorMessage::ConnectPlayer {
                player_id,
                outbound_tx: outbound_tx.clone(),
                respond,
            }
        })
        .await?;

        Ok(PlayerConnectionContext {
            match_id,
            game_type: context.game_type,
            outbound_tx,
            outbound_rx,
        })
    }

    pub async fn hydrate_outbound_message(
        &self,
        msg: OutboundMessage,
    ) -> Result<ServerMessage, ManagerError> {
        match msg {
            OutboundMessage::Close { reason, .. } => Err(ManagerError::PlayerDisconnected(reason)),
            OutboundMessage::PlayerTurn { player_id } => {
                Ok(ServerMessage::PlayerTurn { player_id })
            }
            OutboundMessage::TurnPlayed { pile } => Ok(ServerMessage::TurnPlayed { pile }),
            OutboundMessage::PlayerBidded { player_id, bid } => {
                Ok(ServerMessage::PlayerBidded { player_id, bid })
            }
            OutboundMessage::PlayersManaChanged(mana) => {
                Ok(ServerMessage::PlayersManaChanged(mana))
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
            OutboundMessage::PlayerMercenarySelected {
                player_id,
                mercenary_id,
            } => Ok(ServerMessage::PlayerMercenarySelected {
                player_id,
                mercenary_id,
            }),
            OutboundMessage::RoundEnded(rounds) => Ok(ServerMessage::RoundEnded(rounds)),
            OutboundMessage::PlayerDeck(deck) => Ok(ServerMessage::PlayerDeck(deck)),
            OutboundMessage::PlayerPowerCards(deck) => Ok(ServerMessage::PlayerPowerCards(deck)),
            OutboundMessage::PowerCardPlayed {
                player_id,
                card,
                target_player_id,
                lifes,
            } => Ok(ServerMessage::PowerCardPlayed {
                player_id,
                card,
                target_player_id,
                lifes,
            }),
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
        let Ok(context) = self.sender_for_match(match_id).await else {
            return;
        };

        let message = MatchActorMessage::DisconnectPlayer {
            player_id,
            outbound_tx,
            shutting_down,
        };
        let kind = message.kind();
        let started = Instant::now();
        let result = context.sender.send_async(message).await;
        if result.is_ok() {
            telemetry::inc_actor_queue_depth(context.game_type);
        }
        telemetry::record_actor_message(kind, context.game_type, started.elapsed());

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
                        mercenary_id: status.mercenary_id,
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
        game_type: GameType,
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

        telemetry::inc_actor_queue_depth(game_type);

        let result = rx.await.map_err(|_| ManagerError::ReceiverDisposed)?;
        telemetry::record_actor_message(kind, game_type, started.elapsed());

        result
    }

    async fn sender_for_player(
        &self,
        player_id: &PlayerId,
    ) -> Result<MatchActorContext, ManagerError> {
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

    async fn sender_for_match(
        &self,
        match_id: &MatchId,
    ) -> Result<MatchActorContext, ManagerError> {
        match self.registry.sender_or_mark_loading(match_id).await? {
            SenderLookup::Ready(context) => Ok(context),
            SenderLookup::Load(loading) => {
                let result = self.load_match_actor(match_id).await;
                self.registry.finish_loading(match_id, &loading, &result);
                result
            }
        }
    }

    async fn load_match_actor(
        &self,
        match_id: &MatchId,
    ) -> Result<MatchActorContext, ManagerError> {
        let started = Instant::now();
        let metadata = self.repo.active_metadata(match_id).await?.ok_or_else(|| {
            telemetry::record_actor_start("load", None, started.elapsed(), false);
            LobbyError::InvalidLobby
        })?;
        let game_type = metadata_game_type(&metadata);

        if metadata.is_waiting_stale(self.waiting_lobby_timeout) {
            if let Err(e) = self.repo.delete_metadata(match_id).await {
                tracing::error!("Error deleting stale waiting match metadata: {e}");
            }

            telemetry::record_actor_start("load", Some(game_type), started.elapsed(), false);
            return Err(LobbyError::InvalidLobby.into());
        }

        if metadata.is_playing_stale(self.empty_playing_timeout) {
            if let Err(e) = self.repo.mark_metadata_abandoned(match_id).await {
                tracing::error!("Error abandoning stale playing match metadata: {e}");
            }

            telemetry::record_actor_start("load", Some(game_type), started.elapsed(), false);
            return Err(LobbyError::InvalidLobby.into());
        }

        let metadata_status = metadata.status;
        let events = self.repo.load_events(match_id).await?;

        if events.is_empty() && metadata_status != MatchMetadataStatus::Waiting {
            telemetry::record_actor_start("load", Some(game_type), started.elapsed(), false);
            return Err(LobbyError::InvalidLobby.into());
        }

        let mut actor = self.new_actor(match_id.clone(), game_type);

        actor.restore_from_metadata(metadata);

        for event in events {
            actor.version = actor.version.max(event.sequence + 1);

            if let Err(e) = actor.replay_event(event.event) {
                self.registry.remove_match(match_id);
                telemetry::record_actor_start("load", Some(game_type), started.elapsed(), false);

                return Err(e);
            }
        }

        if actor.is_finished() {
            self.registry.remove_match(match_id);

            if let Err(e) = self.repo.mark_metadata_finished(match_id).await {
                tracing::error!("Error marking stale finished match metadata: {e}");
            }

            self.stats_projector.notify_match_finished(match_id);
            telemetry::record_actor_start("load", Some(game_type), started.elapsed(), false);

            return Err(LobbyError::InvalidLobby.into());
        }

        let (tx, rx): (MatchSender, MatchReceiver) = flume::unbounded();

        self.registry
            .mark_ready(match_id.clone(), tx.clone(), game_type);
        self.background.actor_tasks.spawn(actor.run(rx));
        telemetry::record_actor_start("load", Some(game_type), started.elapsed(), true);

        Ok(MatchActorContext {
            sender: tx,
            game_type,
        })
    }

    fn new_actor(&self, match_id: MatchId, game_type: GameType) -> MatchActor {
        MatchActor::new(
            match_id,
            game_type,
            self.repo.clone(),
            self.stats_projector.clone(),
            self.background.deferred_tasks.clone(),
            self.registry.clone(),
            (self.waiting_lobby_timeout, self.empty_playing_timeout),
        )
    }
}

fn metadata_game_type(metadata: &MatchMetadataDto) -> GameType {
    metadata.settings.game_type()
}

async fn abandon_stale_playing_matches(
    repo: &MatchesRepository,
    registry: &MatchRegistry,
    empty_playing_timeout: Duration,
) -> Result<(), ManagerError> {
    let match_ids = repo.stale_playing_match_ids(empty_playing_timeout).await?;

    for match_id in match_ids {
        if registry.matches.contains_key(&match_id) {
            continue;
        }

        if repo
            .mark_metadata_abandoned_if_stale(&match_id, empty_playing_timeout)
            .await?
        {
            tracing::info!("Abandoned stale playing match {match_id:?}");
            registry.remove_match(&match_id);
        }
    }

    Ok(())
}
