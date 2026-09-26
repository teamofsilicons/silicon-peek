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
    /// Deepgram JWT mints per actor per minute (120).
    pub speech_per_actor_per_minute: u32,
    /// Deepgram JWT mints per org per minute (1200).
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
    /// Outbound HTTP (Ting, Deepgram, GitHub, Honeycomb, Space Station).
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
    /// Deepgram keys whose `/v1/auth/grant` answered 401/403, by
    /// [`crate::deepgram::key_fingerprint`]: until the instant, speech
    /// tokens for them answer `{"mode":"proxy"}` without asking again.
    grant_forbidden: Mutex<HashMap<String, Instant>>,
}

/// How long a "this key may not mint JWTs" verdict is trusted.
pub(crate) const GRANT_VERDICT_TTL: Duration = Duration::from_secs(600);
/// A verdict with less than this left is re-probed by maintenance (which runs
/// every five minutes), so it never lapses while the server runs.
const GRANT_VERDICT_REFRESH: Duration = Duration::from_secs(330);

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
            grant_forbidden: Mutex::new(HashMap::new()),
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

    /// Time left on a cached "grant forbidden" verdict for a key fingerprint.
    pub(crate) fn grant_forbidden_for(&self, fingerprint: &str) -> Option<Duration> {
        let mut verdicts = self.0.grant_forbidden.lock().ok()?;
        let now = Instant::now();
        verdicts.retain(|_, until| *until > now);
        verdicts
            .get(fingerprint)
            .map(|until| until.saturating_duration_since(now))
    }

    /// Keeps the proxy verdict of peek's own Deepgram keys warm, so the first
    /// `speech/token` after a start (or after a verdict expired) does not pay
    /// for a `/v1/auth/grant` round trip (about a second, TLS included).
    ///
    /// `initial` probes every configured key once (`main` does this at
    /// start-up; a key that may mint gets a 1 s JWT that is thrown away).
    /// Later calls (maintenance) only re-probe keys whose "may not mint"
    /// verdict is about to expire; direct-mode keys mint per request anyway.
    /// Org BYO keys are resolved per request and never probed here.
    pub async fn prewarm_speech(&self, initial: bool) {
        let config = &self.0.config.deepgram;
        for key in [&config.api_key, &config.test_api_key]
            .into_iter()
            .flatten()
        {
            let fingerprint = crate::deepgram::key_fingerprint(&config.base_url, key);
            match self.grant_forbidden_for(&fingerprint) {
                Some(left) if left > GRANT_VERDICT_REFRESH => continue,
                None if !initial => continue,
                _ => {}
            }
            match crate::deepgram::grant(self, &config.base_url, key, 1).await {
                Err(crate::deepgram::Failure::Status {
                    status: 401 | 403, ..
                }) => self.remember_grant_forbidden(fingerprint),
                Ok(_) => {}
                Err(_) => tracing::debug!(
                    "the Deepgram grant probe failed; the next token request retries"
                ),
            }
        }
    }

    /// Remembers that a key may not mint JWTs, for [`GRANT_VERDICT_TTL`].
    pub(crate) fn remember_grant_forbidden(&self, fingerprint: String) {
        if let Ok(mut verdicts) = self.0.grant_forbidden.lock() {
            verdicts.insert(fingerprint, Instant::now() + GRANT_VERDICT_TTL);
        }
    }
}
