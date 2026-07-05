use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Client;
use serde::de::DeserializeOwned;

use crate::{
    infra::api::models::Auth,
    models::{
        commands::{CreateLobbyResponse, LobbyInfo},
        game::GameSettings,
        id::{LobbyId, PlayerId},
    },
    services::stats::PlayerStatsResponse,
};

use super::ws::{ClientError, err};

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

    pub async fn try_signup(&self, nickname: &str) -> Result<String, ClientError> {
        Ok(self.try_signup_auth(nickname).await?.token)
    }

    pub async fn signup_auth(&self, nickname: &str) -> Auth {
        self.try_signup_auth(nickname).await.unwrap()
    }

    pub async fn try_signup_auth(&self, nickname: &str) -> Result<Auth, ClientError> {
        let params = serde_json::json!({ "nickname": nickname });

        self.decode_json(
            self.client.post(self.url("/auth/signup")).json(&params),
            "POST /auth/signup",
        )
        .await
    }

    pub async fn create_lobby(&self, token: &str) -> LobbyId {
        self.create_lobby_with_settings(token, None).await
    }

    pub async fn try_create_lobby(&self, token: &str) -> Result<LobbyId, ClientError> {
        self.try_create_lobby_with_settings(token, None).await
    }

    pub async fn create_lobby_with_settings(
        &self,
        token: &str,
        settings: Option<GameSettings>,
    ) -> LobbyId {
        self.try_create_lobby_with_settings(token, settings)
            .await
            .unwrap()
    }

    pub async fn try_create_lobby_with_settings(
        &self,
        token: &str,
        settings: Option<GameSettings>,
    ) -> Result<LobbyId, ClientError> {
        let mut request = self.client.post(self.url("/lobby")).bearer_auth(token);

        if let Some(settings) = settings {
            match settings {
                GameSettings::FodinhaClassic(settings) => {
                    request = request.json(&serde_json::json!({
                        "game_type": "fodinha_classic",
                        "lifes": settings.lifes,
                    }));
                }
                GameSettings::FodinhaPower(settings) => {
                    request = request.json(&serde_json::json!({
                        "game_type": "fodinha_power",
                        "lifes": settings.lifes,
                    }));
                }
            }
        }

        let res: CreateLobbyResponse = self.decode_json(request, "POST /lobby").await?;

        Ok(res.lobby_id)
    }

    pub async fn join_lobby(&self, token: &str, lobby_id: &LobbyId) -> LobbyInfo {
        self.try_join_lobby(token, lobby_id).await.unwrap()
    }

    pub async fn try_join_lobby(
        &self,
        token: &str,
        lobby_id: &LobbyId,
    ) -> Result<LobbyInfo, ClientError> {
        self.decode_json(
            self.client
                .put(self.url(&format!("/lobby/{}", lobby_id.as_str())))
                .bearer_auth(token),
            "PUT /lobby/{id}",
        )
        .await
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

    pub async fn wait_for_my_stats(&self, token: &str, timeout: Duration) -> PlayerStatsResponse {
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

    async fn decode_json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        operation: &str,
    ) -> Result<T, ClientError> {
        let response = request
            .send()
            .await
            .map_err(|e| err!("{operation} request failed: {e}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| err!("{operation} response read failed: {e}"))?;

        if !status.is_success() {
            return Err(err!("{operation} failed with {status}: {body}"));
        }

        serde_json::from_str(&body)
            .map_err(|e| err!("{operation} returned invalid JSON: {e}; body: {body}"))
    }
}
