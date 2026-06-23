use std::collections::HashMap;

use indexmap::IndexMap;

use crate::{
    models::{
        Card, Game, GameError, GameOutcome, LobbyState, Turn,
        commands::GetLobbyDto,
        game::{AppliedGameChange, BiddingState, GameSettings, MatchEvent, NewSet},
        id::{MatchId, PlayerId},
        lobby::{Lobby, LobbyInfoInternal, LobbyPlayerStatus},
    },
    services::{
        LobbyError, ManagerError,
        matches::{
            MatchActorMessage, MatchEntries, MatchReceiver, OutboundMessage, PlayerRoutes,
            PlayerSender, project_match_metadata,
        },
        repositories::matches::{MatchMetadataDto, MatchesRepository},
        stats::StatsProjectorHandle,
    },
};

pub(crate) struct MatchActor {
    match_id: MatchId,
    lobby: Option<Lobby>,
    connections: HashMap<PlayerId, PlayerSender>,
    pub(crate) version: usize,
    repo: MatchesRepository,
    stats_projector: StatsProjectorHandle,
    match_entries: MatchEntries,
    player_routes: PlayerRoutes,
}

enum AppliedEvent {
    None,
    PlayerJoined,
    PlayerStatusChanged,
    GameStarted {
        set: NewSet,
        next: PlayerId,
        possible_bids: Vec<usize>,
    },
    Game(AppliedGameChange),
}

impl MatchActor {
    pub(crate) fn new(
        match_id: MatchId,
        repo: MatchesRepository,
        stats_projector: StatsProjectorHandle,
        match_entries: MatchEntries,
        player_routes: PlayerRoutes,
    ) -> Self {
        Self {
            match_id,
            lobby: None,
            connections: HashMap::new(),
            version: 0,
            repo,
            stats_projector,
            match_entries,
            player_routes,
        }
    }

    pub(crate) async fn run(mut self, rx: MatchReceiver) {
        while let Ok(command) = rx.recv_async().await {
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
            } => match self.handle_disconnect_player(player_id, outbound_tx).await {
                Ok(should_continue) => return should_continue,
                Err(e) => tracing::error!("Error handling player disconnect: {e}"),
            },
            MatchActorMessage::CreateMatch { settings, respond } => {
                respond_once(respond, self.handle_create_match(settings).await);
            }
            MatchActorMessage::JoinLobby { player_id, respond } => {
                respond_once(respond, self.handle_join_lobby(player_id).await);
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

    async fn handle_create_match(&mut self, settings: GameSettings) -> Result<(), ManagerError> {
        if self.lobby.is_some() {
            return Ok(());
        }

        self.repo
            .create_metadata(&self.match_id, settings.clone())
            .await?;
        self.lobby = Some(Lobby::new(settings));

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
                self.player_routes
                    .insert(player_id.clone(), self.match_id.clone());
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

        self.repo
            .add_metadata_player(&self.match_id, &player_id)
            .await?;
        self.apply_player_joined(player_id.clone())?;
        self.broadcast(OutboundMessage::PlayerJoined(player_id.clone()))
            .await;

        Ok(self.lobby()?.get_info(&player_id))
    }

    async fn handle_disconnect_player(
        &mut self,
        player_id: PlayerId,
        outbound_tx: PlayerSender,
    ) -> Result<bool, ManagerError> {
        let is_current_connection = self
            .connections
            .get(&player_id)
            .is_some_and(|current| current.same_channel(&outbound_tx));

        if !is_current_connection {
            return Ok(true);
        }

        self.connections.remove(&player_id);

        let Some(lobby) = self.lobby.as_ref() else {
            return Ok(true);
        };

        if matches!(lobby.state, LobbyState::Playing(_)) {
            return Ok(true);
        }

        if !lobby.players.contains_key(&player_id) {
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
        }

        self.repo
            .set_metadata_player_ready(&self.match_id, &player_id, ready)
            .await?;
        self.apply_player_status_changed(&player_id, ready)?;
        let msg = OutboundMessage::PlayerStatusChange { player_id, ready };
        self.broadcast(msg).await;

        if let Some((players, settings)) = self.start_game_data()? {
            let event = Game::start_match_event(&players, settings)
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
        command: crate::models::commands::GameCommand,
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
                crate::models::commands::GameCommand::PlayTurn { card } => game
                    .validate_turn(Turn {
                        player_id: player_id.clone(),
                        card,
                    })
                    .map_err(ManagerError::Deal)?,
                crate::models::commands::GameCommand::PutBid { bid } => game
                    .validate_bid(&player_id, bid)
                    .map_err(ManagerError::Bid)?,
            }
        };

        let applied = self.persist_apply(event).await?;

        match applied {
            AppliedEvent::Game(AppliedGameChange::BidPlaced {
                player_id,
                bid,
                state,
            }) => {
                self.broadcast_bid(player_id, bid, state).await;
                Ok(ActorResult::Continue)
            }
            AppliedEvent::Game(AppliedGameChange::TurnPlayed(state)) => {
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
            id: self.match_id.clone(),
            player_count: lobby.players.len(),
        }))
    }

