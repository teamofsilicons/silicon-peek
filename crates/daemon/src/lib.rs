//! `peekd`: the per-user Peek daemon (BLUEPRINT §1.1, D3).
//!
//! One peekd runs per macOS account, shipped inside
//! `Peek.app/Contents/Helpers/peekd` and registered by the app as a launchd
//! agent. It owns the slot registry, the send queue, asks and history, the
//! delivery outbox, `ElevenLabs` TTS streamed to the UI, `OpenAI` STT once
//! per recording, per-home session refresh under each home's lock, the
//! telemetry relay, Peek.app's self-update and the stale-CLI watchdog.
//!
//! It serves two roles on `/var/tmp/silicon-peek-<uid>/peekd.sock`: CLIs
//! (one request per connection, authenticated per home) and Peek.app (one
//! long-lived link; peekd holds every token, the app holds none).
//!
//! The library exists so the whole daemon can run in-process in tests
//! ([`daemon::start`] with a [`config::DaemonConfig::rooted`] config).

#[cfg(not(target_os = "macos"))]
compile_error!(
    "peekd runs only on macOS: Peek.app draws the bubbles. On other platforms build silicon-peek-cli, which supports accounts, login, login status, logout and config."
);

pub mod bubbles;
pub mod commands;
pub mod config;
pub mod daemon;
pub mod db;
pub mod drawings;
mod elevenlabs;
pub mod homes;
pub mod logging;
pub mod matching;
pub mod net;
pub mod outbox;
pub mod paths;
mod permissions;
pub mod queue;
pub mod schedule;
mod server;
pub mod settings;
pub mod slots;
pub mod speech;
pub mod speech_request;
pub mod state;
pub mod stt;
mod sys;
pub mod telemetry;
pub mod ui;
pub mod update;
pub mod voice;

pub use config::DaemonConfig;
pub use daemon::{DaemonHandle, start};

/// Sets the process umask to `0o077` (§1.8 step 1); call first in `main`.
pub fn restrict_umask() {
    sys::restrict_umask();
}
