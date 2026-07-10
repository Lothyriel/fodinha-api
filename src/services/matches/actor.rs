use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use indexmap::IndexMap;

use crate::{
    infra::telemetry,
    models::{
        Card, Game, GameError, GameOutcome, LobbyState,
        commands::GetLobbyDto,
        game::{
            AppliedGameChange, AppliedTurn, BiddingState, GameEvent, GameSettings, GameType,
            MatchEvent, NewSet, fodinha_classic, fodinha_power,
        },
        id::{MatchId, MercenaryId, PlayerId},
        lobby::{Lobby, LobbyInfoInternal, LobbyPlayerStatus},
    },
    services::{
        LobbyError, ManagerError, PlayerManaDto, PowerCardDto,
        matches::{
            MatchActorMessage, MatchEntries, MatchReceiver, MatchRegistry, OutboundMessage,
            PlayerRoutes, PlayerSender, WAITING_LOBBY_INACTIVITY_CLOSE_CODE,
            WAITING_LOBBY_INACTIVITY_CLOSE_REASON, project_match_metadata,
        },
        repositories::matches::{MatchMetadataDto, MatchesRepository},
        stats::StatsProjectorHandle,
        tasks::TaskTracker,
    },
};

pub(crate) struct MatchActor {
    match_id: MatchId,
    game_type: GameType,
    lobby: Option<Lobby>,
    creator_id: Option<PlayerId>,
    connections: HashMap<PlayerId, PlayerSender>,
    pub(crate) version: usize,
    repo: MatchesRepository,
    stats_projector: StatsProjectorHandle,
    deferred_tasks: TaskTracker,
    power_card_registry: fodinha_power::PowerCardRegistryStore,
    match_entries: MatchEntries,
    player_routes: PlayerRoutes,
    last_activity: Instant,
    waiting_lobby_timeout: Duration,
    empty_playing_since: Option<Instant>,
    empty_playing_timeout: Duration,
}

pub(crate) struct MatchActorResources {
    pub(crate) repo: MatchesRepository,
    pub(crate) stats_projector: StatsProjectorHandle,
    pub(crate) deferred_tasks: TaskTracker,
    pub(crate) power_card_registry: fodinha_power::PowerCardRegistryStore,
    pub(crate) registry: MatchRegistry,
    pub(crate) waiting_lobby_timeout: Duration,
    pub(crate) empty_playing_timeout: Duration,
}

enum AppliedEvent {
    None,
    PlayerJoined,
    PlayerStatusChanged,
    GameStarted {
        set: NewSet,
        lifes: Option<HashMap<PlayerId, usize>>,
        power_decks: Option<IndexMap<PlayerId, Vec<PowerCardDto>>>,
        mana: Option<HashMap<PlayerId, PlayerManaDto>>,
        next: PlayerId,
        possible_bids: Vec<usize>,
    },
    Game(AppliedGameChange),
}

impl MatchActor {
    pub(crate) fn new(
        match_id: MatchId,
        game_type: GameType,
        resources: MatchActorResources,
    ) -> Self {
        let MatchActorResources {
            repo,
            stats_projector,
            deferred_tasks,
            power_card_registry,
            registry,
            waiting_lobby_timeout,
            empty_playing_timeout,
        } = resources;

        Self {
            match_id,
            game_type,
            lobby: None,
            creator_id: None,
            connections: HashMap::new(),
            version: 0,
            repo,
            stats_projector,
            deferred_tasks,
            power_card_registry,
            match_entries: registry.matches,
            player_routes: registry.player_routes,
            last_activity: Instant::now(),
            waiting_lobby_timeout,
            empty_playing_since: None,
            empty_playing_timeout,
        }
    }

