use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    middleware, routing,
};

use crate::{
    infra::{UserClaims, telemetry},
    models::{
        commands::{CreateLobbyResponse, GetLobbyDto, LobbyInfo},
        game::{GameSettings, GameType, fodinha_classic, fodinha_power},
        id::{DeckId, LobbyId},
    },
    services::{LobbyError, ManagerError},
};

use super::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/", routing::get(get_lobbies))
        .route("/", routing::post(create_lobby))
        .route("/{id}", routing::put(join_lobby))
        .layer(middleware::from_fn(telemetry::http_middleware))
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
    let settings = body.into_settings()?;
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
    power_deck_id: Option<DeckId>,
}

impl CreateLobbyRequest {
    fn into_settings(self) -> Result<GameSettings, LobbyError> {
        match self.game_type {
            GameType::FodinhaClassic => self.into_fodinha_classic_settings(),
            GameType::FodinhaPower => self.into_fodinha_power_settings(),
        }
    }

    fn into_fodinha_classic_settings(self) -> Result<GameSettings, LobbyError> {
        let mut settings = fodinha_classic::GameSettings::default();

        if let Some(lifes) = self.lifes {
            settings.lifes = validate_lifes(
                self.game_type,
                lifes,
                fodinha_classic::MIN_INITIAL_LIFES,
                fodinha_classic::MAX_INITIAL_LIFES,
            )?;
        }

        Ok(GameSettings::FodinhaClassic(settings))
    }

    fn into_fodinha_power_settings(self) -> Result<GameSettings, LobbyError> {
        let power_deck_id = self.power_deck_id.clone().ok_or_else(|| {
            LobbyError::InvalidSettings("power_deck_id is required for Fodinha Power".to_string())
        })?;

        let lifes = if let Some(lifes) = self.lifes {
            validate_lifes(
                self.game_type,
                lifes,
                fodinha_power::MIN_INITIAL_LIFES,
                fodinha_power::MAX_INITIAL_LIFES,
            )?
        } else {
            fodinha_power::DEFAULT_INITIAL_LIFES
        };

        Ok(GameSettings::FodinhaPower(fodinha_power::GameSettings {
            lifes,
            power_deck_id,
            player_mercenaries: Default::default(),
        }))
    }
}

fn validate_lifes(
    game_type: GameType,
    lifes: usize,
    min: usize,
    max: usize,
) -> Result<usize, LobbyError> {
    if lifes < min || lifes > max {
        return Err(LobbyError::InvalidSettings(format!(
            "lifes for {game_type} must be between {min} and {max}"
        )));
    }

    Ok(lifes)
}
