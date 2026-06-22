use std::sync::OnceLock;

use axum::{
    Extension, Json, Router, extract::Request, http::StatusCode, middleware::Next,
    response::IntoResponse, routing,
};
use jsonwebtoken::{
    DecodingKey, EncodingKey, Header, TokenData, Validation,
    errors::Error,
    jwk::{Jwk, JwkSet},
};
use reqwest::header;
use serde_json::{Value, json};

use crate::{
    infra::{AuthError, GoogleUserClaims, UserClaims},
    models::id::{PlayerId, gen_playerid},
    services::dispatcher::ManagerHandle,
};

use super::models::*;

pub fn router() -> Router<ManagerHandle> {
    let auth = axum::middleware::from_fn(middleware);

    Router::new()
        .route("/signup", routing::post(sign_up))
        .route("/profile", routing::post(update).layer(auth))
}

pub static JWT_KEY: OnceLock<String> = OnceLock::new();
const ISSUER: &str = "fodinha.loty.click";

pub async fn middleware(mut req: Request, next: Next) -> Result<impl IntoResponse, AuthError> {
    let token = get_token_from_req(&mut req)
        .await
        .ok_or(AuthError::TokenNotPresent)?;

    let claims = get_claims_from_token(token).await?;

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
    Extension(user_claims): Extension<UserClaims>,
    Json(params): Json<Value>,
) -> impl IntoResponse {
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

    let token = generate_token(params, claim.id).await;

    Json(token).into_response()
}

async fn sign_up(Json(params): Json<Value>) -> impl IntoResponse {
    let token = generate_token(params, gen_playerid()).await;

    Json(token)
}

async fn generate_token(data: Value, id: PlayerId) -> Auth {
    let claims = AnonymousUserClaimsDto {
        id,
        data,
        iss: ISSUER,
        exp: 10000000000,
    };

    let token = jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(get_key_bytes()),
    )
    .expect("Should encode JWT");

    Auth { token }
}

fn get_key_bytes() -> &'static [u8] {
    JWT_KEY.get().expect("JWT_KEY should be set").as_bytes()
}

pub async fn get_claims_from_token(token: &str) -> Result<UserClaims, AuthError> {
    match get_anonymous_claims(token) {
        Ok(c) => Ok(c),
        Err(_) => get_google_claims(token).await,
    }
}

fn get_anonymous_claims(token: &str) -> Result<UserClaims, AuthError> {
    let key = DecodingKey::from_secret(get_key_bytes());

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
