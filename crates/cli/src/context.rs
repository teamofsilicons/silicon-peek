//! Resolution of the global context (BLUEPRINT §7.1, §2.9): store, config,
//! API URL, testing environment, org, HTTP client and telemetry opt-out.
//!
//! Environment variables are read here rather than by clap so that an empty
//! value is an error (`SILICON_ORG=` never silently means "unset"). peek never
//! reads `IAM_TEST_APP_SECRET` or `IAM_TEST_KEY`: those belong to Ting.

use std::{path::PathBuf, time::Duration};

use serde_json::json;
use silicon_peek_client::{
    Error, ErrorCode, Result, Secret,
    config::Config,
    http::Client,
    identity::{ApiUrl, Context, OrgId, SlotKey, TestingSecret, parse_test_selector, resolve_org},
    runtime::{
        RefreshPolicy, SessionSlot, Store,
        store::{silicon_home, store_dir_for},
        testing::{SavedEnvironment, TestingFile},
    },
    telemetry::env_opt_out,
};
use uuid::Uuid;

use crate::{cli::GlobalArgs, input, output::Out};

/// The retry budget the CLI gives a refresh (the session stays resumable
/// for 10 minutes, so a later command continues where this one stopped).
pub fn cli_refresh_policy() -> RefreshPolicy {
    RefreshPolicy::with_delays(vec![Duration::from_secs(1), Duration::from_secs(2)])
}

fn env_value(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(v) if v.is_empty() => Err(Error::invalid_input(format!(
            "{name} is set but empty; give it a value or unset it"
        ))),
        Ok(v) => Ok(Some(v)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(Error::invalid_input(format!("{name} is not valid UTF-8")))
        }
    }
}

/// The store directory for this home (without creating it).
pub fn store_dir() -> Result<PathBuf> {
    let home = silicon_home(std::env::var_os("SILICON_HOME").as_deref())?;
    store_dir_for(&home)
}

/// Opens (creating if needed) this home's store.
pub fn store() -> Result<Store> {
    Store::open(&store_dir()?)
}

/// Opens the store only if it already exists.
pub fn existing_store() -> Result<Option<Store>> {
    let dir = store_dir()?;
    if std::fs::symlink_metadata(&dir).is_ok() {
        Store::open(&dir).map(Some)
    } else {
        Ok(None)
    }
}

/// Parsed global flags plus the environment they fall back to.
#[derive(Clone, Debug, Default)]
pub struct Globals {
    /// The raw flags.
    pub args: GlobalArgs,
}

/// A selected testing environment.
#[derive(Clone, Debug)]
pub struct Testing {
    /// Environment UUID.
    pub id: Uuid,
    /// Human name.
    pub name: String,
    /// Current generation (sent on mutations).
    pub generation: u64,
    /// peek's test app secret.
    pub secret: TestingSecret,
}

/// Everything a session-bound command needs.
#[derive(Clone, Debug)]
pub struct Session {
    /// The per-home store.
    pub store: Store,
    /// peek-server origin.
    pub api: ApiUrl,
    /// production or a testing environment.
    pub context: Context,
    /// The testing environment, when selected.
    pub testing: Option<Testing>,
    /// HTTP client with the testing and telemetry headers applied.
    pub client: Client,
    /// `"<api>#<context>"`.
    pub slot_key: SlotKey,
    /// Whether telemetry is on for this run.
    pub telemetry: bool,
}

impl Globals {
    /// Wraps parsed flags.
    pub fn new(args: &GlobalArgs) -> Self {
        Self { args: args.clone() }
    }

    /// Output settings.
    pub fn out(&self) -> Out {
        Out {
            json: self.args.json,
            quiet: self.args.quiet,
        }
    }

    /// `--api`, else `PEEK_API_URL`.
    pub fn explicit_api(&self) -> Result<Option<ApiUrl>> {
        let raw = match &self.args.api {
            Some(v) if v.is_empty() => {
                return Err(Error::invalid_input("--api is empty; pass an https origin"));
            }
            Some(v) => Some(v.clone()),
            None => env_value("PEEK_API_URL")?,
        };
        raw.as_deref().map(ApiUrl::parse).transpose()
    }

    /// `--test`, else `SILICON_PEEK_TEST`.
    pub fn test_selector(&self) -> Result<Option<Uuid>> {
        let raw = match &self.args.test {
            Some(v) => Some(v.clone()),
            None => env_value("SILICON_PEEK_TEST")?,
        };
        raw.as_deref().map(parse_test_selector).transpose()
    }

    /// Whether a testing secret was supplied (flag, file or env).
    pub fn has_secret(&self) -> Result<bool> {
        Ok(self.args.app_secret_file.is_some()
            || self.args.app_secret.is_some()
            || env_value("PEEK_TEST_APP_SECRET")?.is_some())
    }

