use std::time::Instant;

use axum::{
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    response::IntoResponse,
};
use futures::StreamExt;
use tokio::sync::watch;

use crate::{
    infra::{UserClaims, telemetry},
    models::{
        commands::{ClientCommand, ServerMessage},
        id::{MatchId, PlayerId},
    },
    services::{
        ManagerError,
        matches::{
            CommonOutboundMessage, ManagerHandle, OutboundMessage, OutboundPayload, PlayerReceiver,
            PlayerSender,
        },
    },
};

use super::{ApiState, auth::get_claims_from_token, models::*};

pub async fn handler(
    ws: WebSocketUpgrade,
    State(state): State<ApiState>,
    Query(query): Query<Auth>,
) -> axum::response::Response {
    let started = Instant::now();

    let claims = get_claims_from_token(
        &query.token,
        &state.jwt_key,
        state.google_client_id.as_deref(),
    )
    .await;

    let claims = match claims {
        Ok(c) => c,
        Err(e) => {
            let response = e.into_response();
            telemetry::record_http_endpoint("GET", "/game", response.status(), started.elapsed());
            return response;
        }
    };

    let manager = state.manager.clone();
    let shutdown_rx = state.shutdown_rx.clone();

    let who = claims.id();

    tracing::info!(">>>> {who:?} connected");

    let response = ws
        .on_upgrade(|socket| async move {
            match handle_connection(socket, manager, claims, shutdown_rx).await {
                Ok(_) => tracing::warn!(">>>> {who:?} closed normally"),
                Err(e) => tracing::error!(">>>> {who:?} closed from error: {e}"),
            }
        })
        .into_response();

    let status = response.status();

    telemetry::record_http_endpoint("GET", "/game", status, started.elapsed());

    response
}

async fn handle_connection(
    ws: WebSocket,
    manager: ManagerHandle,
    auth: UserClaims,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<(), ManagerError> {
    let player_id = auth.id();
    let context = manager.connect_player(player_id.clone()).await?;
    let game_type = context.game_type;

    telemetry::inc_active_ws_connections(game_type);

    let connection = PlayerConnection {
        player_id,
        match_id: context.match_id,
        socket: ws,
        manager,
        outbound_tx: context.outbound_tx,
        outbound_rx: context.outbound_rx,
        shutdown_rx,
        shutting_down: false,
    };

    let result = connection.run().await;
    telemetry::dec_active_ws_connections(game_type);

    result
}

struct PlayerConnection {
    player_id: PlayerId,
    match_id: MatchId,
    socket: WebSocket,
    manager: ManagerHandle,
    outbound_tx: PlayerSender,
    outbound_rx: PlayerReceiver,
    shutdown_rx: watch::Receiver<bool>,
    shutting_down: bool,
}

impl PlayerConnection {
    async fn run(mut self) -> Result<(), ManagerError> {
        tracing::debug!(
            "Starting websocket task for {:?} in {:?}",
            self.player_id,
            self.match_id
        );

        let result = self.run_loop().await;

        self.manager
            .disconnect_player(
                &self.match_id,
                self.player_id.clone(),
                self.outbound_tx.clone(),
                self.shutting_down,
            )
            .await;

        result
    }

    async fn run_loop(&mut self) -> Result<(), ManagerError> {
        if *self.shutdown_rx.borrow() {
            return Ok(());
        }

        loop {
            tokio::select! {
                outbound = self.outbound_rx.recv() => {
                    let Some(msg) = outbound else {
                        return Ok(());
                    };

                    match msg {
                        OutboundMessage::Public(OutboundPayload::Common(
                            CommonOutboundMessage::Close { code, reason },
                        ))
                        | OutboundMessage::SinglePlayer(OutboundPayload::Common(
                            CommonOutboundMessage::Close { code, reason },
                        )) => {
                            self.socket
                                .send(Message::Close(Some(CloseFrame {
                                    code,
                                    reason: reason.into(),
                                })))
                                .await
                                .map_err(|e| ManagerError::PlayerDisconnected(e.to_string()))?;
                            return Ok(());
                        }
                        msg => {
                            match self.manager.hydrate_outbound_message(msg).await {
                                Ok(msg) => self.send_server_msg(msg).await?,
                                Err(error) if is_terminal_connection_error(&error) => {
                                    return Err(error);
                                }
                                Err(error) => self.send_error(&error).await?,
                            }
                        }
                    }
                }
                inbound = self.socket.next() => {
                    match inbound {
                        Some(Ok(message)) => {
                            if let Err(error) = self.process_msg(message).await {
                                if is_terminal_connection_error(&error) {
                                    return Err(error);
                                }
                                self.send_error(&error).await?;
                            }
                        }
                        Some(Err(error)) => return Err(ManagerError::PlayerDisconnected(error.to_string())),
                        None => return Ok(()),
                    }
                }
                _ = self.shutdown_rx.changed() => {
                    if *self.shutdown_rx.borrow() {
                        self.shutting_down = true;

                        tracing::info!(
                            "Shutdown signal received, closing websocket for {:?}",
                            self.player_id
                        );
                        let _ = self.socket.send(Message::Close(Some(CloseFrame {
                            code: 1001,
                            reason: "server shutting down".into(),
                        }))).await;
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn process_msg(&mut self, msg: Message) -> Result<(), ManagerError> {
        match msg {
            Message::Text(msg) => {
                let msg = serde_json::from_str(&msg)?;
                tracing::debug!("Received from {:?}: {msg:?}", self.player_id);

                handle_client_command(self.manager.clone(), self.player_id.clone(), msg).await
            }
            Message::Close(c) => {
                let reason = c
                    .map(|c| format!("code: {} | {}", c.code, c.reason))
                    .unwrap_or("empty".to_string());

                tracing::warn!("{:?} sent close message, reason: {reason}", self.player_id);

                Err(ManagerError::PlayerDisconnected(reason))
            }
            _ => Err(ManagerError::InvalidWebsocketMessageType),
        }
    }

    async fn send_error(&mut self, error: &ManagerError) -> Result<(), ManagerError> {
        let msg = ServerMessage::Error {
            msg: error.to_string(),
        };

        self.send_server_msg(msg).await
    }

    async fn send_server_msg(&mut self, msg: ServerMessage) -> Result<(), ManagerError> {
        let msg = serde_json::to_string(&msg).expect("Should be valid json");

        tracing::info!("Sending to {:?}: {msg}", self.player_id);

        self.socket
            .send(Message::Text(msg.into()))
            .await
            .map_err(|e| ManagerError::PlayerDisconnected(e.to_string()))
    }
}

fn is_terminal_connection_error(error: &ManagerError) -> bool {
    matches!(error, ManagerError::PlayerDisconnected(_))
}

async fn handle_client_command(
    manager: ManagerHandle,
    player_id: PlayerId,
    msg: ClientCommand,
) -> Result<(), ManagerError> {
    match msg {
        ClientCommand::GameCommand(command) => manager.game_command(command, player_id).await,
        ClientCommand::PlayerStatusChange { ready } => {
            manager.player_status_change(player_id, ready).await
        }
        ClientCommand::SelectMercenary { mercenary_id } => {
            manager.select_mercenary(player_id, mercenary_id).await
        }
    }
}
