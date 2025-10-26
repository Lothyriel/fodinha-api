use std::net::Ipv6Addr;

use axum::{Router, routing};
use tower_http::cors::{Any, CorsLayer};

use crate::{AppSettings, infra::*, services::manager::Manager};

pub async fn start(manager: Manager, settings: &AppSettings) {
    auth::JWT_KEY
        .set(settings.jwt_key.to_string())
        .expect("Should set jwt key value");

    let auth = axum::middleware::from_fn(auth::middleware);

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/game", routing::get(game::handler).layer(auth.clone()))
        .nest("/lobby", lobby::router().layer(auth))
        .nest("/auth", auth::router())
        .fallback(fallback_handler)
        .with_state(manager)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors);

    let address = (Ipv6Addr::UNSPECIFIED, 3000);

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("Expected to bind to network address");

    tracing::info!("Listening on {:?}", address);

    axum::serve(listener, app)
        .await
        .expect("Expected to start axum");
}
