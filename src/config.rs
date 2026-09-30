//! Configuration, read only from the environment (BLUEPRINT §5.2).
//!
//! Every variable is validated at startup and every problem is reported at
//! once, each naming the variable, what is wrong and how to fix it. Two
//! deliberate exceptions keep a half-provisioned deployment alive:
//!
//! - an empty `PEEK_IAM_APP_SECRET` is allowed: `/readyz` then reports
//!   `iam_config: missing` and IAM routes answer `503 iam_misconfigured`;
//! - Space Station keys never fail startup: a missing or malformed key only
//!   disables that table, with one warning.

use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use silicon_peek_client::Secret;
use url::{Host, Url};

/// Every variable peek-server reads. Any other `PEEK_*` variable is reported
/// as a warning (a typo would otherwise be silently ignored).
pub const VARIABLES: &[&str] = &[
    "PEEK_BIND",
    "PEEK_DATABASE_PATH",
    "PEEK_TEST_DATABASE_PATH",
    "PEEK_PUBLIC_ORIGIN",
    "PEEK_WEB_ORIGINS",
    "PEEK_ENVIRONMENT",
    "PEEK_IAM_BASE_URL",
    "PEEK_IAM_APP_ID",
    "PEEK_IAM_APP_SECRET",
    "PEEK_IAM_WEBHOOK_SECRET",
    "PEEK_IAM_WEBHOOK_KEY_VERSION",
    "PEEK_IAM_WEBHOOK_PREVIOUS_SECRET",
    "PEEK_IAM_WEBHOOK_PREVIOUS_KEY_VERSION",
    "PEEK_IAM_REQUEST_TIMEOUT_SECONDS",
    "PEEK_IAM_SDK_TELEMETRY",
    "PEEK_TING_BASE_URL",
    "PEEK_TING_REQUEST_TIMEOUT_SECONDS",
    "PEEK_HONEYCOMB_URL",
    "PEEK_HONEYCOMB_SERVICE_TOKEN",
    "PEEK_ENCRYPTION_KEY",
    "PEEK_DEEPGRAM_API_KEY",
    "PEEK_DEEPGRAM_TEST_API_KEY",
    "PEEK_DEEPGRAM_BASE_URL",
    "PEEK_OPENAI_API_KEY",
    "PEEK_OPENAI_TEST_API_KEY",
    "PEEK_OPENAI_BASE_URL",
    "PEEK_ELEVENLABS_AGENT_URL",
    "PEEK_DEEPGRAM_TOKEN_TTL_SECONDS",
    "PEEK_DEEPGRAM_MIP_OPT_OUT",
    "PEEK_BYO_DEEPGRAM_HOSTS",
    "PEEK_GITHUB_ISSUES_TOKEN",
    "PEEK_GITHUB_REPO",
    "PEEK_GITHUB_API_URL",
    "PEEK_TELEMETRY",
    "PEEK_BACKEND_TABLE_KEY",
    "PEEK_CLIDAEMON_TABLE_KEY",
    "PEEK_FRONTEND_ANALYTICS_TABLE_KEY",
    "PEEK_FRONTEND_EVENTS_TABLE_KEY",
    "PEEK_TELEMETRY_HOME",
    "PEEK_TELEMETRY_URL",
];

/// The complete, validated server configuration.
#[derive(Clone, Debug)]
pub struct Config {
    /// Listen address (`PEEK_BIND`, default `127.0.0.1:8080`).
    pub bind: SocketAddr,
    /// Production database file (`PEEK_DATABASE_PATH`).
    pub database_path: PathBuf,
    /// Database for every testing context (`PEEK_TEST_DATABASE_PATH`).
    pub test_database_path: PathBuf,
    /// This backend's public origin, advertised by `/api/v1/iam`.
    pub public_origin: String,
    /// Browser origins allowed on the telemetry gateway.
    pub web_origins: Vec<String>,
    /// `production` or `development` (the telemetry `environment`).
    pub environment: DeploymentEnvironment,
    /// IAM settings.
    pub iam: IamConfig,
    /// Ting settings.
    pub ting: TingConfig,
    /// Honeycomb lifecycle participant settings.
    pub honeycomb: HoneycombConfig,
    /// AES-256-GCM key for BYO keys and environment root keys.
    pub encryption_key: EncryptionKey,
    /// TTS token credentials and legacy Deepgram administration.
    pub deepgram: DeepgramConfig,
    /// Direct-client `ElevenLabs` Voice Agent endpoint returned with temporary tokens.
    pub elevenlabs_agent_url: String,
    /// `OpenAI` transcription.
    pub openai: OpenaiConfig,
    /// GitHub issue filing for bug reports.
    pub github: GithubConfig,
    /// Space Station telemetry.
    pub telemetry: TelemetryConfig,
    /// Non-fatal findings to log once at startup (never secret values).
    pub warnings: Vec<String>,
}

/// The deployment tier recorded in telemetry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentEnvironment {
    /// The live service.
    Production,
    /// A developer's machine.
    Development,
}

impl DeploymentEnvironment {
    /// The telemetry spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Development => "development",
        }
    }
}

