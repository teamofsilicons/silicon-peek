//! The daemon's shared state: configuration, database, the UI link, the
//! per-Silicon bubble queues and `--wait` waiters.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::Instant,
};

use silicon_peek_client::{
    api::TestingEnvironment,
    identity::{ActorId, ApiUrl, Context, OrgId},
    ids::{AskId, SendId},
    ipc::cli::AskResult,
    runtime::Store,
};
use tokio::sync::{Notify, oneshot, watch};

use crate::{
    config::DaemonConfig, db::Db, net::HomeRef, net::Net, paths::Paths, settings::SettingsStore,
    speech::Speech, telemetry::Telemetry, ui::UiHub,
};

/// A Silicon in a data context: the key of slots, drawings, asks and
/// deliveries (`(context, org_id, actor_id)`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActorKey {
    /// `production` or a testing environment.
    pub context: Context,
    /// The org.
    pub org: OrgId,
    /// The Silicon.
    pub actor: ActorId,
}

impl ActorKey {
    /// The context as stored.
    #[must_use]
    pub fn context_str(&self) -> String {
        self.context.as_string()
    }
}

/// A CLI request whose `auth` block passed §1.6's checks.
#[derive(Clone, Debug)]
pub struct Caller {
    /// The verified Silicon.
    pub key: ActorKey,
    /// Its home, backend and context.
    pub home: HomeRef,
    /// Its display name, when known.
    pub display_name: Option<String>,
    /// The opened store.
    pub store: Store,
    /// The testing environment, in a testing context.
    pub testing: Option<TestingEnvironment>,
}

impl Caller {
    /// The API origin.
    #[must_use]
    pub fn api_url(&self) -> &ApiUrl {
        &self.home.api_url
    }
}

/// One send in a Silicon's queue.
#[derive(Clone, Debug)]
pub struct Bubble {
    /// The send.
    pub send_id: SendId,
    /// Its ask, if any (asks stay until resolved).
    pub ask_id: Option<AskId>,
    /// Whether `peek.show` reached the current UI connection.
    pub pushed: bool,
    /// When a non-ask bubble must be closed by peekd if the UI never reports
    /// it done.
    pub deadline: Option<Instant>,
    /// Whether it carries a `--show`.
    pub has_show: bool,
    /// Whether it carries a `--speak`.
    pub has_speak: bool,
}

/// A Silicon's bubbles: the one on screen and up to five waiting (§7.4).
#[derive(Clone, Debug, Default)]
pub struct ActorQueue {
    /// On screen (or to be shown when Peek.app connects).
    pub current: Option<Bubble>,
    /// Waiting, oldest first.
    pub waiting: VecDeque<Bubble>,
}

impl ActorQueue {
    /// Whether this queue holds `send_id`.
    #[must_use]
    pub fn contains(&self, send_id: &SendId) -> bool {
        self.current.as_ref().is_some_and(|b| &b.send_id == send_id)
            || self.waiting.iter().any(|b| &b.send_id == send_id)
    }
}

/// State changed only under [`Shared::core`].
#[derive(Debug, Default)]
pub struct Core {
    /// Queues by Silicon.
    pub queues: BTreeMap<ActorKey, ActorQueue>,
    /// Asks being resolved right now (prevents double resolution).
    pub resolving: HashSet<AskId>,
}

impl Core {
    /// Finds the queue holding a send.
    #[must_use]
    pub fn queue_of(&self, send_id: &SendId) -> Option<ActorKey> {
        self.queues
            .iter()
            .find(|(_, q)| q.contains(send_id))
            .map(|(k, _)| k.clone())
    }
}

/// A result handed to a `send --wait` connection; it answers whether the
/// result reached the CLI.
#[derive(Debug)]
pub struct WaiterMsg {
    /// The final state.
    pub result: AskResult,
    /// Written to the CLI (true) or not.
    pub ack: oneshot::Sender<bool>,
}

/// Everything the daemon's tasks share.
#[derive(Debug)]
pub struct Shared {
    /// Configuration.
    pub cfg: DaemonConfig,
    /// Filesystem layout.
    pub paths: Paths,
    /// `peekd.sqlite`.
    pub db: Db,
    /// `settings.json`.
    pub settings: SettingsStore,
    /// Peek.app.
    pub ui: UiHub,
    /// Queues (one async lock serializes every bubble transition).
    pub core: tokio::sync::Mutex<Core>,
    /// `send --wait` connections by ask.
    pub waiters: Mutex<HashMap<AskId, oneshot::Sender<WaiterMsg>>>,
    /// peek-server.
    pub net: Net,
    /// Deepgram and the TTS cache.
    pub speech: Speech,
    /// Telemetry relay.
    pub telemetry: Telemetry,
    /// Wakes the outbox worker.
    pub outbox_wake: Notify,
    /// Wakes the expiry/watchdog timer.
    pub timers_wake: Notify,
    /// Wakes the updater.
    pub update_wake: Notify,
    /// Set while an update is about to quit Peek.app: no new bubble is
    /// pushed to the UI (it is shown by the next UI, or when the update
    /// backs off), so the swap never interrupts an ask that just arrived.
    pub update_swapping: AtomicBool,
    /// Set to true to shut down.
    pub shutdown: watch::Sender<bool>,
    /// The exit status requested by a component (the updater exits 0).
    pub exit_request: Mutex<Option<i32>>,
    /// Last pre-warm per home.
    pub prewarmed: Mutex<HashMap<String, Instant>>,
    /// When peekd last asked `LaunchServices` to open Peek.app.
    pub last_launch: Mutex<Option<Instant>>,
    /// The `UUIDv7` of this process (telemetry `instance_id`).
    pub instance_id: String,
    /// When the daemon started.
    pub started: Instant,
}

impl Shared {
    /// Requests shutdown with `code`.
    pub fn request_exit(&self, code: i32) {
        if let Ok(mut g) = self.exit_request.lock() {
            g.get_or_insert(code);
        }
        self.shutdown.send_replace(true);
    }

    /// Whether shutdown was requested.
    #[must_use]
    pub fn shutting_down(&self) -> bool {
        *self.shutdown.borrow()
    }
}

/// A handle shared by tasks.
pub type SharedRef = Arc<Shared>;
