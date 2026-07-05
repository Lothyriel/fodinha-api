mod auth;
mod game;
mod lobby;
pub mod models;
mod stats;

use std::net::Ipv6Addr;

use axum::{Json, Router, extract::State, response::IntoResponse, routing};
use reqwest::StatusCode;
use tokio::{net::TcpListener, sync::watch};
use tower_http::cors::{Any, CorsLayer};

use crate::{
    AppSettings,
    infra::telemetry,
    models::GameError,
    services::{LobbyError, ManagerError, matches::ManagerHandle},
};

#[derive(Clone)]
pub struct ApiState {
    manager: ManagerHandle,
    jwt_key: String,
    google_client_id: Option<String>,
    shutdown_rx: watch::Receiver<bool>,
}

pub async fn start(
    manager: ManagerHandle,
    settings: &AppSettings,
    shutdown_rx: watch::Receiver<bool>,
) {
    let address = (Ipv6Addr::UNSPECIFIED, 3000);

    let listener = TcpListener::bind(address)
        .await
        .expect("Expected to bind to network address");

    serve_listener(listener, manager, settings, shutdown_rx).await;
}

pub async fn serve_listener(
    listener: TcpListener,
    manager: ManagerHandle,
    settings: &AppSettings,
    shutdown_rx: watch::Receiver<bool>,
) {
    let address = listener
        .local_addr()
        .expect("Expected listener to expose local address");
    let shutdown_manager = manager.clone();
    let app = build_app(manager, settings, shutdown_rx.clone());

    tracing::info!("Listening on {:?}", address);

    let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
        let mut rx = shutdown_rx;
        if *rx.borrow() {
            return;
        }
        let _ = rx.changed().await;
    });

    if let Err(e) = serve.await {
        tracing::error!("Error serving API: {e}");
    }

    tracing::info!("Server shutdown complete");
    shutdown_manager.shutdown().await;
}

fn build_app(
    manager: ManagerHandle,
    settings: &AppSettings,
    shutdown_rx: watch::Receiver<bool>,
) -> Router {
    let state = ApiState {
        manager,
        jwt_key: settings.jwt_key.clone(),
        google_client_id: settings.google_client_id.clone(),
        shutdown_rx,
    };
    let auth = axum::middleware::from_fn_with_state(state.clone(), auth::middleware);

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/readyz", routing::get(readiness_handler))
        .route("/healthz", routing::get(readiness_handler))
        .route("/metrics", routing::get(telemetry::metrics_handler))
        .route("/game", routing::get(game::handler))
        .nest("/lobby", lobby::router().layer(auth))
        .nest("/stats", stats::router(state.clone()))
        .nest("/auth", auth::router(state.clone()))
        .fallback(fallback_handler)
        .with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors)
}

async fn fallback_handler() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "this resource doesn't exist")
}