    /// Whether this run targets a testing environment.
    pub fn is_testing(&self) -> Result<bool> {
        Ok(self.has_secret()? || self.test_selector()?.is_some())
    }

    fn secret(&self) -> Result<Option<TestingSecret>> {
        let raw = if let Some(source) = &self.args.app_secret_file {
            Some(input::read_secret("--app-secret-file", source)?)
        } else if let Some(v) = &self.args.app_secret {
            Some(Secret::new(v.trim()))
        } else {
            env_value("PEEK_TEST_APP_SECRET")?.map(|v| Secret::new(v.trim()))
        };
        raw.map(|s| TestingSecret::parse(s.expose())).transpose()
    }

    /// Refuses two inputs that both read stdin.
    pub fn check_stdin(&self, others: &[(&str, bool)]) -> Result<()> {
        let mut users: Vec<&str> = Vec::new();
        if self.args.app_secret_file.as_deref() == Some("-") {
            users.push("--app-secret-file");
        }
        users.extend(others.iter().filter(|(_, uses)| *uses).map(|(f, _)| *f));
        if users.len() > 1 {
            return Err(Error::new(
                ErrorCode::ConflictingFlags,
                format!(
                    "{} all read stdin; only one input can use `-`",
                    users.join(" and ")
                ),
            )
            .with_hint("pass the other value from a file path instead of -"));
        }
        Ok(())
    }

    /// Refuses `--idempotency-key` on commands that derive their own key.
    pub fn refuse_idempotency_key(&self, command: &str) -> Result<()> {
        if self.args.idempotency_key.is_some() {
            return Err(Error::new(
                ErrorCode::ConflictingFlags,
                format!("--idempotency-key does not apply to `peek {command}`"),
            )
            .with_hint(
                "it overrides the key of `peek ting enroll` and `peek report`; login, refresh and logout derive their keys so a retry replays the same request",
            ));
        }
        Ok(())
    }

    /// `--idempotency-key`, validated.
    pub fn idempotency_key(&self) -> Result<Option<silicon_peek_client::ids::IdempotencyKey>> {
        self.args
            .idempotency_key
            .as_deref()
            .map(silicon_peek_client::ids::IdempotencyKey::parse)
            .transpose()
    }

    /// The org for an org-specific call: `--org`, `SILICON_ORG`, the slot.
    pub fn org(&self, slot: Option<&OrgId>) -> Result<Option<OrgId>> {
        let env = match std::env::var("SILICON_ORG") {
            Ok(v) => Some(v),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(Error::invalid_input("SILICON_ORG is not valid UTF-8"));
            }
        };
        resolve_org(self.args.org.as_deref(), env.as_deref(), slot)
    }

    /// The org of a bearer call, checked against the session's orgs so a
    /// wrong org is reported locally instead of as a 401 that would look like
    /// a revoked session.
    pub fn session_org(&self, slot: &SessionSlot) -> Result<OrgId> {
        let org = self
            .org(Some(&slot.org_id))?
            .unwrap_or_else(|| slot.org_id.clone());
        let allowed = if slot.org_ids.is_empty() {
            vec![slot.org_id.clone()]
        } else {
            slot.org_ids.clone()
        };
        if allowed.contains(&org) {
            Ok(org)
        } else {
            let list: Vec<&str> = allowed.iter().map(OrgId::as_str).collect();
            Err(Error::invalid_input(format!(
                "this session is authorized for org(s) {}; `{org}` is not one of them",
                list.join(", ")
            ))
            .with_hint(format!(
                "drop --org / SILICON_ORG, or log in for {org}: iam silicon-login --app-id peek --grant-org {org} --approve-scopes; peek --org {org} login '<SLT>'"
            ))
            .with_details(json!({"org": org, "session_orgs": list})))
        }
    }

    /// Whether telemetry is on for this run (flag, env, config).
    pub fn telemetry_enabled(&self, config: Option<&Config>) -> bool {
        !self.args.no_telemetry && !env_opt_out() && config.is_none_or(|c| c.telemetry)
    }

    /// Builds the session context. `rediscover` re-reads a saved testing
    /// environment's name and generation from the backend (login, status).
    pub async fn session(&self, store: Store, rediscover: bool) -> Result<Session> {
        let config = store.read_config()?;
        let explicit_api = self.explicit_api()?;
        let selector = self.test_selector()?;
        let secret = self.secret()?;
        let telemetry = self.telemetry_enabled(Some(&config));
        let (api, testing) = if let Some(secret) = secret {
            let saved_api = match selector {
                Some(id) => store
                    .read_testing()?
                    .environments
                    .get(&id)
                    .map(|e| e.api_url.clone()),
                None => None,
            };
            let api = explicit_api
                .or(saved_api)
                .or_else(|| config.api_url.clone())
                .unwrap_or_else(ApiUrl::production);
            let testing = discover(&store, &api, secret, selector).await?;
            (api, Some(testing))
        } else if let Some(id) = selector {
            let file: TestingFile = store.read_testing()?;
            let saved = file.get(id)?.clone();
            let api = explicit_api.unwrap_or_else(|| saved.api_url.clone());
            let mut testing = Testing {
                id,
                name: saved.name.clone(),
                generation: saved.generation,
                secret: saved.app_secret.clone(),
            };
            if rediscover {
                testing = discover(&store, &api, saved.app_secret, Some(id)).await?;
            }
            (api, Some(testing))
        } else {
            let api = explicit_api
                .or_else(|| config.api_url.clone())
                .unwrap_or_else(ApiUrl::production);
            (api, None)
        };
        let context = testing
            .as_ref()
            .map_or(Context::Production, |t| Context::Testing(t.id));
        if let Some(t) = &testing {
            crate::output::set_banner(&t.name, &t.id);
        }
        let mut client = Client::builder(&api)
            .component(concat!("peek-cli/", env!("CARGO_PKG_VERSION")))
            .build()?
            .with_telemetry(telemetry)
            .with_trace_id(crate::telemetry::trace_id());
        if let Some(t) = &testing {
            client = client.with_testing(t.secret.clone(), Some(t.generation));
        }
        crate::telemetry::note_session(telemetry, &client, context);
        Ok(Session {
            slot_key: SlotKey::new(api.clone(), context),
            store,
            api,
            context,
            testing,
            client,
            telemetry,
        })
    }
}

