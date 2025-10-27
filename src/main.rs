use oh_hell::{AppSettings, Manager};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from(
            "debug,hyper=off,rustls=error,tungstenite=error,russh=debug",
        ))
        .with(tracing_subscriber::fmt::layer().with_line_number(true))
        .init();

    let settings = AppSettings::from_env().expect("to load env variables");

    let manager = Manager::from(&settings).await;

    tokio::select! {
        _ = oh_hell::api::start(manager.clone(), &settings) => {}
        _ = oh_hell::ssh::start(manager.clone(), &settings) => {}
    }
}