    pub(crate) async fn run(mut self, rx: MatchReceiver) {
        telemetry::inc_active_actors(self.game_type);

        loop {
            let command = match self.next_command(&rx).await {
                Ok(Some(command)) => command,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("Error handling match actor timeout: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };

            let should_continue = self.handle(command).await;

            if self.is_finished() {
                self.stats_projector.notify_match_finished(&self.match_id);
                self.stop_match();
                break;
            }

            if !should_continue {
                break;
            }
        }

        telemetry::dec_active_actors(self.game_type);
    }

    async fn next_command(
        &mut self,
        rx: &MatchReceiver,
    ) -> Result<Option<MatchActorMessage>, ManagerError> {
        if self.is_waiting_lobby() {
            return match tokio::time::timeout(self.time_until_waiting_timeout(), rx.recv_async())
                .await
            {
                Ok(Ok(command)) => {
                    telemetry::dec_actor_queue_depth(self.game_type);
                    Ok(Some(command))
                }
                Ok(Err(_)) => Ok(None),
                Err(_) => {
                    self.handle_waiting_timeout().await?;
                    Ok(None)
                }
            };
        }

        if self.is_empty_playing_lobby() {
            return match tokio::time::timeout(
                self.time_until_empty_playing_timeout(),
                rx.recv_async(),
            )
            .await
            {
                Ok(Ok(command)) => {
                    telemetry::dec_actor_queue_depth(self.game_type);
                    Ok(Some(command))
                }
                Ok(Err(_)) => Ok(None),
                Err(_) => {
                    self.handle_empty_playing_timeout().await?;
                    Ok(None)
                }
            };
        }

        match rx.recv_async().await {
            Ok(command) => {
                telemetry::dec_actor_queue_depth(self.game_type);
                Ok(Some(command))
            }
            Err(_) => Ok(None),
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
                shutting_down,
            } => match self
                .handle_disconnect_player(player_id, outbound_tx, shutting_down)
                .await
            {
                Ok(should_continue) => return should_continue,
                Err(e) => tracing::error!("Error handling player disconnect: {e}"),
            },
            MatchActorMessage::CreateMatch {
                creator_id,
                settings,
                respond,
            } => {
                respond_once(
                    respond,
                    self.handle_create_match(creator_id, settings).await,
                );
            }
            MatchActorMessage::JoinLobby { player_id, respond } => {
                respond_once(respond, self.handle_join_lobby(player_id).await);
            }
            MatchActorMessage::StatusChange {
                player_id,
                ready,
                respond,
            } => {
                let result = self.handle_status_change(player_id, ready).await;
                let should_continue = !matches!(&result, Err(ManagerError::Database(_)));
                respond_once(respond, result);
                return should_continue;
            }
            MatchActorMessage::SelectMercenary {
                player_id,
                mercenary_id,
                respond,
            } => {
                let result = self.handle_select_mercenary(player_id, mercenary_id).await;
                let should_continue = !matches!(&result, Err(ManagerError::Database(_)));
                respond_once(respond, result);
                return should_continue;
            }
            MatchActorMessage::GameCommand {
                player_id,
                command,
                respond,
            } => {
                let result = self.handle_game_command(player_id, command).await;
                let should_continue = match &result {
                    Ok(ActorResult::Continue) => true,
                    Ok(ActorResult::Stop) => false,
                    Err(ManagerError::Database(_)) => false,
                    Err(_) => true,
                };
                respond_once(respond, result.map(|_| ()));
                return should_continue;
            }
            MatchActorMessage::GetLobbySummary { respond } => {
                respond_once(respond, self.handle_get_lobby_summary());
            }
        }

        true
    }

    async fn handle_create_match(
        &mut self,
        creator_id: PlayerId,
        settings: GameSettings,
    ) -> Result<(), ManagerError> {
        if self.lobby.is_some() {
            return Ok(());
        }

        self.repo
            .create_metadata(&self.match_id, settings.clone(), Some(&creator_id))
            .await?;
        self.lobby = Some(Lobby::new(settings));
        self.creator_id = Some(creator_id);
        self.refresh_waiting_activity();

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

            lobby.get_snapshot(&player_id)
        };

        self.connections.insert(player_id.clone(), outbound_tx);

        self.empty_playing_since = None;
        if self.is_playing_lobby() {
            if let Err(e) = self.repo.touch_metadata(&self.match_id).await {
                tracing::error!("Error touching playing match metadata on reconnect: {e}");
            }
        } else {
            self.touch_lobby_activity().await?;
        }

        self.send_to_player(&player_id, OutboundMessage::Snapshot(snapshot))
            .await;

        Ok(())
    }

    async fn handle_join_lobby(
        &mut self,
        player_id: PlayerId,
    ) -> Result<LobbyInfoInternal, ManagerError> {
        if let Some(lobby) = self.lobby.as_ref() {
            if lobby.players.contains_key(&player_id) {
                let info = lobby.get_info(&player_id);
                self.player_routes
                    .insert(player_id.clone(), self.match_id.clone());
                self.touch_lobby_activity().await?;
                return Ok(info);
            }

            match &lobby.state {
                LobbyState::NotStarted(settings) => {
                    if lobby.players.len() == settings.max_players() {
                        return Err(LobbyError::GameError(GameError::TooManyPlayers).into());
                    }
                }
                LobbyState::Playing(_) => return Err(LobbyError::GameAlreadyStarted.into()),
            }
        } else {
            return Err(LobbyError::InvalidLobby.into());
        }

        self.repo
            .add_metadata_player(&self.match_id, &player_id)
            .await?;
        self.apply_player_joined(player_id.clone())?;
        self.refresh_waiting_activity();
        self.broadcast(OutboundMessage::PlayerJoined(player_id.clone()))
            .await;

        Ok(self.lobby()?.get_info(&player_id))
    }