/// After `409 testing_generation_changed`: the same session with the
/// environment's current generation (re-read from `GET /api/v1/iam` and
/// saved to testing.json).
///
/// # Errors
/// Discovery failures (`testing_secret_invalid`, transport).
pub async fn refresh_generation(s: &Session) -> Result<Session> {
    let Some(t) = &s.testing else {
        return Ok(s.clone());
    };
    let fresh = discover(&s.store, &s.api, t.secret.clone(), Some(t.id)).await?;
    let mut next = s.clone();
    next.client = s
        .client
        .with_testing(fresh.secret.clone(), Some(fresh.generation));
    next.testing = Some(fresh);
    Ok(next)
}

/// `GET /api/v1/iam` with the test secret: learns the environment, saves it
/// to testing.json and selects it.
async fn discover(
    store: &Store,
    api: &ApiUrl,
    secret: TestingSecret,
    expected: Option<Uuid>,
) -> Result<Testing> {
    let probe = Client::builder(api)
        .component(concat!("peek-cli/", env!("CARGO_PKG_VERSION")))
        .build()?
        .with_testing(secret.clone(), None);
    let discovery = probe.discover().await.map_err(|e| {
        if matches!(e.status(), Some(401 | 403)) {
            Error::new(
                ErrorCode::TestingSecretInvalid,
                format!(
                    "the backend at {api} rejected the testing app secret: {}",
                    e.message()
                ),
            )
            .with_hint("get the current peek test secret (honeycomb --test <env> apps rotate-secret 'peek') and pass it with --app-secret-file -")
            .with_request_id(e.request_id().map(str::to_owned))
        } else {
            e
        }
    })?;
    let id = discovery
        .testing_environment_id
        .or_else(|| discovery.testing_environment.as_ref().map(|t| t.id))
        .ok_or_else(|| {
            Error::new(
                ErrorCode::TestingSecretInvalid,
                format!(
                    "the backend at {api} did not recognise this secret as a peek testing secret (no testing_environment_id)"
                ),
            )
            .with_hint("pass peek's own test app secret (ask_…), not Ting's or another app's")
        })?;
    if let Some(want) = expected
        && want != id
    {
        return Err(Error::new(
            ErrorCode::TestingSecretInvalid,
            format!("the secret belongs to testing environment {id}, not --test {want}"),
        )
        .with_hint("drop --test, or pass the secret of that environment"));
    }
    let name = discovery
        .testing_environment
        .as_ref()
        .map_or_else(|| "testing environment".to_owned(), |t| t.name.clone());
    let generation = discovery
        .testing_generation
        .or_else(|| discovery.testing_environment.as_ref().map(|t| t.generation))
        .unwrap_or(0);
    let lock = store.lock_async().await?;
    let mut file = store.read_testing()?;
    let extra = file
        .environments
        .get(&id)
        .map(|e| e.extra.clone())
        .unwrap_or_default();
    file.save(
        id,
        SavedEnvironment {
            api_url: api.clone(),
            app_secret: secret.clone(),
            name: name.clone(),
            generation,
            extra,
        },
    );
    file.selected = Some(id);
    store.write_testing(&lock, &file)?;
    drop(lock);
    Ok(Testing {
        id,
        name,
        generation,
        secret,
    })
}
