use axum::{
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt, stream::SplitSink};

use crate::{
    infra::UserClaims,
    models::id::PlayerId,
    services::{ManagerError, dispatcher::ManagerHandle},
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
    let (sender, mut receiver) = ws.split();

    manager.store_player_connection(auth.id(), sender).await?;

    while let Some(Ok(message)) = receiver.next().await {
        match process_msg(message, manager.clone(), auth.id()).await {
            Ok(_) => {}
            Err(error) => {
                let id = auth.id();
                tracing::error!("{id:?} Error: {error}");
                manager.send_error(&id, error).await;
                break;
            }
        }
    }

    Ok(())
}

async fn process_msg(
    msg: Message,
    manager: ManagerHandle,
    player_id: PlayerId,
) -> Result<(), ManagerError> {
    match msg {
        Message::Text(msg) => {
            let msg = serde_json::from_str(&msg)?;
            tracing::debug!("Received from {player_id:?}: {msg:?}");

            handle_game_msg(msg, manager, player_id).await
        }
        Message::Close(c) => {
            let reason = c
                .map(|c| format!("code: {} | {}", c.code, c.reason))
                .unwrap_or("empty".to_string());

            tracing::warn!("{player_id:?} sent close message, reason: {}", reason);

            Err(ManagerError::PlayerDisconnected(reason))
        }
        _ => Err(ManagerError::InvalidWebsocketMessageType),
    }
}

pub async fn send_disconnect(&self, player_id: &PlayerId, reason: ManagerError) {
    let mut manager = self.inner.connections.lock().await;

    let connection = match manager.get_mut(player_id) {
        Some(c) => c,
        None => {
            tracing::error!("{player_id:?} disconnected");
            return;
        }
    };

    let code = match reason {
        ManagerError::PlayerDisconnected(_) => 1001,
        ManagerError::InvalidWebsocketMessageType => 1003,
        ManagerError::Lobby(_) => 1008,
        ManagerError::Deal(_) | ManagerError::Bid(_) => 1008,
        ManagerError::UnexpectedMessage(_) => 1008,
        ManagerError::Database(_) => 1011,
        ManagerError::Unauthorized(_) => 3000,
        ManagerError::ReceiverDisposed => 1011,
    };

    let send_close = connection
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_string().into(),
        })))
        .await;

    if let Err(e) = send_close {
        tracing::error!("Failed to send close message: {e}")
    }
}

async fn handle_game_msg(
    msg: GameMessage,
    manager: ManagerHandle,
    player_id: PlayerId,
) -> Result<(), ManagerError> {
    let result = match msg {
        GameMessage::PlayTurn { card } => manager.play_turn(card, player_id).await,
        GameMessage::PutBid { bid } => manager.bid(bid, player_id).await,
        GameMessage::PlayerStatusChange { ready } => {
            manager.player_status_change(player_id, ready).await
        }
    };

    // TODO all these messages should be broadcasted cause every client needs to know them
    // maybe take a look at the `old` setup of sending the message here
    // and then send only specifics messages inside the manager (but is prob not worth the hassle)

    Ok(result?)
}

type Connection = SplitSink<WebSocket, Message>;

async fn send_msg(msg: &ServerMessage, player: &PlayerId, connection: &mut Connection) {
    let msg = serde_json::to_string(msg).expect("Should be valid json");

    tracing::info!("Sending to {player:?}: {msg}");

    let send = connection
        .send(Message::Text(msg.into()))
        .await
        .map_err(|e| ManagerError::PlayerDisconnected(e.to_string()));

    if let Err(e) = send {
        tracing::error!("Error sending msg to: {player:?} | {e}");
    }
}
