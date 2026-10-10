//! The daemon's shared state: configuration, database, the UI link, the
//! per-Silicon bubble queues and `--wait` waiters.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::{Duration, Instant},
};

use silicon_peek_client::{
    identity::{AccountId, ActorId, ApiUrl, Context},
    ids::{AskId, SendId},
    ipc::cli::AskResult,
    runtime::Store,
    schema::limits,
    timestamp::Timestamp,
};
use tokio::sync::{Notify, oneshot, watch};

use crate::{
    config::DaemonConfig, db::Db, net::HomeRef, net::Net, paths::Paths, settings::SettingsStore,
    speech::Speech, telemetry::Telemetry, ui::UiHub,
};

/// A Silicon in a data context: the key of slots, drawings, asks and
/// deliveries. Ownership uses the immutable account UUID; the public handle is metadata.
#[derive(Clone, Debug)]
pub struct ActorKey {
    /// `production` or a testing environment.
    pub context: Context,
    /// The account.
    pub account: AccountId,
    /// The Silicon.
    pub actor: ActorId,
}

impl PartialEq for ActorKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for ActorKey {}
impl PartialOrd for ActorKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ActorKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.context, &self.account, self.actor.actor_type()).cmp(&(
            other.context,
            &other.account,
            other.actor.actor_type(),
        ))
    }
}
impl std::hash::Hash for ActorKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(
            &(self.context, &self.account, self.actor.actor_type()),
            state,
        );
    }
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
    /// Whether the session slot records an active Ting enrollment.
    pub ting_subscribed: Option<bool>,
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
#[allow(clippy::struct_excessive_bools)] // independent facts: pushed, shown, show and speak parts
pub struct Bubble {
    /// The send.
    pub send_id: SendId,
    /// Its ask, if any (asks stay until resolved).
    pub ask_id: Option<AskId>,
    /// Whether `peek.show` reached the current UI connection.
    pub pushed: bool,
    /// Whether the UI reported `shown` for it (or, for a Peek.app older than
    /// build 1002, whether `peek.show` reached it).
    pub shown: bool,
    /// When a non-ask bubble must be closed by peekd if the UI never reports
    /// it done (armed when it is shown).
    pub deadline: Option<Instant>,
    /// Whether it carries a `--show`.
    pub has_show: bool,
    /// Whether it carries a `--speak`.
    pub has_speak: bool,
    /// `--replace`: the send this one took over, for its `peek.show`.
    pub replaces: Option<SendId>,
}

impl Bubble {
    /// A bubble not yet pushed to the UI.
    #[must_use]
    pub fn new(send_id: SendId, ask_id: Option<AskId>, has_show: bool, has_speak: bool) -> Self {
        Self {
            send_id,
            ask_id,
            pushed: false,
            shown: false,
            deadline: None,
            has_show,
            has_speak,
            replaces: None,
        }
    }
}

/// A Silicon's queue (contract §6.2): the current send (on screen or held),
/// up to five waiting, and due scheduled sends waiting for a free spot.
///
/// Invariants: no current ⇒ nothing waits; overflow only while five wait;
/// at most five wait.
#[derive(Clone, Debug, Default)]
pub struct ActorQueue {
    /// On screen (or held until the Carbon or Peek.app is back).
    pub current: Option<Bubble>,
    /// Waiting, oldest first (at most [`limits::QUEUE_MAX`]).
    pub waiting: VecDeque<Bubble>,
    /// Due scheduled sends waiting for a free spot (`sends.overflow = 1`).
    pub overflow: VecDeque<Bubble>,
    /// The waiting count last sent to the UI (`peek.show` or `queue.state`).
    pub last_badge: Option<u32>,
}

impl ActorQueue {
    /// Whether this queue holds `send_id`.
    #[must_use]
    pub fn contains(&self, send_id: &SendId) -> bool {
        self.current.as_ref().is_some_and(|b| &b.send_id == send_id)
            || self.waiting.iter().any(|b| &b.send_id == send_id)
            || self.overflow.iter().any(|b| &b.send_id == send_id)
    }

    /// Sends waiting behind the current one: waiting plus overflow.
    #[must_use]
    pub fn waiting_count(&self) -> u32 {
        u32::try_from(self.waiting.len() + self.overflow.len()).unwrap_or(u32::MAX)
    }

    /// Whether the queue holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.current.is_none() && self.waiting.is_empty() && self.overflow.is_empty()
    }

    /// Whether the §6.2 invariants hold.
    #[must_use]
    pub fn invariants_hold(&self) -> bool {
        (self.current.is_some() || (self.waiting.is_empty() && self.overflow.is_empty()))
            && (self.overflow.is_empty() || self.waiting.len() == limits::QUEUE_MAX)
            && self.waiting.len() <= limits::QUEUE_MAX
    }
}

/// peekd's wall clock: unix time plus an offset tests use to pretend the
/// Mac slept ([`crate::DaemonHandle::advance_wall_clock`]). Expiry and
/// scheduling compare these times on every timer pass, never tokio's
/// monotonic clock (which stops while the Mac sleeps).
#[derive(Debug, Default)]
pub struct WallClock {
    offset_ms: AtomicI64,
}

impl WallClock {
    /// Now, in unix milliseconds.
    #[must_use]
    pub fn now_ms(&self) -> i64 {
        Timestamp::now()
            .unix_ms()
            .saturating_add(self.offset_ms.load(Ordering::SeqCst))
    }

    /// Moves the clock forward.
    pub fn advance(&self, by: Duration) {
        let ms = i64::try_from(by.as_millis()).unwrap_or(i64::MAX);
        self.offset_ms.fetch_add(ms, Ordering::SeqCst);
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
    /// `ElevenLabs` TTS, `OpenAI` STT, and the local audio cache.
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
    /// The wall clock expiry and scheduling use.
    pub clock: WallClock,
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

    /// The wall clock, in unix milliseconds.
    #[must_use]
    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }
}

/// A handle shared by tasks.
pub type SharedRef = Arc<Shared>;
