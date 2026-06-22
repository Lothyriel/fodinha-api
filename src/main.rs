use oh_hell::{AppSettings, infra, services::manager::GameManager};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from(
            "debug,hyper=off,rustls=error,tungstenite=error,russh=info",
        ))
        .with(tracing_subscriber::fmt::layer().with_line_number(true))
        .init();

    let settings = AppSettings::from_env().expect("to load env variables");

    let handle = GameManager::start(&settings).await;

    infra::api::start(handle, &settings).await;
}