/// IAM settings.
#[derive(Clone, Debug)]
pub struct IamConfig {
    /// IAM API origin (`PEEK_IAM_BASE_URL`).
    pub base_url: String,
    /// The IAM application ID; always `peek`.
    pub app_id: String,
    /// The production app secret; `None` until `honeycomb apps create` ran.
    pub app_secret: Option<Secret>,
    /// Webhook signing keys: `(version, secret)`, current first.
    pub webhook_keys: Vec<(i64, Secret)>,
    /// Per-request IAM timeout.
    pub request_timeout: Duration,
    /// Whether the IAM SDK's own telemetry is on (`x-iam-telemetry: on`, and
    /// the SDK's recorder if an `IAM_TELEMETRY_KEY` is configured). Defaults
    /// to `PEEK_TELEMETRY`; `PEEK_IAM_SDK_TELEMETRY=off` keeps the SDK from
    /// ever looking for a developer's `~/.silicon-iam` telemetry key.
    pub sdk_telemetry: bool,
}

/// Ting settings.
#[derive(Clone, Debug)]
pub struct TingConfig {
    /// Ting API origin (`PEEK_TING_BASE_URL`).
    pub base_url: String,
    /// Per-request Ting timeout.
    pub request_timeout: Duration,
}

/// Honeycomb settings.
#[derive(Clone, Debug)]
pub struct HoneycombConfig {
    /// Honeycomb API origin, for testing-environment activity reports.
    pub base_url: String,
    /// The lifecycle participant bearer token; `None` disables the routes.
    pub service_token: Option<Secret>,
}

/// A 32-byte AES key. `Debug` never shows it.
#[derive(Clone)]
pub struct EncryptionKey(pub(crate) [u8; 32]);

impl fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EncryptionKey([REDACTED])")
    }
}

/// `OpenAI` transcription settings. Testing never uses the production key.
#[derive(Clone, Debug)]
pub struct OpenaiConfig {
    /// Production key (`PEEK_OPENAI_API_KEY`).
    pub api_key: Option<Secret>,
    /// Isolated testing key (`PEEK_OPENAI_TEST_API_KEY`).
    pub test_api_key: Option<Secret>,
    /// API origin (`PEEK_OPENAI_BASE_URL`).
    pub base_url: String,
}

/// Deepgram credentials for `ElevenLabs` TTS, plus legacy BYO administration.
#[derive(Clone, Debug)]
pub struct DeepgramConfig {
    /// Production credential for TTS connection tokens.
    pub api_key: Option<Secret>,
    /// Isolated testing credential for TTS connection tokens.
    pub test_api_key: Option<Secret>,
    /// Deepgram origin.
    pub base_url: String,
    /// Temporary token lifetime in seconds (1–3600; default 30).
    pub token_ttl_seconds: u32,
    /// Legacy privacy setting, retained for configuration compatibility.
    pub mip_opt_out: bool,
    /// Hosts an org's BYO key may name as its base URL
    /// (`PEEK_BYO_DEEPGRAM_HOSTS`, default `*.deepgram.com`).
    pub byo_hosts: ByoHosts,
}

/// One entry of [`ByoHosts`].
#[derive(Clone, Debug, PartialEq, Eq)]
enum HostPattern {
    /// `*`: any host name.
    Any,
    /// `host`: exactly this host.
    Exact(String),
    /// `*.domain`: any subdomain of `domain` (not `domain` itself).
    Subdomains(String),
}

/// The hosts an org's own Deepgram key may send peek-server's requests to.
///
/// An org owner or admin chooses the BYO base URL, and peek-server then
/// calls it for every member's speech: without a policy an org could point
/// the shared backend at any host (its own server, or ports on the backend's
/// loopback). BYO base URLs are therefore always `https://`, never an IP
/// address or `localhost`, and must match this operator-set allowlist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByoHosts(Vec<HostPattern>);

/// `PEEK_BYO_DEEPGRAM_HOSTS`' default: Deepgram's own API hosts.
pub const DEFAULT_BYO_DEEPGRAM_HOSTS: &str = "*.deepgram.com";

fn valid_host_name(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    host.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            (1..=63).contains(&l.len())
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
        // A dotted-decimal "host" is an IP address, not a name.
        && !labels.iter().all(|l| l.bytes().all(|b| b.is_ascii_digit()))
}

