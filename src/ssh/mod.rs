mod app;
mod backend;

use crate::{AppSettings, Manager, ssh::app::AppServer};

pub async fn start(_manager: Manager, settings: &AppSettings) {
    AppServer::new()
        .run(settings)
        .await
        .expect("Failed running server");
}

#[derive(thiserror::Error, Debug)]
pub enum SshError {
    #[error("{0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Russh(#[from] russh::Error),
}
