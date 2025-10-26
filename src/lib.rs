pub mod api;
mod infra;
mod models;
mod services;
pub mod ssh;

pub use services::manager::Manager;

pub struct AppSettings {
    pub jwt_key: String,
    pub mongo_conn_string: String,
    pub ssh_host_key: String,
}

impl AppSettings {
    pub fn from_env() -> Self {
        let mongo_conn_string = std::env::var("MONGO_CONN_STRING")
            .unwrap_or_else(|_| "mongodb://localhost/?retryWrites=true".to_string());

        let jwt_key = std::env::var("JWT_KEY").expect("JWT_KEY var is missing");

        let ssh_host_key = std::env::var("SSH_HOST_KEY").expect("SSH_HOST_KEY var is missing");

        Self {
            ssh_host_key,
            mongo_conn_string,
            jwt_key,
        }
    }
}