impl ByoHosts {
    /// Parses a comma-separated list of `host`, `*.domain` or `*`.
    ///
    /// # Errors
    /// Why an entry is not a host pattern.
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut patterns = Vec::new();
        for entry in value.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let lower = entry.to_ascii_lowercase();
            let pattern = if lower == "*" {
                HostPattern::Any
            } else if let Some(domain) = lower.strip_prefix("*.") {
                if !valid_host_name(domain) {
                    return Err(format!(
                        "entry `{entry}` must be `*.` followed by a domain such as deepgram.com"
                    ));
                }
                HostPattern::Subdomains(domain.to_owned())
            } else if valid_host_name(&lower) && !lower.ends_with(".localhost") {
                HostPattern::Exact(lower)
            } else {
                return Err(format!(
                    "entry `{entry}` must be a host name (such as api.deepgram.com), `*.domain` or `*`; IP addresses and localhost are never allowed"
                ));
            };
            patterns.push(pattern);
        }
        if patterns.is_empty() {
            return Err("must list at least one host".to_owned());
        }
        Ok(Self(patterns))
    }

    fn allows(&self, host: &str) -> bool {
        self.0.iter().any(|p| match p {
            HostPattern::Any => true,
            HostPattern::Exact(h) => h == host,
            HostPattern::Subdomains(d) => host
                .strip_suffix(d.as_str())
                .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')),
        })
    }

    fn describe(&self) -> String {
        self.0
            .iter()
            .map(|p| match p {
                HostPattern::Any => "*".to_owned(),
                HostPattern::Exact(h) => h.clone(),
                HostPattern::Subdomains(d) => format!("*.{d}"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Checks an org's BYO base URL: an `https://` origin (no credentials,
    /// path or query) whose host is a name (never an IP address or
    /// `localhost`) this allowlist accepts. Returns the normalized origin.
    ///
    /// # Errors
    /// Why the URL is refused (safe to show the org admin).
    pub fn check_origin(&self, value: &str) -> Result<String, String> {
        let url = Url::parse(value).map_err(|e| format!("is not a URL ({e})"))?;
        if url.scheme() != "https" {
            return Err("must use https://".to_owned());
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("must not contain credentials".to_owned());
        }
        if url.query().is_some() || url.fragment().is_some() || !matches!(url.path(), "" | "/") {
            return Err(
                "must be an origin (scheme, host and optional port) without a path".to_owned(),
            );
        }
        let host = match url.host() {
            Some(Host::Domain(d)) => d.to_ascii_lowercase(),
            Some(Host::Ipv4(_) | Host::Ipv6(_)) => {
                return Err("must name a host, not an IP address".to_owned());
            }
            None => return Err("must name a host".to_owned()),
        };
        if host == "localhost" || host.ends_with(".localhost") || !valid_host_name(&host) {
            return Err(format!("names `{host}`, which is not a public host name"));
        }
        if url.port() == Some(0) {
            return Err("must not use port 0".to_owned());
        }
        if !self.allows(&host) {
            return Err(format!(
                "names host `{host}`, which this peek-server does not allow for an org's Deepgram key (allowed: {})",
                self.describe()
            ));
        }
        Ok(url.origin().ascii_serialization())
    }
}

/// GitHub settings.
#[derive(Clone, Debug)]
pub struct GithubConfig {
    /// Fine-grained token with issues:write on the repository.
    pub issues_token: Option<Secret>,
    /// `owner/name`.
    pub repo: String,
    /// GitHub API origin (`PEEK_GITHUB_API_URL`, default `https://api.github.com`).
    pub api_url: String,
}

/// Space Station settings. Keys are `None` when missing or malformed.
#[derive(Clone, Debug)]
pub struct TelemetryConfig {
    /// `PEEK_TELEMETRY` (off disables every recorder and the gateway).
    pub enabled: bool,
    /// `table-peekbackend-…`, recorded directly.
    pub backend_key: Option<Secret>,
    /// `table-peekclidaemon-…`, relayed by the gateway.
    pub clidaemon_key: Option<Secret>,
    /// `table-peekfrontendanalytics-…`, relayed by the gateway.
    pub analytics_key: Option<Secret>,
    /// `table-peekfrontendevents-…`, relayed by the gateway.
    pub events_key: Option<Secret>,
    /// The `SpaceClient` spool directory (always explicit).
    pub home: PathBuf,
    /// Space Station origin; `None` when invalid (telemetry off).
    pub url: Option<String>,
}

/// One configuration problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigProblem {
    /// The variable.
    pub variable: &'static str,
    /// What is wrong and how to fix it (never the value of a secret).
    pub problem: String,
}

/// Every configuration problem found at startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError(pub Vec<ConfigProblem>);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "peek-server cannot start: {} configuration problem(s)",
            self.0.len()
        )?;
        for p in &self.0 {
            writeln!(f, "  {}: {}", p.variable, p.problem)?;
        }
        write!(
            f,
            "Set these in the environment (production: /etc/peek/runtime.env, rendered from Secrets Manager silicon-peek/production/runtime; local: .env, see .env.example)."
        )
    }
}

impl std::error::Error for ConfigError {}

struct Reader<'a> {
    lookup: &'a dyn Fn(&str) -> Option<String>,
    problems: Vec<ConfigProblem>,
    warnings: Vec<String>,
}

