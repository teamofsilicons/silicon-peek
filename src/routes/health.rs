//! `GET /healthz` and `GET /readyz`.

use axum::{Json, extract::State, http::StatusCode};
use silicon_peek_client::api::{Health, Ready, ReadyChecks};

use crate::state::AppState;

/// Liveness: the process answers.
#[allow(clippy::unused_async)] // axum handlers are async by contract
pub(crate) async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok".to_owned(),
        service: "peek".to_owned(),
        version: silicon_peek_client::VERSION.to_owned(),
    })
}

/// Readiness: both databases answer and the ACCOUNTS app secret is configured.
/// Speech provider configuration is reported separately; missing provider keys
/// do not make the core server unready (speech degrades to text, §5.5).
pub(crate) async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<Ready>) {
    let db = match state.0.production_db.ping().await {
        Ok(()) => "ok".to_owned(),
        Err(_) => "error: database unavailable".to_owned(),
    };
    let config = &state.0.config;
    let accounts_config = if config.accounts.app_secret.is_some() {
        "ok"
    } else {
        "missing"
    };
    let checks = ReadyChecks {
        elevenlabs: if config.deepgram.api_key.is_some() {
            "configured"
        } else {
            "missing"
        }
        .to_owned(),
        openai: if config.openai.api_key.is_some() {
            "configured"
        } else {
            "missing"
        }
        .to_owned(),
        db: db.clone(),
        accounts_config: accounts_config.to_owned(),
        ting_config: "ok".to_owned(),
        deepgram: if config.deepgram.api_key.is_some() {
            "configured"
        } else {
            "missing"
        }
        .to_owned(),
    };
    let ready = db == "ok" && accounts_config == "ok";
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(Ready {
            status: if ready { "ready" } else { "not_ready" }.to_owned(),
            checks,
        }),
    )
}
