pub mod infra;
pub mod services;

pub use fodinha_core::models;

use config::{Config, ConfigError, Environment};

#[derive(Debug, serde::Deserialize)]
pub struct AppSettings {
    pub jwt_key: String,
    pub google_client_id: Option<String>,
    pub mongo_conn_string: String,
    pub mongo_database: String,
    pub mongo_max_pool_size: u32,
    pub object_storage_access_key_id: String,
    pub object_storage_bucket: String,
    pub object_storage_endpoint: String,
    pub object_storage_force_path_style: bool,
    pub object_storage_public_base_url: Option<String>,
    pub object_storage_region: String,
    pub object_storage_secret_access_key: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            jwt_key: String::new(),
            google_client_id: None,
            mongo_conn_string: "mongodb://localhost/?retryWrites=true".to_string(),
            mongo_database: "oh_hell".to_string(),
            mongo_max_pool_size: 64,
            object_storage_access_key_id: "minioadmin".to_string(),
            object_storage_bucket: "fodinha-card-definitions".to_string(),
            object_storage_endpoint: "http://localhost:9000".to_string(),
            object_storage_force_path_style: true,
            object_storage_public_base_url: Some(
                "http://localhost:9000/fodinha-card-definitions".to_string(),
            ),
            object_storage_region: "auto".to_string(),
            object_storage_secret_access_key: "minioadmin".to_string(),
        }
    }
}

impl AppSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        dotenv::dotenv().ok();

        let cfg = Config::builder()
            .set_default("mongo_conn_string", "mongodb://localhost/?retryWrites=true")?
            .set_default("mongo_database", "oh_hell")?
            .set_default("mongo_max_pool_size", 64)?
            .set_default("object_storage_access_key_id", "minioadmin")?
            .set_default("object_storage_bucket", "fodinha-card-definitions")?
            .set_default("object_storage_endpoint", "http://localhost:9000")?
            .set_default("object_storage_force_path_style", true)?
            .set_default(
                "object_storage_public_base_url",
                "http://localhost:9000/fodinha-card-definitions",
            )?
            .set_default("object_storage_region", "auto")?
            .set_default("object_storage_secret_access_key", "minioadmin")?
            .add_source(Environment::default())
            .build()?;

        let settings = cfg.try_deserialize()?;

        Ok(settings)
    }
}
