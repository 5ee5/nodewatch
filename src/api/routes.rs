use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
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
