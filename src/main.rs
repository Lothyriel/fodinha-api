use fodinha_api::{AppSettings, infra, services::manager::GameManager};
use tokio::sync::watch;

#[tokio::main]
async fn main() {
    let _telemetry_guard = infra::telemetry::init();

    let settings = AppSettings::from_env().expect("to load env variables");

    let handle = GameManager::start(&settings).await;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Expected to install SIGTERM handler");

        tokio::select! {
            _ = sigterm.recv() => tracing::info!("Received SIGTERM, initiating graceful shutdown"),
            _ = tokio::signal::ctrl_c() => tracing::info!("Received SIGINT, initiating graceful shutdown"),
        }

        let _ = shutdown_tx.send(true);
    });

    infra::api::start(handle, &settings, shutdown_rx).await;
}
