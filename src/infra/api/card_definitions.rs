use axum::{
    Extension, Json, Router,
    extract::{Multipart, Path, State},
    middleware, routing,
};

use std::collections::HashMap;

use crate::{
    infra::{UserClaims, telemetry},
    models::id::{CardId, MercenaryId},
    services::{
        card_definitions::{
            CardDefinitionError, CreateCardDefinitionAssetInput,
            CreateCardDefinitionFromAssetInput, CreateCardDefinitionInput, CreatePowerDeckInput,
            UpdateCardDefinitionInput,
        },
        repositories::{
            card_decks::{CardDeckKind, CardDeckStatus},
            card_definitions::CardDefinitionKind,
        },
    },
};

use super::ApiState;

const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_SCRIPT_BYTES: usize = 128 * 1024;

pub fn cards_router() -> Router<ApiState> {
    Router::new()
        .route("/", routing::get(list_cards))
        .route("/", routing::post(create_card))
        .route("/{card_id}", routing::put(update_card))
        .route("/assets", routing::post(create_card_asset))
        .route("/from-asset", routing::post(create_card_from_asset))
        .layer(middleware::from_fn(telemetry::http_middleware))
}

pub fn decks_router() -> Router<ApiState> {
    Router::new()
        .route("/", routing::get(list_decks))
        .route("/", routing::post(create_deck))
        .layer(middleware::from_fn(telemetry::http_middleware))
}

async fn list_cards(
    State(state): State<ApiState>,
) -> Result<Json<Vec<crate::services::card_definitions::CardDefinitionResponse>>, CardDefinitionError>
{
    Ok(Json(state.manager.card_definitions().await?))
}

async fn create_card(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    multipart: Multipart,
) -> Result<Json<crate::services::card_definitions::CardDefinitionResponse>, CardDefinitionError> {
    let input = read_create_card_input(multipart).await?;
    let card = state
        .manager
        .create_card_definition(user_claims.id(), input)
        .await?;

    Ok(Json(card))
}

async fn update_card(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    Path(card_id): Path<CardId>,
    multipart: Multipart,
) -> Result<Json<crate::services::card_definitions::CardDefinitionResponse>, CardDefinitionError> {
    let input = read_update_card_input(multipart).await?;
    let card = state
        .manager
        .update_card_definition(user_claims.id(), card_id, input)
        .await?;

    Ok(Json(card))
}

async fn create_card_asset(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    multipart: Multipart,
) -> Result<Json<crate::services::card_definitions::CardDefinitionAssetResponse>, CardDefinitionError>
{
    let input = read_create_card_asset_input(multipart).await?;
    let asset = state
        .manager
        .create_card_definition_asset(user_claims.id(), input)
        .await?;

    Ok(Json(asset))
}

async fn create_card_from_asset(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    Json(body): Json<CreateCardFromAssetRequest>,
) -> Result<Json<crate::services::card_definitions::CardDefinitionResponse>, CardDefinitionError> {
    let card = state
        .manager
        .create_card_definition_from_asset(
            user_claims.id(),
            CreateCardDefinitionFromAssetInput {
                asset_id: body.asset_id,
                kind: body.kind.unwrap_or(CardDefinitionKind::Community),
                name: body.name,
                description: body.description.unwrap_or_default(),
            },
        )
        .await?;

    Ok(Json(card))
}

async fn list_decks(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
) -> Result<Json<Vec<crate::services::card_definitions::PowerDeckResponse>>, CardDefinitionError> {
    Ok(Json(state.manager.power_decks(&user_claims.id()).await?))
}

async fn create_deck(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    Json(body): Json<CreatePowerDeckRequest>,
) -> Result<Json<crate::services::card_definitions::PowerDeckResponse>, CardDefinitionError> {
    let deck = state
        .manager
        .create_power_deck(
            user_claims.id(),
            CreatePowerDeckInput {
                kind: body.kind.unwrap_or(CardDeckKind::Community),
                name: body.name,
                description: body.description.unwrap_or_default(),
                card_ids: body.card_ids,
                generic_card_ids: body.generic_card_ids.unwrap_or_default(),
                mercenary_card_ids: body.mercenary_card_ids.unwrap_or_default(),
                status: body.status,
            },
        )
        .await?;

    Ok(Json(deck))
}

#[derive(serde::Deserialize)]
struct CreatePowerDeckRequest {
    kind: Option<CardDeckKind>,
    status: Option<CardDeckStatus>,
    name: String,
    description: Option<String>,
    card_ids: Vec<CardId>,
    generic_card_ids: Option<Vec<CardId>>,
    mercenary_card_ids: Option<HashMap<MercenaryId, Vec<CardId>>>,
}

