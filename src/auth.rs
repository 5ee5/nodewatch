//! Authentication: access token -> server-side session cookie.
//!
//! - Browsers POST the access token once to `/api/login` and receive an opaque
//!   random session id in an HttpOnly cookie. The token itself is never stored
//!   in a cookie or echoed back.
//! - Scripts may instead send `Authorization: Bearer <token>` on every request.
//! - Sessions live in memory only; a restart logs everyone out.

use std::collections::HashMap;
use std::fs::File;
use std::hint::black_box;
use std::io::{self, Read};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Extension;
use axum::Json;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::json;
use tracing::{error, warn};

use crate::api::AppState;
use crate::config::{Config, Secret};

pub const COOKIE_NAME: &str = "nw_session";
const MAX_SESSIONS: usize = 1024;
const FAILED_LOGIN_DELAY: Duration = Duration::from_millis(500);

pub struct Auth {
    token: Secret,
    cookie_secure: bool,
    ttl: Duration,
    sessions: Mutex<HashMap<String, Instant>>,
}

impl Auth {
    pub fn new(config: &Config) -> Arc<Self> {
        Arc::new(Self {
            token: config.token.clone(),
            cookie_secure: config.cookie_secure,
            ttl: config.session_ttl,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    fn verify_token(&self, candidate: &str) -> bool {
        ct_eq(self.token.expose().as_bytes(), candidate.as_bytes())
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, Instant>> {
        // The map stays consistent even if a holder panicked.
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Creates a session and returns its id.
    fn create_session(&self) -> io::Result<String> {
        let id = random_id()?;
        let now = Instant::now();
        let mut sessions = self.sessions();
        sessions.retain(|_, expiry| *expiry > now);
        if sessions.len() >= MAX_SESSIONS {
            // Evict the session closest to expiry rather than refusing logins.
            if let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, e)| **e)
                .map(|(k, _)| k.clone())
            {
                sessions.remove(&oldest);
            }
        }
        sessions.insert(id.clone(), now + self.ttl);
        Ok(id)
    }

    fn session_valid(&self, id: &str) -> bool {
        let mut sessions = self.sessions();
        match sessions.get(id) {
            Some(expiry) if *expiry > Instant::now() => true,
            Some(_) => {
                sessions.remove(id);
                false
            }
            None => false,
        }
    }

    fn revoke(&self, id: &str) {
        self.sessions().remove(id);
    }

    fn set_cookie(&self, id: &str) -> String {
        self.cookie(id, self.ttl.as_secs())
    }

    fn clear_cookie(&self) -> String {
        self.cookie("", 0)
    }

    fn cookie(&self, value: &str, max_age: u64) -> String {
        let secure = if self.cookie_secure { "; Secure" } else { "" };
        format!(
            "{COOKIE_NAME}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age}{secure}"
        )
    }
}

/// Constant-time (best effort) comparison that also hides length differences.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= usize::from(x ^ y);
    }
    black_box(diff) == 0
}

/// 256 random bits from the kernel CSPRNG, hex encoded.
fn random_id() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn session_cookie_value(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value)
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let (scheme, token) = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.trim()
                .to_ascii_lowercase()
                .starts_with("application/json")
        })
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn with_cookie(cookie: &str, body: Response) -> Response {
    match HeaderValue::from_str(cookie) {
        Ok(value) => {
            let mut res = body;
            res.headers_mut().append(header::SET_COOKIE, value);
            res
        }
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    }
}

/// Rejects unauthenticated requests: JSON 401 for the API, redirect for pages.
pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let authorized = bearer_token(headers).is_some_and(|t| state.auth.verify_token(t))
        || session_cookie_value(headers).is_some_and(|id| state.auth.session_valid(id));
    if authorized {
        return next.run(req).await;
    }
    if req.uri().path().starts_with("/api/") {
        let mut res = error_response(StatusCode::UNAUTHORIZED, "authentication required");
        res.headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        res
    } else {
        Redirect::to("/login").into_response()
    }
}

#[derive(Deserialize)]
pub struct LoginRequest {
    token: String,
}

/// `POST /api/login` with `{"token": "..."}`. `Json` already rejects any
/// content type other than `application/json`, which blocks cross-site form posts.
pub async fn login(
    State(state): State<AppState>,
    // `ConnectInfo` is a request extension; `Extension` supports `Option`, so
    // the handler also works in tests that have no real socket.
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(req): Json<LoginRequest>,
) -> Response {
    if !state.auth.verify_token(&req.token) {
        warn!(peer = ?peer.map(|Extension(ConnectInfo(addr))| addr.ip()), "failed login attempt");
        tokio::time::sleep(FAILED_LOGIN_DELAY).await;
        return error_response(StatusCode::UNAUTHORIZED, "invalid token");
    }
    match state.auth.create_session() {
        Ok(id) => with_cookie(
            &state.auth.set_cookie(&id),
            Json(json!({ "status": "ok" })).into_response(),
        ),
        Err(e) => {
            error!("cannot create session: {e}");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

/// `POST /api/logout`: idempotent; always clears the cookie.
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !is_json(&headers) {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "content-type must be application/json",
        );
    }
    if let Some(id) = session_cookie_value(&headers) {
        state.auth.revoke(id);
    }
    with_cookie(
        &state.auth.clear_cookie(),
        Json(json!({ "status": "ok" })).into_response(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_basics() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(!ct_eq(b"abc", b"abc\0"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn session_ids_are_64_hex_and_unique() {
        let a = random_id().unwrap();
        let b = random_id().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn parses_session_cookie_among_others() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; nw_session=xyz; b=2"),
        );
        assert_eq!(session_cookie_value(&h), Some("xyz"));
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("not_nw_session=xyz"),
        );
        assert_eq!(session_cookie_value(&h), None);
    }

    #[test]
    fn parses_bearer_case_insensitively() {
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("bearer tok"),
        );
        assert_eq!(bearer_token(&h), Some("tok"));
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic tok"));
        assert_eq!(bearer_token(&h), None);
    }
}