impl Reader<'_> {
    fn raw(&self, var: &'static str) -> Option<String> {
        (self.lookup)(var)
    }

    /// The trimmed value, `None` when unset or empty.
    fn value(&self, var: &'static str) -> Option<String> {
        self.raw(var)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    }

    fn problem(&mut self, variable: &'static str, problem: impl Into<String>) {
        self.problems.push(ConfigProblem {
            variable,
            problem: problem.into(),
        });
    }

    fn origin(&mut self, var: &'static str, default: &str) -> String {
        let value = self.value(var).unwrap_or_else(|| default.to_owned());
        match parse_origin(&value) {
            Ok(origin) => origin,
            Err(why) => {
                self.problem(var, why);
                default.to_owned()
            }
        }
    }

    fn seconds(&mut self, var: &'static str, default: u64, min: u64, max: u64) -> u64 {
        let Some(value) = self.value(var) else {
            return default;
        };
        match value.parse::<u64>() {
            Ok(n) if (min..=max).contains(&n) => n,
            _ => {
                self.problem(
                    var,
                    format!("must be a whole number from {min} to {max}, got `{value}`"),
                );
                default
            }
        }
    }

    fn secret(&mut self, var: &'static str, min: usize, max: usize, what: &str) -> Option<Secret> {
        let value = self.value(var)?;
        if !(min..=max).contains(&value.len()) || !value.bytes().all(|b| b.is_ascii_graphic()) {
            self.problem(
                var,
                format!(
                    "must be {what}: {min}–{max} visible ASCII characters without spaces (got {} characters)",
                    value.chars().count()
                ),
            );
            return None;
        }
        Some(Secret::new(value))
    }

    fn table_key(&mut self, var: &'static str, table: &str) -> Option<Secret> {
        let value = self.value(var)?;
        let prefix = format!("table-{table}-");
        let ok = value
            .strip_prefix(&prefix)
            .is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()));
        if ok {
            Some(Secret::new(value))
        } else {
            self.warnings.push(format!(
                "{var} is not a {table} table key (expected {prefix}<32 hex>); the {table} table is disabled"
            ));
            None
        }
    }
}

impl Config {
    /// Reads the process environment.
    ///
    /// # Errors
    /// [`ConfigError`] listing every invalid variable.
    pub fn from_env() -> Result<Self, ConfigError> {
        let lookup = |k: &str| std::env::var(k).ok();
        let mut config = Self::from_lookup(&lookup)?;
        for (key, _) in std::env::vars_os() {
            if let Some(key) = key.to_str()
                && key.starts_with("PEEK_")
                && !VARIABLES.contains(&key)
            {
                config.warnings.push(format!(
                    "{key} is set but peek-server does not read it (typo?); known variables are listed in .env.example"
                ));
            }
        }
        Ok(config)
    }

    /// Reads configuration through `lookup` (tests pass a map; nothing is
    /// read from the process environment).
    ///
    /// # Errors
    /// [`ConfigError`] listing every invalid variable.
    #[allow(clippy::too_many_lines)] // one linear list of variables reads best in one place
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let mut r = Reader {
            lookup,
            problems: Vec::new(),
            warnings: Vec::new(),
        };

        let bind_raw = r
            .value("PEEK_BIND")
            .unwrap_or_else(|| "127.0.0.1:8080".to_owned());
        let bind = bind_raw.parse::<SocketAddr>().unwrap_or_else(|_| {
            r.problem(
                "PEEK_BIND",
                format!("must be an IP:port such as 127.0.0.1:8080, got `{bind_raw}`"),
            );
            SocketAddr::from(([127, 0, 0, 1], 8080))
        });

        let database_path = PathBuf::from(
            r.value("PEEK_DATABASE_PATH")
                .unwrap_or_else(|| "peek.sqlite".to_owned()),
        );
        let test_database_path = PathBuf::from(
            r.value("PEEK_TEST_DATABASE_PATH")
                .unwrap_or_else(|| "testing.sqlite".to_owned()),
        );
        if database_path == test_database_path {
            r.problem(
                "PEEK_TEST_DATABASE_PATH",
                "must differ from PEEK_DATABASE_PATH: production and testing data never share a file",
            );
        }

        let public_origin = r.origin(
            "PEEK_PUBLIC_ORIGIN",
            silicon_peek_client::identity::DEFAULT_API_URL,
        );