    async fn handle_disconnect_player(
        &mut self,
        player_id: PlayerId,
        outbound_tx: PlayerSender,
        shutting_down: bool,
    ) -> Result<bool, ManagerError> {
        let is_current_connection = self
            .connections
            .get(&player_id)
            .is_some_and(|current| current.same_channel(&outbound_tx));

        if !is_current_connection {
            return Ok(true);
        }

        self.connections.remove(&player_id);

        let should_handle_waiting_disconnect = matches!(
            self.lobby.as_ref().map(|lobby| &lobby.state),
            Some(LobbyState::NotStarted(_))
        );

        if !should_handle_waiting_disconnect {
            self.refresh_empty_playing_activity();
            if self.is_empty_playing_lobby()
                && let Err(e) = self.repo.touch_metadata(&self.match_id).await
            {
                tracing::error!("Error touching empty playing match metadata: {e}");
            }

            return Ok(true);
        }

        if !self.lobby()?.players.contains_key(&player_id) {
            return Ok(true);
        }

        if shutting_down {
            if self.connections.is_empty() {
                self.stop_match();
                return Ok(false);
            }
            return Ok(true);
        }

        if self.connections.is_empty() {
            self.touch_lobby_activity().await?;
            return Ok(true);
        }

        self.repo
            .remove_metadata_player(&self.match_id, &player_id)
            .await?;
        self.apply_player_left(&player_id)?;
        self.broadcast(OutboundMessage::PlayerLeft {
            player_id: player_id.clone(),
        })
        .await;

        if self.lobby()?.players.is_empty() {
            self.repo.delete_metadata(&self.match_id).await?;
            self.stop_match();

            return Ok(false);
        }

        self.refresh_waiting_activity();

        Ok(true)
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

            if ready
                && matches!(
                    lobby.state,
                    LobbyState::NotStarted(GameSettings::FodinhaPower(_))
                )
                && lobby
                    .players
                    .get(&player_id)
                    .and_then(|status| status.mercenary_id.as_ref())
                    .is_none()
            {
                return Err(LobbyError::MercenaryRequired.into());
            }
        }

        self.repo
            .set_metadata_player_ready(&self.match_id, &player_id, ready)
            .await?;
        self.apply_player_status_changed(&player_id, ready)?;
        self.refresh_waiting_activity();
        let msg = OutboundMessage::PlayerStatusChange { player_id, ready };
        self.broadcast(msg).await;

        if let Some((players, settings)) = self.start_game_data()? {
            let power_card_registry = self.power_card_registry.snapshot();
            let event = Game::start_match_event(&players, settings, &power_card_registry)
                .map_err(|e| ManagerError::Lobby(LobbyError::GameError(e)))?;
            let applied = match self.persist_apply(event).await {
                Ok(applied) => applied,
                Err(e) => {
                    if matches!(e, ManagerError::Database(_)) {
                        tracing::error!(
                            "Database error persisting event for match {:?}, stopping actor: {e}",
                            self.match_id
                        );
                        self.stop_match();
                    }
                    return Err(e);
                }
            };

            if let AppliedEvent::GameStarted {
                set,
                lifes,
                power_decks,
                mana,
                next,
                possible_bids,
            } = applied
            {
                self.refresh_empty_playing_activity();
                self.broadcast_snapshots().await;
                self.init_set(
                    set.decks,
                    lifes,
                    power_decks,
                    mana,
                    set.upcard,
                    next,
                    possible_bids,
                )
                .await;
            }
        }

