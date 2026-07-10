use crate::models::id::PlayerId;

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "type", content = "data")]
pub enum UserClaims {
    Anonymous(AnonymousUserClaims),
    Google(GoogleUserClaims),
}

impl UserClaims {
    pub fn id(&self) -> PlayerId {
        match self {
            Self::Anonymous(a) => a.id.clone(),
            Self::Google(g) => g.email.clone(),
        }
    }
    pub fn role(&self) -> UserRole {
        match self {
            Self::Anonymous(a) => a.role,
            Self::Google(g) => g.role,
        }
    }
    pub fn is_admin(&self) -> bool {
        self.role().is_admin()
    }
    pub fn with_role(mut self, role: UserRole) -> Self {
        match &mut self {
            Self::Anonymous(a) => a.role = role,
            Self::Google(g) => g.role = role,
        }
        self
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "PascalCase")]
pub enum UserRole {
    #[serde(alias = "admin", alias = "ADMIN")]
    Admin,
    #[default]
    #[serde(alias = "player", alias = "PLAYER")]
    Player,
}

impl UserRole {
    pub fn is_admin(self) -> bool {
        matches!(self, Self::Admin)
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct AnonymousUserClaims {
    pub id: PlayerId,
    pub data: serde_json::Value,
    #[serde(default)]
    pub role: UserRole,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub struct GoogleUserClaims {
    pub email: PlayerId,
    pub name: String,
    pub picture: String,
    pub nickname: Option<String>,
    pub picture_override: Option<String>,
    #[serde(default)]
    pub role: UserRole,
}
