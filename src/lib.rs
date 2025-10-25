pub mod infra;
pub mod models;
pub mod services;

use std::net::Ipv4Addr;

use axum::{Router, routing};
use infra::auth::JWT_KEY;
use services::{
    manager::Manager,
    repositories::{game::GamesRepository, get_mongo_client},
};

use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::infra::game;

pub async fn start_app() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from(
            "debug,hyper=off,rustls=error,tungstenite=error",
        ))
        .with(tracing_subscriber::fmt::layer().with_line_number(true))
        .init();

    dotenv::dotenv().ok();

    JWT_KEY
        .set(std::env::var("JWT_KEY").expect("JWT_KEY var is missing"))
        .expect("Should set jwt key value");

    let db = get_mongo_client()
        .await
        .expect("Expected to create mongo client")
        .database("oh_hell");

    let manager = Manager::new(GamesRepository::new(&db));

    let auth = axum::middleware::from_fn(infra::auth::middleware);

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/game", routing::get(game::handler).layer(auth.clone()))
        .nest("/lobby", infra::lobby::router().layer(auth))
        .nest("/auth", infra::auth::router())
        .fallback(infra::fallback_handler)
        .with_state(manager)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors);

    let address = (Ipv4Addr::UNSPECIFIED, 3000);

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("Expected to bind to network address");

    tracing::info!("Listening on {:?}", address);

    axum::serve(listener, app)
        .await
        .expect("Expected to start axum");
}