        Ok(())
    }

    async fn handle_select_mercenary(
        &mut self,
        player_id: PlayerId,
        mercenary_id: MercenaryId,
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

        self.repo
            .set_metadata_player_mercenary(&self.match_id, &player_id, &mercenary_id)
            .await?;
        self.apply_player_mercenary_selected(&player_id, mercenary_id.clone())?;
        self.refresh_waiting_activity();
        self.broadcast(OutboundMessage::PlayerMercenarySelected {
            player_id,
            mercenary_id,
        })
        .await;

        Ok(())
    }

    async fn handle_game_command(
        &mut self,
        player_id: PlayerId,
        command: crate::models::game::GameCommand,
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

            game.validate_command(&player_id, command)?
        };

        let applied = self.persist_apply(event).await?;

        match applied {
            AppliedEvent::Game(AppliedGameChange::BidPlaced {
                player_id,
                bid,
                state,
                mana,
            }) => {
                self.broadcast_bid(player_id, bid, state, mana).await;
                Ok(ActorResult::Continue)
            }
            AppliedEvent::Game(AppliedGameChange::TurnPlayed(turn)) => {
                let ended = self.broadcast_turn(turn).await;

                if ended {
                    Ok(ActorResult::Stop)
                } else {
                    Ok(ActorResult::Continue)
                }
            }
            AppliedEvent::Game(AppliedGameChange::PowerCardPlayed(outcome)) => {
                let ended = self.broadcast_power_card(outcome).await;

                if ended {
                    Ok(ActorResult::Stop)
                } else {
                    Ok(ActorResult::Continue)
                }
            }
            AppliedEvent::Game(AppliedGameChange::PowerPhaseSkipped(outcome)) => {
                let ended = self.broadcast_power_phase_skip(outcome).await;

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
            id: self.match_id.clone(),
            game_type: lobby.game_type(),
            player_count: lobby.players.len(),
        }))
    }

    pub(crate) fn replay_event(&mut self, event: MatchEvent) -> Result<(), ManagerError> {
        self.apply_event(event).map(|_| ())
    }

    pub(crate) fn restore_from_metadata(&mut self, metadata: MatchMetadataDto) {
        self.creator_id = metadata.creator_id();
        let updated_at = metadata.updated_at;

        let ready_players: std::collections::HashSet<_> =
            metadata.ready_players.into_iter().collect();
        let mut lobby = Lobby::new(metadata.settings);

        for player_id in metadata.players {
            let id = PlayerId(player_id.into());
            let mercenary_id = metadata
                .player_mercenaries
                .get(id.as_str())
                .map(|mercenary_id| MercenaryId(mercenary_id.as_str().into()));

            lobby.players.insert(
                id.clone(),
                LobbyPlayerStatus {
                    ready: ready_players.contains(id.as_str()),
                    mercenary_id,
                },
            );
            self.player_routes.insert(id, self.match_id.clone());
        }

        self.lobby = Some(lobby);
        self.restore_waiting_activity(updated_at);
    }

    fn apply_event(&mut self, event: MatchEvent) -> Result<AppliedEvent, ManagerError> {
        match event {
            MatchEvent::MatchCreated { settings } => {
                if self.lobby.is_none() {
                    self.lobby = Some(Lobby::new(settings));
                }

                Ok(AppliedEvent::None)
            }
            MatchEvent::PlayerJoined { user_claims } => {
                self.apply_player_joined(user_claims.id())?;

                Ok(AppliedEvent::PlayerJoined)
            }
            MatchEvent::PlayerStatusChanged { player_id, ready } => {
                self.apply_player_status_changed(&player_id, ready)?;

                Ok(AppliedEvent::PlayerStatusChanged)
            }
            MatchEvent::Game(GameEvent::FodinhaClassic(
                fodinha_classic::MatchEvent::GameStarted { settings, set },
            )) => {
                let lobby = self.lobby_mut()?;
                let players = lobby.get_players_id();
                let game = Game::FodinhaClassic(
                    fodinha_classic::Game::from_started(&players, settings, set.clone())
                        .map_err(|e| ManagerError::Lobby(LobbyError::GameError(e)))?,
                );

                lobby.state = LobbyState::Playing(game);

                let game = match &lobby.state {
                    LobbyState::Playing(game) => game,
                    LobbyState::NotStarted(_) => unreachable!("game was just started"),
                };

                Ok(AppliedEvent::GameStarted {
                    set,
                    lifes: None,
                    power_decks: None,
                    mana: None,
                    next: game.get_bidding_player(),
                    possible_bids: game.get_possible_bids(),
                })
            }
            MatchEvent::Game(GameEvent::FodinhaPower(fodinha_power::MatchEvent::GameStarted {
                settings,
                mut set,
                power_set,
                draw_seed,
                passive_effects,
            })) => {
                let power_card_registry = self.power_card_registry.snapshot();
                let lobby = self.lobby_mut()?;
                let players = lobby.get_players_id();
                let mut power_game = fodinha_power::Game::from_started(
                    &players,
                    settings,
                    set.clone(),
                    power_set.clone(),
                    draw_seed,
                    power_card_registry,
                )
                .map_err(|e| ManagerError::Lobby(LobbyError::GameError(e)))?;
                let (passive_mana, passive_power_decks) =
                    power_game.apply_start_effects(&passive_effects);
                let game = Game::FodinhaPower(power_game);
                let mut power_decks = power_decks_to_dto(&power_set.decks);
                let mut mana = power_mana_to_dto(&power_set.mana);
                let lifes =
                    (!passive_effects.lifes.is_empty()).then(|| passive_effects.lifes.clone());

                for (player_id, deck) in &passive_effects.decks {
                    set.decks.insert(player_id.clone(), deck.clone());
                }

                mana.extend(passive_mana);
                power_decks.extend(passive_power_decks);

                lobby.state = LobbyState::Playing(game);

                let game = match &lobby.state {
                    LobbyState::Playing(game) => game,
                    LobbyState::NotStarted(_) => unreachable!("game was just started"),
                };

                Ok(AppliedEvent::GameStarted {
                    set,
                    lifes,
                    power_decks: Some(power_decks),
                    mana: Some(mana),
                    next: game.get_bidding_player(),
                    possible_bids: game.get_possible_bids(),
                })
            }
            MatchEvent::Game(event) => {
                let lobby = self.lobby_mut()?;
                let game = match &mut lobby.state {
                    LobbyState::NotStarted(_) => return Err(LobbyError::GameNotStarted.into()),
                    LobbyState::Playing(game) => game,
                };

                Ok(AppliedEvent::Game(game.apply_match_event(event).map_err(
                    |e| ManagerError::Lobby(LobbyError::GameError(e)),
                )?))
            }
        }
    }

    async fn persist_apply(&mut self, event: MatchEvent) -> Result<AppliedEvent, ManagerError> {
        self.repo
            .append_event(&self.match_id, self.version, event.clone())
            .await?;
        self.version += 1;

        let applied = self.apply_event(event.clone())?;

        let repo = self.repo.clone();
        let match_id = self.match_id.clone();
        let finished = self.is_finished();

        if should_project_match_metadata(&event, finished) {
            self.deferred_tasks.spawn(async move {
                if let Err(e) = project_match_metadata(&repo, &match_id, &event, finished).await {
                    tracing::error!("Error projecting match metadata: {e}");
                }
            });
        }

        Ok(applied)
    }

    async fn broadcast_bid(
        &self,
        player_id: PlayerId,
        bid: usize,
        state: BiddingState,
        mana: HashMap<PlayerId, PlayerManaDto>,
    ) {
        let msg = OutboundMessage::PlayerBidded {
            player_id: player_id.clone(),
            bid,
        };
        self.broadcast(msg).await;

        if !mana.is_empty() {
            self.broadcast(OutboundMessage::PlayersManaChanged(mana))
                .await;
        }

        if self.is_finished()
            && let Some(lifes) = self.current_lifes()
        {
            self.broadcast(OutboundMessage::GameEnded { lifes }).await;
            return;
        }

        let msg = match state {
            BiddingState::Active {
                possible_bids,
                next,
            } => OutboundMessage::PlayerBiddingTurn {
                player_id: next,
                possible_bids,
            },
            BiddingState::Ended { next } => self
                .current_phase_message()
                .unwrap_or(OutboundMessage::PlayerTurn { player_id: next }),
        };

        self.broadcast(msg).await;
        self.refresh_power_hands().await;
    }

    async fn broadcast_turn(&self, turn: AppliedTurn) -> bool {
        let state = turn.state;
        let msg = OutboundMessage::TurnPlayed { pile: state.pile };
        self.broadcast(msg).await;

        match state.outcome {
            GameOutcome::SetEnded {
                lifes,
                upcard,
                decks,
                next,
                possible,
            } => {
                let msg = OutboundMessage::SetEnded { lifes };
                self.broadcast(msg).await;

                self.init_set(
                    decks,
                    turn.lifes,
                    turn.power_decks,
                    turn.mana,
                    upcard,
                    next,
                    possible,
                )
                .await;

                false
            }
            GameOutcome::SetPending { next: _ } => {
                if let Some(msg) = self.current_phase_message() {
                    self.broadcast(msg).await;
                }

                false
            }
            GameOutcome::RoundEnded { rounds, next } => {
                let msg = OutboundMessage::RoundEnded(rounds);
                self.broadcast(msg).await;

                let msg = OutboundMessage::PlayerTurn { player_id: next };
                self.broadcast(msg).await;

                false
            }
            GameOutcome::TurnPlayed { next } => {
                let msg = OutboundMessage::PlayerTurn { player_id: next };
                self.broadcast(msg).await;

                false
            }
            GameOutcome::Ended { lifes } => {
                let msg = OutboundMessage::GameEnded { lifes };
                self.broadcast(msg).await;

                true
            }
        }
    }

    async fn broadcast_power_card(&self, outcome: fodinha_power::PowerCardOutcome) -> bool {
        let lifes = outcome.lifes.clone();
        let next_set = outcome.next_set.clone();
        let decks = outcome.decks.clone();
        let power_decks = outcome.power_decks.clone();
        let mana = outcome.mana.clone();

        self.broadcast(OutboundMessage::PowerCardPlayed {
            player_id: outcome.player_id,
            card: outcome.card,
            target_player_id: outcome.target_player_id,
            lifes: outcome.lifes,
        })
        .await;

        if next_set.is_none() {
            if !mana.is_empty() {
                self.broadcast(OutboundMessage::PlayersManaChanged(mana.clone()))
                    .await;
            }

            for (player_id, deck) in decks.clone() {
                self.send_to_player(&player_id, OutboundMessage::PlayerDeck(deck))
                    .await;
            }

            for (player_id, deck) in power_decks.clone() {
                self.send_to_player(
                    &player_id,
                    OutboundMessage::PlayerPowerCards(
                        self.authoritative_power_cards(&player_id, deck),
                    ),
                )
                .await;
            }
        }

        if next_set.is_some() {
            self.broadcast(OutboundMessage::SetEnded {
                lifes: lifes.clone(),
            })
            .await;

            if let Some(OutboundMessage::PlayerBiddingTurn {
                player_id: next,
                possible_bids,
            }) = self.current_phase_message()
            {
                self.init_set(
                    decks.into_iter().collect::<IndexMap<_, _>>(),
                    Some(lifes),
                    Some(power_decks.into_iter().collect::<IndexMap<_, _>>()),
                    Some(mana),
                    next_set.expect("resolved next set is required").upcard,
                    next,
                    possible_bids,
                )
                .await;
            }
        } else if outcome.ended {
            self.broadcast(OutboundMessage::GameEnded { lifes }).await;
        } else if let Some(msg) = self.current_phase_message() {
            self.broadcast(msg).await;
            self.refresh_power_hands().await;
        }

        outcome.ended
    }

    async fn broadcast_power_phase_skip(
        &self,
        outcome: fodinha_power::PowerPhaseSkipOutcome,
    ) -> bool {
        let lifes = outcome.lifes.clone();
        let next_set = outcome.next_set.clone();
        let decks = outcome.decks.clone();
        let power_decks = outcome.power_decks.clone();
        let mana = outcome.mana.clone();

        if !outcome.changed_lifes.is_empty() {
            self.broadcast(OutboundMessage::PlayersLifesChanged(
                outcome.changed_lifes.clone(),
            ))
            .await;
        }

        if next_set.is_none() {
            if !mana.is_empty() {
                self.broadcast(OutboundMessage::PlayersManaChanged(mana.clone()))
                    .await;
            }

            for (player_id, deck) in decks.clone() {
                self.send_to_player(&player_id, OutboundMessage::PlayerDeck(deck))
                    .await;
            }

            for (player_id, deck) in power_decks.clone() {
                self.send_to_player(
                    &player_id,
                    OutboundMessage::PlayerPowerCards(
                        self.authoritative_power_cards(&player_id, deck),
                    ),
                )
                .await;
            }
        }

        if next_set.is_some() {
            self.broadcast(OutboundMessage::SetEnded {
                lifes: lifes.clone(),
            })
            .await;

            if let Some(OutboundMessage::PlayerBiddingTurn {
                player_id: next,
                possible_bids,
            }) = self.current_phase_message()
            {
                self.init_set(
                    decks.into_iter().collect::<IndexMap<_, _>>(),
                    Some(lifes),
                    Some(power_decks.into_iter().collect::<IndexMap<_, _>>()),
                    Some(mana),
                    next_set.expect("resolved next set is required").upcard,
                    next,
                    possible_bids,
                )
                .await;
            }
        } else if outcome.ended {
            self.broadcast(OutboundMessage::GameEnded { lifes }).await;
        } else if let Some(msg) = self.current_phase_message() {
            self.broadcast(msg).await;
            self.refresh_power_hands().await;
        }

        outcome.ended
    }

    async fn init_set(
        &self,
        decks: IndexMap<PlayerId, Vec<Card>>,
        lifes: Option<HashMap<PlayerId, usize>>,
        power_decks: Option<IndexMap<PlayerId, Vec<PowerCardDto>>>,
        mana: Option<HashMap<PlayerId, PlayerManaDto>>,
        upcard: Card,
        next: PlayerId,
        possible_bids: Vec<usize>,
    ) {
        self.broadcast(OutboundMessage::SetStart { upcard }).await;

        if let Some(lifes) = lifes
            && !lifes.is_empty()
        {
            self.broadcast(OutboundMessage::PlayersLifesChanged(lifes))
                .await;
        }

        if let Some(mana) = mana
            && !mana.is_empty()
        {
            self.broadcast(OutboundMessage::PlayersManaChanged(mana))
                .await;
        }

        for (player, deck) in decks {
            self.send_to_player(&player, OutboundMessage::PlayerDeck(deck))
                .await;
        }

        if let Some(power_decks) = power_decks {
            for (player, deck) in power_decks {
                self.send_to_player(
                    &player,
                    OutboundMessage::PlayerPowerCards(
                        self.authoritative_power_cards(&player, deck),
                    ),
                )
                .await;
            }
        }

        if self.is_finished()
            && let Some(lifes) = self.current_lifes()
        {
            self.broadcast(OutboundMessage::GameEnded { lifes }).await;
            return;
        }

        self.broadcast(OutboundMessage::PlayerBiddingTurn {
            player_id: next,
            possible_bids,
        })
        .await;
    }

    fn current_phase_message(&self) -> Option<OutboundMessage> {
        let lobby = self.lobby.as_ref()?;
        let LobbyState::Playing(game) = &lobby.state else {
            return None;
        };
        let player_id = game.current_player()?;

        match game.get_stage_dto() {
            crate::services::GameStageDto::Bidding { possible_bids } => {
                Some(OutboundMessage::PlayerBiddingTurn {
                    player_id,
                    possible_bids,
                })
            }
            crate::services::GameStageDto::Power { phase } => {
                Some(OutboundMessage::PlayerPowerTurn { player_id, phase })
            }
            crate::services::GameStageDto::Dealing => {
                Some(OutboundMessage::PlayerTurn { player_id })
            }
        }
    }

    fn current_lifes(&self) -> Option<HashMap<PlayerId, usize>> {
        let lobby = self.lobby.as_ref()?;
        let LobbyState::Playing(game) = &lobby.state else {
            return None;
        };

        Some(game.get_lifes())
    }

    async fn send_to_player(&self, player_id: &PlayerId, msg: OutboundMessage) {
        let Some(sender) = self.connections.get(player_id).cloned() else {
            tracing::debug!("No active connection for player {player_id:?}");
            return;
        };

        if let Err(e) = sender.send(msg).await {
            tracing::error!("Error enqueueing message to {player_id:?}: {e}");
        }
    }

    async fn broadcast(&self, msg: OutboundMessage) {
        let connections: Vec<_> = self
            .connections
            .iter()
            .map(|(player_id, sender)| (player_id.clone(), sender.clone()))
            .collect();

        for (player_id, sender) in connections {
            if let Err(e) = sender.send(msg.clone()).await {
                tracing::error!("Error enqueueing message to {player_id:?}: {e}");
            }
        }
    }

    async fn broadcast_snapshots(&self) {
        let Some(lobby) = self.lobby.as_ref() else {
            return;
        };

        let player_ids: Vec<_> = self.connections.keys().cloned().collect();

        for player_id in player_ids {
            let snapshot = lobby.get_snapshot(&player_id);

            self.send_to_player(&player_id, OutboundMessage::Snapshot(snapshot))
                .await;
        }
    }

    async fn refresh_power_hands(&self) {
        let Some(lobby) = self.lobby.as_ref() else {
            return;
        };
        let LobbyState::Playing(game) = &lobby.state else {
            return;
        };
        for player_id in self.connections.keys() {
            if let Some(cards) = game.get_game_info(player_id).power_cards {
                self.send_to_player(player_id, OutboundMessage::PlayerPowerCards(cards))
                    .await;
            }
        }
    }

    async fn close_connections(&self, code: u16, reason: &str) {
        self.broadcast(OutboundMessage::Close {
            code,
            reason: reason.to_string(),
        })
        .await;
    }

    fn stop_match(&mut self) {
        self.match_entries.remove(&self.match_id);

        if let Some(lobby) = self.lobby.as_ref() {
            for player_id in lobby.players.keys() {
                self.player_routes.remove(player_id);
            }
        }

        self.connections.clear();
    }

    async fn handle_waiting_timeout(&mut self) -> Result<(), ManagerError> {
        if !self.is_waiting_lobby() {
            return Ok(());
        }

        self.close_connections(
            WAITING_LOBBY_INACTIVITY_CLOSE_CODE,
            WAITING_LOBBY_INACTIVITY_CLOSE_REASON,
        )
        .await;
        self.repo.delete_metadata(&self.match_id).await?;
        self.stop_match();

        Ok(())
    }

    async fn handle_empty_playing_timeout(&mut self) -> Result<(), ManagerError> {
        if !self.is_empty_playing_lobby() {
            return Ok(());
        }

        self.repo.mark_metadata_abandoned(&self.match_id).await?;
        self.stop_match();

        Ok(())
    }

    fn is_waiting_lobby(&self) -> bool {
        matches!(
            self.lobby.as_ref().map(|lobby| &lobby.state),
            Some(LobbyState::NotStarted(_))
        )
    }

    fn time_until_waiting_timeout(&self) -> Duration {
        self.waiting_lobby_timeout
            .saturating_sub(self.last_activity.elapsed())
    }

    fn is_empty_playing_lobby(&self) -> bool {
        self.is_playing_lobby() && self.connections.is_empty() && self.empty_playing_since.is_some()
    }

    fn is_playing_lobby(&self) -> bool {
        matches!(
            self.lobby.as_ref().map(|lobby| &lobby.state),
            Some(LobbyState::Playing(_))
        )
    }

    fn time_until_empty_playing_timeout(&self) -> Duration {
        let Some(empty_since) = self.empty_playing_since else {
            return self.empty_playing_timeout;
        };

        self.empty_playing_timeout
            .saturating_sub(empty_since.elapsed())
    }

    fn refresh_waiting_activity(&mut self) {
        if self.is_waiting_lobby() {
            self.last_activity = Instant::now();
        }
    }

    fn refresh_empty_playing_activity(&mut self) {
        if self.is_playing_lobby() && self.connections.is_empty() {
            self.empty_playing_since.get_or_insert_with(Instant::now);
        } else {
            self.empty_playing_since = None;
        }
    }

    async fn touch_lobby_activity(&mut self) -> Result<(), ManagerError> {
        if !self.is_waiting_lobby() {
            return Ok(());
        }

        self.repo.touch_metadata(&self.match_id).await?;
        self.refresh_waiting_activity();

        Ok(())
    }

    fn restore_waiting_activity(&mut self, updated_at: i64) {
        if !self.is_waiting_lobby() {
            return;
        }

        let idle_seconds = (chrono::Utc::now().timestamp() - updated_at).max(0) as u64;
        let idle = Duration::from_secs(idle_seconds).min(self.waiting_lobby_timeout);

        self.last_activity = Instant::now() - idle;
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

        let mut settings = settings.clone();

        if let GameSettings::FodinhaPower(power_settings) = &mut settings {
            if lobby
                .players
                .values()
                .any(|status| status.mercenary_id.is_none())
            {
                return Err(LobbyError::MercenaryRequired.into());
            }

            power_settings.player_mercenaries = lobby
                .players
                .iter()
                .filter_map(|(player_id, status)| {
                    status
                        .mercenary_id
                        .clone()
                        .map(|mercenary_id| (player_id.clone(), mercenary_id))
                })
                .collect();
        }

        Ok(Some((lobby.get_players_id(), settings)))
    }

    pub(crate) fn is_finished(&self) -> bool {
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

    fn authoritative_power_cards(
        &self,
        player_id: &PlayerId,
        fallback: Vec<PowerCardDto>,
    ) -> Vec<PowerCardDto> {
        self.lobby
            .as_ref()
            .and_then(|lobby| match &lobby.state {
                LobbyState::Playing(game) => game.get_game_info(player_id).power_cards,
                LobbyState::NotStarted(_) => None,
            })
            .unwrap_or(fallback)
    }

    fn lobby_mut(&mut self) -> Result<&mut Lobby, ManagerError> {
        self.lobby
            .as_mut()
            .ok_or_else(|| LobbyError::InvalidLobby.into())
    }

    fn apply_player_joined(&mut self, player_id: PlayerId) -> Result<(), ManagerError> {
        let lobby = self.lobby_mut()?;

        if lobby.players.contains_key(&player_id) {
            self.player_routes
                .insert(player_id.clone(), self.match_id.clone());
            return Ok(());
        }

        lobby.players.insert(
            player_id.clone(),
            LobbyPlayerStatus {
                ready: false,
                mercenary_id: None,
            },
        );

        self.player_routes.insert(player_id, self.match_id.clone());

        Ok(())
    }

    fn apply_player_status_changed(
        &mut self,
        player_id: &PlayerId,
        ready: bool,
    ) -> Result<(), ManagerError> {
        let lobby = self.lobby_mut()?;
        let player = lobby
            .players
            .get_mut(player_id)
            .ok_or(LobbyError::WrongLobby)?;

        player.ready = ready;

        Ok(())
    }

    fn apply_player_mercenary_selected(
        &mut self,
        player_id: &PlayerId,
        mercenary_id: MercenaryId,
    ) -> Result<(), ManagerError> {
        let lobby = self.lobby_mut()?;
        let player = lobby
            .players
            .get_mut(player_id)
            .ok_or(LobbyError::WrongLobby)?;

        player.mercenary_id = Some(mercenary_id);

        Ok(())
    }

    fn apply_player_left(&mut self, player_id: &PlayerId) -> Result<(), ManagerError> {
        let lobby = self.lobby_mut()?;

        lobby.players.shift_remove(player_id);
        self.player_routes.remove(player_id);

        Ok(())
    }
}

