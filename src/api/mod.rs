//! HTTP layer: router construction. Handlers live in `routes`.
//!
//! No CORS layer on purpose: the dashboard is same-origin, so emitting no CORS
//! headers is the strictest policy (browsers block cross-origin reads).

mod routes;

use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{HeaderValue, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};

use crate::auth::{self, Auth};
use crate::collector::SharedSnapshot;

/// Login bodies are tiny; anything bigger is rejected early.
const MAX_BODY_BYTES: usize = 4 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub snapshot: SharedSnapshot,
    pub auth: Arc<Auth>,
}

pub fn build_router(state: AppState) -> Router {
    // Unmatched paths are 404 for everyone: auth is a `route_layer`, so it only
    // runs for routes that exist.
    let protected = Router::new()
        .route("/", get(routes::index))
        .route("/app.js", get(routes::app_js))
        .route("/api/stats", get(routes::stats))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    let public = Router::new()
        .route("/login", get(routes::login_page))
        .route("/login.js", get(routes::login_js))
        .route("/style.css", get(routes::style_css))
        .route("/api/health", get(routes::health))
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout));

    public
        .merge(protected)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

async fn security_headers(req: Request, next: Next) -> Response {
    let is_api = req.uri().path().starts_with("/api/");
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'self'; frame-ancestors 'none'"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if is_api {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use super::*;
    use crate::collector::Collector;
    use crate::config::Config;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn config() -> Config {
        let vars: HashMap<&str, &str> = HashMap::from([("NODEWATCH_TOKEN", TOKEN)]);
        Config::from_lookup(
            |k| vars.get(k).map(|v| v.to_string()),
            |_| Err(std::io::Error::other("no files in tests")),
        )
        .unwrap()
    }

    fn app_with(cfg: &Config, with_snapshot: bool) -> Router {
        let snapshot = SharedSnapshot::default();
        if with_snapshot {
            snapshot.set(Collector::new(Duration::from_secs(2)).unwrap().sample());
        }
        build_router(AppState {
            snapshot,
            auth: Auth::new(cfg),
        })
    }

    fn app() -> Router {
        app_with(&config(), true)
    }

    async fn send(app: &Router, req: Request<Body>) -> Response {
        app.clone().oneshot(req).await.unwrap()
    }

    async fn text(res: Response) -> String {
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    async fn json_of(res: Response) -> Value {
        serde_json::from_str(&text(res).await).unwrap()
    }

    fn get_req(uri: &str) -> axum::http::request::Builder {
        Request::get(uri)
    }

    async fn login(app: &Router, token: &str) -> Response {
        send(
            app,
            Request::post("/api/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"token":"{token}"}}"#)))
                .unwrap(),
        )
        .await
    }

    /// `nw_session=<id>` pair from a login response.
    fn cookie_pair(res: &Response) -> String {
        let set = res
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        set.split(';').next().unwrap().to_owned()
    }

    fn set_cookie(res: &Response) -> String {
        res.headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    }

    async fn stats_with_cookie(app: &Router, cookie: &str) -> StatusCode {
        send(
            app,
            get_req("/api/stats")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .status()
    }

    #[tokio::test]
    async fn health_is_public_and_minimal() {
        let res = send(&app(), get_req("/api/health").body(Body::empty()).unwrap()).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(json_of(res).await, serde_json::json!({ "status": "ok" }));
    }

    #[tokio::test]
    async fn stats_requires_authentication() {
        let res = send(&app(), get_req("/api/stats").body(Body::empty()).unwrap()).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert!(res.headers().contains_key(header::WWW_AUTHENTICATE));
        assert!(json_of(res).await["error"].is_string());
    }

    #[tokio::test]
    async fn stats_accepts_bearer_token_and_has_expected_shape() {
        let res = send(
            &app(),
            get_req("/api/stats")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = json_of(res).await;
        for key in [
            "collected_at_ms",
            "interval_secs",
            "system",
            "cpu",
            "memory",
            "disks",
        ] {
            assert!(body.get(key).is_some(), "missing {key}");
        }
        assert!(body["memory"]["total_bytes"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn wrong_bearer_token_is_rejected() {
        let res = send(
            &app(),
            get_req("/api/stats")
                .header(header::AUTHORIZATION, "Bearer wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authorized_stats_is_503_before_first_sample() {
        let app = app_with(&config(), false);
        let res = send(
            &app,
            get_req("/api/stats")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn bad_login_is_401_without_cookie_or_token_echo() {
        let res = login(&app(), "definitely-not-the-token").await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert!(!res.headers().contains_key(header::SET_COOKIE));
        assert!(!text(res).await.contains("definitely-not-the-token"));
    }

    #[tokio::test]
    async fn login_sets_hardened_cookie_that_then_authorizes() {
        let app = app();
        let res = login(&app, TOKEN).await;
        assert_eq!(res.status(), StatusCode::OK);

        let set = set_cookie(&res);
        for flag in ["HttpOnly", "SameSite=Strict", "Path=/", "Max-Age=43200"] {
            assert!(set.contains(flag), "{flag} missing in {set}");
        }
        assert!(!set.contains("Secure"), "Secure must be opt-in: {set}");
        assert!(set.starts_with("nw_session="));

        // The access token must not leak through headers or body.
        assert!(!format!("{:?}", res.headers()).contains(TOKEN));
        let cookie = cookie_pair(&res);
        assert!(!text(res).await.contains(TOKEN));

        assert_eq!(stats_with_cookie(&app, &cookie).await, StatusCode::OK);
        assert_eq!(
            stats_with_cookie(&app, "nw_session=forged").await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn secure_flag_follows_config() {
        let mut cfg = config();
        cfg.cookie_secure = true;
        let res = login(&app_with(&cfg, true), TOKEN).await;
        assert!(set_cookie(&res).ends_with("; Secure"));
    }

    #[tokio::test]
    async fn login_requires_json_content_type() {
        let res = send(
            &app(),
            Request::post("/api/login")
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from(format!(r#"{{"token":"{TOKEN}"}}"#)))
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert!(!res.headers().contains_key(header::SET_COOKIE));
    }

    #[tokio::test]
    async fn oversized_body_is_rejected() {
        let big = "a".repeat(8 * 1024);
        let res = login(&app(), &big).await;
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn logout_invalidates_session_and_clears_cookie() {
        let app = app();
        let cookie = cookie_pair(&login(&app, TOKEN).await);
        assert_eq!(stats_with_cookie(&app, &cookie).await, StatusCode::OK);

        let res = send(
            &app,
            Request::post("/api/logout")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(set_cookie(&res).contains("Max-Age=0"));

        assert_eq!(
            stats_with_cookie(&app, &cookie).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn logout_requires_json_content_type() {
        let res = send(
            &app(),
            Request::post("/api/logout").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn expired_session_is_rejected() {
        let mut cfg = config();
        cfg.session_ttl = Duration::from_millis(20);
        let app = app_with(&cfg, true);
        let cookie = cookie_pair(&login(&app, TOKEN).await);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            stats_with_cookie(&app, &cookie).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn pages_redirect_to_login_without_a_session() {
        for path in ["/", "/app.js"] {
            let res = send(&app(), get_req(path).body(Body::empty()).unwrap()).await;
            assert_eq!(res.status(), StatusCode::SEE_OTHER, "{path}");
            assert_eq!(res.headers()[header::LOCATION], "/login");
        }
    }

    #[tokio::test]
    async fn login_assets_are_public_with_correct_types() {
        for (path, ctype) in [
            ("/login", "text/html"),
            ("/login.js", "text/javascript"),
            ("/style.css", "text/css"),
        ] {
            let res = send(&app(), get_req(path).body(Body::empty()).unwrap()).await;
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert!(
                res.headers()[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .starts_with(ctype),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn dashboard_is_served_after_login() {
        let app = app();
        let cookie = cookie_pair(&login(&app, TOKEN).await);
        for (path, needle) in [("/", "NodeWatch"), ("/app.js", "/api/stats")] {
            let res = send(
                &app,
                get_req(path)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert!(text(res).await.contains(needle), "{path}");
        }
    }

    #[test]
    fn frontend_is_csp_safe() {
        // CSP is `default-src 'self'`: no inline scripts, handlers or style attributes.
        for (name, src) in [
            ("index.html", include_str!("../../static/index.html")),
            ("login.html", include_str!("../../static/login.html")),
        ] {
            assert!(!src.contains("style=\""), "{name}: inline style");
            assert!(!src.contains(" onclick="), "{name}: inline handler");
            assert!(!src.contains("<script>"), "{name}: inline script");
            assert!(
                !src.contains("http://") && !src.contains("https://"),
                "{name}: external ref"
            );
        }
        for (name, src) in [
            ("app.js", include_str!("../../static/app.js")),
            ("login.js", include_str!("../../static/login.js")),
        ] {
            assert!(!src.contains("innerHTML"), "{name}: innerHTML");
        }
    }

    #[tokio::test]
    async fn unknown_paths_are_404_not_redirects() {
        let res = send(&app(), get_req("/nope").body(Body::empty()).unwrap()).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn no_cors_headers_and_security_headers_present() {
        let res = send(&app(), get_req("/api/health").body(Body::empty()).unwrap()).await;
        let h = res.headers();
        assert!(!h.keys().any(|k| k.as_str().starts_with("access-control-")));
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(h[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(h[header::CACHE_CONTROL], "no-store");
        assert!(
            h[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("frame-ancestors 'none'")
        );
    }
}
