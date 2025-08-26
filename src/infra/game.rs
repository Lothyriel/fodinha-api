use std::net::SocketAddr;

use axum::{
    Extension,
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use futures::StreamExt;

use crate::{
    infra::ClientMessage,
    services::manager::{Manager, ManagerError, PlayerId},
};

use super::{ClientGameMessage, auth::UserClaims};

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(who): ConnectInfo<SocketAddr>,
    State(manager): State<Manager>,
    Extension(auth): Extension<UserClaims>,
) -> impl IntoResponse {
    tracing::info!(">>>> {who} connected");

    ws.on_upgrade(move |socket| async move {
        match handle_connection(socket, manager, auth).await {
            Ok(_) => tracing::warn!(">>>> {who} closed normally"),
            Err(e) => tracing::error!(">>>> {who} closed from error: {e}"),
        }
    })
}

async fn handle_connection(
    socket: WebSocket,
    manager: Manager,
    auth: UserClaims,
) -> Result<(), ManagerError> {
    let (sender, mut receiver) = socket.split();

    manager.store_player_connection(auth.id(), sender).await?;

    tokio::spawn(async move {
        while let Some(Ok(message)) = receiver.next().await {
            match process_msg(message, manager.clone(), auth.id()).await {
                Ok(_) => {}
                Err(error) => {
                    let id = auth.id();
                    tracing::error!("{id} Error: {error}");
                    manager.send_error(&id, error).await;
                    break;
                }
            }
        }
    });

    Ok(())
}

async fn process_msg(
    msg: Message,
    manager: Manager,
    player_id: PlayerId,
) -> Result<(), ManagerError> {
    match msg {
        Message::Text(msg) => {
            let msg = serde_json::from_str(&msg)?;
            tracing::debug!("Received from {player_id}: {msg:?}");

            match msg {
                ClientMessage::Game(g) => handle_game_msg(g, manager, player_id).await,
                ClientMessage::Auth { token: a } => {
                    tracing::error!("Unexpected auth message {a}");
                    Err(ManagerError::UnexpectedValidMessage(
                        "Expected game message",
                    ))
                }
            }
        }
        Message::Close(c) => {
            let reason = c
                .map(|c| format!("code: {} | {}", c.code, c.reason))
                .unwrap_or("empty".to_string());

            tracing::warn!("{player_id} sent close message, reason: {}", reason);

            Err(ManagerError::PlayerDisconnected(reason))
        }
        _ => Err(ManagerError::InvalidWebsocketMessageType),
    }
}

async fn handle_game_msg(
    msg: ClientGameMessage,
    manager: Manager,
    player_id: PlayerId,
) -> Result<(), ManagerError> {
    let result = match msg {
        ClientGameMessage::PlayTurn { card } => manager.play_turn(card, player_id).await,
        ClientGameMessage::PutBid { bid } => manager.bid(bid, player_id).await,
        ClientGameMessage::Reconnect => manager.reconnect(player_id).await,
        ClientGameMessage::PlayerStatusChange { ready } => {
            manager.player_status_change(player_id, ready).await
        }
    };

    // TODO all these messages should be broadcasted cause every client needs to know them
    // maybe take a look at the `old` setup of sending the message here
    // and then send only specifics messages inside the manager (but is prob not worth the hassle)

    Ok(result?)
}
