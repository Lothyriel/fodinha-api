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
    services::{ManagerError, dispatcher::ManagerHandle},
};

pub fn router() -> Router<ManagerHandle> {
    Router::new()
        .route("/", routing::get(get_lobbies))
        .route("/", routing::post(create_lobby))
        .route("/{id}", routing::put(join_lobby))
}

async fn get_lobbies(State(manager): State<ManagerHandle>) -> Json<Vec<GetLobbyDto>> {
    Json(manager.get_lobbies().await)
}

async fn join_lobby(
    State(manager): State<ManagerHandle>,
    Extension(user_claims): Extension<UserClaims>,
    Path(id): Path<LobbyId>,
) -> Result<Json<LobbyInfo>, ManagerError> {
    let response = manager.join_lobby(id, user_claims).await?;

    Ok(Json(response))
}

async fn create_lobby(
    State(manager): State<ManagerHandle>,
    Extension(user_claims): Extension<UserClaims>,
) -> Result<Json<CreateLobbyResponse>, ManagerError> {
    let response = manager
        .create_lobby(user_claims.id(), Default::default())
        .await?;

    Ok(Json(response))
}
