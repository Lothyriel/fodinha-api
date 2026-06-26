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
        matches::{ManagerHandle, PlayerReceiver, PlayerSender},
    },
};

use super::{ApiState, auth::get_claims_from_token, models::*};

pub async fn handler(
    ws: WebSocketUpgrade,
    State(state): State<ApiState>,
    Query(query): Query<Auth>,
) -> axum::response::Response {
    let started = Instant::now();

    let claims = get_claims_from_token(&query.token, &state.jwt_key, &state.google_client_id).await;

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
            telemetry::inc_active_ws_connections();
            match handle_connection(socket, manager, claims, shutdown_rx).await {
                Ok(_) => tracing::warn!(">>>> {who:?} closed normally"),
                Err(e) => tracing::error!(">>>> {who:?} closed from error: {e}"),
            }
            telemetry::dec_active_ws_connections();
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

    manager.upsert_user(&auth).await?;

    let context = manager.connect_player(player_id.clone()).await?;

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

    connection.run().await
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

                    let msg = self.manager.hydrate_outbound_message(msg).await?;
                    self.send_server_msg(msg).await?;
                }
                inbound = self.socket.next() => {
                    match inbound {
                        Some(Ok(message)) => {
                            if let Err(error) = self.process_msg(message).await {
                                self.send_error(&error).await;
                                return Err(error);
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

    async fn send_error(&mut self, error: &ManagerError) {
        let msg = ServerMessage::Error {
            msg: error.to_string(),
        };

        if let Err(error) = self.send_server_msg(msg).await {
            tracing::error!("Failed to send websocket error: {error}");
        }
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

async fn handle_client_command(
    manager: ManagerHandle,
    player_id: PlayerId,
    msg: ClientCommand,
) -> Result<(), ManagerError> {
    match msg {
        ClientCommand::PlayTurn { card } => manager.play_turn(card, player_id).await,
        ClientCommand::PutBid { bid } => manager.bid(bid, player_id).await,
        ClientCommand::PlayerStatusChange { ready } => {
            manager.player_status_change(player_id, ready).await
        }
    }
}
