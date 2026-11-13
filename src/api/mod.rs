//! HTTP layer: router construction. Handlers live in `routes`.

mod routes;

use axum::Router;
use axum::routing::get;

use crate::collector::SharedSnapshot;

#[derive(Clone)]
pub struct AppState {
    pub snapshot: SharedSnapshot,
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(routes::health))
        .route("/api/stats", get(routes::stats))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::Collector;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    async fn get_json(app: Router, uri: &str) -> (StatusCode, Value) {
        let res = app
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap())
    }

    fn state() -> AppState {
        AppState {
            snapshot: SharedSnapshot::default(),
        }
    }

    #[tokio::test]
    async fn health_is_public_and_minimal() {
        let (status, body) = get_json(build_router(state()), "/api/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!({ "status": "ok" }));
    }

    #[tokio::test]
    async fn stats_is_503_before_first_sample() {
        let (status, body) = get_json(build_router(state()), "/api/stats").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body["error"].is_string());
    }

    #[tokio::test]
    async fn stats_returns_expected_shape() {
        let st = state();
        st.snapshot.set(Collector::new().unwrap().sample());
        let (status, body) = get_json(build_router(st), "/api/stats").await;
        assert_eq!(status, StatusCode::OK);
        for key in ["collected_at_ms", "system", "cpu", "memory", "disks"] {
            assert!(body.get(key).is_some(), "missing {key}");
        }
        assert!(body["memory"]["total_bytes"].as_u64().unwrap() > 0);
        assert!(body["cpu"]["cores"].is_array());
    }
}