fn should_project_match_metadata(event: &MatchEvent, match_finished: bool) -> bool {
    match event {
        MatchEvent::MatchCreated { .. }
        | MatchEvent::PlayerJoined { .. }
        | MatchEvent::Game(GameEvent::FodinhaClassic(fodinha_classic::MatchEvent::GameStarted {
            ..
        }))
        | MatchEvent::Game(GameEvent::FodinhaPower(fodinha_power::MatchEvent::GameStarted {
            ..
        })) => true,
        MatchEvent::Game(GameEvent::FodinhaClassic(fodinha_classic::MatchEvent::TurnPlayed {
            ..
        }))
        | MatchEvent::Game(GameEvent::FodinhaPower(fodinha_power::MatchEvent::TurnPlayed {
            ..
        }))
        | MatchEvent::Game(GameEvent::FodinhaPower(fodinha_power::MatchEvent::PowerCardPlayed {
            ..
        }))
        | MatchEvent::Game(GameEvent::FodinhaPower(
            fodinha_power::MatchEvent::PowerPhaseSkipped { .. },
        )) => match_finished,
        MatchEvent::Game(GameEvent::FodinhaClassic(fodinha_classic::MatchEvent::BidPlaced {
            ..
        }))
        | MatchEvent::Game(GameEvent::FodinhaPower(fodinha_power::MatchEvent::BidPlaced {
            ..
        }))
        | MatchEvent::PlayerStatusChanged { .. } => false,
    }
}

fn power_decks_to_dto(
    decks: &IndexMap<PlayerId, Vec<fodinha_power::PowerCard>>,
) -> IndexMap<PlayerId, Vec<PowerCardDto>> {
    decks
        .iter()
        .map(|(player_id, deck)| {
            (
                player_id.clone(),
                deck.iter().map(fodinha_power::PowerCard::to_dto).collect(),
            )
        })
        .collect()
}

fn power_mana_to_dto(
    mana: &IndexMap<PlayerId, fodinha_power::PlayerMana>,
) -> HashMap<PlayerId, PlayerManaDto> {
    mana.iter()
        .map(|(player_id, mana)| {
            (
                player_id.clone(),
                PlayerManaDto {
                    current: mana.current,
                    max: mana.max,
                },
            )
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum ActorResult {
    Continue,
    Stop,
}

fn respond_once<T>(
    respond: tokio::sync::oneshot::Sender<Result<T, ManagerError>>,
    result: Result<T, ManagerError>,
) {
    let _ = respond.send(result);
}
