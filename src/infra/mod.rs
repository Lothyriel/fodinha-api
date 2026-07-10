pub use fodinha_core::infra::{AnonymousUserClaims, GoogleUserClaims, UserClaims, UserRole};

pub mod api;
pub mod repositories;
pub mod telemetry;

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
    #[error("Google audience is not configured in this api")]
    MissingGoogleClientId,
}

#[cfg(test)]
mod tests {
    use super::{AnonymousUserClaims, GoogleUserClaims, UserRole};
    use crate::models::id::PlayerId;

    #[test]
    fn anonymous_user_claims_role_defaults_to_player_when_missing() {
        let claims: AnonymousUserClaims = serde_json::from_value(serde_json::json!({
            "id": "guest-id",
            "data": {
                "nickname": "Guest"
            }
        }))
        .expect("anonymous claims without role should deserialize");

        assert_eq!(claims.id, PlayerId("guest-id".into()));
        assert_eq!(claims.role, UserRole::Player);
    }

    #[test]
    fn google_user_claims_role_defaults_to_player_when_missing() {
        let claims: GoogleUserClaims = serde_json::from_value(serde_json::json!({
            "email": "player@example.com",
            "name": "Google Name",
            "picture": "google-picture"
        }))
        .expect("google claims without role should deserialize");

        assert_eq!(claims.email, PlayerId("player@example.com".into()));
        assert_eq!(claims.role, UserRole::Player);
    }
}
