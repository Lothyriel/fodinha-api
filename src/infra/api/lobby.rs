use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    middleware, routing,
};

use crate::{
    infra::UserClaims,
    models::{
        commands::{CreateLobbyResponse, GetLobbyDto, LobbyInfo},
        game::{GameSettings, GameType, fodinha_classic, fodinha_power},
        id::LobbyId,
    },
    services::ManagerError,
};

use super::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/", routing::get(get_lobbies))
        .route("/", routing::post(create_lobby))
        .route("/{id}", routing::put(join_lobby))
        .layer(middleware::from_fn(
            crate::infra::telemetry::http_middleware,
        ))
}

async fn get_lobbies(State(state): State<ApiState>) -> Json<Vec<GetLobbyDto>> {
    Json(state.manager.get_lobbies().await)
}

async fn join_lobby(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    Path(id): Path<LobbyId>,
) -> Result<Json<LobbyInfo>, ManagerError> {
    let response = state.manager.join_lobby(id, user_claims.id()).await?;

    Ok(Json(response))
}

async fn create_lobby(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    Json(body): Json<CreateLobbyRequest>,
) -> Result<Json<CreateLobbyResponse>, ManagerError> {
    let settings = body.into_settings();
    let response = state
        .manager
        .create_lobby(user_claims.id(), settings)
        .await?;

    Ok(Json(response))
}

#[derive(serde::Deserialize)]
struct CreateLobbyRequest {
    game_type: GameType,
    lifes: Option<usize>,
}

impl CreateLobbyRequest {
    fn into_settings(self) -> GameSettings {
        match self.game_type {
            GameType::FodinhaClassic => self.into_fodinha_classic_settings(),
            GameType::FodinhaPower => self.into_fodinha_power_settings(),
        }
    }

    fn into_fodinha_classic_settings(self) -> GameSettings {
        let mut settings = fodinha_classic::GameSettings::default();

        if let Some(lifes) = self.lifes {
            settings.lifes = lifes.max(1);
        }

        GameSettings::FodinhaClassic(settings)
    }

    fn into_fodinha_power_settings(self) -> GameSettings {
        let mut settings = fodinha_power::GameSettings::default();

        if let Some(lifes) = self.lifes {
            settings.lifes = lifes.max(1);
        }

        GameSettings::FodinhaPower(settings)
    }
}
