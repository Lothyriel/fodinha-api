mod auth;
mod game;
mod lobby;
mod models;

use std::net::Ipv6Addr;

use axum::{Json, Router, response::IntoResponse, routing};
use reqwest::StatusCode;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};

use crate::{
    AppSettings,
    models::GameError,
    services::{LobbyError, ManagerError, dispatcher::ManagerHandle},
};

#[derive(Clone)]
pub struct ApiState {
    manager: ManagerHandle,
    jwt_key: String,
}

pub async fn start(manager: ManagerHandle, settings: &AppSettings) {
    let address = (Ipv6Addr::UNSPECIFIED, 3000);

    let listener = TcpListener::bind(address)
        .await
        .expect("Expected to bind to network address");

    serve_listener(listener, manager, settings).await;
}

pub async fn serve_listener(listener: TcpListener, manager: ManagerHandle, settings: &AppSettings) {
    let address = listener
        .local_addr()
        .expect("Expected listener to expose local address");
    let app = build_app(manager, settings);

    tracing::info!("Listening on {:?}", address);

    axum::serve(listener, app)
        .await
        .expect("Expected to start axum");
}

fn build_app(manager: ManagerHandle, settings: &AppSettings) -> Router {
    let state = ApiState {
        manager,
        jwt_key: settings.jwt_key.clone(),
    };
    let auth = axum::middleware::from_fn_with_state(state.clone(), auth::middleware);

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/game", routing::get(game::handler))
        .nest("/lobby", lobby::router().layer(auth))
        .nest("/auth", auth::router(state.clone()))
        .fallback(fallback_handler)
        .with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors)
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
    use std::{collections::HashMap, net::Ipv4Addr};

    use futures::{SinkExt, StreamExt, stream::FusedStream};
    use mongodb::Database;

    use reqwest::Client;
    use tokio::{
        net::{TcpListener, TcpStream},
        task::JoinHandle,
        time::{Duration, sleep, timeout},
    };
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
        services::{
            dispatcher::ManagerHandle, manager::GameManager, repositories::get_mongo_client,
        },
    };

    use super::{auth::get_claims_from_token, models::*, serve_listener};

    type WebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

    const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
    const MONGO_CONN_STRING: &str = "mongodb://localhost/?retryWrites=true";
    const SERVER_START_TIMEOUT: Duration = Duration::from_millis(200);
    const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
    const TEST_JWT_KEY: &str = "very-random-secret-key";
    const WS_TIMEOUT: Duration = Duration::from_secs(5);

    type Deck = Vec<Card>;

    enum EndMessage {
        Set(HashMap<PlayerId, usize>),
        Game,
    }

    struct TestPlayerData {
        connection: WebSocket,
        deck: Deck,
    }

    type TestPlayersData = HashMap<PlayerId, TestPlayerData>;

    struct TestServer {
        base_url: String,
        ws_url: String,
        mongo_database: String,
        manager: ManagerHandle,
        database: Option<Database>,
        handle: Option<JoinHandle<()>>,
    }

    impl TestServer {
        async fn start() -> Self {
            let mongo_database = format!("oh_hell_test_{}", nanoid::nanoid!(10));

            Self::start_with_database(mongo_database).await
        }

        async fn start_with_database(mongo_database: String) -> Self {
            let mongo_conn_string = MONGO_CONN_STRING.to_string();
            let settings = AppSettings {
                jwt_key: TEST_JWT_KEY.to_string(),
                mongo_conn_string: mongo_conn_string.clone(),
                mongo_database: mongo_database.clone(),
                ..Default::default()
            };

            let client = get_mongo_client(&mongo_conn_string)
                .await
                .expect("Expected to create mongo client");
            let database = client.database(&mongo_database);
            let manager = GameManager::start(&settings).await;
            let server_manager = manager.clone();
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .expect("Expected to bind test API listener");
            let address = listener
                .local_addr()
                .expect("Expected test listener address");
            let base_url = format!("http://{address}");
            let ws_url = format!("ws://{address}");
            let handle = tokio::spawn(async move {
                serve_listener(listener, manager, &settings).await;
            });

            let server = Self {
                base_url,
                ws_url,
                mongo_database,
                manager: server_manager,
                database: Some(database),
                handle: Some(handle),
            };

            server.wait_until_ready().await;

            server
        }

        fn url(&self, path: &str) -> String {
            format!("{}{}", self.base_url, path)
        }

        fn websocket_url(&self, path: &str) -> String {
            format!("{}{}", self.ws_url, path)
        }

        async fn wait_until_ready(&self) {
            let client = Client::builder()
                .timeout(SERVER_START_TIMEOUT)
                .build()
                .expect("Expected to build test HTTP client");

            for _ in 0..50 {
                if client.get(&self.base_url).send().await.is_ok() {
                    return;
                }

                sleep(Duration::from_millis(50)).await;
            }

            panic!("API server did not start");
        }

        async fn wait_until_match_actor_stopped(&self) {
            timeout(WS_TIMEOUT, async {
                loop {
                    if self.manager.match_senders.is_empty()
                        && self.manager.active_player_route_count() == 0
                    {
                        return;
                    }

                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("Timed out waiting for match actor cleanup");
        }

        async fn stop_without_dropping_database(mut self) -> String {
            let mongo_database = self.mongo_database.clone();

            self.stop_server().await;

            self.database.take();

            mongo_database
        }

        async fn shutdown(mut self) {
            self.stop_server().await;

            if let Some(database) = self.database.take() {
                timeout(SHUTDOWN_TIMEOUT, database.drop())
                    .await
                    .expect("Timed out dropping test database")
                    .expect("Expected to drop test database");
            }
        }

        async fn stop_server(&mut self) {
            self.manager.match_senders.clear();

            if let Some(handle) = self.handle.take() {
                handle.abort();
                let _ = timeout(SHUTDOWN_TIMEOUT, handle).await;
            }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.manager.match_senders.clear();

            if let Some(handle) = self.handle.take() {
                handle.abort();
            }
        }
    }

    #[tokio::test]
    async fn test_example() {
        let server = TestServer::start().await;

        let client = http_client();

        let tokens = get_players(&client, &server, 7).await;
        let first_token = tokens[0].clone();

        let mut player_data = join_lobby(&client, &server, tokens).await;

        ready(&mut player_data).await;

        'game: loop {
            get_decks(&mut player_data).await;

            play_set(&mut player_data).await;

            let mut set_lifes = None;

            for p in player_data.values_mut() {
                match recv_game_or_set_ended(&mut p.connection).await {
                    EndMessage::Set(lifes) => set_lifes = Some(lifes),
                    EndMessage::Game => break 'game,
                }
            }

            if let Some(lifes) = set_lifes {
                player_data
                    .retain(|player_id, _| lifes.get(player_id).copied().unwrap_or_default() > 0);
            }
        }

        server.wait_until_match_actor_stopped().await;
        assert_ws_closes_without_snapshot(&server, &first_token).await;

        drop(player_data);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_reconnect_gets_snapshot() {
        let server = TestServer::start().await;

        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        for (i, token) in tokens.iter().enumerate() {
            let lobby = join_lobby_http(&client, &server, token, &lobby_id).await;
            assert!(matches!(lobby, LobbyInfo::NotStarted(players) if players.len() == i + 1));
        }

        let claims = get_claims_from_token(&tokens[0], TEST_JWT_KEY)
            .await
            .unwrap();
        let mut first_connection = connect_ws(&server, &tokens[0]).await;
        let snapshot = get_snapshot(&mut first_connection).await;

        assert!(
            matches!(snapshot, LobbyInfo::NotStarted(players) if players.contains_key(&claims.id()))
        );

        first_connection.close(None).await.unwrap();

        let mut reconnected = connect_ws(&server, &tokens[0]).await;
        let snapshot = get_snapshot(&mut reconnected).await;

        assert!(
            matches!(snapshot, LobbyInfo::NotStarted(players) if players.contains_key(&claims.id()))
        );

        drop(reconnected);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_lazy_loads_match_actor_from_events() {
        let server = TestServer::start().await;

        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        for token in &tokens {
            join_lobby_http(&client, &server, token, &lobby_id).await;
        }

        let mongo_database = server.stop_without_dropping_database().await;
        let server = TestServer::start_with_database(mongo_database).await;

        assert!(server.manager.match_senders.is_empty());
        assert_eq!(server.manager.active_player_route_count(), 0);

        let mut connection = connect_ws(&server, &tokens[0]).await;
        let snapshot = get_snapshot(&mut connection).await;

        assert!(matches!(snapshot, LobbyInfo::NotStarted(players) if players.len() == 2));
        assert_eq!(server.manager.match_senders.len(), 1);
        assert_eq!(server.manager.active_player_route_count(), 2);

        drop(connection);
        server.shutdown().await;
    }

    async fn get_players(client: &Client, server: &TestServer, count: usize) -> Vec<String> {
        let mut players = vec![];

        for i in 0..count {
            let player = login(client, server, i).await;
            players.push(player);
        }

        players
    }

    fn http_client() -> Client {
        Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .expect("Expected to build test HTTP client")
    }

    async fn recv_game_or_set_ended(socket: &mut WebSocket) -> EndMessage {
        match recv_msg(socket).await {
            ServerMessage::SetEnded { lifes } => EndMessage::Set(lifes),
            ServerMessage::GameEnded { lifes: _ } => EndMessage::Game,
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

    async fn join_lobby(
        client: &Client,
        server: &TestServer,
        tokens: Vec<String>,
    ) -> TestPlayersData {
        let lobby_id = create_lobby(client, server, &tokens[0]).await;
        let player_count = tokens.len();

        for (i, p) in tokens.iter().enumerate() {
            let lobby = join_lobby_http(client, server, p, &lobby_id).await;
            assert!(matches!(lobby, LobbyInfo::NotStarted(players) if players.len() == i + 1));
        }

        let mut connections = HashMap::new();

        for p in tokens {
            let claims = get_claims_from_token(&p, TEST_JWT_KEY).await.unwrap();

            let mut connection = connect_ws(server, &p).await;
            let snapshot = get_snapshot(&mut connection).await;

            assert!(
                matches!(snapshot, LobbyInfo::NotStarted(players) if players.len() == player_count)
            );

            let data = TestPlayerData {
                connection,
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

    async fn get_snapshot(stream: &mut WebSocket) -> LobbyInfo {
        match assert_game_msg(stream, |m| matches!(m, ServerMessage::Snapshot(_))).await {
            ServerMessage::Snapshot(info) => info,
            _ => panic!("Should be a Snapshot message"),
        }
    }

    async fn assert_game_msg<F>(stream: &mut WebSocket, predicate: F) -> ServerMessage
    where
        F: FnOnce(&ServerMessage) -> bool,
    {
        let msg = recv_msg(stream).await;

        match predicate(&msg) {
            true => msg,
            false => panic!("Message not expected {msg:?}"),
        }
    }

    async fn send_msg<T: serde::Serialize>(stream: &mut WebSocket, msg: T) {
        let msg = serde_json::to_string(&msg).unwrap();

        stream.send(Message::Text(msg.into())).await.unwrap();
    }

    async fn connect_ws(server: &TestServer, token: &str) -> WebSocket {
        let req = server
            .websocket_url(&format!("/game?token={token}"))
            .into_client_request()
            .unwrap();

        let (stream, _) = timeout(WS_TIMEOUT, connect_async(req))
            .await
            .expect("Timed out connecting websocket")
            .expect("Failed to connect WebSocket");

        assert!(!stream.is_terminated());

        stream
    }

    async fn assert_ws_closes_without_snapshot(server: &TestServer, token: &str) {
        let req = server
            .websocket_url(&format!("/game?token={token}"))
            .into_client_request()
            .unwrap();

        let result = timeout(WS_TIMEOUT, connect_async(req))
            .await
            .expect("Timed out connecting websocket");
        let Ok((mut stream, _)) = result else {
            return;
        };

        match timeout(WS_TIMEOUT, stream.next())
            .await
            .expect("Timed out waiting for websocket close")
        {
            None | Some(Err(_)) | Some(Ok(Message::Close(_))) => {}
            Some(Ok(msg)) => panic!("Expected websocket to close without snapshot, got {msg:?}"),
        }
    }

    async fn recv_msg(stream: &mut WebSocket) -> ServerMessage {
        let msg = timeout(WS_TIMEOUT, stream.next())
            .await
            .expect("Timed out waiting for websocket message")
            .expect("Expected websocket message")
            .expect("Expected valid websocket message");

        match msg {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            m => panic!("Error: {m}"),
        }
    }

    async fn join_lobby_http(
        client: &Client,
        server: &TestServer,
        token: &str,
        lobby_id: &LobbyId,
    ) -> LobbyInfo {
        let lobby_id = lobby_id.as_str();

        let res = client
            .put(server.url(&format!("/lobby/{lobby_id}")))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();

        res.json().await.unwrap()
    }

    async fn create_lobby(client: &Client, server: &TestServer, token: &str) -> LobbyId {
        let res = client
            .post(server.url("/lobby"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        let status = res.status();
        let body = res.text().await.unwrap();

        assert!(
            status.is_success(),
            "Expected lobby creation to succeed, got {status}: {body}"
        );

        let res: CreateLobbyResponse = serde_json::from_str(&body).unwrap();

        res.lobby_id
    }

    async fn login(client: &Client, server: &TestServer, number: usize) -> String {
        let params = serde_json::json!({
            "nickname": format!("Player {number}"),
        });

        let res = client
            .post(server.url("/auth/signup"))
            .json(&params)
            .send()
            .await
            .unwrap();

        let res: Auth = res.json().await.unwrap();

        res.token
    }
}
