use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    routing,
};

use crate::{
    infra::UserClaims,
    models::{
        commands::{CreateLobbyResponse, GetLobbyDto, LobbyInfo},
        game::GameSettings,
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
}

async fn get_lobbies(State(state): State<ApiState>) -> Json<Vec<GetLobbyDto>> {
    Json(state.manager.get_lobbies().await)
}

async fn join_lobby(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    Path(id): Path<LobbyId>,
) -> Result<Json<LobbyInfo>, ManagerError> {
    let response = state.manager.join_lobby(id, user_claims).await?;

    Ok(Json(response))
}

async fn create_lobby(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    body: Option<Json<CreateLobbyRequest>>,
) -> Result<Json<CreateLobbyResponse>, ManagerError> {
    let settings = body
        .map(|Json(body)| body.into_settings())
        .unwrap_or_default();
    let response = state
        .manager
        .create_lobby(user_claims.id(), settings)
        .await?;

    Ok(Json(response))
}

#[derive(Default, serde::Deserialize)]
struct CreateLobbyRequest {
    lifes: Option<usize>,
}

impl CreateLobbyRequest {
    fn into_settings(self) -> GameSettings {
        let mut settings = GameSettings::default();

        if let Some(lifes) = self.lifes {
            settings.lifes = lifes.max(1);
        }

        settings
    }
}
