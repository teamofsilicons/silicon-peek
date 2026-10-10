//! Resolution of the global context (BLUEPRINT §7.1, §2.9): store, config,
//! API URL, testing environment, account, HTTP client and telemetry opt-out.
//!
//! Environment variables are read here rather than by clap so that an empty
//! value is an error (`SILICON_ACCOUNT=` never silently means "unset"). peek never
//! reads `ACCOUNTS_TEST_APP_SECRET` or `ACCOUNTS_TEST_KEY`: those belong to Ting.

use std::{path::PathBuf, time::Duration};

use silicon_peek_client::{
    Error, ErrorCode, Result,
    config::Config,
    http::Client,
    identity::{AccountId, ApiUrl, Context, SlotKey},
    runtime::{
        RefreshPolicy, SessionSlot, Store,
        store::{silicon_home, store_dir_for},
    },
    telemetry::env_opt_out,
};

use crate::{cli::GlobalArgs, output::Out};

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

static PROFILE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Selects the command's explicit account profile before any store access.
pub fn initialize_profile(args: &GlobalArgs) -> Result<()> {
    let profile = match &args.profile {
        Some(profile) => profile.clone(),
        None => env_value("PEEK_PROFILE")?.unwrap_or_else(|| "default".into()),
    };
    if profile.is_empty()
        || profile.len() > 64
        || !profile
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
    {
        return Err(Error::invalid_input(
            "--profile uses 1–64 lowercase letters, numbers, underscores or hyphens",
        ));
    }
    let _ = PROFILE.set(profile);
    Ok(())
}

/// Whether the selected profile is the original default store.
pub fn default_profile() -> bool {
    PROFILE.get().is_none_or(|name| name == "default")
}

/// The store directory for this home (without creating it).
pub fn store_dir() -> Result<PathBuf> {
    let home = silicon_home(std::env::var_os("SILICON_HOME").as_deref())?;
    let root = store_dir_for(&home)?;
    let profile = PROFILE.get().map_or("default", String::as_str);
    Ok(if profile == "default" {
        root
    } else {
        root.join("profiles").join(profile)
    })
}

/// Opens (creating if needed) this home's store.
pub fn store() -> Result<Store> {
    let dir = store_dir()?;
    if PROFILE.get().is_some_and(|profile| profile != "default") {
        let profiles = dir
            .parent()
            .ok_or_else(|| Error::internal("profile path has no parent"))?;
        let root = profiles
            .parent()
            .ok_or_else(|| Error::internal("profile path has no root"))?;
        silicon_peek_client::runtime::fs::ensure_private_dir(root)?;
        silicon_peek_client::runtime::fs::ensure_private_dir(profiles)?;
    }
    Store::open(&dir)
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

/// Everything a session-bound command needs.
#[derive(Clone, Debug)]
pub struct Session {
    /// The per-home store.
    pub store: Store,
    /// peek-server origin.
    pub api: ApiUrl,
    /// production or a testing environment.
    pub context: Context,
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

    /// Refuses two inputs that both read stdin.
    pub fn check_stdin(&self, others: &[(&str, bool)]) -> Result<()> {
        let mut users: Vec<&str> = Vec::new();
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

    /// The selected profile owns exactly one authenticated account.
    pub fn account(&self, slot: Option<&AccountId>) -> Result<Option<AccountId>> {
        Ok(slot.cloned())
    }
    pub fn session_account(&self, slot: &SessionSlot) -> Result<AccountId> {
        Ok(slot.account_id.clone())
    }

    /// Whether telemetry is on for this run (flag, env, config).
    pub fn telemetry_enabled(&self, config: Option<&Config>) -> bool {
        !self.args.no_telemetry && !env_opt_out() && config.is_none_or(|c| c.telemetry)
    }

    /// Build a client for the selected profile and backend.
    pub async fn session(&self, store: Store, _rediscover: bool) -> Result<Session> {
        let config = store.read_config()?;
        let api = self
            .explicit_api()?
            .or(config.api_url.clone())
            .unwrap_or_else(ApiUrl::production);
        let telemetry = self.telemetry_enabled(Some(&config));
        let context = Context::Production;
        let client = Client::builder(&api)
            .component(concat!("peek-cli/", env!("CARGO_PKG_VERSION")))
            .build()?
            .with_telemetry(telemetry)
            .with_trace_id(crate::telemetry::trace_id());
        crate::telemetry::note_session(telemetry, &client, context);
        Ok(Session {
            slot_key: SlotKey::new(api.clone(), context),
            store,
            api,
            context,
            client,
            telemetry,
        })
    }
}
