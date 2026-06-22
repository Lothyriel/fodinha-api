mod app;
mod backend;

use crate::{AppSettings, services::dispatcher::ManagerHandle};

pub async fn start(_manager: ManagerHandle, settings: &AppSettings) {
    app::Server::new()
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
