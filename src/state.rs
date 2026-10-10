//! Shared application state.

use std::{sync::Arc, time::Duration};

use silicon_peek_client::timestamp::unix_now;

use crate::{
    accounts::Accounts,
    config::Config,
    crypto::Sealer,
    db::Db,
    ratelimit::RateLimiter,
    store,
    telemetry::{EventSink, Telemetry},
};

/// Rate limits. The defaults are the blueprint's numbers (§5.2, §6.3);
/// tests lower them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Speech calls per actor per minute (120).
    pub speech_per_actor_per_minute: u32,
    /// Speech calls per account per minute (1200).
    pub speech_per_account_per_minute: u32,
    /// Bug reports per client IP per hour (10).
    pub reports_per_ip_per_hour: u32,
    /// Relayed telemetry events per minute, gateway-wide (6000).
    pub telemetry_events_per_minute: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            speech_per_actor_per_minute: 120,
            speech_per_account_per_minute: 1200,
            reports_per_ip_per_hour: 10,
            telemetry_events_per_minute: 6000,
        }
    }
}

/// Why the server could not be assembled.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    /// A database could not be opened or migrated.
    #[error("{0}")]
    Database(String),
    /// The ACCOUNTS client could not be built.
    #[error("cannot build the ACCOUNTS client: {0}")]
    Accounts(String),
    /// The webhook keyring is invalid.
    #[error("invalid ACCOUNTS webhook signing keys: {0}")]
    Webhook(String),
    /// The HTTP client could not be built.
    #[error("cannot build the HTTP client: {0}")]
    Http(String),
}

pub(crate) struct RateLimits {
    pub(crate) speech_actor: RateLimiter,
    pub(crate) speech_account: RateLimiter,
    pub(crate) reports_ip: RateLimiter,
    pub(crate) telemetry: RateLimiter,
}

/// Everything handlers share. Cheap to clone.
#[derive(Clone)]
pub struct AppState(pub(crate) Arc<Inner>);

pub(crate) struct Inner {
    pub(crate) config: Config,
    pub(crate) production_db: Db,
    pub(crate) accounts: Option<Arc<Accounts>>,
    /// Outbound HTTP (Ting, speech providers, GitHub, Silicon Apps, Space Station).
    /// Redirects are never followed; each call sets its own timeout.
    pub(crate) http: reqwest::Client,
    pub(crate) sealer: Sealer,
    pub(crate) telemetry: Telemetry,
    pub(crate) limits: RateLimits,
}

impl AppState {
    /// Opens both databases, builds the ACCOUNTS connector and the verifier.
    ///
    /// # Errors
    /// [`StartupError`] naming what could not be set up.
    pub fn new(config: Config, sink: Option<Arc<dyn EventSink>>) -> Result<Self, StartupError> {
        Self::with_limits(config, sink, Limits::default())
    }

    /// As [`AppState::new`], with explicit rate limits.
    ///
    /// # Errors
    /// [`StartupError`] naming what could not be set up.
    pub fn with_limits(
        config: Config,
        sink: Option<Arc<dyn EventSink>>,
        limits: Limits,
    ) -> Result<Self, StartupError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let production_db =
            Db::open(&config.database_path).map_err(|e| StartupError::Database(e.to_string()))?;
        let accounts = Accounts::new(&config.accounts)
            .map_err(|e| StartupError::Accounts(e.to_string()))?
            .map(Arc::new);
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("silicon-peek/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| StartupError::Http(e.to_string()))?;
        let sink = if config.telemetry.enabled { sink } else { None };
        let telemetry = Telemetry::new(sink, config.environment.as_str());
        let sealer = Sealer::new(&config.encryption_key.0);
        Ok(Self(Arc::new(Inner {
            production_db,
            accounts,
            http,
            sealer,
            telemetry,
            limits: RateLimits {
                speech_actor: RateLimiter::new(
                    limits.speech_per_actor_per_minute,
                    Duration::from_secs(60),
                ),
                speech_account: RateLimiter::new(
                    limits.speech_per_account_per_minute,
                    Duration::from_secs(60),
                ),
                reports_ip: RateLimiter::new(
                    limits.reports_per_ip_per_hour,
                    Duration::from_secs(3600),
                ),
                telemetry: RateLimiter::new(
                    limits.telemetry_events_per_minute,
                    Duration::from_secs(60),
                ),
            },
            config,
        })))
    }

    /// Periodic housekeeping: expires idempotency and webhook dedupe rows and
    /// reports testing-environment activity to Silicon Apps. `main` runs it
    /// every five minutes.
    pub async fn maintenance(&self) {
        for db in [&self.0.production_db] {
            match db.call(|conn| Ok(store::gc(conn, unix_now())?)).await {
                Ok(n) if n > 0 => {
                    tracing::info!(removed = n, "expired idempotency and webhook records");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(code = %e.code(), "housekeeping failed"),
            }
        }
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.0.config
    }
}
