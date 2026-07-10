use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};

use fodinha_core::models::{
    commands::{MatchSnapshot, ServerMessage},
    id::PlayerId,
};

pub const WS_TIMEOUT: Duration = Duration::from_secs(30);
pub const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub type WebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Clone)]
pub struct ClientError(pub String);

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ClientError {}

impl From<String> for ClientError {
    fn from(s: String) -> Self {
        ClientError(s)
    }
}

impl From<&str> for ClientError {
    fn from(s: &str) -> Self {
        ClientError(s.to_string())
    }
}

macro_rules! err {
    ($($arg:tt)*) => {
        ClientError(format!($($arg)*))
    };
}

pub(crate) use err;

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

    pub async fn connect(&self, token: &str) -> Result<WebSocket, ClientError> {
        let req = self
            .ws_url_path(&format!("/game?token={token}"))
            .into_client_request()
            .map_err(|e| err!("Invalid WS request: {e}"))?;

        match tokio::time::timeout(WS_CONNECT_TIMEOUT, tokio_tungstenite::connect_async(req)).await
        {
            Ok(Ok((stream, _))) => Ok(stream),
            Ok(Err(e)) => Err(err!("Failed to connect WebSocket: {e}")),
            Err(_) => Err(err!("Timed out connecting WebSocket")),
        }
    }

    pub async fn send_msg<T: serde::Serialize>(
        stream: &mut WebSocket,
        msg: T,
    ) -> Result<(), ClientError> {
        let msg = serde_json::to_string(&msg).map_err(|e| err!("JSON serialization: {e}"))?;
        stream
            .send(Message::Text(msg.into()))
            .await
            .map_err(|e| err!("WS send failed: {e}"))
    }

    pub async fn recv_msg(stream: &mut WebSocket) -> Result<ServerMessage, ClientError> {
        let msg = tokio::time::timeout(WS_TIMEOUT, stream.next())
            .await
            .map_err(|_| err!("Timed out waiting for WS message"))?
            .ok_or_else(|| err!("WebSocket closed"))
            .and_then(|r| r.map_err(|e| err!("WebSocket error: {e}")))?;

        match msg {
            Message::Text(t) => {
                let parsed: ServerMessage =
                    serde_json::from_str(&t).map_err(|e| err!("JSON deserialization: {e}"))?;

                if let ServerMessage::Error { msg } = &parsed {
                    return Err(err!("Server error: {msg}"));
                }

                Ok(parsed)
            }
            Message::Close(_) => Err(err!("WebSocket closed by server")),
            m => Err(err!("Unexpected WS message type: {m}")),
        }
    }

    pub async fn get_snapshot(stream: &mut WebSocket) -> Result<MatchSnapshot, ClientError> {
        match Self::assert_msg(stream, |m| matches!(m, ServerMessage::Snapshot(_))).await? {
            ServerMessage::Snapshot(info) => Ok(info),
            _ => Err(err!("Should be a Snapshot message")),
        }
    }

    pub async fn assert_msg<F>(
        stream: &mut WebSocket,
        predicate: F,
    ) -> Result<ServerMessage, ClientError>
    where
        F: FnOnce(&ServerMessage) -> bool,
    {
        let msg = Self::recv_msg(stream).await?;

        if predicate(&msg) {
            Ok(msg)
        } else {
            Err(err!("Unexpected message: {msg:?}"))
        }
    }

    pub async fn get_next_turn_player(stream: &mut WebSocket) -> Result<PlayerId, ClientError> {
        match Self::assert_msg(stream, validate_player_turn).await? {
            ServerMessage::PlayerTurn { player_id } => Ok(player_id),
            _ => Err(err!("Should be a PlayerTurn message")),
        }
    }

    pub async fn get_next_bidding_player(stream: &mut WebSocket) -> Result<PlayerId, ClientError> {
        match Self::assert_msg(stream, validate_bidding_turn).await? {
            ServerMessage::PlayerBiddingTurn {
                player_id,
                possible_bids: _,
            } => Ok(player_id),
            _ => Err(err!("Should be a PlayerBiddingTurn message")),
        }
    }

    pub async fn get_deck(
        stream: &mut WebSocket,
    ) -> Result<Vec<fodinha_core::models::Card>, ClientError> {
        match Self::assert_msg(stream, |m| matches!(m, ServerMessage::PlayerDeck(_))).await? {
            ServerMessage::PlayerDeck(c) => Ok(c),
            _ => Err(err!("Should be a PlayerDeck message")),
        }
    }

    pub async fn close(stream: &mut WebSocket) -> Result<(), ClientError> {
        stream
            .close(None)
            .await
            .map_err(|e| err!("WS close failed: {e}"))
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
