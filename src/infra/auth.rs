use std::{net::SocketAddr, sync::OnceLock};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::IntoResponse,
    routing, Extension, Json, Router,
};
use jsonwebtoken::{
    errors::Error,
    jwk::{Jwk, JwkSet},
    DecodingKey, EncodingKey, Header, TokenData, Validation,
};
use serde_json::json;

use crate::services::{
    manager::{Manager, PlayerId},
    repositories::auth::LoginDto,
};

pub fn router() -> Router<Manager> {
    Router::new().route("/login", routing::post(login)).route(
        "/profile",
        routing::post(update_profile).layer(axum::middleware::from_fn(middleware)),
    )
}

pub static JWT_KEY: OnceLock<String> = OnceLock::new();

pub async fn middleware(mut req: Request, next: Next) -> Result<impl IntoResponse, AuthError> {
    let token = get_token_from_req(&mut req)
        .await
        .ok_or(AuthError::TokenNotPresent)?;

    let claims = get_claims_from_token(token).await?;

    req.extensions_mut().insert(claims);

    Ok(next.run(req).await)
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct ProfileParams {
    pub nickname: String,
    pub picture: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AnonymousUserClaimsDto {
    id: PlayerId,
    picture: String,
    name: String,
    iss: String,
    exp: usize,
}

async fn update_profile(
    State(manager): State<Manager>,
    ConnectInfo(who): ConnectInfo<SocketAddr>,
    Extension(user_claims): Extension<UserClaims>,
    Json(params): Json<ProfileParams>,
) -> Result<Json<TokenResponse>, impl IntoResponse> {
    let claim = match user_claims {
        UserClaims::Anonymous(c) => c,
        UserClaims::Google(_) => {
            let response = (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Google claim not supported for now...",
            );
            return Err(response.into_response());
        }
    };

    Ok(generate_token(params, manager, who, claim.id).await)
}

async fn login(
    State(manager): State<Manager>,
    ConnectInfo(who): ConnectInfo<SocketAddr>,
    Json(params): Json<ProfileParams>,
) -> Json<TokenResponse> {
    generate_token(params, manager, who, generate_username()).await
}

const ALPHABET: [char; 67] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I',
    'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '_', 'a',
    'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't',
    'u', 'v', 'w', 'x', 'y', 'z', '-', '.', '!', '*',
];

fn generate_username() -> PlayerId {
    nanoid::nanoid!(10, &ALPHABET).into()
}

async fn generate_token(
    params: ProfileParams,
    manager: Manager,
    who: SocketAddr,
    id: PlayerId,
) -> Json<TokenResponse> {
    let claims = AnonymousUserClaimsDto {
        id,
        picture: params.picture,
        name: params.nickname,
        iss: "https://fodinha.click".to_string(),
        exp: 10000000000,
    };

    let dto = LoginDto::new(claims.id.to_string(), who.to_string());
    tokio::spawn(save_login(manager, dto));

    let token = jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(get_key().as_bytes()),
    )
    .expect("Should encode JWT");

    Json(TokenResponse { token })
}

async fn save_login(manager: Manager, dto: LoginDto) {
    let insert = manager.auth_repo.insert_login(&dto).await;

    if let Err(e) = insert {
        tracing::error!("Error while saving login info | {e}")
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct TokenResponse {
    pub token: String,
}

fn get_key() -> &'static str {
    JWT_KEY.get().expect("JWT_KEY should be set")
}

pub async fn get_claims_from_token(token: &str) -> Result<UserClaims, AuthError> {
    match get_anonymous_claims(token) {
        Ok(c) => Ok(c),
        Err(_) => get_google_claims(token).await,
    }
}

async fn get_token_from_req(req: &mut Request) -> Option<&str> {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .and_then(|value| value.starts_with("Bearer ").then(|| &value[7..]))
}

fn get_anonymous_claims(token: &str) -> Result<UserClaims, AuthError> {
    let key = DecodingKey::from_secret(get_key().as_bytes());

    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);

    validation.validate_exp = false;

    let claims: AnonymousUserClaimsDto = jsonwebtoken::decode(token, &key, &validation)?.claims;

    let claims = AnonymousUserClaims {
        id: claims.id,
        picture: claims.picture,
        name: claims.name,
    };

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

#[derive(thiserror::Error, Debug)]
pub enum AuthError {
    #[error("Auth token not found on the request")]
    TokenNotPresent,
    #[error("Invalid KeyId ('kid') on token")]
    InvalidKid,
    #[error("Invalid token: ({0})")]
    JwtValidation(#[from] jsonwebtoken::errors::Error),
    #[error("Error during certificate retrieval: ({0})")]
    IO(#[from] reqwest::Error),
}

impl IntoResponse for AuthError {
    fn into_response(self) -> axum::response::Response {
        let body = Json(json!({"error": self.to_string() }));

        (StatusCode::UNAUTHORIZED, body).into_response()
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "type", content = "data")]
pub enum UserClaims {
    Anonymous(AnonymousUserClaims),
    Google(GoogleUserClaims),
}

impl UserClaims {
    pub fn id(&self) -> PlayerId {
        match self {
            UserClaims::Anonymous(a) => a.id.clone(),
            UserClaims::Google(g) => g.email.clone(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct AnonymousUserClaims {
    id: PlayerId,
    picture: String,
    name: String,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub struct GoogleUserClaims {
    pub email: PlayerId,
    pub name: String,
    pub picture: String,
}