        let mut web_origins = Vec::new();
        let origins_raw = r
            .raw("PEEK_WEB_ORIGINS")
            .unwrap_or_else(|| "https://peek.teamofsilicons.com".to_owned());
        for entry in origins_raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match parse_origin(entry) {
                Ok(origin) => web_origins.push(origin),
                Err(why) => r.problem("PEEK_WEB_ORIGINS", format!("entry `{entry}` {why}")),
            }
        }

        let environment = match r.value("PEEK_ENVIRONMENT").as_deref() {
            None | Some("production") => DeploymentEnvironment::Production,
            Some("development") => DeploymentEnvironment::Development,
            Some(other) => {
                r.problem(
                    "PEEK_ENVIRONMENT",
                    format!("must be `production` or `development`, got `{other}`"),
                );
                DeploymentEnvironment::Production
            }
        };

        let iam_base_url = r.origin("PEEK_IAM_BASE_URL", silicon_peek_client::IAM_URL);
        let app_id = r
            .value("PEEK_IAM_APP_ID")
            .unwrap_or_else(|| silicon_peek_client::APP_ID.to_owned());
        if app_id != silicon_peek_client::APP_ID {
            r.problem(
                "PEEK_IAM_APP_ID",
                format!(
                    "must be `{}` (Ting types are `peek.*` and enrollment names app `peek`), got `{app_id}`",
                    silicon_peek_client::APP_ID
                ),
            );
        }
        let app_secret = r.value("PEEK_IAM_APP_SECRET").and_then(|v| {
            let ok = v
                .strip_prefix("ask_")
                .is_some_and(|rest| (16..=508).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'));
            if ok {
                Some(Secret::new(v))
            } else {
                r.problem(
                    "PEEK_IAM_APP_SECRET",
                    "must be the IAM app secret `ask_…` printed once by `honeycomb apps create` (or empty until then)",
                );
                None
            }
        });

        let mut webhook_keys = Vec::new();
        let current_version = r.seconds("PEEK_IAM_WEBHOOK_KEY_VERSION", 1, 1, 1_000_000);
        match r.secret("PEEK_IAM_WEBHOOK_SECRET", 32, 512, "the webhook signing secret") {
            Some(secret) => webhook_keys.push((to_i64(current_version), secret)),
            None if r.value("PEEK_IAM_WEBHOOK_SECRET").is_none() => r.problem(
                "PEEK_IAM_WEBHOOK_SECRET",
                "is required: the IAM webhook receiver verifies every delivery's HMAC with it; generate one with `openssl rand -hex 32` and use the same value in application.json",
            ),
            None => {}
        }
        if let Some(previous) = r.secret(
            "PEEK_IAM_WEBHOOK_PREVIOUS_SECRET",
            32,
            512,
            "the previous webhook signing secret",
        ) {
            let version = r.seconds("PEEK_IAM_WEBHOOK_PREVIOUS_KEY_VERSION", 0, 1, 1_000_000);
            if version == 0 {
                r.problem(
                    "PEEK_IAM_WEBHOOK_PREVIOUS_KEY_VERSION",
                    "is required when PEEK_IAM_WEBHOOK_PREVIOUS_SECRET is set (the key version IAM signed with before the rotation)",
                );
            } else if version == current_version {
                r.problem(
                    "PEEK_IAM_WEBHOOK_PREVIOUS_KEY_VERSION",
                    "must differ from PEEK_IAM_WEBHOOK_KEY_VERSION",
                );
            } else {
                webhook_keys.push((to_i64(version), previous));
            }
        }
        let iam_timeout = r.seconds("PEEK_IAM_REQUEST_TIMEOUT_SECONDS", 5, 1, 120);

        let ting_base_url = r.origin(
            "PEEK_TING_BASE_URL",
            "https://backend.ting.teamofsilicons.com",
        );
        let ting_timeout = r.seconds("PEEK_TING_REQUEST_TIMEOUT_SECONDS", 15, 1, 120);

        let honeycomb_url = r.origin(
            "PEEK_HONEYCOMB_URL",
            "https://backend.honeycomb.teamofsilicons.com",
        );
        let service_token = r.secret(
            "PEEK_HONEYCOMB_SERVICE_TOKEN",
            32,
            512,
            "the Honeycomb lifecycle participant token",
        );

        let encryption_key = match r.value("PEEK_ENCRYPTION_KEY") {
            None => {
                r.problem(
                    "PEEK_ENCRYPTION_KEY",
                    "is required (AES-256-GCM key for BYO Deepgram keys and testing-environment root keys); generate one with `openssl rand -hex 32`",
                );
                EncryptionKey([0; 32])
            }
            Some(hex_key) => {
                if let Some(key) = decode_key(&hex_key) {
                    EncryptionKey(key)
                } else {
                    r.problem(
                        "PEEK_ENCRYPTION_KEY",
                        format!(
                            "must be exactly 64 hex characters (32 bytes), got {} characters; generate one with `openssl rand -hex 32`",
                            hex_key.chars().count()
                        ),
                    );
                    EncryptionKey([0; 32])
                }
            }
        };

        let openai = OpenaiConfig {
            api_key: r.secret("PEEK_OPENAI_API_KEY", 16, 512, "an OpenAI API key"),
            test_api_key: r.secret("PEEK_OPENAI_TEST_API_KEY", 16, 512, "an OpenAI API key"),
            base_url: r.origin("PEEK_OPENAI_BASE_URL", "https://api.openai.com"),
        };
        let elevenlabs_agent_url = {
            let default = "wss://agent.deepgram.com/v1/agent/converse";
            let value = r
                .value("PEEK_ELEVENLABS_AGENT_URL")
                .unwrap_or_else(|| default.to_owned());
            match parse_agent_url(&value) {
                Ok(url) => url,
                Err(why) => {
                    r.problem("PEEK_ELEVENLABS_AGENT_URL", why);
                    default.to_owned()
                }
            }
        };
        let deepgram = DeepgramConfig {
            api_key: r.secret("PEEK_DEEPGRAM_API_KEY", 16, 512, "a Deepgram API key"),
            test_api_key: r.secret("PEEK_DEEPGRAM_TEST_API_KEY", 16, 512, "a Deepgram API key"),
            base_url: r.origin("PEEK_DEEPGRAM_BASE_URL", "https://api.deepgram.com"),
            token_ttl_seconds: u32::try_from(r.seconds(
                "PEEK_DEEPGRAM_TOKEN_TTL_SECONDS",
                30,
                1,
                3600,
            ))
            .unwrap_or(30),
            byo_hosts: {
                let raw = r
                    .value("PEEK_BYO_DEEPGRAM_HOSTS")
                    .unwrap_or_else(|| DEFAULT_BYO_DEEPGRAM_HOSTS.to_owned());
                ByoHosts::parse(&raw).unwrap_or_else(|why| {
                    r.problem("PEEK_BYO_DEEPGRAM_HOSTS", why);
                    ByoHosts(vec![HostPattern::Subdomains("deepgram.com".to_owned())])
                })
            },
            // Retain the legacy setting's validation for existing deployments.
            mip_opt_out: match r.value("PEEK_DEEPGRAM_MIP_OPT_OUT").as_deref() {
                None | Some("true") => true,
                Some(other) => {
                    r.problem(
                        "PEEK_DEEPGRAM_MIP_OPT_OUT",
                        format!("must be `true` (legacy compatibility setting), got `{other}`"),
                    );
                    true
                }
            },
        };

        let repo = r
            .value("PEEK_GITHUB_REPO")
            .unwrap_or_else(|| "teamofsilicons/silicon-peek".to_owned());
        if !valid_repo(&repo) {
            r.problem(
                "PEEK_GITHUB_REPO",
                format!(
                    "must be `owner/name` (for example teamofsilicons/silicon-peek), got `{repo}`"
                ),
            );
        }
        let github = GithubConfig {
            issues_token: r.secret(
                "PEEK_GITHUB_ISSUES_TOKEN",
                20,
                512,
                "a fine-grained GitHub token",
            ),
            repo,
            api_url: r.origin("PEEK_GITHUB_API_URL", "https://api.github.com"),
        };

        let telemetry = read_telemetry(&mut r);
        let sdk_telemetry = match r.value("PEEK_IAM_SDK_TELEMETRY") {
            None => telemetry.enabled,
            Some(v) if silicon_peek_client::telemetry::is_off_value(&v) => false,
            Some(v) if matches!(v.to_ascii_lowercase().as_str(), "on" | "1" | "true" | "yes") => {
                telemetry.enabled
            }
            Some(v) => {
                r.problem(
                    "PEEK_IAM_SDK_TELEMETRY",
                    format!("must be `on` or `off`, got `{v}`"),
                );
                telemetry.enabled
            }
        };

        if r.problems.is_empty() {
            Ok(Self {
                bind,
                database_path,
                test_database_path,
                public_origin,
                web_origins,
                environment,
                iam: IamConfig {
                    base_url: iam_base_url,
                    app_id,
                    app_secret,
                    webhook_keys,
                    request_timeout: Duration::from_secs(iam_timeout),
                    sdk_telemetry,
                },
                ting: TingConfig {
                    base_url: ting_base_url,
                    request_timeout: Duration::from_secs(ting_timeout),
                },
                honeycomb: HoneycombConfig {
                    base_url: honeycomb_url,
                    service_token,
                },
                encryption_key,
                deepgram,
                elevenlabs_agent_url,
                openai,
                github,
                telemetry,
                warnings: r.warnings,
            })
        } else {
            Err(ConfigError(r.problems))
        }
    }
}