#[derive(serde::Deserialize)]
struct CreateCardFromAssetRequest {
    asset_id: CardId,
    kind: Option<CardDefinitionKind>,
    name: String,
    description: Option<String>,
}

async fn read_create_card_asset_input(
    mut multipart: Multipart,
) -> Result<CreateCardDefinitionAssetInput, CardDefinitionError> {
    let mut image = Vec::new();
    let mut script = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| CardDefinitionError::Invalid(error.to_string()))?
    {
        let field_name = field.name().unwrap_or_default().to_string();

        match field_name.as_str() {
            "image" => {
                image = field_bytes(field, MAX_IMAGE_BYTES, "image").await?;
            }
            "script" => {
                if let Some(file_name) = field.file_name()
                    && !file_name.ends_with(".lua")
                {
                    return Err(CardDefinitionError::Invalid(
                        "script must be a .lua file".to_string(),
                    ));
                }

                script = field_bytes(field, MAX_SCRIPT_BYTES, "script").await?;
            }
            _ => {}
        }
    }

    Ok(CreateCardDefinitionAssetInput { image, script })
}

async fn read_create_card_input(
    mut multipart: Multipart,
) -> Result<CreateCardDefinitionInput, CardDefinitionError> {
    let mut name = String::new();
    let mut description = String::new();
    let mut kind = CardDefinitionKind::Community;
    let mut image = Vec::new();
    let mut script = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| CardDefinitionError::Invalid(error.to_string()))?
    {
        let field_name = field.name().unwrap_or_default().to_string();

        match field_name.as_str() {
            "name" => {
                name = field
                    .text()
                    .await
                    .map_err(|error| CardDefinitionError::Invalid(error.to_string()))?;
            }
            "description" => {
                description = field
                    .text()
                    .await
                    .map_err(|error| CardDefinitionError::Invalid(error.to_string()))?;
            }
            "kind" => {
                let value = field
                    .text()
                    .await
                    .map_err(|error| CardDefinitionError::Invalid(error.to_string()))?;
                kind = value
                    .parse::<CardDefinitionKind>()
                    .map_err(CardDefinitionError::Invalid)?;
            }
            "image" => {
                image = field_bytes(field, MAX_IMAGE_BYTES, "image").await?;
            }
            "script" => {
                if let Some(file_name) = field.file_name()
                    && !file_name.ends_with(".lua")
                {
                    return Err(CardDefinitionError::Invalid(
                        "script must be a .lua file".to_string(),
                    ));
                }

                script = field_bytes(field, MAX_SCRIPT_BYTES, "script").await?;
            }
            _ => {}
        }
    }

    Ok(CreateCardDefinitionInput {
        kind,
        name,
        description,
        image,
        script,
    })
}

async fn read_update_card_input(
    mut multipart: Multipart,
) -> Result<UpdateCardDefinitionInput, CardDefinitionError> {
    let mut name = String::new();
    let mut description = String::new();
    let mut kind = None;
    let mut image = None;
    let mut script = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| CardDefinitionError::Invalid(error.to_string()))?
    {
        let field_name = field.name().unwrap_or_default().to_string();

        match field_name.as_str() {
            "name" => {
                name = field
                    .text()
                    .await
                    .map_err(|error| CardDefinitionError::Invalid(error.to_string()))?;
            }
            "description" => {
                description = field
                    .text()
                    .await
                    .map_err(|error| CardDefinitionError::Invalid(error.to_string()))?;
            }
            "kind" => {
                let value = field
                    .text()
                    .await
                    .map_err(|error| CardDefinitionError::Invalid(error.to_string()))?;
                kind = Some(
                    value
                        .parse::<CardDefinitionKind>()
                        .map_err(CardDefinitionError::Invalid)?,
                );
            }
            "image" => {
                image = Some(field_bytes(field, MAX_IMAGE_BYTES, "image").await?);
            }
            "script" => {
                if let Some(file_name) = field.file_name()
                    && !file_name.ends_with(".lua")
                {
                    return Err(CardDefinitionError::Invalid(
                        "script must be a .lua file".to_string(),
                    ));
                }

                script = Some(field_bytes(field, MAX_SCRIPT_BYTES, "script").await?);
            }
            _ => {}
        }
    }

    Ok(UpdateCardDefinitionInput {
        kind,
        name,
        description,
        image,
        script,
    })
}

async fn field_bytes(
    field: axum::extract::multipart::Field<'_>,
    max_bytes: usize,
    label: &str,
) -> Result<Vec<u8>, CardDefinitionError> {
    let bytes = field
        .bytes()
        .await
        .map_err(|error| CardDefinitionError::Invalid(error.to_string()))?;

    if bytes.len() > max_bytes {
        return Err(CardDefinitionError::Invalid(format!(
            "{label} must be at most {max_bytes} bytes"
        )));
    }

    Ok(bytes.to_vec())
}
