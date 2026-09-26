//! peek-server access for one Silicon home: a client for the slot's API
//! (with the testing secret in a testing context) and `fresh_session` with
//! peekd's margins (§2.5). peekd reads each home's store under that home's
//! `session.lock` and never copies tokens into its own database.

use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, PoisonError, RwLock},
    time::Duration,
};

use silicon_peek_client::{
    Error, ErrorCode, Result,
    http::Client,
    identity::{ApiUrl, Context},
    runtime::{RefreshPolicy, SessionSlot, Store, fresh_session_with},
};

/// Which home, backend and context a network call acts for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeRef {
    /// Canonical `<SILICON_HOME>/.peek`.
    pub home_path: String,
    /// The slot's backend.
    pub api_url: ApiUrl,
    /// `production` or a testing environment.
    pub context: Context,
}

/// Shared connection pool to peek-server.
#[derive(Clone, Debug)]
pub struct Net {
    http: reqwest::Client,
    /// Homes whose mirrored config (`config.sync`) opts out of telemetry:
    /// the home's CLI runs in an environment that opts out, which its
    /// `config.json` does not show.
    opted_out: Arc<RwLock<HashSet<String>>>,
}

impl Net {
    /// A pool with peekd's timeouts (no redirects).
    ///
    /// # Errors
    /// `internal_error` if TLS cannot initialize.
    pub fn new() -> Result<Self> {
        silicon_peek_client::http::ensure_crypto_provider();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(90))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!(
                "peekd/",
                env!("CARGO_PKG_VERSION"),
                " silicon-peek-client/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|e| Error::internal(format!("could not initialize the HTTP client: {e}")))?;
        Ok(Self {
            http,
            opted_out: Arc::default(),
        })
    }

    /// Records whether a home's mirrored config opts out of telemetry.
    pub fn set_home_opted_out(&self, home_path: &str, opted_out: bool) {
        let mut set = self
            .opted_out
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if opted_out {
            set.insert(home_path.to_owned());
        } else {
            set.remove(home_path);
        }
    }

    /// Whether a home's mirrored config opts out of telemetry.
    #[must_use]
    pub fn home_opted_out(&self, home_path: &str) -> bool {
        self.opted_out
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(home_path)
    }

    /// A client for `api` sharing the pool (no session, no testing secret).
    ///
    /// # Errors
    /// `internal_error` if the client cannot be built.
    pub fn api_client(&self, api: &ApiUrl) -> Result<Client> {
        Client::builder(api).http_client(self.http.clone()).build()
    }

    /// Opens the home's store (without creating anything) and builds a
    /// client for its API, carrying the saved testing secret and generation
    /// in a testing context.
    ///
    /// # Errors
    /// `invalid_silicon_home` for a vanished home; `testing_secret_invalid`
    /// when the environment is not saved there.
    pub fn client(&self, home: &HomeRef) -> Result<(Store, Client)> {
        let store = Store::open_existing(Path::new(&home.home_path))?;
        // A home that opted out (`peek config telemetry off`, or a CLI
        // environment that opts out, mirrored by `config.sync`) sends
        // `X-Peek-Telemetry: off` on everything peekd does for it, so the
        // backend records nothing about its deliveries, speech or drawings.
        let telemetry = store.read_config().map_or(true, |c| c.telemetry)
            && !self.home_opted_out(&home.home_path);
        let mut client = Client::builder(&home.api_url)
            .http_client(self.http.clone())
            .build()?
            .with_telemetry(telemetry);
        if let Context::Testing(id) = home.context {
            let testing = store.read_testing()?;
            let env = testing.get(id)?;
            client = client.with_testing(env.app_secret.clone(), Some(env.generation));
        }
        Ok((store, client))
    }

    /// After `409 testing_generation_changed`: re-reads the environment's
    /// generation from `GET /api/v1/iam` (with the saved test secret) and
    /// saves it to the home's testing.json under the store lock. Returns the
    /// new generation.
    ///
    /// # Errors
    /// `testing_secret_invalid` when the secret no longer names this
    /// environment; transport and store failures.
    pub async fn refresh_generation(&self, home: &HomeRef) -> Result<u64> {
        let Context::Testing(id) = home.context else {
            return Err(Error::internal(
                "only a testing environment has a generation to refresh",
            ));
        };
        let store = Store::open_existing(Path::new(&home.home_path))?;
        let secret = store.read_testing()?.get(id)?.app_secret.clone();
        let discovery = self
            .api_client(&home.api_url)?
            .with_testing(secret, None)
            .discover()
            .await?;
        let found = discovery
            .testing_environment_id
            .or_else(|| discovery.testing_environment.as_ref().map(|t| t.id));
        if found != Some(id) {
            return Err(Error::new(
                ErrorCode::TestingSecretInvalid,
                format!("the saved test secret no longer names testing environment {id}"),
            )
            .with_hint("log in again with the environment's current peek test secret"));
        }
        let generation = discovery
            .testing_generation
            .or_else(|| discovery.testing_environment.as_ref().map(|t| t.generation))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::UnexpectedResponse,
                    "GET /api/v1/iam named the environment but no generation",
                )
            })?;
        let lock = store.lock_async().await?;
        let mut file = store.read_testing()?;
        if let Some(env) = file.environments.get_mut(&id) {
            env.generation = generation;
        }
        store.write_testing(&lock, &file)?;
        drop(lock);
        tracing::info!(environment = %id, generation, "testing environment generation changed; saved the new one");
        Ok(generation)
    }

    /// A session usable for at least `margin` and a client carrying it.
    ///
    /// # Errors
    /// `not_logged_in`, `session_rejected`, or a retryable refresh failure.
    pub async fn session(
        &self,
        home: &HomeRef,
        margin: Duration,
        policy: &RefreshPolicy,
    ) -> Result<(Client, SessionSlot, Store)> {
        let (store, client) = self.client(home)?;
        let slot = fresh_session_with(&store, &client, home.context, margin, policy).await?;
        let authed = client.with_session(slot.access_token.clone(), slot.org_id.clone());
        Ok((authed, slot, store))
    }
}

/// Whether an error means the Silicon must log in (or re-consent) before
/// anything can be sent with its authority.
#[must_use]
pub fn needs_authority(e: &Error) -> bool {
    matches!(
        e.code(),
        ErrorCode::NotLoggedIn
            | ErrorCode::SessionRejected
            | ErrorCode::ReconsentRequired
            | ErrorCode::Unauthenticated
            | ErrorCode::AuthorityRequired
            | ErrorCode::RecipientNotRegistered
            | ErrorCode::TestingGenerationChanged
            | ErrorCode::TestingSecretInvalid
            | ErrorCode::InvalidSiliconHome
            | ErrorCode::HomeTokenMismatch
            | ErrorCode::PrivateApplicationOrganizationRequired
    )
}