fn read_telemetry(r: &mut Reader<'_>) -> TelemetryConfig {
    let enabled = match r.value("PEEK_TELEMETRY") {
        None => true,
        Some(v) if silicon_peek_client::telemetry::is_off_value(&v) => false,
        Some(v) if matches!(v.to_ascii_lowercase().as_str(), "on" | "1" | "true" | "yes") => true,
        Some(v) => {
            r.problem(
                "PEEK_TELEMETRY",
                format!("must be `on` or `off` (0/false/off/no also turn it off), got `{v}`"),
            );
            true
        }
    };
    let url_raw = r
        .value("PEEK_TELEMETRY_URL")
        .unwrap_or_else(|| "https://backend.spacestation.teamofsilicons.com".to_owned());
    let url = match parse_origin(&url_raw) {
        Ok(url) => Some(url),
        Err(why) => {
            r.warnings
                .push(format!("PEEK_TELEMETRY_URL {why}; telemetry is disabled"));
            None
        }
    };
    TelemetryConfig {
        enabled,
        backend_key: r.table_key("PEEK_BACKEND_TABLE_KEY", "peekbackend"),
        clidaemon_key: r.table_key("PEEK_CLIDAEMON_TABLE_KEY", "peekclidaemon"),
        analytics_key: r.table_key("PEEK_FRONTEND_ANALYTICS_TABLE_KEY", "peekfrontendanalytics"),
        events_key: r.table_key("PEEK_FRONTEND_EVENTS_TABLE_KEY", "peekfrontendevents"),
        home: PathBuf::from(
            r.value("PEEK_TELEMETRY_HOME")
                .unwrap_or_else(|| "telemetry".to_owned()),
        ),
        url,
    }
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn decode_key(hex_key: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(hex_key).ok()?;
    bytes.try_into().ok()
}

fn valid_repo(repo: &str) -> bool {
    let part = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    repo.split_once('/')
        .is_some_and(|(owner, name)| part(owner) && part(name))
}

/// An encrypted WebSocket endpoint, with plain connections allowed only for local mocks.
fn parse_agent_url(value: &str) -> Result<String, String> {
    let url = Url::parse(value).map_err(|_| "must be a valid wss:// URL".to_owned())?;
    if url.host_str().is_none()
        || !((url.scheme() == "wss"
            && url.host_str() == Some("agent.deepgram.com")
            && url.port_or_known_default() == Some(443)
            && url.path() == "/v1/agent/converse")
            || (url.scheme() == "ws" && is_loopback(&url)))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "must be wss://agent.deepgram.com/v1/agent/converse without credentials, query or fragment (ws:// only on loopback)"
                .to_owned(),
        );
    }
    Ok(url.to_string())
}

