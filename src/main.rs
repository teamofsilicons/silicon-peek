//! `peek-server`: the Peek backend (BLUEPRINT §5.2).
//!
//! Configuration comes only from the environment (a local `.env` is loaded
//! for development). Logs are JSON lines on stdout. SIGTERM and Ctrl-C shut
//! the server down gracefully: in-flight requests finish, then telemetry is
//! flushed.
#![forbid(unsafe_code)]

use std::{net::SocketAddr, process::ExitCode, time::Duration};

use silicon_peek::{app, config::Config, state::AppState, telemetry};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

const MAINTENANCE_EVERY: Duration = Duration::from_secs(300);

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,silicon_peek=info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_target(true),
        )
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "cannot listen for Ctrl-C");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                tracing::error!(error = %e, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutdown requested; finishing in-flight requests");
}

async fn run(config: Config) -> ExitCode {
    for warning in &config.warnings {
        tracing::warn!("{warning}");
    }
    let bind = config.bind;
    let sink = telemetry::backend_sink(&config.telemetry);
    if config.telemetry.enabled && sink.is_none() {
        tracing::warn!("backend telemetry is off: PEEK_BACKEND_TABLE_KEY is missing or invalid");
    }
    if config.iam.app_secret.is_none() {
        tracing::warn!(
            "PEEK_IAM_APP_SECRET is empty: /readyz reports iam_config missing and IAM routes answer 503 iam_misconfigured until it is set"
        );
    }
    let state = match AppState::new(config, sink.clone()) {
        Ok(state) => state,
        Err(e) => {
            tracing::error!(error = %e, "peek-server cannot start");
            eprintln!("peek-server cannot start: {e}");
            return ExitCode::from(1);
        }
    };
    let listener = match tokio::net::TcpListener::bind(bind).await {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!(error = %e, %bind, "cannot bind");
            eprintln!("peek-server cannot listen on {bind}: {e}; set PEEK_BIND to a free address");
            return ExitCode::from(1);
        }
    };
    let maintenance = {
        let state = state.clone();
        tokio::spawn(async move {
            state.prewarm_speech(true).await;
            let mut every = tokio::time::interval(MAINTENANCE_EVERY);
            loop {
                every.tick().await;
                state.maintenance().await;
                state.prewarm_speech(false).await;
            }
        })
    };
    tracing::info!(%bind, version = env!("CARGO_PKG_VERSION"), "peek-server listening");
    let served = axum::serve(
        listener,
        app::router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;
    maintenance.abort();
    if let Some(sink) = sink
        && !sink.flush()
    {
        tracing::warn!("some telemetry was not handed off before exit; it stays spooled");
    }
    match served {
        Ok(()) => {
            tracing::info!("peek-server stopped");
            ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!(error = %e, "the server failed");
            ExitCode::from(1)
        }
    }
}

fn main() -> ExitCode {
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Local development only: production has no .env in /var/lib/peek.
    let _ = dotenvy::dotenv();
    init_tracing();
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(e) => {
            tracing::error!(problems = e.0.len(), "invalid configuration");
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("peek-server cannot start its async runtime: {e}");
            return ExitCode::from(1);
        }
    };
    runtime.block_on(run(config))
}
