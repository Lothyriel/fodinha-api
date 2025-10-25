#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use api::{
        infra::{
            auth::{TokenResponse, get_claims_from_token},
            lobby::CreateLobbyResponse,
            *,
        },
        models::Card,
        services::manager::{LobbyId, PlayerId},
    };
    use futures::{SinkExt, StreamExt, stream::FusedStream};
    use reqwest::Client;
    use tokio::{net::TcpStream, task};
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

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
        task::spawn(api::start_app());
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

        let msg = ClientMessage::PlayTurn {
            card: next.deck.swap_remove(0),
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

        send_msg(&mut next.connection, ClientMessage::PutBid { bid }).await;

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
            assert!(lobby.players.len() == i + 1);
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
        let msg = ClientMessage::PlayerStatusChange { ready: true };

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

    async fn send_msg(stream: &mut WebSocket, msg: ClientMessage) {
        let msg = serde_json::to_string(&msg).unwrap();

        stream.send(Message::Text(msg.into())).await.unwrap();
    }

    async fn connect_ws(token: &str) -> WebSocket {
        let uri = "ws://localhost:3000/game".parse().unwrap();

        let req = tokio_tungstenite::tungstenite::ClientRequestBuilder::new(uri)
            .with_header("Authorization", format!("Bearer {token}"));

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

    async fn join_lobby_http(client: &Client, token: &str, lobby_id: &str) -> JoinLobbyDto {
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

        let res: TokenResponse = res.json().await.unwrap();

        res.token
    }
}
