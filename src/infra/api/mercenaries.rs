use axum::{
    Extension, Json, Router,
    extract::{Multipart, Path, State},
    middleware, routing,
};

use crate::{
    infra::{UserClaims, telemetry},
    models::id::{MercenaryId, gen_mercenaryid},
    services::mercenaries::{MercenaryError, MercenaryResponse, UpsertMercenaryInput},
};

use super::ApiState;

const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_SCRIPT_BYTES: usize = 128 * 1024;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/", routing::get(list_mercenaries))
        .route("/", routing::post(create_mercenary))
        .route("/{mercenary_id}", routing::put(update_mercenary))
        .layer(middleware::from_fn(telemetry::http_middleware))
}

async fn list_mercenaries(
    State(state): State<ApiState>,
) -> Result<Json<Vec<MercenaryResponse>>, MercenaryError> {
    Ok(Json(state.manager.mercenaries().await?))
}

async fn create_mercenary(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    multipart: Multipart,
) -> Result<Json<MercenaryResponse>, MercenaryError> {
    let input = read_upsert_mercenary_input(None, multipart).await?;
    let mercenary = state
        .manager
        .create_mercenary(user_claims.id(), input)
        .await?;

    Ok(Json(mercenary))
}

async fn update_mercenary(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    Path(mercenary_id): Path<MercenaryId>,
    multipart: Multipart,
) -> Result<Json<MercenaryResponse>, MercenaryError> {
    let input = read_upsert_mercenary_input(Some(mercenary_id.clone()), multipart).await?;
    let mercenary = state
        .manager
        .update_mercenary(user_claims.id(), mercenary_id, input)
        .await?;

    Ok(Json(mercenary))
}

async fn read_upsert_mercenary_input(
    path_mercenary_id: Option<MercenaryId>,
    mut multipart: Multipart,
) -> Result<UpsertMercenaryInput, MercenaryError> {
    let has_path_mercenary_id = path_mercenary_id.is_some();
    let mercenary_id =
        path_mercenary_id.or_else(|| (!has_path_mercenary_id).then(gen_mercenaryid));
    let mut name = String::new();
    let mut subtitle = String::new();
    let mut description = String::new();
    let mut style = String::new();
    let mut temper = String::new();
    let mut banner = None;
    let mut icon = None;
    let mut passive_script = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| MercenaryError::Invalid(error.to_string()))?
    {
        let field_name = field.name().unwrap_or_default().to_string();

        match field_name.as_str() {
            "id" | "mercenary_id" => {
                let _ = field
                    .text()
                    .await
                    .map_err(|error| MercenaryError::Invalid(error.to_string()))?;
            }
            "name" => {
                name = field
                    .text()
                    .await
                    .map_err(|error| MercenaryError::Invalid(error.to_string()))?;
            }
            "subtitle" => {
                subtitle = field
                    .text()
                    .await
                    .map_err(|error| MercenaryError::Invalid(error.to_string()))?;
            }
            "description" => {
                description = field
                    .text()
                    .await
                    .map_err(|error| MercenaryError::Invalid(error.to_string()))?;
            }
            "style" => {
                style = field
                    .text()
                    .await
                    .map_err(|error| MercenaryError::Invalid(error.to_string()))?;
            }
            "temper" => {
                temper = field
                    .text()
                    .await
                    .map_err(|error| MercenaryError::Invalid(error.to_string()))?;
            }
            "banner" => {
                banner = Some(field_bytes(field, MAX_IMAGE_BYTES, "banner").await?);
            }
            "icon" => {
                icon = Some(field_bytes(field, MAX_IMAGE_BYTES, "icon").await?);
            }
            "passive_script" | "script" => {
                if let Some(file_name) = field.file_name()
                    && !file_name.ends_with(".lua")
                {
                    return Err(MercenaryError::Invalid(
                        "passive script must be a .lua file".to_string(),
                    ));
                }

                passive_script =
                    Some(field_bytes(field, MAX_SCRIPT_BYTES, "passive script").await?);
            }
            _ => {}
        }
    }

    Ok(UpsertMercenaryInput {
        mercenary_id: mercenary_id
            .ok_or_else(|| MercenaryError::Invalid("mercenary id is required".to_string()))?,
        name,
        subtitle,
        description,
        style,
        temper,
        banner,
        icon,
        passive_script,
    })
}

async fn field_bytes(
    field: axum::extract::multipart::Field<'_>,
    max_bytes: usize,
    label: &str,
) -> Result<Vec<u8>, MercenaryError> {
    let bytes = field
        .bytes()
        .await
        .map_err(|error| MercenaryError::Invalid(error.to_string()))?;

    if bytes.len() > max_bytes {
        return Err(MercenaryError::Invalid(format!(
            "{label} must be at most {max_bytes} bytes"
        )));
    }

    Ok(bytes.to_vec())
}
