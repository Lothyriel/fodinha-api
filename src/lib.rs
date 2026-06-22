pub mod infra;
pub mod models;
pub mod services;

use config::{Config, ConfigError, Environment};

#[derive(Debug, serde::Deserialize, Default)]
pub struct AppSettings {
    pub jwt_key: String,
    pub mongo_conn_string: String,
    pub mongo_database: String,
    pub ssh_host_key: String,
    pub ssh_port: u16,
}

impl AppSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        dotenv::dotenv().ok();

        let cfg = Config::builder()
            .set_default("ssh_port", 2222)?
            .set_default("mongo_conn_string", "mongodb://localhost/?retryWrites=true")?
            .set_default("mongo_database", "oh_hell")?
            .add_source(Environment::default())
            .build()?;

        let settings = cfg.try_deserialize()?;

        Ok(settings)
    }
}
