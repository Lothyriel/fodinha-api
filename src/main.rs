use oh_hell::{AppSettings, infra, services::manager::GameManager};

#[tokio::main]
async fn main() {
    let _telemetry_guard = infra::telemetry::init();

    let settings = AppSettings::from_env().expect("to load env variables");

    let handle = GameManager::start(&settings).await;

    infra::api::start(handle, &settings).await;
}
