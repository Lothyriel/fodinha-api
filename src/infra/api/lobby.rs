use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    routing,
};

use crate::{
    infra::UserClaims,
    models::{
        commands::{CreateLobbyResponse, GetLobbyDto, LobbyInfo},
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
) -> Result<Json<CreateLobbyResponse>, ManagerError> {
    let response = state
        .manager
        .create_lobby(user_claims.id(), Default::default())
        .await?;

    Ok(Json(response))
}
