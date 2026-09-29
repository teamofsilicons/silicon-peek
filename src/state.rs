//! Shared application state.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use silicon_iam_client::{WebhookSecret, WebhookSecretKeyring, WebhookVerifier, models};
use silicon_peek_client::timestamp::unix_now;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    config::Config,
    crypto::Sealer,
    db::Db,
    iam::{IamConnector, SdkConnector},
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
    /// Speech calls per org per minute (1200).
    pub speech_per_org_per_minute: u32,
    /// Bug reports per client IP per hour (10).
    pub reports_per_ip_per_hour: u32,
    /// Relayed telemetry events per minute, gateway-wide (6000).
    pub telemetry_events_per_minute: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            speech_per_actor_per_minute: 120,
            speech_per_org_per_minute: 1200,
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
    /// The IAM client could not be built.
    #[error("cannot build the IAM client: {0}")]
    Iam(String),
    /// The webhook keyring is invalid.
    #[error("invalid IAM webhook signing keys: {0}")]
    Webhook(String),
    /// The HTTP client could not be built.
    #[error("cannot build the HTTP client: {0}")]
    Http(String),
}

pub(crate) struct RateLimits {
    pub(crate) speech_actor: RateLimiter,
    pub(crate) speech_org: RateLimiter,
    pub(crate) reports_ip: RateLimiter,
    pub(crate) telemetry: RateLimiter,
}

/// Everything handlers share. Cheap to clone.
#[derive(Clone)]
pub struct AppState(pub(crate) Arc<Inner>);

pub(crate) struct Inner {
    pub(crate) config: Config,
    pub(crate) production_db: Db,
    pub(crate) testing_db: Db,
    pub(crate) iam: Arc<dyn IamConnector>,
    /// Outbound HTTP (Ting, speech providers, GitHub, Honeycomb, Space Station).
    /// Redirects are never followed; each call sets its own timeout.
    pub(crate) http: reqwest::Client,
    pub(crate) sealer: Sealer,
    pub(crate) webhooks: WebhookVerifier,
    pub(crate) telemetry: Telemetry,
    pub(crate) limits: RateLimits,
    /// OBO catalogs by context, cached for five minutes (§3.4).
    pub(crate) catalogs: Mutex<HashMap<String, (Instant, models::OboEndpointCatalog)>>,
    /// Per-environment request fences: requests hold a read guard, lifecycle
    /// effects (clean, purge) take the write guard.
    env_locks: Mutex<HashMap<Uuid, Arc<RwLock<()>>>>,
}

impl AppState {
    /// Opens both databases, builds the IAM connector and the verifier.
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
        let testing_db = Db::open(&config.test_database_path)
            .map_err(|e| StartupError::Database(e.to_string()))?;
        let iam = SdkConnector::new(&config.iam).map_err(|e| StartupError::Iam(e.to_string()))?;
        let mut keys = config.iam.webhook_keys.iter();
        let (version, secret) = keys
            .next()
            .ok_or_else(|| StartupError::Webhook("no webhook signing secret".to_owned()))?;
        let mut keyring = WebhookSecretKeyring::new(
            *version,
            WebhookSecret::new(secret.expose().to_owned())
                .map_err(|e| StartupError::Webhook(e.to_string()))?,
        )
        .map_err(|e| StartupError::Webhook(e.to_string()))?;
        for (version, secret) in keys {
            keyring
                .insert(
                    *version,
                    WebhookSecret::new(secret.expose().to_owned())
                        .map_err(|e| StartupError::Webhook(e.to_string()))?,
                )
                .map_err(|e| StartupError::Webhook(e.to_string()))?;
        }
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
            testing_db,
            iam: Arc::new(iam),
            http,
            sealer,
            webhooks: WebhookVerifier::new(keyring),
            telemetry,
            limits: RateLimits {
                speech_actor: RateLimiter::new(
                    limits.speech_per_actor_per_minute,
                    Duration::from_secs(60),
                ),
                speech_org: RateLimiter::new(
                    limits.speech_per_org_per_minute,
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
            catalogs: Mutex::new(HashMap::new()),
            env_locks: Mutex::new(HashMap::new()),
            config,
        })))
    }

    /// Periodic housekeeping: expires idempotency and webhook dedupe rows and
    /// reports testing-environment activity to Honeycomb. `main` runs it
    /// every five minutes.
    pub async fn maintenance(&self) {
        for db in [&self.0.production_db, &self.0.testing_db] {
            match db.call(|conn| Ok(store::gc(conn, unix_now())?)).await {
                Ok(n) if n > 0 => {
                    tracing::info!(removed = n, "expired idempotency and webhook records");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(code = %e.code(), "housekeeping failed"),
            }
        }
        let reported = crate::honeycomb::report_activity(self).await;
        if reported > 0 {
            tracing::info!(
                reported,
                "reported testing-environment activity to Honeycomb"
            );
        }
    }

    /// The request fence of a testing environment.
    pub(crate) fn env_lock(&self, environment_id: Uuid) -> Arc<RwLock<()>> {
        match self.0.env_locks.lock() {
            Ok(mut locks) => Arc::clone(locks.entry(environment_id).or_default()),
            Err(_) => Arc::new(RwLock::new(())),
        }
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.0.config
    }
}