/// Whether a URL host is loopback (`localhost`, 127/8, `::1`).
pub(crate) fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => d == "localhost",
        Some(Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
        Some(Host::Ipv6(ip)) => IpAddr::V6(ip).is_loopback(),
        None => false,
    }
}

/// Parses an origin: `https://host[:port]`, or `http://` on loopback only.
/// Returns it without a trailing slash.
pub(crate) fn parse_origin(value: &str) -> Result<String, String> {
    let url = Url::parse(value).map_err(|e| format!("is not a URL ({e})"))?;
    let https = url.scheme() == "https";
    if !(https || (url.scheme() == "http" && is_loopback(&url))) {
        return Err(
            "must use https:// (http:// is allowed only for localhost/127.0.0.1)".to_owned(),
        );
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("must not contain credentials".to_owned());
    }
    if url.query().is_some() || url.fragment().is_some() || !matches!(url.path(), "" | "/") {
        return Err("must be an origin (scheme, host and optional port) without a path".to_owned());
    }
    if url.host_str().is_none() || url.port() == Some(0) {
        return Err("must name a host".to_owned());
    }
    Ok(url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn base() -> HashMap<&'static str, String> {
        HashMap::from([
            ("PEEK_IAM_WEBHOOK_SECRET", "w".repeat(64)),
            ("PEEK_ENCRYPTION_KEY", "ab".repeat(32)),
        ])
    }

    fn load(map: &HashMap<&'static str, String>) -> Result<Config, ConfigError> {
        Config::from_lookup(&|k| map.get(k).cloned())
    }

    #[test]
    fn minimal_config_uses_blueprint_defaults() -> Result<(), ConfigError> {
        let c = load(&base())?;
        assert_eq!(c.bind, SocketAddr::from(([127, 0, 0, 1], 8080)));
        assert_eq!(c.iam.base_url, "https://backend.iam.teamofsilicons.com");
        assert_eq!(c.iam.app_id, "peek");
        assert!(c.iam.app_secret.is_none(), "an empty app secret is allowed");
        assert_eq!(c.iam.request_timeout, Duration::from_secs(5));
        assert_eq!(c.ting.request_timeout, Duration::from_secs(15));
        assert_eq!(c.deepgram.token_ttl_seconds, 30);
        assert!(c.deepgram.mip_opt_out);
        assert_eq!(c.public_origin, "https://backend.peek.teamofsilicons.com");
        assert_eq!(c.web_origins, ["https://peek.teamofsilicons.com"]);
        assert!(c.telemetry.enabled);
        assert!(c.telemetry.backend_key.is_none());
        Ok(())
    }

    #[test]
    fn every_problem_is_reported_at_once() {
        let map = HashMap::from([
            ("PEEK_BIND", "nope".to_owned()),
            ("PEEK_IAM_BASE_URL", "http://iam.example".to_owned()),
            ("PEEK_IAM_APP_ID", "other".to_owned()),
            ("PEEK_IAM_APP_SECRET", "secret".to_owned()),
            ("PEEK_ENCRYPTION_KEY", "short".to_owned()),
            ("PEEK_IAM_REQUEST_TIMEOUT_SECONDS", "0".to_owned()),
            ("PEEK_DATABASE_PATH", "same.sqlite".to_owned()),
            ("PEEK_TEST_DATABASE_PATH", "same.sqlite".to_owned()),
        ]);
        let Err(ConfigError(problems)) = load(&map) else {
            panic!("invalid configuration must fail");
        };
        let vars: Vec<_> = problems.iter().map(|p| p.variable).collect();
        for expected in [
            "PEEK_BIND",
            "PEEK_IAM_BASE_URL",
            "PEEK_IAM_APP_ID",
            "PEEK_IAM_APP_SECRET",
            "PEEK_ENCRYPTION_KEY",
            "PEEK_IAM_REQUEST_TIMEOUT_SECONDS",
            "PEEK_TEST_DATABASE_PATH",
            "PEEK_IAM_WEBHOOK_SECRET",
        ] {
            assert!(vars.contains(&expected), "{expected} missing from {vars:?}");
        }
        let text = ConfigError(problems).to_string();
        assert!(!text.contains("secret\n"), "secret values are never echoed");
    }

    #[test]
    fn loopback_http_is_allowed_for_local_fakes() -> Result<(), ConfigError> {
        let mut map = base();
        map.insert("PEEK_IAM_BASE_URL", "http://127.0.0.1:9999/".to_owned());
        map.insert("PEEK_TING_BASE_URL", "http://localhost:9998".to_owned());
        let c = load(&map)?;
        assert_eq!(c.iam.base_url, "http://127.0.0.1:9999");
        assert_eq!(c.ting.base_url, "http://localhost:9998");
        Ok(())
    }

    #[test]
    fn telemetry_keys_never_fail_startup() -> Result<(), ConfigError> {
        let mut map = base();
        map.insert("PEEK_BACKEND_TABLE_KEY", "table-other-00".to_owned());
        map.insert(
            "PEEK_CLIDAEMON_TABLE_KEY",
            format!("table-peekclidaemon-{}", "a".repeat(32)),
        );
        map.insert("PEEK_TELEMETRY_URL", "ftp://x".to_owned());
        let c = load(&map)?;
        assert!(c.telemetry.backend_key.is_none());
        assert!(c.telemetry.clidaemon_key.is_some());
        assert!(c.telemetry.url.is_none());
        assert_eq!(c.warnings.len(), 2);
        Ok(())
    }

    #[test]
    fn webhook_rotation_keeps_both_versions() -> Result<(), ConfigError> {
        let mut map = base();
        map.insert("PEEK_IAM_WEBHOOK_KEY_VERSION", "2".to_owned());
        map.insert("PEEK_IAM_WEBHOOK_PREVIOUS_SECRET", "p".repeat(40));
        map.insert("PEEK_IAM_WEBHOOK_PREVIOUS_KEY_VERSION", "1".to_owned());
        let c = load(&map)?;
        let versions: Vec<_> = c.iam.webhook_keys.iter().map(|(v, _)| *v).collect();
        assert_eq!(versions, [2, 1]);
        Ok(())
    }

    #[test]
    fn env_example_documents_every_variable_with_valid_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/.env.example");
        let map: HashMap<String, String> =
            dotenvy::from_path_iter(path)?.collect::<Result<_, _>>()?;
        for key in map.keys() {
            assert!(
                VARIABLES.contains(&key.as_str()) || key == "RUST_LOG",
                "{key} is not read"
            );
        }
        for var in VARIABLES {
            assert!(map.contains_key(*var), "{var} is missing from .env.example");
        }
        let config = Config::from_lookup(&|k| map.get(k).cloned())?;
        assert!(config.iam.app_secret.is_none());
        assert!(!config.iam.sdk_telemetry);
        Ok(())
    }

    #[test]
    fn byo_base_urls_are_https_deepgram_hosts_by_default() -> Result<(), ConfigError> {
        let hosts = load(&base())?.deepgram.byo_hosts;
        assert_eq!(
            hosts.check_origin("https://api.eu.deepgram.com/"),
            Ok("https://api.eu.deepgram.com".to_owned())
        );
        assert_eq!(
            hosts.check_origin("https://API.deepgram.com:443"),
            Ok("https://api.deepgram.com".to_owned())
        );
        for bad in [
            "http://api.deepgram.com",
            "http://127.0.0.1:8084",
            "https://127.0.0.1:8084",
            "https://[::1]",
            "https://169.254.169.254",
            "https://10.0.0.1",
            "https://localhost",
            "https://deepgram.localhost",
            "https://deepgram.com",
            "https://evil-deepgram.com",
            "https://api.deepgram.com.evil.example",
            "https://attacker.example",
            "https://u:p@api.deepgram.com",
            "https://api.deepgram.com/v1",
        ] {
            assert!(hosts.check_origin(bad).is_err(), "{bad} must be refused");
        }
        let mut map = base();
        map.insert(
            "PEEK_BYO_DEEPGRAM_HOSTS",
            "dg.acme.example, *.deepgram.com".to_owned(),
        );
        let hosts = load(&map)?.deepgram.byo_hosts;
        assert!(hosts.check_origin("https://dg.acme.example:8443").is_ok());
        assert!(hosts.check_origin("https://other.acme.example").is_err());
        map.insert("PEEK_BYO_DEEPGRAM_HOSTS", "*".to_owned());
        let hosts = load(&map)?.deepgram.byo_hosts;
        assert!(hosts.check_origin("https://anything.example").is_ok());
        assert!(hosts.check_origin("https://127.0.0.1").is_err());
        assert!(hosts.check_origin("https://localhost").is_err());
        for bad in [
            "127.0.0.1",
            "localhost",
            "*.",
            "*.com",
            "http://x.example",
            ",",
        ] {
            map.insert("PEEK_BYO_DEEPGRAM_HOSTS", bad.to_owned());
            assert!(load(&map).is_err(), "{bad} must be refused");
        }
        Ok(())
    }

    #[test]
    fn voice_agent_endpoints_are_canonical_or_local_mocks() {
        for endpoint in [
            "wss://agent.deepgram.com/v1/agent/converse",
            "ws://127.0.0.1:9123/agent",
            "ws://[::1]:9000/agent",
        ] {
            assert!(parse_agent_url(endpoint).is_ok(), "{endpoint}");
        }
        for endpoint in [
            "wss://attacker.example/agent",
            "wss://agent.deepgram.com/v1/listen",
            "wss://agent.deepgram.com:444/v1/agent/converse",
            "ws://agent.deepgram.com/v1/agent/converse",
            "wss://user:pass@agent.deepgram.com/v1/agent/converse",
            "wss://agent.deepgram.com/v1/agent/converse?key=value",
        ] {
            assert!(parse_agent_url(endpoint).is_err(), "{endpoint}");
        }
    }

    #[test]
    fn origins_are_normalised() {
        assert_eq!(
            parse_origin("https://api.deepgram.com/"),
            Ok("https://api.deepgram.com".to_owned())
        );
        assert!(parse_origin("https://x.example/path").is_err());
        assert!(parse_origin("https://u:p@x.example").is_err());
        assert!(parse_origin("http://10.0.0.1").is_err());
        assert_eq!(
            parse_origin("http://[::1]:8080"),
            Ok("http://[::1]:8080".to_owned())
        );
    }
}
