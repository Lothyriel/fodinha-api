use axum::{
    Extension, Json, Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware,
    middleware::Next,
    response::IntoResponse,
    routing,
};
use chrono::{Duration, Utc};
use jsonwebtoken::{
    DecodingKey, EncodingKey, Header, TokenData, Validation,
    errors::Error,
    jwk::{Jwk, JwkSet},
};
use reqwest::header;
use serde_json::{Value, json};

use crate::{
    infra::{AnonymousUserClaims, AuthError, GoogleUserClaims, UserClaims},
    models::id::gen_playerid,
};

use super::{ApiState, models::*};

pub fn router(state: ApiState) -> Router<ApiState> {
    let auth = axum::middleware::from_fn_with_state(state, middleware);

    Router::new()
        .route("/google", routing::post(exchange_google_token))
        .route("/refresh", routing::post(refresh))
        .route("/signup", routing::post(sign_up))
        .route("/profile", routing::post(update).layer(auth))
        .layer(middleware::from_fn(
            crate::infra::telemetry::http_middleware,
        ))
}

const ISSUER: &str = "fodinha.loty.click";
const ACCESS_TOKEN_TTL_SECONDS: i64 = 60 * 60;
const MAX_NICKNAME_LENGTH: usize = 24;
const REFRESH_TOKEN_TTL_DAYS: i64 = 30;

pub async fn middleware(
    State(state): State<ApiState>,
    mut req: Request,
    next: Next,
) -> Result<impl IntoResponse, AuthError> {
    let token = get_token_from_req(&mut req)
        .await
        .ok_or(AuthError::TokenNotPresent)?;

    let claims =
        get_claims_from_token(token, &state.jwt_key, state.google_client_id.as_deref()).await?;

    req.extensions_mut().insert(claims);

    Ok(next.run(req).await)
}

async fn get_token_from_req(req: &mut Request) -> Option<&str> {
    get_token_from_headers(req.headers())
}

fn get_token_from_headers(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .and_then(|value| value.starts_with("Bearer ").then(|| &value[7..]))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AccessTokenClaimsDto {
    user: UserClaims,
    iss: String,
    exp: usize,
}

#[derive(serde::Deserialize)]
struct GoogleExchangeRequest {
    credential: String,
}

#[derive(serde::Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

async fn update(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    Json(params): Json<Value>,
) -> impl IntoResponse {
    if let Err(response) = validate_nickname(&params) {
        return response.into_response();
    }

    let user = match user_claims {
        UserClaims::Anonymous(claim) => UserClaims::Anonymous(AnonymousUserClaims {
            id: claim.id,
            data: params,
            role: claim.role,
        }),
        UserClaims::Google(claim) => UserClaims::Google(GoogleUserClaims {
            email: claim.email,
            name: claim.name,
            picture: claim.picture,
            nickname: params
                .get("nickname")
                .and_then(Value::as_str)
                .filter(|nickname| !nickname.is_empty())
                .map(ToOwned::to_owned),
            picture_override: params
                .get("picture")
                .and_then(Value::as_str)
                .filter(|picture| !picture.is_empty())
                .map(ToOwned::to_owned),
            role: claim.role,
        }),
    };

    let user = match state.manager.upsert_user(&user).await {
        Ok(user) => user,
        Err(e) => return e.into_response(),
    };

    let token = match issue_auth_session(&state, user).await {
        Ok(token) => token,
        Err(error) => return error.into_response(),
    };

    Json(token).into_response()
}

async fn sign_up(State(state): State<ApiState>, Json(params): Json<Value>) -> impl IntoResponse {
    if let Err(response) = validate_nickname(&params) {
        return response.into_response();
    }

    let claims = AnonymousUserClaims {
        id: gen_playerid(),
        data: params,
        role: Default::default(),
    };
    let user = UserClaims::Anonymous(claims.clone());

    let user = match state.manager.upsert_user(&user).await {
        Ok(user) => user,
        Err(e) => return e.into_response(),
    };

    let token = match issue_auth_session(&state, user).await {
        Ok(token) => token,
        Err(error) => return error.into_response(),
    };

    Json(token).into_response()
}

async fn exchange_google_token(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(params): Json<GoogleExchangeRequest>,
) -> impl IntoResponse {
    let claims = get_google_claims(&params.credential, state.google_client_id.as_deref()).await;

    let mut claims = match claims {
        Ok(claims) => claims,
        Err(error) => return error.into_response(),
    };

    if let Some(token) = get_token_from_headers(&headers)
        && let Ok(UserClaims::Anonymous(guest)) = get_access_token_claims(token, &state.jwt_key)
    {
        claims = merge_guest_profile(claims, &guest);
    }

    let claims = match state.manager.upsert_user(&claims).await {
        Ok(claims) => claims,
        Err(error) => return error.into_response(),
    };

    let token = match issue_auth_session(&state, claims).await {
        Ok(token) => token,
        Err(error) => return error.into_response(),
    };

    Json(token).into_response()
}

async fn refresh(
    State(state): State<ApiState>,
    Json(params): Json<RefreshRequest>,
) -> impl IntoResponse {
    let player_id = match state.manager.refresh_player_id(&params.refresh_token).await {
        Ok(Some(player_id)) => player_id,
        Ok(None) => return AuthError::InvalidRefreshToken.into_response(),
        Err(error) => return error.into_response(),
    };

    let user = match state.manager.user(&player_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return AuthError::InvalidRefreshToken.into_response(),
        Err(error) => return error.into_response(),
    };

    let token = match issue_auth_session(&state, user).await {
        Ok(token) => token,
        Err(error) => return error.into_response(),
    };

    Json(token).into_response()
}

fn validate_nickname(params: &Value) -> Result<(), (StatusCode, Json<Value>)> {
    let Some(nickname) = params.get("nickname").and_then(Value::as_str) else {
        return Ok(());
    };

    if nickname.chars().count() <= MAX_NICKNAME_LENGTH {
        return Ok(());
    }

    let body = Json(json!({
        "error": format!("Nickname must be at most {MAX_NICKNAME_LENGTH} characters")
    }));

    Err((StatusCode::BAD_REQUEST, body))
}

fn generate_access_token(claims: &UserClaims, jwt_key: &str) -> String {
    let claims = AccessTokenClaimsDto {
        user: claims.clone(),
        iss: ISSUER.to_string(),
        exp: (Utc::now() + Duration::seconds(ACCESS_TOKEN_TTL_SECONDS)).timestamp() as usize,
    };

    jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(jwt_key.as_bytes()),
    )
    .expect("Should encode JWT")
}

