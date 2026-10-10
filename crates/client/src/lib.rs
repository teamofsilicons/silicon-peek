//! The shared Rust library of [Peek](https://peek.teamofsilicons.com): quick,
//! voice-first exchanges between Carbons and Silicons on a Mac.
//!
//! It is linked into the `peek` CLI, the per-user daemon `peekd` and the
//! backend `peek-server`, so all three agree on every byte they exchange.
//!
//! # Stateless by default
//!
//! Without features the crate holds no state and touches no files:
//!
//! - [`http::Client`]: a stateless client for every peek-server route. It never
//!   stores or refreshes a session behind the caller's back.
//! - Wire types: the [IPC protocol v1](ipc) between the CLI, peekd and
//!   Peek.app with its [framing codec](ipc::frame); [`schema`] for `--show`,
//!   `--ask` and send options with the exact limits; [`ting`] payloads and
//!   delivery bodies; [`api`] request/response types; [`config`].
//! - The [`error`] model: stable codes, messages, hints and CLI exit codes.
//!
//! # The `runtime` feature
//!
//! `runtime` adds what the CLI and peekd share on one machine: the per-home
//! store `$SILICON_HOME/.peek`, [`runtime::fresh_session`] (the one refresh
//! function both use, serialized by a file lock with a deterministic
//! idempotency key), login/logout bookkeeping and the peekd IPC client.
//!
//! ```no_run
//! # async fn run() -> silicon_peek_client::Result<()> {
//! use silicon_peek_client::{http::Client, identity::ApiUrl};
//!
//! let client = Client::new(&ApiUrl::production())?;
//! let discovery = client.discover().await?;
//! assert_eq!(discovery.app_id, "peek");
//! # Ok(())
//! # }
//! ```
#![forbid(unsafe_code)]

pub mod api;
pub mod config;
pub mod error;
pub mod http;
pub mod identity;
pub mod ids;
pub mod ipc;
pub mod json;
pub mod num;
pub mod schema;
mod secret;
pub mod telemetry;
pub mod timestamp;
pub mod ting;

#[cfg(feature = "runtime")]
pub mod runtime;

pub use error::{Error, ErrorCode, ErrorObject, ExitCode, Result};
pub use secret::{Secret, constant_time_eq};

/// This build's version (one version for the CLI, peekd, Peek.app and the
/// server).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The ACCOUNTS application ID.
pub const APP_ID: &str = "peek";

/// The account that owns the peek application.
pub const OWNER_ACCOUNT: &str = "si:tos";

/// The peek-server API version.
pub const API_VERSION: &str = "v1";

/// ACCOUNTS's backend.
pub const ACCOUNTS_URL: &str = "https://accounts.teamofsilicons.com";

/// ACCOUNTS's consent UI.
pub const AUTH_URL: &str = "https://accounts.teamofsilicons.com/authorize";

/// Online documentation.
pub const DOCS_URL: &str = "https://peek.teamofsilicons.com/docs";

/// Source repository.
pub const REPOSITORY_URL: &str = "https://github.com/teamofsilicons/silicon-peek";

/// Scopes a session must hold; missing ones mean `reconsent_required`.
pub const REQUIRED_SCOPES: [&str; 1] = ["profile"];

/// Every scope peek requests (BLUEPRINT D26), sorted.
pub const ALL_SCOPES: [&str; 1] = ["profile"];

/// The platform string of this build (`macos-aarch64`, `linux-x86_64`, …).
#[must_use]
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Whether this platform runs Peek.app and peekd (macOS only); elsewhere only
/// the ACCOUNTS contract commands work.
#[must_use]
pub fn platform_has_app() -> bool {
    cfg!(target_os = "macos")
}

/// The platforms docs page named in [`platform_unsupported`].
pub const PLATFORMS_DOCS_URL: &str = "https://peek.teamofsilicons.com/docs/platforms";

/// The platforms that run Peek.app and peekd.
pub const FULL_PLATFORMS: [&str; 2] = ["macos-aarch64", "macos-x86_64"];

fn mac_purpose(command: &str) -> &'static str {
    match command {
        "send" => "shows a bubble on a Mac through Peek.app",
        "register side" => "claims a position on a Mac screen through Peek.app",
        "register drawing" => "validates and runs the drawing inside Peek.app on a Mac",
        "unregister" => "releases a position on a Mac through Peek.app",
        "status" => "reports this Silicon's state in Peek.app on a Mac",
        "history" => "reads the send history peekd keeps on a Mac",
        c if c.starts_with("ask") => "reads the asks peekd keeps on a Mac",
        c if c.starts_with("app") => "manages Peek.app, which runs only on a Mac",
        c if c.starts_with("daemon") => "talks to peekd, which runs inside Peek.app on a Mac",
        _ => "needs Peek.app, which runs only on a Mac",
    }
}

/// The error every Mac-bound command returns on other platforms: exactly
/// BLUEPRINT §4.2's JSON for this build's [`platform`].
#[must_use]
pub fn platform_unsupported(command: &str) -> Error {
    platform_unsupported_on(command, &platform())
}

/// [`platform_unsupported`] for an explicit `platform` string:
///
/// ```json
/// {"code":"platform_unsupported","message":"peek send shows a bubble on a Mac through Peek.app; this peek runs on linux-x86_64.",
///  "hint":"Run this Silicon on macOS 26+ with Peek.app installed, or use dm for text conversations.","retryable":false,
///  "details":{"platform":"linux-x86_64","supported_platforms":["macos-aarch64","macos-x86_64"],"docs_url":"https://peek.teamofsilicons.com/docs/platforms"}}
/// ```
#[must_use]
pub fn platform_unsupported_on(command: &str, platform: &str) -> Error {
    Error::new(
        ErrorCode::PlatformUnsupported,
        format!(
            "peek {command} {}; this peek runs on {platform}.",
            mac_purpose(command)
        ),
    )
    .with_hint(
        "Run this Silicon on macOS 26+ with Peek.app installed, or use dm for text conversations.",
    )
    .with_retryable(false)
    .with_details(serde_json::json!({
        "platform": platform,
        "supported_platforms": FULL_PLATFORMS,
        "docs_url": PLATFORMS_DOCS_URL,
    }))
}
