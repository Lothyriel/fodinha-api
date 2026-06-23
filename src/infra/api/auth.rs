use axum::{
    Extension, Json, Router,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::IntoResponse,
    routing,
};
use jsonwebtoken::{
    DecodingKey, EncodingKey, Header, TokenData, Validation,
    errors::Error,
    jwk::{Jwk, JwkSet},
};
use reqwest::header;
use serde_json::{Value, json};

use crate::{
    infra::{AnonymousUserClaims, AuthError, GoogleUserClaims, UserClaims},
    models::id::{PlayerId, gen_playerid},
};

use super::{ApiState, models::*};

pub fn router(state: ApiState) -> Router<ApiState> {
    let auth = axum::middleware::from_fn_with_state(state, middleware);

    Router::new()
        .route("/signup", routing::post(sign_up))
        .route("/profile", routing::post(update).layer(auth))
}

const ISSUER: &str = "fodinha.loty.click";
const MAX_NICKNAME_LENGTH: usize = 24;

pub async fn middleware(
    State(state): State<ApiState>,
    mut req: Request,
    next: Next,
) -> Result<impl IntoResponse, AuthError> {
    let token = get_token_from_req(&mut req)
        .await
        .ok_or(AuthError::TokenNotPresent)?;

    let claims = get_claims_from_token(token, &state.jwt_key).await?;

    if let Err(e) = state.manager.upsert_user(&claims).await {
        tracing::error!("Error upserting authenticated user: {e}");
    }

    req.extensions_mut().insert(claims);

    Ok(next.run(req).await)
}

async fn get_token_from_req(req: &mut Request) -> Option<&str> {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .and_then(|value| value.starts_with("Bearer ").then(|| &value[7..]))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AnonymousUserClaimsDto {
    id: PlayerId,
    data: Value,
    iss: &'static str,
    exp: usize,
}

async fn update(
    State(state): State<ApiState>,
    Extension(user_claims): Extension<UserClaims>,
    Json(params): Json<Value>,
) -> impl IntoResponse {
    if let Err(response) = validate_nickname(&params) {
        return response;
    }

    let claim = match user_claims {
        UserClaims::Anonymous(c) => c,
        UserClaims::Google(_) => {
            let response = (
                StatusCode::NOT_IMPLEMENTED,
                "Google claim not supported for now...",
            );
            return response.into_response();
        }
    };

    let claims = AnonymousUserClaims {
        id: claim.id,
        data: params,
    };
    let user = UserClaims::Anonymous(claims.clone());

    if let Err(e) = state.manager.upsert_user(&user).await {
        return e.into_response();
    }

    let token = generate_token(&claims, &state.jwt_key);

    Json(token).into_response()
}

async fn sign_up(State(state): State<ApiState>, Json(params): Json<Value>) -> impl IntoResponse {
    if let Err(response) = validate_nickname(&params) {
        return response;
    }

    let claims = AnonymousUserClaims {
        id: gen_playerid(),
        data: params,
    };
    let user = UserClaims::Anonymous(claims.clone());

    if let Err(e) = state.manager.upsert_user(&user).await {
        return e.into_response();
    }

    let token = generate_token(&claims, &state.jwt_key);

    Json(token).into_response()
}

fn validate_nickname(params: &Value) -> Result<(), axum::response::Response> {
    let Some(nickname) = params.get("nickname").and_then(Value::as_str) else {
        return Ok(());
    };

    if nickname.chars().count() <= MAX_NICKNAME_LENGTH {
        return Ok(());
    }

    let body = Json(json!({
        "error": format!("Nickname must be at most {MAX_NICKNAME_LENGTH} characters")
    }));

    Err((StatusCode::BAD_REQUEST, body).into_response())
}

fn generate_token(claim: &AnonymousUserClaims, jwt_key: &str) -> Auth {
    let claims = AnonymousUserClaimsDto {
        id: claim.id.clone(),
        data: claim.data.clone(),
        iss: ISSUER,
        exp: 10000000000,
    };

    let token = jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(jwt_key.as_bytes()),
    )
    .expect("Should encode JWT");

    Auth { token }
}

pub async fn get_claims_from_token(token: &str, jwt_key: &str) -> Result<UserClaims, AuthError> {
    match get_anonymous_claims(token, jwt_key) {
        Ok(c) => Ok(c),
        Err(_) => get_google_claims(token).await,
    }
}

fn get_anonymous_claims(token: &str, jwt_key: &str) -> Result<UserClaims, AuthError> {
    let key = DecodingKey::from_secret(jwt_key.as_bytes());

    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);

    validation.validate_exp = false;
    validation.set_issuer(&[ISSUER]);

    let claims = jsonwebtoken::decode(token, &key, &validation)?.claims;

    Ok(UserClaims::Anonymous(claims))
}

async fn get_google_claims(token: &str) -> Result<UserClaims, AuthError> {
    let header = jsonwebtoken::decode_header(token)?;
    let kid = header.kid.ok_or(AuthError::InvalidKid)?;
    let jwks = get_google_jwks().await?;
    let jwk = jwks.find(&kid).ok_or(AuthError::InvalidKid)?;
    let token_data = decode_google_claims(token, jwk)?;
    let claims = UserClaims::Google(token_data.claims);

    Ok(claims)
}

fn decode_google_claims(token: &str, jwk: &Jwk) -> Result<TokenData<GoogleUserClaims>, Error> {
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);

    validation.set_issuer(&["https://accounts.google.com"]);

    // TODO set google audience
    // TODO set /.well-known
    validation.set_audience(&[
        "824653628296-ahr9jr3aqgr367mul4p359dj4plsl67a.apps.googleusercontent.com",
    ]);

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
