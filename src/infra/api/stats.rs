use axum::{
    Extension, Json, Router, middleware,
    extract::{Query, State},
    routing,
};

use crate::{
    infra::UserClaims,
    services::{ManagerError, stats::PlayerStatsResponse},
};

use super::{ApiState, auth};

pub fn router(state: ApiState) -> Router<ApiState> {
    let auth = axum::middleware::from_fn_with_state(state, auth::middleware);

    Router::new()
        .route("/", routing::get(leaderboard))
        .route("/me", routing::get(my_stats).layer(auth))
        .layer(middleware::from_fn(crate::infra::telemetry::http_middleware))
}

#[derive(serde::Deserialize)]
struct StatsQuery {
    limit: Option<i64>,
}

async fn leaderboard(
    State(state): State<ApiState>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<PlayerStatsResponse>>, ManagerError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let stats = state.manager.leaderboard(limit).await?;

    Ok(Json(stats))
}

async fn my_stats(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
) -> Result<Json<Option<PlayerStatsResponse>>, ManagerError> {
    let stats = state.manager.player_stats(&user_claims.id()).await?;

    Ok(Json(stats))
}
