use crate::models::id::PlayerId;

pub mod api;

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
    pub id: PlayerId,
    pub data: serde_json::Value,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub struct GoogleUserClaims {
    pub email: PlayerId,
    pub name: String,
    pub picture: String,
    #[serde(default)]
    pub nickname: Option<String>,
    #[serde(default)]
    pub picture_override: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum AuthError {
    #[error("Auth token not found on the request headers")]
    TokenNotPresent,
    #[error("Invalid refresh token")]
    InvalidRefreshToken,
    #[error("Invalid KeyId ('kid') on token")]
    InvalidKid,
    #[error("Invalid token: ({0})")]
    JwtValidation(#[from] jsonwebtoken::errors::Error),
    #[error("Error during certificate retrieval: ({0})")]
    IO(#[from] reqwest::Error),
}
