use axum::{
    Router,
    body::Body,
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH},
    },
    response::Response,
    routing,
};

use crate::models::game::power_lua::{
    FODINHA_LUA_DEFINITIONS, MERCENARY_PASSIVE_TEMPLATE, POWER_CARD_TEMPLATE,
};

use super::ApiState;

const CONTENT_TYPE_VALUE: HeaderValue = HeaderValue::from_static("text/plain; charset=utf-8");
const CACHE_CONTROL_VALUE: HeaderValue = HeaderValue::from_static("public, max-age=300");

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/fodinha.d.lua", routing::get(fodinha_definitions))
        .route(
            "/templates/power-card.lua",
            routing::get(power_card_template),
        )
        .route(
            "/templates/mercenary-passive.lua",
            routing::get(mercenary_passive_template),
        )
}

async fn fodinha_definitions(headers: HeaderMap) -> Response {
    lua_text_response(headers, FODINHA_LUA_DEFINITIONS)
}

async fn power_card_template(headers: HeaderMap) -> Response {
    lua_text_response(headers, POWER_CARD_TEMPLATE)
}

async fn mercenary_passive_template(headers: HeaderMap) -> Response {
    lua_text_response(headers, MERCENARY_PASSIVE_TEMPLATE)
}

fn lua_text_response(request_headers: HeaderMap, content: &'static str) -> Response {
    let etag = content_etag(content);
    let not_modified = request_headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == etag);

    let mut response = if not_modified {
        Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .body(Body::empty())
            .expect("valid lua 304 response")
    } else {
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(content))
            .expect("valid lua text response")
    };

    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, CONTENT_TYPE_VALUE);
    headers.insert(CACHE_CONTROL, CACHE_CONTROL_VALUE);
    headers.insert(
        ETAG,
        HeaderValue::from_str(&etag).expect("content hash should produce a valid ETag"),
    );

    response
}

fn content_etag(content: &str) -> String {
    format!("\"{:016x}\"", fnv1a64(content.as_bytes()))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;

    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }

    hash
}
