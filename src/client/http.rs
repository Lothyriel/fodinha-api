use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Client;

use crate::{
    infra::api::models::Auth,
    models::{
        commands::{CreateLobbyResponse, LobbyInfo},
        game::GameSettings,
        id::{LobbyId, PlayerId},
    },
    services::stats::PlayerStatsResponse,
};

pub const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct HttpClient {
    client: Client,
    pub base_url: String,
}

impl HttpClient {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .expect("Expected to build HTTP client"),
            base_url,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    pub async fn signup(&self, nickname: &str) -> String {
        let auth = self.signup_auth(nickname).await;
        auth.token
    }

    pub async fn signup_auth(&self, nickname: &str) -> Auth {
        let params = serde_json::json!({ "nickname": nickname });

        self.client
            .post(self.url("/auth/signup"))
            .json(&params)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    pub async fn create_lobby(&self, token: &str) -> LobbyId {
        self.create_lobby_with_settings(token, None).await
    }

    pub async fn create_lobby_with_settings(
        &self,
        token: &str,
        settings: Option<GameSettings>,
    ) -> LobbyId {
        let mut request = self.client.post(self.url("/lobby")).bearer_auth(token);

        if let Some(settings) = settings {
            request = request.json(&serde_json::json!({ "lifes": settings.lifes }));
        }

        let res = request.send().await.unwrap();
        let body = res.text().await.unwrap();
        let res: CreateLobbyResponse = serde_json::from_str(&body).unwrap();

        res.lobby_id
    }

    pub async fn join_lobby(&self, token: &str, lobby_id: &LobbyId) -> LobbyInfo {
        self.client
            .put(self.url(&format!("/lobby/{}", lobby_id.as_str())))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    pub async fn get_lobbies(&self) -> serde_json::Value {
        self.client
            .get(self.url("/lobby"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    pub async fn get_my_stats(&self, token: &str) -> Option<PlayerStatsResponse> {
        self.client
            .get(self.url("/stats/me"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    pub async fn wait_for_my_stats(
        &self,
        token: &str,
        timeout: Duration,
    ) -> PlayerStatsResponse {
        tokio::time::timeout(timeout, async {
            loop {
                if let Some(stats) = self.get_my_stats(token).await {
                    return stats;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("Timed out waiting for my stats")
    }

    pub fn player_id_from_token(token: &str) -> PlayerId {
        let parts: Vec<&str> = token.split('.').collect();
        let payload_b64 = parts.get(1).expect("Invalid JWT format");
        let payload_bytes = URL_SAFE_NO_PAD
            .decode(payload_b64)
            .expect("Invalid JWT base64 payload");
        let payload_str = String::from_utf8(payload_bytes).expect("Invalid UTF-8 in JWT payload");
        let json: serde_json::Value =
            serde_json::from_str(&payload_str).expect("Invalid JSON in JWT payload");

        let user = &json["user"];
        let id = match user["type"].as_str() {
            Some("Anonymous") => user["data"]["id"].as_str(),
            Some("Google") => user["data"]["email"].as_str(),
            _ => None,
        }
        .expect("Could not extract player id from token");

        PlayerId(std::sync::Arc::from(id))
    }
}
