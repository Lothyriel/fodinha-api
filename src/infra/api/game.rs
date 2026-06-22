use axum::{
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use futures::StreamExt;
use tokio::sync::oneshot;

use crate::{
    infra::UserClaims,
    models::{
        commands::{ClientCommand, GameCommand, ServerMessage},
        id::{LobbyId, PlayerId},
    },
    services::{
        ManagerError,
        dispatcher::{GameActorCommand, GameSender, ManagerHandle, PlayerReceiver},
    },
};

use super::{auth::get_claims_from_token, models::*};

pub async fn handler(
    ws: WebSocketUpgrade,
    State(manager): State<ManagerHandle>,
    Query(query): Query<Auth>,
) -> impl IntoResponse {
    let claims = match get_claims_from_token(&query.token).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let who = claims.id();

    tracing::info!(">>>> {who:?} connected");

    ws.on_upgrade(|socket| async move {
        match handle_connection(socket, manager, claims).await {
            Ok(_) => tracing::warn!(">>>> {who:?} closed normally"),
            Err(e) => tracing::error!(">>>> {who:?} closed from error: {e}"),
        }
    })
}

async fn handle_connection(
    ws: WebSocket,
    manager: ManagerHandle,
    auth: UserClaims,
) -> Result<(), ManagerError> {
    let player_id = auth.id();
    let context = manager.connect_player(player_id.clone()).await?;
    let connection = PlayerConnection {
        player_id,
        game_id: context.game_id,
        socket: ws,
        game_tx: context.game_tx,
        outbound_rx: context.outbound_rx,
    };

    connection.run().await
}

struct PlayerConnection {
    player_id: PlayerId,
    game_id: LobbyId,
    socket: WebSocket,
    game_tx: GameSender,
    outbound_rx: PlayerReceiver,
}

impl PlayerConnection {
    async fn run(mut self) -> Result<(), ManagerError> {
        tracing::debug!(
            "Starting websocket task for {:?} in {:?}",
            self.player_id,
            self.game_id
        );

        let result = self.run_loop().await;

        let _ = self
            .game_tx
            .send_async(GameActorCommand::DisconnectPlayer {
                player_id: self.player_id.clone(),
            })
            .await;

        result
    }

    async fn run_loop(&mut self) -> Result<(), ManagerError> {
        loop {
            tokio::select! {
                outbound = self.outbound_rx.recv() => {
                    let Some(msg) = outbound else {
                        return Ok(());
                    };

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
            }
        }
    }

    async fn process_msg(&mut self, msg: Message) -> Result<(), ManagerError> {
        match msg {
            Message::Text(msg) => {
                let msg = serde_json::from_str(&msg)?;
                tracing::debug!("Received from {:?}: {msg:?}", self.player_id);

                handle_game_msg(self.player_id.clone(), self.game_tx.clone(), msg).await
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

async fn handle_game_msg(
    player_id: PlayerId,
    game_tx: GameSender,
    msg: ClientCommand,
) -> Result<(), ManagerError> {
    match msg {
        ClientCommand::PlayTurn { card } => {
            request(&game_tx, |respond| GameActorCommand::GameCommand {
                player_id,
                command: GameCommand::PlayTurn { card },
                respond,
            })
            .await
        }
        ClientCommand::PutBid { bid } => {
            request(&game_tx, |respond| GameActorCommand::GameCommand {
                player_id,
                command: GameCommand::PutBid { bid },
                respond,
            })
            .await
        }
        ClientCommand::PlayerStatusChange { ready } => {
            request(&game_tx, |respond| GameActorCommand::StatusChange {
                player_id,
                ready,
                respond,
            })
            .await
        }
        ClientCommand::Reconnect => {
            request(&game_tx, |respond| GameActorCommand::Reconnect {
                player_id,
                respond,
            })
            .await
        }
    }
}

async fn request(
    game_tx: &GameSender,
    build: impl FnOnce(oneshot::Sender<Result<(), ManagerError>>) -> GameActorCommand,
) -> Result<(), ManagerError> {
    let (tx, rx) = oneshot::channel();

    game_tx
        .send_async(build(tx))
        .await
        .map_err(|_| ManagerError::ReceiverDisposed)?;

    rx.await.map_err(|_| ManagerError::ReceiverDisposed)?
}
