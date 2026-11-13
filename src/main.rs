mod api;
mod auth;
mod collector;
mod config;

use std::net::SocketAddr;
use std::process::ExitCode;

use tokio::net::TcpListener;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = match config::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            error!("configuration error: {e}");
            return ExitCode::FAILURE;
        }
    };

    if !config.bind.ip().is_loopback() {
        warn!(
            "binding to non-loopback address {}: traffic is plain HTTP; \
             put a TLS proxy in front (see README)",
            config.bind
        );
    }

    let snapshot = match collector::spawn(config.refresh_interval).await {
        Ok(s) => s,
        Err(e) => {
            error!("cannot start metrics collector: {e}");
            return ExitCode::FAILURE;
        }
    };
    let state = api::AppState {
        snapshot,
        auth: auth::Auth::new(&config),
    };

    let listener = match TcpListener::bind(config.bind).await {
        Ok(l) => l,
        Err(e) => {
            error!("cannot bind {}: {e}", config.bind);
            return ExitCode::FAILURE;
        }
    };
    info!("listening on http://{}", config.bind);

    let served = axum::serve(
        listener,
        api::build_router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;
    if let Err(e) = served {
        error!("server error: {e}");
        return ExitCode::FAILURE;
    }
    info!("shut down cleanly");
    ExitCode::SUCCESS
}

async fn shutdown_signal() {
    if tokio::signal::ctrl_c().await.is_err() {
        // Could not install the handler; never resolve rather than exit immediately.
        std::future::pending::<()>().await;
    }
}
