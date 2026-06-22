mod auth;
mod game;
mod lobby;
mod models;

use std::net::Ipv6Addr;

use axum::{Json, Router, response::IntoResponse, routing};
use reqwest::StatusCode;
use tower_http::cors::{Any, CorsLayer};

use crate::{
    AppSettings,
    models::GameError,
    services::{LobbyError, ManagerError, dispatcher::ManagerHandle},
};

pub async fn start(manager: ManagerHandle, settings: &AppSettings) {
    auth::JWT_KEY
        .set(settings.jwt_key.to_string())
        .expect("Should set jwt key value");

    let auth = axum::middleware::from_fn(auth::middleware);

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/game", routing::get(game::handler))
        .nest("/lobby", lobby::router().layer(auth))
        .nest("/auth", auth::router())
        .fallback(fallback_handler)
        .with_state(manager)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors);

    let address = (Ipv6Addr::UNSPECIFIED, 3000);

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("Expected to bind to network address");

    tracing::info!("Listening on {:?}", address);

    axum::serve(listener, app)
        .await
        .expect("Expected to start axum");
}

async fn fallback_handler() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "this resource doesn't exist")
}

impl IntoResponse for LobbyError {
    fn into_response(self) -> axum::response::Response {
        let code = match &self {
            LobbyError::InvalidLobby => StatusCode::NOT_FOUND,
            LobbyError::GameAlreadyStarted => StatusCode::CONFLICT,
            LobbyError::GameNotStarted => StatusCode::PRECONDITION_FAILED,
            LobbyError::WrongLobby => StatusCode::FORBIDDEN,
            LobbyError::PlayerNotInLobby => StatusCode::FORBIDDEN,
            LobbyError::GameError(e) => match e {
                GameError::NotEnoughPlayers => StatusCode::CONFLICT,
                GameError::TooManyPlayers => StatusCode::CONFLICT,
                GameError::InvalidDeal(_) => StatusCode::UNPROCESSABLE_ENTITY,
                GameError::InvalidBid(_) => StatusCode::UNPROCESSABLE_ENTITY,
                GameError::InvalidStage => StatusCode::UNPROCESSABLE_ENTITY,
            },
        };

        (code, Json(serde_json::json!({"error": self.to_string()}))).into_response()
    }
}

