#[derive(serde::Serialize, serde::Deserialize)]
pub struct Auth {
    pub token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}
