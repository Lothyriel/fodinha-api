use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};

use crate::models::{
    commands::{MatchSnapshot, ServerMessage},
    id::PlayerId,
};

pub const WS_TIMEOUT: Duration = Duration::from_secs(5);

pub type WebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Clone)]
pub struct WsClient {
    pub ws_url: String,
}

impl WsClient {
    pub fn new(ws_url: String) -> Self {
        Self { ws_url }
    }

    fn ws_url_path(&self, path: &str) -> String {
        format!("{}{}", self.ws_url, path)
    }

    pub async fn connect(&self, token: &str) -> WebSocket {
        let req = self
            .ws_url_path(&format!("/game?token={token}"))
            .into_client_request()
            .unwrap();

        let (stream, _) = tokio::time::timeout(WS_TIMEOUT, tokio_tungstenite::connect_async(req))
            .await
            .expect("Timed out connecting websocket")
            .expect("Failed to connect WebSocket");

        stream
    }

    pub async fn send_msg<T: serde::Serialize>(stream: &mut WebSocket, msg: T) {
        let msg = serde_json::to_string(&msg).unwrap();
        stream.send(Message::Text(msg.into())).await.unwrap();
    }

    pub async fn recv_msg(stream: &mut WebSocket) -> ServerMessage {
        let msg = tokio::time::timeout(WS_TIMEOUT, stream.next())
            .await
            .expect("Timed out waiting for websocket message")
            .expect("Expected websocket message")
            .expect("Expected valid websocket message");

        match msg {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            m => panic!("Unexpected message: {m}"),
        }
    }

    pub async fn get_snapshot(stream: &mut WebSocket) -> MatchSnapshot {
        match Self::assert_msg(stream, |m| matches!(m, ServerMessage::Snapshot(_))).await {
            ServerMessage::Snapshot(info) => info,
            _ => panic!("Should be a Snapshot message"),
        }
    }

    pub async fn assert_msg<F>(stream: &mut WebSocket, predicate: F) -> ServerMessage
    where
        F: FnOnce(&ServerMessage) -> bool,
    {
        let msg = Self::recv_msg(stream).await;

        match predicate(&msg) {
            true => msg,
            false => panic!("Message not expected {msg:?}"),
        }
    }

    pub async fn get_next_turn_player(stream: &mut WebSocket) -> PlayerId {
        match Self::assert_msg(stream, validate_player_turn).await {
            ServerMessage::PlayerTurn { player_id } => player_id,
            _ => panic!("Should be a PlayerTurn message"),
        }
    }

    pub async fn get_next_bidding_player(stream: &mut WebSocket) -> PlayerId {
        match Self::assert_msg(stream, validate_bidding_turn).await {
            ServerMessage::PlayerBiddingTurn {
                player_id,
                possible_bids: _,
            } => player_id,
            _ => panic!("Should be a PlayerBiddingTurn message"),
        }
    }

    pub async fn get_deck(stream: &mut WebSocket) -> Vec<crate::models::Card> {
        match Self::assert_msg(stream, |m| matches!(m, ServerMessage::PlayerDeck(_))).await {
            ServerMessage::PlayerDeck(c) => c,
            _ => panic!("Should be a PlayerDeck message"),
        }
    }

    pub async fn assert_ws_closes_without_snapshot(&self, token: &str) {
        let req = self
            .ws_url_path(&format!("/game?token={token}"))
            .into_client_request()
            .unwrap();

        let result =
            tokio::time::timeout(WS_TIMEOUT, tokio_tungstenite::connect_async(req)).await;

        let Ok(Ok((mut stream, _))) = result else {
            return;
        };

        match tokio::time::timeout(WS_TIMEOUT, stream.next()).await {
            Ok(None | Some(Err(_)) | Some(Ok(Message::Close(_)))) => {}
            Ok(Some(Ok(msg))) => {
                panic!("Expected websocket to close without snapshot, got {msg:?}")
            }
            Err(_) => {}
        }
    }

    pub async fn close(stream: &mut WebSocket) {
        stream.close(None).await.unwrap();
    }
}

pub fn validate_player_turn(m: &ServerMessage) -> bool {
    matches!(m, ServerMessage::PlayerTurn { player_id: _ })
}

pub fn validate_bidding_turn(m: &ServerMessage) -> bool {
    matches!(
        m,
        ServerMessage::PlayerBiddingTurn {
            player_id: _,
            possible_bids: _
        }
    )
}

pub fn validate_player_bidded(m: &ServerMessage) -> bool {
    matches!(
        m,
        ServerMessage::PlayerBidded {
            player_id: _,
            bid: _
        }
    )
}

pub fn validate_player_status_change(m: &ServerMessage) -> bool {
    matches!(
        m,
        ServerMessage::PlayerStatusChange {
            player_id: _,
            ready: _
        }
    )
}

pub fn validate_set_start(m: &ServerMessage) -> bool {
    matches!(m, ServerMessage::SetStart { upcard: _ })
}

pub fn validate_round_ended(m: &ServerMessage) -> bool {
    matches!(m, ServerMessage::RoundEnded(_))
}

pub fn validate_turn_played(m: &ServerMessage) -> bool {
    matches!(m, ServerMessage::TurnPlayed { pile: _ })
}