impl IntoResponse for ManagerError {
    fn into_response(self) -> axum::response::Response {
        let code = match self {
            ManagerError::PlayerDisconnected(_) => StatusCode::GONE,
            ManagerError::Deal(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ManagerError::Bid(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ManagerError::InvalidWebsocketMessageType => StatusCode::BAD_REQUEST,
            ManagerError::UnexpectedMessage(_) => StatusCode::BAD_REQUEST,
            ManagerError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ManagerError::ReceiverDisposed => StatusCode::from_u16(499).expect("valid http code"),
            ManagerError::Unauthorized(e) => return e.into_response(),
            ManagerError::Lobby(e) => return e.into_response(),
        };

        (code, Json(serde_json::json!({"error": self.to_string()}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use futures::{SinkExt, StreamExt, stream::FusedStream};

    use reqwest::Client;
    use tokio::{net::TcpStream, task};
    use tokio_tungstenite::{
        MaybeTlsStream, WebSocketStream, connect_async,
        tungstenite::{Message, client::IntoClientRequest},
    };

    use crate::{
        AppSettings,
        models::{
            Card,
            commands::{ClientCommand, CreateLobbyResponse, LobbyInfo, ServerMessage},
            id::{LobbyId, PlayerId},
        },
        services::manager::GameManager,
    };

    use super::{auth::get_claims_from_token, models::*, start};

    const URL: &str = "http://localhost:3000";

    type WebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

    type Deck = Vec<Card>;

    struct TestPlayerData {
        connection: WebSocket,
        deck: Deck,
    }

    type TestPlayersData = HashMap<PlayerId, TestPlayerData>;

    #[tokio::test]
    async fn test_example() {
        task::spawn(async {
            let settings = AppSettings {
                jwt_key: "very-random-secret-key".to_string(),
                mongo_conn_string: "mongodb://localhost/?retryWrites=true".to_string(),
                ..Default::default()
            };

            let handle = GameManager::new().start(&settings).await;

            start(handle, &settings).await
        });

        let client = reqwest::Client::new();

        let tokens = get_players(&client, 7).await;

        let mut player_data = join_lobby(&client, tokens).await;

        ready(&mut player_data).await;

        'game: loop {
            get_decks(&mut player_data).await;

            play_set(&mut player_data).await;

            for p in player_data.values_mut() {
                if assert_game_or_set_ended(&mut p.connection).await {
                    break 'game;
                }
            }
        }
    }

    async fn get_players(client: &Client, count: usize) -> Vec<String> {
        let mut players = vec![];

        for i in 0..count {
            let player = login(client, i).await;
            players.push(player);
        }

        players
    }

    async fn assert_game_or_set_ended(socket: &mut WebSocket) -> bool {
        match recv_msg(socket).await {
            ServerMessage::SetEnded { lifes } => {
                println!("Asserted game msg {:?}", ServerMessage::SetEnded { lifes });
                false
            }
            ServerMessage::GameEnded { lifes } => {
                println!("Asserted game msg {:?}", ServerMessage::GameEnded { lifes });
                true
            }
            msg => panic!("Expected Set or Game end | {msg:?}"),
        }
    }

    async fn play_set(players: &mut TestPlayersData) {
        let rounds_count = players.values().next().unwrap().deck.len();

        bidding(players, rounds_count).await;

        for i in 0..rounds_count {
            play_round(players, i == rounds_count - 1).await;
        }
    }

    async fn play_round(players: &mut TestPlayersData, last: bool) {
        for _ in 0..players.len() {
            play_turn(players).await;
        }

        if !last {
            for p in players.values_mut() {
                assert_game_msg(&mut p.connection, validate_round_ended).await;
            }
        }
    }

    async fn play_turn(players: &mut TestPlayersData) {
        let first_connection = players.values_mut().next().unwrap();

        let next = get_next_turn_player(&mut first_connection.connection).await;

        for p in players.values_mut().skip(1) {
            get_next_turn_player(&mut p.connection).await;
        }

        let next = players.get_mut(&next).unwrap();

        let msg = ClientCommand::PlayTurn {
            card: next.deck.pop().unwrap(),
        };

        send_msg(&mut next.connection, msg).await;

        for p in players.values_mut() {
            assert_game_msg(&mut p.connection, validate_turn_played).await;
        }
    }

    async fn bidding(players: &mut TestPlayersData, bid: usize) {
        for _ in 0..players.len() {
            bid_turn(players, bid).await;
        }
    }

    async fn bid_turn(players: &mut TestPlayersData, bid: usize) {
        let data = players.values_mut().next().unwrap();

        let next = get_next_bidding_player(&mut data.connection).await;

        for p in players.values_mut().skip(1) {
            get_next_bidding_player(&mut p.connection).await;
        }

        let next = players.get_mut(&next).unwrap();

        send_msg(&mut next.connection, ClientCommand::PutBid { bid }).await;

        for p in players.values_mut() {
            assert_game_msg(&mut p.connection, validate_player_bidded).await;
        }
    }

    async fn get_decks(players: &mut TestPlayersData) {
        for p in players.values_mut() {
            assert_game_msg(&mut p.connection, validate_set_start).await;
        }

        for p in players.values_mut() {
            p.deck = get_deck(&mut p.connection).await;
        }
    }

    async fn join_lobby(client: &Client, tokens: Vec<String>) -> TestPlayersData {
        let lobby_id = create_lobby(client, &tokens[0]).await;

        for (i, p) in tokens.iter().enumerate() {
            let lobby = join_lobby_http(client, p, &lobby_id).await;
            assert!(matches!(lobby, LobbyInfo::NotStarted(players) if players.len() == i + 1));
        }

        let mut connections = HashMap::new();

        for p in tokens {
            let claims = get_claims_from_token(&p).await.unwrap();

            let data = TestPlayerData {
                connection: connect_ws(&p).await,
                deck: Vec::new(),
            };

            connections.insert(claims.id(), data);
        }

        connections
    }

    async fn ready(players: &mut TestPlayersData) {
        let msg = ClientCommand::PlayerStatusChange { ready: true };

        for p in players.values_mut() {
            send_msg(&mut p.connection, msg).await;
        }

        for _ in 0..players.len() {
            for p in players.values_mut() {
                assert_game_msg(&mut p.connection, validate_player_status_change).await;
            }
        }
    }

    fn validate_round_ended(m: &ServerMessage) -> bool {
        matches!(m, ServerMessage::RoundEnded(_))
    }

    fn validate_turn_played(m: &ServerMessage) -> bool {
        matches!(m, ServerMessage::TurnPlayed { pile: _ })
    }

    fn validate_player_turn(m: &ServerMessage) -> bool {
        matches!(m, ServerMessage::PlayerTurn { player_id: _ })
    }

    fn validate_bidding_turn(m: &ServerMessage) -> bool {
        matches!(
            m,
            ServerMessage::PlayerBiddingTurn {
                player_id: _,
                possible_bids: _
            }
        )
    }

    fn validate_player_bidded(m: &ServerMessage) -> bool {
        matches!(
            m,
            ServerMessage::PlayerBidded {
                player_id: _,
                bid: _
            }
        )
    }

    fn validate_player_status_change(m: &ServerMessage) -> bool {
        matches!(
            m,
            ServerMessage::PlayerStatusChange {
                player_id: _,
                ready: _
            }
        )
    }

    fn validate_set_start(m: &ServerMessage) -> bool {
        matches!(m, ServerMessage::SetStart { upcard: _ })
    }

    async fn get_next_turn_player(stream: &mut WebSocket) -> PlayerId {
        match assert_game_msg(stream, validate_player_turn).await {
            ServerMessage::PlayerTurn { player_id } => player_id,
            _ => panic!("Should be a PlayerTurn message"),
        }
    }

    async fn get_next_bidding_player(stream: &mut WebSocket) -> PlayerId {
        match assert_game_msg(stream, validate_bidding_turn).await {
            ServerMessage::PlayerBiddingTurn {
                player_id,
                possible_bids: _,
            } => player_id,
            _ => panic!("Should be a PlayerTurn message"),
        }
    }

    async fn get_deck(stream: &mut WebSocket) -> Deck {
        match assert_game_msg(stream, |m| matches!(m, ServerMessage::PlayerDeck(_))).await {
            ServerMessage::PlayerDeck(c) => c,
            _ => panic!("Should be a PlayerDeck message"),
        }
    }

    async fn assert_game_msg<F>(stream: &mut WebSocket, predicate: F) -> ServerMessage
    where
        F: FnOnce(&ServerMessage) -> bool,
    {
        let msg = recv_msg(stream).await;

        match predicate(&msg) {
            true => {
                println!("Asserted game msg {msg:?}");
                msg
            }
            false => panic!("Message not expected {msg:?}"),
        }
    }

    async fn send_msg<T: serde::Serialize>(stream: &mut WebSocket, msg: T) {
        let msg = serde_json::to_string(&msg).unwrap();

        stream.send(Message::Text(msg.into())).await.unwrap();
    }

    async fn connect_ws(token: &str) -> WebSocket {
        let req = format!("ws://localhost:3000/game?token={token}")
            .into_client_request()
            .unwrap();

        let (stream, _) = connect_async(req)
            .await
            .expect("Failed to connect WebSocket");

        assert!(!stream.is_terminated());

        stream
    }

    async fn recv_msg(stream: &mut WebSocket) -> ServerMessage {
        let msg = stream.next().await.unwrap().unwrap();

        match msg {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            m => panic!("Error: {m}"),
        }
    }

    async fn join_lobby_http(client: &Client, token: &str, lobby_id: &LobbyId) -> LobbyInfo {
        let lobby_id = lobby_id.as_str();

        let res = client
            .put(format!("{URL}/lobby/{lobby_id}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();

        res.json().await.unwrap()
    }

    async fn create_lobby(client: &Client, token: &str) -> LobbyId {
        let res = client
            .post(format!("{URL}/lobby"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();

        let res: CreateLobbyResponse = res.json().await.unwrap();

        res.lobby_id
    }

    async fn login(client: &Client, number: usize) -> String {
        let params = serde_json::json!({
            "nickname": format!("Player {number}"),
        });

        let res = client
            .post(format!("{URL}/auth/signup"))
            .json(&params)
            .send()
            .await
            .unwrap();

        let res: Auth = res.json().await.unwrap();

        res.token
    }
}
