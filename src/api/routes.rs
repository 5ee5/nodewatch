use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use super::AppState;

/// Liveness probe. Deliberately public and free of host information.
pub async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// Latest metrics snapshot. Cheap: clones an `Arc`, never touches the OS.
pub async fn stats(State(state): State<AppState>) -> Response {
    match state.snapshot.get() {
        Some(snapshot) => Json(snapshot.as_ref()).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "metrics not yet available" })),
        )
            .into_response(),
    }
}

// Frontend assets are compiled into the binary: one file to deploy and no
// filesystem serving, hence no path-traversal surface.

fn asset(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            // Always revalidate so an upgraded binary is picked up immediately.
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

const HTML: &str = "text/html; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";

pub async fn index() -> Response {
    asset(HTML, include_str!("../../static/index.html"))
}

pub async fn app_js() -> Response {
    asset(JS, include_str!("../../static/app.js"))
}

pub async fn login_page() -> Response {
    asset(HTML, include_str!("../../static/login.html"))
}

pub async fn login_js() -> Response {
    asset(JS, include_str!("../../static/login.js"))
}

pub async fn style_css() -> Response {
    asset(CSS, include_str!("../../static/style.css"))
}