async fn readiness_handler(State(state): State<ApiState>) -> StatusCode {
    if *state.shutdown_rx.borrow() {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    }
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
            ManagerError::GameCommand(_) => StatusCode::UNPROCESSABLE_ENTITY,
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
    use mongodb::{
        Database,
        bson::{Document, doc},
    };

    use reqwest::{Client, StatusCode};
    use tokio::{
        net::{TcpListener, TcpStream},
        sync::watch,
        task::JoinHandle,
        time::{Duration, sleep, timeout},
    };
    use tokio_tungstenite::{
        MaybeTlsStream, WebSocketStream, connect_async,
        tungstenite::{Message, client::IntoClientRequest},
    };

    use crate::{
        AppSettings,
        infra::UserClaims,
        models::{
            Card,
            commands::{
                ClientCommand, CreateLobbyResponse, LobbyInfo, MatchSnapshot, ServerMessage,
            },
            game::{GameCommand, GameType, fodinha_classic, fodinha_power},
            id::{LobbyId, PlayerId},
        },
        services::{
            manager::GameManager,
            matches::{ManagerHandle, WAITING_LOBBY_INACTIVITY_CLOSE_CODE},
            repositories::get_mongo_client,
            stats::PlayerStatsResponse,
        },
    };

    use super::{auth::get_claims_from_token, models::*, serve_listener};

    type WebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

    const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
    const MONGO_CONN_STRING: &str = "mongodb://localhost/?retryWrites=true";
    const SERVER_START_TIMEOUT: Duration = Duration::from_millis(200);
    const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
    const WAITING_LOBBY_TIMEOUT: Duration = Duration::from_secs(3 * 60);
    const EMPTY_PLAYING_TIMEOUT: Duration = Duration::from_secs(10 * 60);
    const ABANDONED_MATCH_SCAN_INTERVAL: Duration = Duration::from_secs(60);
    const TEST_GOOGLE_CLIENT_ID: Option<&str> =
        Some("824653628296-ahr9jr3aqgr367mul4p359dj4plsl67a.apps.googleusercontent.com");
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
        shutdown_tx: watch::Sender<bool>,
        handle: Option<JoinHandle<()>>,
    }

    impl TestServer {
        async fn start() -> Self {
            Self::start_with_timeouts(
                WAITING_LOBBY_TIMEOUT,
                EMPTY_PLAYING_TIMEOUT,
                ABANDONED_MATCH_SCAN_INTERVAL,
            )
            .await
        }

        async fn start_with_waiting_lobby_timeout(waiting_lobby_timeout: Duration) -> Self {
            Self::start_with_timeouts(
                waiting_lobby_timeout,
                EMPTY_PLAYING_TIMEOUT,
                ABANDONED_MATCH_SCAN_INTERVAL,
            )
            .await
        }

        async fn start_with_timeouts(
            waiting_lobby_timeout: Duration,
            empty_playing_timeout: Duration,
            abandoned_match_scan_interval: Duration,
        ) -> Self {
            let mongo_database = format!("oh_hell_test_{}", nanoid::nanoid!(10));

            Self::start_with_database_and_timeouts(
                mongo_database,
                waiting_lobby_timeout,
                empty_playing_timeout,
                abandoned_match_scan_interval,
            )
            .await
        }

        async fn start_with_database(mongo_database: String) -> Self {
            Self::start_with_database_and_timeouts(
                mongo_database,
                WAITING_LOBBY_TIMEOUT,
                EMPTY_PLAYING_TIMEOUT,
                ABANDONED_MATCH_SCAN_INTERVAL,
            )
            .await
        }

        async fn start_with_database_and_timeouts(
            mongo_database: String,
            waiting_lobby_timeout: Duration,
            empty_playing_timeout: Duration,
            abandoned_match_scan_interval: Duration,
        ) -> Self {
            let mongo_conn_string = MONGO_CONN_STRING.to_string();
            let settings = AppSettings {
                jwt_key: TEST_JWT_KEY.to_string(),
                google_client_id: TEST_GOOGLE_CLIENT_ID.map(String::from),
                mongo_conn_string: mongo_conn_string.clone(),
                mongo_database: mongo_database.clone(),
                mongo_max_pool_size: 10,
            };

            let client = get_mongo_client(&mongo_conn_string, settings.mongo_max_pool_size)
                .await
                .expect("Expected to create mongo client");

            let database = client.database(&mongo_database);
            let manager = GameManager::start_with_timeouts(
                &settings,
                waiting_lobby_timeout,
                empty_playing_timeout,
                abandoned_match_scan_interval,
            )
            .await;
            let server_manager = manager.clone();
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .expect("Expected to bind test API listener");
            let address = listener
                .local_addr()
                .expect("Expected test listener address");
            let base_url = format!("http://{address}");
            let ws_url = format!("ws://{address}");
            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            let handle = tokio::spawn(async move {
                serve_listener(listener, manager, &settings, shutdown_rx).await;
            });

            let server = Self {
                base_url,
                ws_url,
                mongo_database,
                manager: server_manager,
                database: Some(database),
                shutdown_tx,
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
                    if self.manager.registry.matches.is_empty()
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
            let _ = self.shutdown_tx.send(true);

            if let Some(mut handle) = self.handle.take() {
                if timeout(SHUTDOWN_TIMEOUT, &mut handle).await.is_err() {
                    handle.abort();
                    let _ = handle.await;
                    self.manager.abort_background_tasks();
                }
            }

            self.manager.shutdown().await;
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            let _ = self.shutdown_tx.send(true);
            self.manager.abort_background_tasks();

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
        let claims = get_claims_from_token(&tokens[0], TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
            .await
            .unwrap();
        let first_token = tokens[0].clone();
        let mut player_data = join_lobby(&client, &server, tokens).await;

        ready(&mut player_data).await;

        let mut first_connection = player_data.remove(&claims.id()).unwrap().connection;
        first_connection.close(None).await.unwrap();

        let mut reconnected = connect_ws(&server, &first_token).await;
        let snapshot = get_snapshot(&mut reconnected).await;

        assert!(
            matches!(snapshot, MatchSnapshot::Playing(data) if data.players.contains_key(&claims.id()))
        );

        drop(reconnected);
        drop(player_data);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_playing_match_abandoned_after_all_players_disconnect() {
        let server = TestServer::start_with_timeouts(
            WAITING_LOBBY_TIMEOUT,
            Duration::from_millis(200),
            ABANDONED_MATCH_SCAN_INTERVAL,
        )
        .await;

        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let first_token = tokens[0].clone();
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        for token in &tokens {
            join_lobby_http(&client, &server, token, &lobby_id).await;
        }

        let mut player_data = connect_players(&server, tokens).await;
        ready(&mut player_data).await;

        for player in player_data.values_mut() {
            player.connection.close(None).await.unwrap();
        }

        server.wait_until_match_actor_stopped().await;

        let metadata = server
            .database
            .as_ref()
            .unwrap()
            .collection::<Document>("MatchMetadata")
            .find_one(doc! { "match_id": lobby_id.as_str() })
            .await
            .unwrap()
            .expect("match metadata should remain for abandoned match");

        assert_eq!(metadata.get_str("status").unwrap(), "abandoned");
        assert_ws_closes_without_snapshot(&server, &first_token).await;

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_janitor_abandons_stale_playing_metadata_without_actor() {
        let server = TestServer::start().await;
        let match_id = LobbyId("stale-playing-match".into());

        server
            .database
            .as_ref()
            .unwrap()
            .collection::<Document>("MatchMetadata")
            .insert_one(doc! {
                "match_id": match_id.as_str(),
                "status": "playing",
                "updated_at": 0_i64,
                "players": [],
                "ready_players": [],
            })
            .await
            .unwrap();

        server
            .manager
            .abandon_stale_playing_matches(Duration::from_secs(1))
            .await
            .unwrap();

        let metadata = server
            .database
            .as_ref()
            .unwrap()
            .collection::<Document>("MatchMetadata")
            .find_one(doc! { "match_id": match_id.as_str() })
            .await
            .unwrap()
            .expect("stale playing metadata should remain");

        assert_eq!(metadata.get_str("status").unwrap(), "abandoned");

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_restart_restores_active_game_after_bid() {
        let server = TestServer::start().await;

        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let first_claims = get_claims_from_token(&tokens[0], TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
            .await
            .unwrap();
        let second_claims = get_claims_from_token(&tokens[1], TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
            .await
            .unwrap();
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        for token in &tokens {
            join_lobby_http(&client, &server, token, &lobby_id).await;
        }

        server
            .manager
            .player_status_change(first_claims.id(), true)
            .await
            .unwrap();
        server
            .manager
            .player_status_change(second_claims.id(), true)
            .await
            .unwrap();

        let (mut first_connection, mut second_connection) = tokio::join!(
            connect_ws(&server, &tokens[0]),
            connect_ws(&server, &tokens[1])
        );
        let (first_snapshot, second_snapshot) = tokio::join!(
            get_snapshot(&mut first_connection),
            get_snapshot(&mut second_connection)
        );

        let (bidding_player, chosen_bid) = match &first_snapshot {
            MatchSnapshot::Playing(data) => match &data.game.stage {
                crate::services::GameStageDto::Bidding { possible_bids } => (
                    PlayerId(data.game.current_player.clone().into()),
                    *possible_bids.first().expect("expected possible bids"),
                ),
                stage => panic!("Expected bidding stage, got {stage:?}"),
            },
            snapshot => panic!("Expected playing snapshot, got {snapshot:?}"),
        };

        assert!(matches!(second_snapshot, MatchSnapshot::Playing(_)));

        server
            .manager
            .bid(chosen_bid, bidding_player.clone())
            .await
            .unwrap();

        drop(first_connection);
        drop(second_connection);

        let mongo_database = server.stop_without_dropping_database().await;
        let server = TestServer::start_with_database(mongo_database).await;

        assert!(server.manager.registry.matches.is_empty());
        assert_eq!(server.manager.active_player_route_count(), 0);

        let (mut first_connection, mut second_connection) = tokio::join!(
            connect_ws(&server, &tokens[0]),
            connect_ws(&server, &tokens[1])
        );
        let (first_snapshot, second_snapshot) = tokio::join!(
            get_snapshot(&mut first_connection),
            get_snapshot(&mut second_connection)
        );

        for snapshot in [first_snapshot, second_snapshot] {
            match snapshot {
                MatchSnapshot::Playing(data) => {
                    assert_eq!(data.players.len(), 2);
                    assert!(data.game.info.iter().any(
                        |player| player.id == bidding_player && player.bid == Some(chosen_bid)
                    ));
                }
                snapshot => panic!("Expected playing snapshot after restart, got {snapshot:?}"),
            }
        }

        assert_eq!(server.manager.registry.matches.len(), 1);
        assert_eq!(server.manager.active_player_route_count(), 2);

        drop(first_connection);
        drop(second_connection);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_concurrent_lazy_loads_match_actor_from_events() {
        let server = TestServer::start().await;

        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        for token in &tokens {
            join_lobby_http(&client, &server, token, &lobby_id).await;
        }

        let mongo_database = server.stop_without_dropping_database().await;
        let server = TestServer::start_with_database(mongo_database).await;

        assert!(server.manager.registry.matches.is_empty());
        assert_eq!(server.manager.active_player_route_count(), 0);

        let (mut first_connection, mut second_connection) = tokio::join!(
            connect_ws(&server, &tokens[0]),
            connect_ws(&server, &tokens[1])
        );
        let (first_snapshot, second_snapshot) = tokio::join!(
            get_snapshot(&mut first_connection),
            get_snapshot(&mut second_connection)
        );

        assert!(matches!(first_snapshot, MatchSnapshot::Waiting(players) if players.len() == 2));
        assert!(matches!(second_snapshot, MatchSnapshot::Waiting(players) if players.len() == 2));
        assert_eq!(server.manager.registry.matches.len(), 1);
        assert_eq!(server.manager.active_player_route_count(), 2);

        drop(first_connection);
        drop(second_connection);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_unique_index_prevents_duplicate_event_sequence() {
        let server = TestServer::start().await;

        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        for token in &tokens {
            join_lobby_http(&client, &server, token, &lobby_id).await;
        }

        let first_claims = get_claims_from_token(&tokens[0], TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
            .await
            .unwrap();
        let second_claims = get_claims_from_token(&tokens[1], TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
            .await
            .unwrap();

        server
            .manager
            .player_status_change(first_claims.id(), true)
            .await
            .unwrap();
        server
            .manager
            .player_status_change(second_claims.id(), true)
            .await
            .unwrap();

        let repo = crate::services::repositories::matches::MatchesRepository::new(
            server.database.as_ref().unwrap(),
        );

        let duplicate = repo
            .append_event(
                &lobby_id,
                0,
                crate::models::game::MatchEvent::MatchCreated {
                    settings: crate::models::game::GameSettings::default(),
                },
            )
            .await;

        assert!(
            duplicate.is_err(),
            "Duplicate (match_id, sequence) must be rejected by unique index"
        );

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_waiting_lobby_survives_restart() {
        let server = TestServer::start().await;

        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        for token in &tokens {
            join_lobby_http(&client, &server, token, &lobby_id).await;
        }

        let mongo_database = server.stop_without_dropping_database().await;

        let server = TestServer::start_with_database(mongo_database).await;

        assert!(server.manager.registry.matches.is_empty());
        assert_eq!(server.manager.active_player_route_count(), 0);

        let lobbies = server.manager.get_lobbies().await;
        assert!(
            lobbies
                .iter()
                .any(|l| l.id == lobby_id && l.player_count == 2),
            "Waiting lobby should survive restart and be listed"
        );

        let lobby = join_lobby_http(&client, &server, &tokens[0], &lobby_id).await;
        assert!(
            matches!(lobby, LobbyInfo::NotStarted(players) if players.len() == 2),
            "Rejoined lobby should still have 2 players"
        );

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_signup_rejects_long_nickname() {
        let server = TestServer::start().await;
        let client = http_client();
        let params = serde_json::json!({
            "nickname": "x".repeat(25),
        });

        let res = client
            .post(server.url("/auth/signup"))
            .json(&params)
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_refresh_renews_anonymous_session() {
        let server = TestServer::start().await;
        let client = http_client();
        let auth = login_auth(&client, &server, 1).await;
        let refresh_token = auth.refresh_token.clone().expect("refresh token missing");
        let original_player_id =
            get_claims_from_token(&auth.token, TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
                .await
                .unwrap()
                .id();

        let refreshed = refresh_auth(&client, &server, &refresh_token).await;
        let refreshed_player_id =
            get_claims_from_token(&refreshed.token, TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
                .await
                .unwrap()
                .id();

        assert_eq!(refreshed_player_id, original_player_id);
        assert!(refreshed.refresh_token.is_some());
        assert_ne!(refreshed.refresh_token, auth.refresh_token);

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_refresh_rejects_invalid_token() {
        let server = TestServer::start().await;
        let client = http_client();
        let res = client
            .post(server.url("/auth/refresh"))
            .json(&serde_json::json!({ "refresh_token": "invalid-token" }))
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_lobby_changes_do_not_write_match_events() {
        let server = TestServer::start().await;
        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        for token in &tokens {
            join_lobby_http(&client, &server, token, &lobby_id).await;
        }

        let player_id = get_claims_from_token(&tokens[0], TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
            .await
            .unwrap()
            .id();

        server
            .manager
            .player_status_change(player_id, true)
            .await
            .unwrap();

        let events = server
            .database
            .as_ref()
            .unwrap()
            .collection::<Document>("MatchEvents")
            .count_documents(doc! { "match_id": lobby_id.as_str() })
            .await
            .unwrap();

        assert_eq!(events, 0);

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_waiting_lobby_disconnect_removes_non_last_player_and_keeps_final_player() {
        let server = TestServer::start().await;
        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        for token in &tokens {
            join_lobby_http(&client, &server, token, &lobby_id).await;
        }

        let second_player_id =
            get_claims_from_token(&tokens[1], TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
                .await
                .unwrap()
                .id();
        let creator_player_id =
            get_claims_from_token(&tokens[0], TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
                .await
                .unwrap()
                .id();
        let mut first_connection = connect_ws(&server, &tokens[0]).await;
        let mut second_connection = connect_ws(&server, &tokens[1]).await;

        assert!(
            matches!(get_snapshot(&mut first_connection).await, MatchSnapshot::Waiting(players) if players.len() == 2)
        );
        assert!(
            matches!(get_snapshot(&mut second_connection).await, MatchSnapshot::Waiting(players) if players.len() == 2)
        );

        second_connection.close(None).await.unwrap();

        assert!(matches!(
            recv_msg(&mut first_connection).await,
            ServerMessage::PlayerLeft { player_id } if player_id == second_player_id
        ));

        let lobbies = server.manager.get_lobbies().await;
        assert_eq!(lobbies.len(), 1);
        assert_eq!(lobbies[0].player_count, 1);

        first_connection.close(None).await.unwrap();

        timeout(WS_TIMEOUT, async {
            loop {
                let lobbies = server.manager.get_lobbies().await;

                if matches!(lobbies.as_slice(), [lobby] if lobby.player_count == 1) {
                    return;
                }

                sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("Timed out waiting for waiting lobby to remain open");

        let mut reconnected = connect_ws(&server, &tokens[0]).await;
        let snapshot = get_snapshot(&mut reconnected).await;

        assert!(
            matches!(snapshot, MatchSnapshot::Waiting(players) if players.len() == 1 && players.contains_key(&creator_player_id))
        );

        drop(reconnected);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_last_non_creator_disconnect_keeps_waiting_lobby_open() {
        let server = TestServer::start().await;
        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        for token in &tokens {
            join_lobby_http(&client, &server, token, &lobby_id).await;
        }

        let creator_player_id =
            get_claims_from_token(&tokens[0], TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
                .await
                .unwrap()
                .id();
        let second_player_id =
            get_claims_from_token(&tokens[1], TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
                .await
                .unwrap()
                .id();
        let mut creator_connection = connect_ws(&server, &tokens[0]).await;
        let mut second_connection = connect_ws(&server, &tokens[1]).await;

        assert!(
            matches!(get_snapshot(&mut creator_connection).await, MatchSnapshot::Waiting(players) if players.len() == 2)
        );
        assert!(
            matches!(get_snapshot(&mut second_connection).await, MatchSnapshot::Waiting(players) if players.len() == 2)
        );

        creator_connection.close(None).await.unwrap();

        assert!(matches!(
            recv_msg(&mut second_connection).await,
            ServerMessage::PlayerLeft { player_id } if player_id == creator_player_id
        ));

        second_connection.close(None).await.unwrap();

        timeout(WS_TIMEOUT, async {
            loop {
                let lobbies = server.manager.get_lobbies().await;

                if matches!(lobbies.as_slice(), [lobby] if lobby.player_count == 1) {
                    return;
                }

                sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("Timed out waiting for waiting lobby to remain open");

        let mut reconnected = connect_ws(&server, &tokens[1]).await;
        let snapshot = get_snapshot(&mut reconnected).await;

        assert!(
            matches!(snapshot, MatchSnapshot::Waiting(players) if players.len() == 1 && players.contains_key(&second_player_id))
        );

        drop(reconnected);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_last_non_creator_disconnect_expires_waiting_lobby_after_timeout() {
        let server = TestServer::start_with_waiting_lobby_timeout(Duration::from_millis(200)).await;
        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        for token in &tokens {
            join_lobby_http(&client, &server, token, &lobby_id).await;
        }

        let creator_player_id =
            get_claims_from_token(&tokens[0], TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
                .await
                .unwrap()
                .id();
        let mut creator_connection = connect_ws(&server, &tokens[0]).await;
        let mut second_connection = connect_ws(&server, &tokens[1]).await;

        assert!(
            matches!(get_snapshot(&mut creator_connection).await, MatchSnapshot::Waiting(players) if players.len() == 2)
        );
        assert!(
            matches!(get_snapshot(&mut second_connection).await, MatchSnapshot::Waiting(players) if players.len() == 2)
        );

        creator_connection.close(None).await.unwrap();

        assert!(matches!(
            recv_msg(&mut second_connection).await,
            ServerMessage::PlayerLeft { player_id } if player_id == creator_player_id
        ));

        second_connection.close(None).await.unwrap();
        server.wait_until_match_actor_stopped().await;

        assert!(server.manager.get_lobbies().await.is_empty());

        let metadata = server
            .database
            .as_ref()
            .unwrap()
            .collection::<Document>("MatchMetadata")
            .find_one(doc! { "match_id": lobby_id.as_str() })
            .await
            .unwrap();

        assert!(metadata.is_none());

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_waiting_lobby_timeout_resets_on_activity() {
        let server = TestServer::start_with_waiting_lobby_timeout(Duration::from_millis(200)).await;
        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        join_lobby_http(&client, &server, &tokens[0], &lobby_id).await;

        sleep(Duration::from_millis(125)).await;

        join_lobby_http(&client, &server, &tokens[1], &lobby_id).await;

        sleep(Duration::from_millis(125)).await;

        assert!(
            matches!(server.manager.get_lobbies().await.as_slice(), [lobby] if lobby.id == lobby_id)
        );

        server.wait_until_match_actor_stopped().await;
        assert!(server.manager.get_lobbies().await.is_empty());

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_waiting_lobby_timeout_closes_connected_players_with_inactivity_code() {
        let server = TestServer::start_with_waiting_lobby_timeout(Duration::from_millis(200)).await;
        let client = http_client();
        let tokens = get_players(&client, &server, 1).await;
        let lobby_id = create_lobby(&client, &server, &tokens[0]).await;

        join_lobby_http(&client, &server, &tokens[0], &lobby_id).await;

        let mut connection = connect_ws(&server, &tokens[0]).await;

        assert!(
            matches!(get_snapshot(&mut connection).await, MatchSnapshot::Waiting(players) if players.len() == 1)
        );

        let msg = timeout(WS_TIMEOUT, connection.next())
            .await
            .expect("Timed out waiting for inactivity websocket close")
            .expect("Expected websocket close message")
            .expect("Expected valid websocket close message");

        match msg {
            Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), WAITING_LOBBY_INACTIVITY_CLOSE_CODE);
            }
            Message::Close(None) => panic!("Expected close code"),
            msg => panic!("Expected close message, got {msg:?}"),
        }

        server.wait_until_match_actor_stopped().await;
        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_create_lobby_accepts_custom_lifes() {
        let server = TestServer::start().await;

        let client = http_client();
        let tokens = get_players(&client, &server, 2).await;
        let lobby_id = create_lobby_with_lifes(&client, &server, &tokens[0], 1).await;

        for (i, token) in tokens.iter().enumerate() {
            let lobby = join_lobby_http(&client, &server, token, &lobby_id).await;
            assert!(matches!(lobby, LobbyInfo::NotStarted(players) if players.len() == i + 1));
        }

        let mut player_data = connect_players(&server, tokens.clone()).await;

        ready(&mut player_data).await;
        get_decks(&mut player_data).await;
        play_set(&mut player_data).await;

        for p in player_data.values_mut() {
            assert!(matches!(
                recv_game_or_set_ended(&mut p.connection).await,
                EndMessage::Game
            ));
        }

        server.wait_until_match_actor_stopped().await;

        let updated_token = update_profile(&client, &server, &tokens[0], "Renamed Player").await;
        let my_stats = wait_for_my_stats(&client, &server, &updated_token).await;
        assert_eq!(my_stats.games_played, 1);
        assert_eq!(my_stats.bid_count, 1);
        assert_eq!(stats_nickname(&my_stats), Some("Renamed Player"));

        let leaderboard = wait_for_leaderboard(&client, &server, 2).await;
        assert_eq!(
            leaderboard
                .iter()
                .map(|stats| stats.games_played)
                .sum::<i64>(),
            2
        );
        assert_eq!(
            leaderboard
                .iter()
                .map(|stats| stats.matches_won)
                .sum::<i64>(),
            1
        );
        assert!(
            leaderboard
                .iter()
                .any(|stats| stats_nickname(stats) == Some("Renamed Player"))
        );

        drop(player_data);
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

        let msg = ClientCommand::GameCommand(GameCommand::FodinhaClassic(
            fodinha_classic::GameCommand::PlayTurn {
                card: next.deck.pop().unwrap(),
            },
        ));

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

        send_msg(
            &mut next.connection,
            ClientCommand::GameCommand(GameCommand::FodinhaClassic(
                fodinha_classic::GameCommand::PutBid { bid },
            )),
        )
        .await;

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

        for (i, p) in tokens.iter().enumerate() {
            let lobby = join_lobby_http(client, server, p, &lobby_id).await;
            assert!(matches!(lobby, LobbyInfo::NotStarted(players) if players.len() == i + 1));
        }

        connect_players(server, tokens).await
    }

    async fn connect_players(server: &TestServer, tokens: Vec<String>) -> TestPlayersData {
        let player_count = tokens.len();

        let mut connections = HashMap::new();

        for p in tokens {
            let claims = get_claims_from_token(&p, TEST_JWT_KEY, TEST_GOOGLE_CLIENT_ID)
                .await
                .unwrap();

            let mut connection = connect_ws(server, &p).await;
            let snapshot = get_snapshot(&mut connection).await;

            assert!(
                matches!(snapshot, MatchSnapshot::Waiting(players) if players.len() == player_count)
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
            send_msg(&mut p.connection, msg.clone()).await;
        }

        for _ in 0..players.len() {
            for p in players.values_mut() {
                assert_game_msg(&mut p.connection, validate_player_status_change).await;
            }
        }

        for p in players.values_mut() {
            let snapshot = get_snapshot(&mut p.connection).await;
            assert!(
                matches!(snapshot, MatchSnapshot::Playing(_)),
                "Expected playing snapshot after game start"
            );
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

    async fn get_snapshot(stream: &mut WebSocket) -> MatchSnapshot {
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

    async fn wait_for_my_stats(
        client: &Client,
        server: &TestServer,
        token: &str,
    ) -> PlayerStatsResponse {
        timeout(WS_TIMEOUT, async {
            loop {
                if let Some(stats) = get_my_stats(client, server, token).await {
                    return stats;
                }

                sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("Timed out waiting for my stats")
    }

    async fn get_my_stats(
        client: &Client,
        server: &TestServer,
        token: &str,
    ) -> Option<PlayerStatsResponse> {
        client
            .get(server.url("/stats/me"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn wait_for_leaderboard(
        client: &Client,
        server: &TestServer,
        expected: usize,
    ) -> Vec<PlayerStatsResponse> {
        timeout(WS_TIMEOUT, async {
            loop {
                let stats = get_leaderboard(client, server).await;

                if stats.len() >= expected {
                    return stats;
                }

                sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("Timed out waiting for leaderboard stats")
    }

    async fn get_leaderboard(client: &Client, server: &TestServer) -> Vec<PlayerStatsResponse> {
        client
            .get(server.url("/stats"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn update_profile(
        client: &Client,
        server: &TestServer,
        token: &str,
        nickname: &str,
    ) -> String {
        let params = serde_json::json!({
            "nickname": nickname,
        });

        let res = client
            .post(server.url("/auth/profile"))
            .bearer_auth(token)
            .json(&params)
            .send()
            .await
            .unwrap();

        assert!(res.status().is_success());

        let res: Auth = res.json().await.unwrap();

        res.token
    }

    fn stats_nickname(stats: &PlayerStatsResponse) -> Option<&str> {
        match stats.player.as_ref()? {
            UserClaims::Anonymous(claims) => claims.data.get("nickname")?.as_str(),
            UserClaims::Google(claims) => claims.nickname.as_deref().or(Some(claims.name.as_str())),
        }
    }

    async fn create_lobby(client: &Client, server: &TestServer, token: &str) -> LobbyId {
        create_lobby_with_optional_lifes(client, server, token, None).await
    }

    async fn create_lobby_with_lifes(
        client: &Client,
        server: &TestServer,
        token: &str,
        lifes: usize,
    ) -> LobbyId {
        create_lobby_with_optional_lifes(client, server, token, Some(lifes)).await
    }

    async fn create_lobby_with_optional_lifes(
        client: &Client,
        server: &TestServer,
        token: &str,
        lifes: Option<usize>,
    ) -> LobbyId {
        let mut params = serde_json::json!({ "game_type": "fodinha_classic" });

        if let Some(lifes) = lifes {
            params["lifes"] = serde_json::json!(lifes);
        }

        let res = client
            .post(server.url("/lobby"))
            .bearer_auth(token)
            .json(&params)
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
        login_auth(client, server, number).await.token
    }

    async fn login_auth(client: &Client, server: &TestServer, number: usize) -> Auth {
        let params = serde_json::json!({
            "nickname": format!("Player {number}"),
        });

        let res = client
            .post(server.url("/auth/signup"))
            .json(&params)
            .send()
            .await
            .unwrap();

        res.json().await.unwrap()
    }

    async fn refresh_auth(client: &Client, server: &TestServer, refresh_token: &str) -> Auth {
        let res = client
            .post(server.url("/auth/refresh"))
            .json(&serde_json::json!({ "refresh_token": refresh_token }))
            .send()
            .await
            .unwrap();

        assert!(res.status().is_success());

        res.json().await.unwrap()
    }
}