    pub(crate) fn replay_event(&mut self, event: MatchEvent) -> Result<(), ManagerError> {
        self.apply_event(event).map(|_| ())
    }

    pub(crate) fn restore_from_metadata(&mut self, metadata: MatchMetadataDto) {
        let ready_players: std::collections::HashSet<_> =
            metadata.ready_players.into_iter().collect();
        let mut lobby = Lobby::new(metadata.settings.unwrap_or_default());

        for player_id in metadata.players {
            let id = PlayerId(player_id.into());

            lobby.players.insert(
                id.clone(),
                LobbyPlayerStatus {
                    ready: ready_players.contains(id.as_str()),
                },
            );
            self.player_routes.insert(id, self.match_id.clone());
        }

        self.lobby = Some(lobby);
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
            MatchEvent::GameStarted { settings, set } => {
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
            event @ (MatchEvent::BidPlaced { .. } | MatchEvent::TurnPlayed { .. }) => {
                let lobby = self.lobby_mut()?;
                let game = match &mut lobby.state {
                    LobbyState::NotStarted(_) => return Err(LobbyError::GameNotStarted.into()),
                    LobbyState::Playing(game) => game,
                };

                Ok(AppliedEvent::Game(game.apply_match_event(event)))
            }
        }
    }

    async fn persist_apply(&mut self, event: MatchEvent) -> Result<AppliedEvent, ManagerError> {
        self.repo
            .append_event(&self.match_id, self.version, event.clone())
            .await?;
        self.version += 1;

        let applied = self.apply_event(event.clone())?;

        if let Err(e) =
            project_match_metadata(&self.repo, &self.match_id, &event, self.is_finished()).await
        {
            tracing::error!("Error projecting match metadata: {e}");
        }

        Ok(applied)
    }

    async fn broadcast_bid(&self, player_id: PlayerId, bid: usize, state: BiddingState) {
        let msg = OutboundMessage::PlayerBidded {
            player_id: player_id.clone(),
            bid,
        };
        self.broadcast(msg).await;

        let msg = match state {
            BiddingState::Active {
                possible_bids,
                next,
            } => OutboundMessage::PlayerBiddingTurn {
                player_id: next,
                possible_bids,
            },
            BiddingState::Ended { next } => OutboundMessage::PlayerTurn { player_id: next },
        };

        self.broadcast(msg).await;
    }

    async fn broadcast_turn(&self, state: crate::models::game::DealState) -> bool {
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

                self.init_set(decks, upcard, next, possible).await;

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

    async fn init_set(
        &self,
        decks: IndexMap<PlayerId, Vec<Card>>,
        upcard: Card,
        next: PlayerId,
        possible_bids: Vec<usize>,
    ) {
        self.broadcast(OutboundMessage::SetStart { upcard }).await;

        for (player, deck) in decks {
            self.send_to_player(&player, OutboundMessage::PlayerDeck(deck))
                .await;
        }

        self.broadcast(OutboundMessage::PlayerBiddingTurn {
            player_id: next,
            possible_bids,
        })
        .await;
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

    fn stop_match(&mut self) {
        self.match_entries.remove(&self.match_id);

        if let Some(lobby) = self.lobby.as_ref() {
            for player_id in lobby.players.keys() {
                self.player_routes.remove(player_id);
            }
        }

        self.connections.clear();
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

        lobby
            .players
            .insert(player_id.clone(), LobbyPlayerStatus { ready: false });

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

    fn apply_player_left(&mut self, player_id: &PlayerId) -> Result<(), ManagerError> {
        let lobby = self.lobby_mut()?;

        lobby.players.shift_remove(player_id);
        self.player_routes.remove(player_id);

        Ok(())
    }
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
