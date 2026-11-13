use axum::Json;
use serde_json::{Value, json};

/// Liveness probe. Deliberately public and free of host information.
pub async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