async fn issue_auth_session(
    state: &ApiState,
    user: UserClaims,
) -> Result<Auth, crate::services::ManagerError> {
    let refresh_token = nanoid::nanoid!(48);
    let expires_at = (Utc::now() + Duration::days(REFRESH_TOKEN_TTL_DAYS)).timestamp();

    state
        .manager
        .store_refresh_token(&user.id(), &refresh_token, expires_at)
        .await?;

    Ok(Auth {
        token: generate_access_token(&user, &state.jwt_key),
        refresh_token: Some(refresh_token),
    })
}

fn merge_guest_profile(mut google: UserClaims, guest: &AnonymousUserClaims) -> UserClaims {
    let UserClaims::Google(claims) = &mut google else {
        return google;
    };

    claims.nickname = guest
        .data
        .get("nickname")
        .and_then(Value::as_str)
        .filter(|nickname| !nickname.is_empty())
        .map(ToOwned::to_owned);
    claims.picture_override = guest
        .data
        .get("picture")
        .and_then(Value::as_str)
        .filter(|picture| !picture.is_empty())
        .map(ToOwned::to_owned);

    google
}

pub async fn get_claims_from_token(
    token: &str,
    jwt_key: &str,
    _google_client_id: Option<&str>,
) -> Result<UserClaims, AuthError> {
    get_access_token_claims(token, jwt_key)
}

fn get_access_token_claims(token: &str, jwt_key: &str) -> Result<UserClaims, AuthError> {
    let key = DecodingKey::from_secret(jwt_key.as_bytes());

    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.set_issuer(&[ISSUER]);

    let claims = jsonwebtoken::decode::<AccessTokenClaimsDto>(token, &key, &validation)?.claims;

    Ok(claims.user)
}

async fn get_google_claims(
    token: &str,
    google_client_id: Option<&str>,
) -> Result<UserClaims, AuthError> {
    let Some(google_client_id) = google_client_id else {
        return Err(AuthError::MissingGoogleClientId);
    };

    let header = jsonwebtoken::decode_header(token)?;
    let kid = header.kid.ok_or(AuthError::InvalidKid)?;
    let jwks = get_google_jwks().await?;
    let jwk = jwks.find(&kid).ok_or(AuthError::InvalidKid)?;
    let token_data = decode_google_claims(token, jwk, google_client_id)?;
    let claims = UserClaims::Google(token_data.claims);

    Ok(claims)
}

fn decode_google_claims(
    token: &str,
    jwk: &Jwk,
    google_client_id: &str,
) -> Result<TokenData<GoogleUserClaims>, Error> {
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);

    validation.set_issuer(&["https://accounts.google.com"]);
    validation.set_audience(&[google_client_id]);

    jsonwebtoken::decode::<GoogleUserClaims>(token, &DecodingKey::from_jwk(jwk)?, &validation)
}

async fn get_google_jwks() -> Result<JwkSet, reqwest::Error> {
    let response = reqwest::get("https://www.googleapis.com/oauth2/v3/certs").await?;

    response.json().await
}

impl IntoResponse for AuthError {
    fn into_response(self) -> axum::response::Response {
        let body = Json(json!({"error": self.to_string() }));

        (StatusCode::UNAUTHORIZED, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::merge_guest_profile;
    use crate::{
        infra::{AnonymousUserClaims, GoogleUserClaims, UserClaims},
        models::id::PlayerId,
    };

    #[test]
    fn merge_guest_profile_prefers_existing_guest_nickname_and_picture() {
        let google = UserClaims::Google(GoogleUserClaims {
            email: PlayerId("player@example.com".into()),
            name: "Google Name".to_string(),
            picture: "google-picture".to_string(),
            nickname: None,
            picture_override: None,
            role: Default::default(),
        });
        let guest = AnonymousUserClaims {
            id: PlayerId("guest-id".into()),
            data: serde_json::json!({
                "nickname": "Guest Hero",
                "picture": "guest-picture"
            }),
            role: Default::default(),
        };

        let merged = merge_guest_profile(google, &guest);

        let UserClaims::Google(merged) = merged else {
            panic!("Expected Google claims");
        };

        assert_eq!(merged.nickname.as_deref(), Some("Guest Hero"));
        assert_eq!(merged.picture_override.as_deref(), Some("guest-picture"));
        assert_eq!(merged.name, "Google Name");
        assert_eq!(merged.picture, "google-picture");
    }
}
