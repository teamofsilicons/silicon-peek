//! The `peek` grammar (BLUEPRINT §7.1–§7.2). One definition drives parsing,
//! `--help`, the leaf help printed after a missing argument, and
//! `peek commands --json` (see `experience.rs`).
//!
//! Every node says what it is for (`about`), how it is used with its
//! neighbours (`long_about`), then lists its flags; `experience::notes` adds
//! Examples, `Next:` and the Docs/Source/Rust links as `after_help`.

// Doc comments here are clap's help text, printed verbatim to users and
// agents; Markdown backticks and link brackets would show up literally.
#![allow(clippy::doc_markdown, clippy::doc_link_with_quotes)]

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// The root `after_help`, verbatim from BLUEPRINT §7.2.
pub const ROOT_AFTER_HELP: &str = "\
Start (Silicon):  peek iam --json · peek login <SLT> · peek register side <1-8> · peek register drawing ./logo.js
Then:             peek send --speak \"…\" --show '{…}'   |   peek send --speak \"…\" --ask '{…}'
Answers arrive as Ting events of type peek.ask.answered (route them in your flow; see peek docs ting).
State: $SILICON_HOME/.peek (else ~/.peek). Test mode: peek --test <env-uuid> <command>.
Explore: peek commands · peek <command> --help · peek docs <topic>
Docs: https://peek.teamofsilicons.com/docs · Source: https://github.com/teamofsilicons/silicon-peek
Rust: https://crates.io/crates/silicon-peek-client · Bugs: peek report --help";

/// Peek: quick, voice-first exchanges between Carbons and Silicons on a Mac.
#[derive(Debug, Parser)]
#[command(
    name = "peek",
    bin_name = "peek",
    version,
    about = "Speak, show and ask on a Carbon's Mac screen, and get the answer back through Ting.",
    long_about = "Peek gives each Silicon one of eight positions around the Mac screen and a small \
JavaScript drawing for its bubble. With the peek CLI a Silicon speaks a sentence (Deepgram Aura-2), \
shows up to three text or image elements, or asks one self-contained question (text, single choice, \
multiple choice, slider or range). Peek.app draws the bubble with Liquid Glass; the Carbon answers by \
voice, keyboard or click, and the answer comes back to the asking Silicon as a Ting event.\n\n\
The CLI is a tree: every command explains what it is for, how it fits with its neighbours, then its \
flags. It never prompts, prints exactly one JSON value with --json, and every error says what failed, \
why, and the command that fixes it.",
    after_help = ROOT_AFTER_HELP,
    subcommand_required = true,
    arg_required_else_help = true,
    disable_help_subcommand = true,
    max_term_width = 110
)]
pub struct Cli {
    /// Flags every command accepts.
    #[command(flatten)]
    pub global: GlobalArgs,
    /// The command to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Global flags (BLUEPRINT §7.1). Environment variables are read by peek
/// itself (not by clap) so an empty value is reported as invalid instead of
/// being silently ignored.
#[derive(Debug, Args, Clone, Default)]
pub struct GlobalArgs {
    /// Print exactly one JSON value on stdout. On failure stdout stays empty and one
    /// {"error":{…}} object goes to stderr.
    #[arg(long, global = true)]
    pub json: bool,

    /// Organization for org-specific calls [env: SILICON_ORG]. Precedence: --org, then
    /// SILICON_ORG, then the session's org. An empty value is invalid.
    #[arg(long, global = true, value_name = "ORG")]
    pub org: Option<String>,

    /// Use a saved testing environment by its UUID [env: SILICON_PEEK_TEST]. Save one first
    /// with --app-secret-file. A secret passed here is refused.
    #[arg(long, global = true, value_name = "ENV_UUID")]
    pub test: Option<String>,

    /// peek's test app secret (ask_…) for a testing environment; `-` reads one line from
    /// stdin. peek discovers the environment with it and saves it to testing.json.
    #[arg(
        long,
        global = true,
        value_name = "PATH|-",
        conflicts_with = "app_secret"
    )]
    pub app_secret_file: Option<String>,

    /// peek's test app secret inline [env: PEEK_TEST_APP_SECRET, value hidden]. Prefer
    /// --app-secret-file - so the secret stays out of shell history.
    #[arg(long, global = true, value_name = "ask_…", hide_env_values = true)]
    pub app_secret: Option<String>,

    /// peek-server origin [env: PEEK_API_URL; default https://backend.peek.teamofsilicons.com].
    /// HTTPS only, except http on loopback.
    #[arg(long, global = true, value_name = "URL")]
    pub api: Option<String>,

    /// Override the generated idempotency key of `peek ting enroll` or `peek report`
    /// (16–255 visible ASCII). Reuse a key only to retry the exact same request.
    #[arg(long, global = true, value_name = "KEY")]
    pub idempotency_key: Option<String>,

    /// Turn telemetry off for this process [also off when PEEK_TELEMETRY,
    /// SPACE_STATION_TELEMETRY or SILICON_TELEMETRY is 0/false/off/no].
    #[arg(long, global = true)]
    pub no_telemetry: bool,

    /// No human hints or `Next:` lines on stderr (errors are still printed). NO_COLOR is
    /// honoured; peek never colours its own output.
    #[arg(short, long, global = true)]
    pub quiet: bool,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// IAM discovery: static, offline, works before login (Stemcell contract).
    #[command(
        long_about = "Prints peek's IAM discovery document: the app id `peek`, its owning org `tos`, \
the backend, IAM and consent URLs, how to mint an SLT, and where the docs, source and Rust client live. \
It needs no network and no session, and has no side effects, so Stemcell can call it on every \
candidate binary. With --test it also reads the environment's name and generation from the backend.\n\n\
Use it first, before `peek login`, to learn the app id to pass to `iam silicon-login --app-id peek`."
    )]
    Iam,

    /// Exchange an IAM short-lived token (SLT) for this home's peek session (Stemcell contract).
    #[command(
        long_about = "Exchanges an IAM short-lived token (SLT) for this home's peek session, stored only \
in $SILICON_HOME/.peek/session.json (0600). The SLT is single use and lives two minutes; mint it with \
`iam silicon-login --app-id peek --grant-org <org> --approve-scopes` (a Silicon) or `iam login --app-id \
peek --grant-org <org>` (a Carbon).\n\n\
The exchange is idempotent (its key is derived from the SLT) and retried on transport errors and 5xx. \
If the outcome stays uncertain, `peek login --recover` replays the same exchange within 10 minutes. On \
success peek enrolls you as a Ting recipient, attaches this home to peekd, and on a Mac installs and \
starts Peek.app in the background after printing its result. `peek login status` verifies the session \
later; `peek logout` ends it.",
        args_conflicts_with_subcommands = true,
        subcommand_precedence_over_arg = true
    )]
    Login(LoginArgs),

    /// Revoke this home's peek session (Stemcell contract).
    #[command(
        long_about = "Ends this home's peek session: writes a logged-out marker, revokes the refresh \
family at the backend, and cancels this Silicon's undelivered answers on this Mac. Prints \
{\"authenticated\":false,\"remote_revocation\":\"confirmed\"|\"pending\"} and exits 0 either way: \
`pending` means the local session is gone and the backend revocation is retried by the next peek run. \
Log in again with `peek login '<SLT>'`.\n\n\
The Ting recipient grant belongs to the Silicon, not to this home, so other homes of the same Silicon \
(Stemcell and a hand-run home, for example) keep receiving answers. Add --revoke-ting to remove the \
grant too; every home of this Silicon then needs `peek ting enroll`."
    )]
    Logout(LogoutArgs),

    /// This home's configuration: strict JSON merge, show, get, unset, telemetry, home.
    #[command(
        long_about = "Configuration for this home, stored in $SILICON_HOME/.peek/config.json. \
`peek config set '<json-object>'` is the Stemcell contract: a strict merge of known keys (telemetry, \
voice, language, notify, api_url, delivery_max_age_hours); `null` resets a key. The result is printed \
and pushed to peekd.",
        subcommand_required = true,
        arg_required_else_help = true
    )]
    Config {
        /// The config action.
        #[command(subcommand)]
        command: ConfigCommand,
    },

    /// Ting recipient enrollment for this Silicon.
    #[command(
        long_about = "peek delivers answers, dismissals and Carbon messages to the asking Silicon \
through Ting. `peek login` enrolls the Silicon as a Ting recipient for peek; `peek ting enroll` \
repeats that explicitly. peek never re-enrolls on its own, because that would undo a grant the Silicon \
revoked on purpose.",
        subcommand_required = true,
        arg_required_else_help = true
    )]
    Ting {
        /// The Ting action.
        #[command(subcommand)]
        command: TingCommand,
    },

    /// Claim a position on the screen, or register the bubble's drawing.
    #[command(
        long_about = "Before a Silicon can send, it needs a position (`peek register side <1-8>`) \
and a drawing (`peek register drawing ./logo.js`). Positions are 1 top, then clockwise to 8 top-left; \
each Silicon holds exactly one. The drawing is a small JavaScript file that Peek.app validates and \
runs for the bubble.",
        subcommand_required = true,
        arg_required_else_help = true
    )]
    Register {
        /// What to register.
        #[command(subcommand)]
        command: RegisterCommand,
    },

    /// Release the position and delete the drawing (locally and on the server).
    #[command(
        long_about = "Releases this Silicon's position and shortcut, deletes its drawing on this Mac \
and on the backend, and cancels its pending asks, queued sends and scheduled sends. No Ting events are \
sent. Register again with `peek register side <1-8>` and `peek register drawing <FILE.js>`."
    )]
    Unregister,

    /// Speak, show or ask on the Carbon's screen; returns immediately (or waits with --wait).
    #[command(
        long_about = "Speaks a sentence, shows up to three text or image elements, or asks one \
question in this Silicon's bubble. Everything is validated before anything is shown, and the command \
returns as soon as the bubble is queued: {\"send_id\",\"ask_id\",\"slot\",\"status\",\"speech\",\"warnings\",\
\"queue_position\",\"waiting\",\"expires_at\",\"schedule_id\",\"due_at\",\"tz\",\"replaced_send_id\"}.\n\n\
Sends always queue: a new send never replaces what is on screen. It is shown when the one before it is \
done, in order; at most five wait behind the one on screen (then queue_full, exit 4; see `peek queue`). \
--replace takes over this Silicon's bubble at once. --expires-in / --expires-at drop a send that is not \
shown or finished in time (you get peek.send.expired or peek.ask.expired). --in / --at schedule it \
instead (see `peek schedule list`).\n\n\
Answers to --ask arrive later as Ting events of type peek.ask.answered (see `peek docs ting`); inspect \
them locally with `peek ask get <ASK_ID>`. With --wait the command stays open and prints the answer \
itself (then no Ting event is sent for it). Needs a position and a drawing first (`peek register`)."
    )]
    Send(Box<SendArgs>),

    /// This Silicon's queue on this position: the send on screen and the ones waiting (at most 5).
    #[command(
        args_conflicts_with_subcommands = true,
        long_about = "Lists this Silicon's queue on this Mac: the send on screen (or held while the \
Carbon's screen is locked, Peek is paused or Peek.app is not running) and the ones waiting behind it, \
in the order they will be shown, with their IDs, kind, age, summary and deadline. At most five wait; \
a send that would be the sixth fails with queue_full (exit 4). Remove one with `peek cancel <SEND_ID>` \
or every waiting one with `peek queue clear`. Scheduled sends are counted here and listed by \
`peek schedule list`."
    )]
    Queue {
        /// The queue action (default: list).
        #[command(subcommand)]
        command: Option<QueueCommand>,
    },

    /// Withdraw one send: on screen, waiting or scheduled (any kind, asks included). No Ting event is sent.
    #[command(
        long_about = "Withdraws one of this Silicon's sends wherever it is: the bubble on screen slides \
away (its speech stops), a waiting one is dropped before it is shown, a scheduled one is never sent. An \
ask is cancelled without an answer. Nothing is sent to Ting for your own action. A send that already \
closed is reported as it closed (exit 0). Takes a send ID (snd_…), an ask ID (ask_…) or a schedule ID \
(sch_…)."
    )]
    Cancel {
        /// snd_…, ask_… or sch_…
        #[arg(value_name = "SEND_ID")]
        id: String,
    },

    /// One-time scheduled sends (peek send --in / --at): list, cancel, clear.
    #[command(
        long_about = "Scheduled sends are made with `peek send … --in <DURATION>` or `--at <DATETIME>` \
(one time, no recurrence; at most 500 per Silicon). When one comes due it joins this Silicon's queue \
like any send and you receive peek.schedule.due, then peek.send.shown when it appears. If the Mac \
sleeps or Peek.app is not running at the due time, it is shown once they are back (unless its \
--expires-at passed: then it expires unseen).",
        subcommand_required = true,
        arg_required_else_help = true
    )]
    Schedule {
        /// The schedule action.
        #[command(subcommand)]
        command: ScheduleCommand,
    },

    /// Local state of this Silicon's asks: get, list, cancel.
    #[command(
        long_about = "Reads the asks peekd keeps for this Silicon on this Mac. Works even when Ting \
delivery is not set up. `peek ask cancel` slides an ask away without sending a Ting event.",
        subcommand_required = true,
        arg_required_else_help = true
    )]
    Ask {
        /// The ask action.
        #[command(subcommand)]
        command: AskCommand,
    },

    /// This Silicon's recent sends on this Mac, newest first.
    #[command(
        long_about = "Lists this Silicon's recent sends on this Mac, newest first: kind, timestamps, \
how each closed, and the state of its ask. History never leaves the Mac. Page with --before \
<SEND_ID>; inspect one ask with `peek ask get <ASK_ID>`."
    )]
    History(HistoryArgs),

    /// One view: position, drawing, queue, asks, deliveries, Peek.app and peekd.
    #[command(
        long_about = "Everything that matters for this Silicon on this Mac in one view: its position, \
drawing, queued sends, pending asks, delivery backlog (and whether deliveries wait for a valid session \
or Ting enrollment), and whether Peek.app and peekd run. For identity use `peek login status`; for \
fixes use `peek doctor`."
    )]
    Status,

    /// Org-wide settings (org admins only).
    #[command(
        long_about = "Settings that apply to a whole organization. Only its owners and admins can \
change them; the org is --org, then SILICON_ORG, then the session's org.",
        subcommand_required = true,
        arg_required_else_help = true
    )]
    Org {
        /// The org setting.
        #[command(subcommand)]
        command: OrgCommand,
    },

    /// Manage Peek.app on this Mac: status, install, update, uninstall.
    #[command(
        long_about = "Peek.app draws the bubbles and hosts peekd, the per-user helper. It lives at \
~/Applications/Peek.app and updates itself. Every command that needs the Mac installs and starts it on \
demand; these commands do it explicitly. On Linux and Windows `peek app status` answers \
{\"supported\":false} and the rest exit 4.",
        subcommand_required = true,
        arg_required_else_help = true
    )]
    App {
        /// The app action.
        #[command(subcommand)]
        command: AppCommand,
    },

    /// peekd, the per-user helper inside Peek.app: status, restart.
    #[command(
        long_about = "peekd runs inside Peek.app as a launchd agent, one per macOS user. It keeps \
positions, queues, asks and the delivery outbox, calls Deepgram, and serves every Silicon home on this \
Mac over /var/tmp/silicon-peek-<uid>/peekd.sock.",
        subcommand_required = true,
        arg_required_else_help = true
    )]
    Daemon {
        /// The daemon action.
        #[command(subcommand)]
        command: DaemonCommand,
    },

    /// Bundled offline manuals.
    #[command(
        long_about = "The pages of https://peek.teamofsilicons.com/docs, bundled into the binary so \
they work offline. Without a topic it lists them; --search finds lines across all of them."
    )]
    Docs(DocsArgs),

    /// Machine-readable command tree, generated from this grammar.
    #[command(
        long_about = "Lists every public command with its description, usage, full help and \
arguments. With --json it prints the whole tree as data for agents; it is generated from the same \
definitions as --help, so it is always complete."
    )]
    Commands,

    /// File a bug report, optionally with the pull request that fixes it.
    #[command(
        long_about = "Files a bug with the peek maintainers. peek is open source \
(https://github.com/teamofsilicons/silicon-peek): the best report says exactly how to reproduce the \
bug, and links the pull request that fixes it with --pr. By default the backend stores the report and \
files a GitHub issue; --via gh files it with your own `gh` instead. Only your text, the PR, the peek \
version and platform (plus `peek doctor` output with --attach-status) are sent; no logs or credentials. \
Reports are not retried automatically."
    )]
    Report(ReportArgs),

    /// How peek is updated (Honeycomb); never replaces itself.
    #[command(
        long_about = "peek never replaces its own binary. Honeycomb's per-home worker updates the CLI \
every minute, and peekd updates Peek.app. This prints what is installed and the command to update by \
hand: honeycomb update 'peek'."
    )]
    Update,

    /// Diagnostics with exact fixes.
    #[command(
        long_about = "Runs every check and prints the exact fix for anything that is wrong: store \
permissions, session and rejection state, backend readiness, Honeycomb version, peekd socket and \
protocol, Peek.app install and signature, the background item, the install log, microphone \
permission, hotkeys, the delivery backlog and Ting enrollment. Checks that cannot run degrade to a \
warning; the command itself exits 0."
    )]
    Doctor,

    /// Internal: install and start Peek.app after `peek login`, then attach this home.
    #[command(name = "__after-login", hide = true)]
    AfterLogin(AfterLoginArgs),
}

/// `peek logout`.
#[derive(Debug, Args)]
pub struct LogoutArgs {
    /// Also revoke this Silicon's Ting recipient grant. It is shared by every home of the
    /// Silicon: answers stop reaching all of them until `peek ting enroll`.
    #[arg(long)]
    pub revoke_ting: bool,
}

/// `peek login`.
#[derive(Debug, Args)]
pub struct LoginArgs {
    /// The IAM short-lived token. Prefer --token-file - to keep it out of process lists.
    #[arg(value_name = "SLT")]
    pub slt: Option<String>,

    /// Read the SLT from a file, or from stdin with `-` (one line; the trailing newline is
    /// stripped). stdin must not be a terminal: peek never prompts.
    #[arg(long, value_name = "PATH|-", conflicts_with = "slt")]
    pub token_file: Option<String>,

    /// Replay an interrupted login (same SLT, same idempotency key) within 10 minutes.
    #[arg(long, conflicts_with_all = ["slt", "token_file"])]
    pub recover: bool,

    /// `peek login status`.
    #[command(subcommand)]
    pub command: Option<LoginCommand>,
}

/// `peek login …` subcommands.
#[derive(Debug, Subcommand)]
pub enum LoginCommand {
    /// Live-verified identity: {"authenticated":bool,…} (Stemcell contract).
    #[command(
        long_about = "Reports whether this home has a working peek session, verified live: peek \
refreshes the access token if it expires within 60 s, then asks the backend who it is. It answers \
{\"authenticated\":false,\"reason\":\"no_session\"|\"logged_out\"|\"rejected\",…} with exit 0 when there is \
no usable session, and exits 5 with nothing on stdout when the backend or IAM cannot be reached (so a \
network problem is never mistaken for a logout). peekd is never asked whether you are authenticated; \
a missing peekd only sets daemon.attached:false."
    )]
    Status,
}

/// `peek config …`.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Strictly merge a JSON object into config.json and print the result (Stemcell contract).
    #[command(
        long_about = "Merges one JSON object into this home's config.json atomically and prints the \
resulting config. Parsing is strict: a non-object, a duplicate key or an unknown key fails with exit 2 \
and details.valid_keys, and nothing is written. `null` resets a key to its default. The result is \
also pushed to peekd (best effort).\n\n\
Keys: telemetry (bool), voice (aura-2-<name>-<en|es|de|fr|nl|it|ja> or null), language (BCP 47 primary \
subtag or null), notify (subset of [\"speech_finished\",\"show_dismissed\",\"shown\"]), api_url (https origin or \
null), delivery_max_age_hours (1–168)."
    )]
    Set {
        /// One JSON object, for example '{"notify":["speech_finished"]}'.
        #[arg(value_name = "JSON-OBJECT")]
        object: String,
    },
    /// Print the whole config.
    Show,
    /// Print one key's value.
    Get {
        /// The key (telemetry, voice, language, notify, api_url, delivery_max_age_hours).
        #[arg(value_name = "KEY")]
        key: String,
    },
    /// Reset one key to its default.
    Unset {
        /// The key to reset.
        #[arg(value_name = "KEY")]
        key: String,
    },
    /// Turn this home's telemetry on or off.
    Telemetry {
        /// on or off.
        #[arg(value_enum, value_name = "on|off")]
        state: OnOff,
    },
    /// Move this home's store to DIR/.peek (a pointer file stays in the default store).
    #[command(
        long_about = "Moves this home's peek store to <DIR>/.peek and writes a pointer file named \
`home` into the default store ($SILICON_HOME/.peek), so every later peek run finds it. The session, \
config, testing environments and daemon token are moved; if the target already holds a different \
peek store, nothing is changed. Point back with `peek config home \"$SILICON_HOME\"`."
    )]
    Home {
        /// An existing directory; the store becomes <DIR>/.peek.
        #[arg(value_name = "DIR")]
        dir: PathBuf,
    },
}

/// `on` / `off`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OnOff {
    /// Enable.
    On,
    /// Disable.
    Off,
}

/// `peek ting …`.
#[derive(Debug, Subcommand)]
pub enum TingCommand {
    /// (Re)register this Silicon as a Ting recipient for peek.
    #[command(
        long_about = "Registers this Silicon as a Ting recipient for peek (or re-registers it). Run \
it when `peek login status` shows \"ting\":{\"subscribed\":false} or `peek status` reports deliveries \
waiting on recipient_not_registered. Prints {\"subscribed\":true,\"subscription_id\":\"sub_…\"}."
    )]
    Enroll,
}

/// `peek register …`.
#[derive(Debug, Subcommand)]
pub enum RegisterCommand {
    /// Claim a position, or move to it (1 top, then clockwise to 8 top-left).
    #[command(
        long_about = "Claims a position on the screen for this Silicon, or moves it there. Positions: \
1 top, 2 top-right, 3 right, 4 bottom-right, 5 bottom, 6 bottom-left, 7 left, 8 top-left. A Silicon \
holds exactly one: registering another number moves it (moved_from names the old one) and its drawing \
keeps running. A position held by another Silicon fails with side_taken (exit 4) and lists the free \
ones. The Carbon reaches the bubble with ctrl+cmd+<position> (the modifier is configurable in \
Peek → Settings; the JSON `hotkey` names the current one)."
    )]
    Side {
        /// The position, 1–8.
        #[arg(value_name = "1-8")]
        index: u64,
    },
    /// Validate, store and activate the bubble's JavaScript drawing.
    #[command(
        long_about = "Reads a JavaScript drawing (relative to the current directory, at most 256 \
KiB), validates it inside Peek.app for 90 offscreen frames with the same engine that draws on screen, \
and activates it for this Silicon. A failing drawing exits 4 with drawing_invalid and the previous \
drawing stays active. Validation can start Peek.app, so it may take up to 120 s on a cold start. The \
drawing API is in `peek docs drawing`."
    )]
    Drawing {
        /// The drawing file (.js), resolved against the current directory.
        #[arg(value_name = "FILE.js")]
        file: PathBuf,
        /// Validate only; keep the active drawing.
        #[arg(long)]
        check: bool,
        /// Also write a PNG grid of the test frames to this path.
        #[arg(long, value_name = "OUT.png")]
        preview: Option<PathBuf>,
        /// Print this test frame's display list (JSON). Test frames are
        /// numbered from 0 (0–89), as in validation errors.
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(0..90))]
        dump_frame: Option<u32>,
    },
}

/// `peek send`.
#[derive(Debug, Args)]
pub struct SendArgs {
    /// Speak this text with Deepgram Aura-2 (1–2000 characters).
    #[arg(long, value_name = "TEXT")]
    pub speak: Option<String>,

    /// Show 1–3 text or image elements: JSON, @FILE (relative to the current directory) or
    /// `-` (stdin). {"elements":[{"type":"text","text":"…"},{"type":"image","path":"./a.png","caption":"…"}]}
    #[arg(long, value_name = "JSON|@FILE|-")]
    pub show: Option<String>,

    /// Ask one question: JSON, @FILE or `-`. {"question":"…","type":"single_choice","options":["Yes","No"]};
    /// types: text, single_choice, multiple_choice, slider, range.
    #[arg(long, value_name = "JSON|@FILE|-")]
    pub ask: Option<String>,

    /// TTS voice, overriding config `voice` and the per-language default.
    #[arg(long, value_name = "aura-2-NAME-LANG")]
    pub voice: Option<String>,

    /// Force the TTS language instead of detecting it (en, es, de, fr, nl, it, ja).
    #[arg(long, value_name = "BCP47")]
    pub lang: Option<String>,

    /// Seconds a --show stays up without speech, or after the speech ends (1–120).
    #[arg(long, value_name = "SECS")]
    pub duration: Option<u64>,

    /// Drop the send if it is not shown/finished in time: 90s, 15m, 2h, 1d, 1h30m or plain
    /// seconds (10 s – 7 d). Any kind. An ask that expires sends peek.ask.expired; a show or
    /// speak sends peek.send.expired.
    #[arg(long, value_name = "DURATION")]
    pub expires_in: Option<String>,

    /// Absolute deadline: 2026-09-27T18:00, "2026-09-27 18:00", 18:00, …Z or …+05:30 (Mac
    /// local time unless an offset or --tz is given). Not with --expires-in.
    #[arg(long, value_name = "DATETIME")]
    pub expires_at: Option<String>,

    /// Schedule: send after this long (1 s – 365 d; same units as --expires-in). One-time,
    /// no recurrence.
    #[arg(id = "in", long = "in", value_name = "DURATION")]
    pub in_: Option<String>,

    /// Schedule: send at this time (Mac local time unless an offset or --tz is given; past
    /// times are refused).
    #[arg(long, value_name = "DATETIME")]
    pub at: Option<String>,

    /// IANA time zone for --at/--expires-at values without an offset, e.g. Asia/Kolkata.
    #[arg(long, value_name = "IANA")]
    pub tz: Option<String>,

    /// Take over this Silicon's bubble now (any kind, even an ask: it is cancelled without a
    /// ting). Default: queue behind it.
    #[arg(long)]
    pub replace: bool,

    /// Opt in to Ting events for this send: speech_finished, show_dismissed, shown
    /// (comma-separated). Defaults to config `notify`.
    #[arg(long, value_name = "LIST")]
    pub notify: Option<String>,

    /// Asks only: keep the command open and print the answer (default 120 s, at most 600).
    /// Use --wait, --wait 60 or --wait=60. Not with --in/--at.
    #[arg(
        long,
        value_name = "SECS",
        num_args = 0..=1,
        default_missing_value = "120"
    )]
    pub wait: Option<u64>,
}

/// `peek queue …`.
#[derive(Debug, Subcommand)]
pub enum QueueCommand {
    /// Same as `peek queue`.
    List,
    /// Drop every waiting send (and due scheduled sends waiting for room); with --all also the one on screen.
    #[command(
        long_about = "Withdraws every send waiting in this Silicon's queue (asks are cancelled without \
an answer), including scheduled sends that came due and wait for room. The send on screen stays unless \
--all is given. No Ting events are sent."
    )]
    Clear {
        /// Also withdraw the send on screen.
        #[arg(long)]
        all: bool,
    },
}

/// `peek schedule …`.
#[derive(Debug, Subcommand)]
pub enum ScheduleCommand {
    /// Scheduled sends that are not due yet, soonest first.
    List,
    /// Cancel one scheduled send before it is due.
    Cancel {
        /// sch_… (or its snd_…)
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Cancel every scheduled send of this Silicon.
    Clear,
}

/// `peek ask …`.
#[derive(Debug, Subcommand)]
pub enum AskCommand {
    /// Local state and answer of one ask.
    #[command(
        long_about = "Prints one ask's local state: pending, answered (with the answer and how it \
was given), dismissed, expired or cancelled, plus its Ting delivery status. Works while Ting delivery \
is not set up."
    )]
    Get {
        /// The ask ID (ask_…), from `peek send`.
        #[arg(value_name = "ASK_ID")]
        ask_id: String,
    },
    /// This Silicon's asks, newest first.
    List {
        /// Only asks in this state.
        #[arg(long, value_enum, value_name = "STATE")]
        state: Option<AskStateArg>,
        /// At most this many (1–200).
        #[arg(long, value_name = "N")]
        limit: Option<u32>,
    },
    /// Slide an ask away without answering; no Ting event is sent.
    Cancel {
        /// The ask ID (ask_…).
        #[arg(value_name = "ASK_ID")]
        ask_id: String,
    },
}

/// Ask states accepted by `peek ask list --state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum AskStateArg {
    /// Waiting for the Carbon.
    Pending,
    /// Answered.
    Answered,
    /// Closed by the Carbon without an answer.
    Dismissed,
    /// Ran past --expires-in or --expires-at.
    Expired,
    /// Cancelled by the Silicon.
    Cancelled,
    /// Taken over by the Silicon's own --replace.
    Replaced,
}

/// `peek history`.
#[derive(Debug, Args)]
pub struct HistoryArgs {
    /// At most this many items (1–200).
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,
    /// Only sends older than this send ID (paging).
    #[arg(long, value_name = "SEND_ID")]
    pub before: Option<String>,
}

/// `peek org …`.
#[derive(Debug, Subcommand)]
pub enum OrgCommand {
    /// Bring your own provider key for the org.
    #[command(subcommand_required = true, arg_required_else_help = true)]
    Byo {
        /// The provider.
        #[command(subcommand)]
        command: ByoCommand,
    },
}

/// `peek org byo …`.
#[derive(Debug, Subcommand)]
pub enum ByoCommand {
    /// The org's own Deepgram key for every member's speech and transcription.
    #[command(
        long_about = "Makes peek use the org's own Deepgram key for every member's speech and \
transcription. The key needs Deepgram's Member role or higher; it is validated before it is saved, \
stored sealed on the backend and never returned. Once set there is no silent fallback to peek's key: a \
key that stops working makes speech fail with speech_unavailable and a reason. Non-admins get \
not_org_admin (exit 4).",
        subcommand_required = true,
        arg_required_else_help = true
    )]
    Deepgram {
        /// The action.
        #[command(subcommand)]
        command: DeepgramCommand,
    },
}

/// `peek org byo deepgram …`.
#[derive(Debug, Subcommand)]
pub enum DeepgramCommand {
    /// Store the org's Deepgram key (validated first).
    Set {
        /// Read the key from a file, or from stdin with `-` (one line).
        #[arg(long, value_name = "PATH|-", required = true)]
        key_file: String,
        /// A Deepgram API base URL other than https://api.deepgram.com (for example the EU endpoint).
        #[arg(long, value_name = "URL")]
        base_url: Option<String>,
    },
    /// Whether a key is configured (the key itself is never returned).
    Show,
    /// Remove the org's key; peek's own key is used again.
    Delete,
}

/// `peek app …`.
#[derive(Debug, Subcommand)]
pub enum AppCommand {
    /// Installed build, signature, whether the app and peekd run, offered builds, install log.
    Status,
    /// Install ~/Applications/Peek.app from this CLI's package if missing, then start it.
    #[command(
        long_about = "Installs ~/Applications/Peek.app from the Peek.app.zip shipped next to this \
CLI if it is missing (verifying its Developer ID signature and removing quarantine), offers the \
bundled build to the running app, and starts it in the background. It never modifies an existing app; \
updates belong to peekd (`peek app update`)."
    )]
    Install,
    /// Ask peekd to install the newest offered build now (the swap waits until nothing is on screen).
    Update,
    /// Unregister the login item and helper, and move Peek.app to the Trash.
    #[command(
        long_about = "Asks Peek.app to unregister its login item and the peekd agent and to move \
itself to the Trash. `honeycomb uninstall 'peek'` removes only the CLI, never the app."
    )]
    Uninstall,
}

/// `peek daemon …`.
#[derive(Debug, Subcommand)]
pub enum DaemonCommand {
    /// Whether peekd runs, its version, protocol, socket, UI and attached homes (no login needed).
    Status,
    /// Restart peekd through launchd (or start Peek.app when the agent is not loaded).
    Restart,
}

/// `peek docs`.
#[derive(Debug, Args)]
pub struct DocsArgs {
    /// The topic to print: start, carbon, silicon, cli, show, ask, drawing, ting, iam, testing,
    /// telemetry, privacy, platforms, development, versioning, troubleshooting.
    #[arg(value_name = "TOPIC", conflicts_with_all = ["search", "all"])]
    pub topic: Option<String>,
    /// Search every topic; prints matching lines with excerpts (at most 320 characters).
    #[arg(long, value_name = "TEXT", conflicts_with = "all")]
    pub search: Option<String>,
    /// Print every topic.
    #[arg(long)]
    pub all: bool,
}

/// `peek report`.
#[derive(Debug, Args)]
pub struct ReportArgs {
    /// What you ran, what happened, and what you expected. The first line becomes the title.
    #[arg(value_name = "MESSAGE")]
    pub message: String,
    /// The pull request that fixes it: https://github.com/teamofsilicons/silicon-peek/pull/<n>.
    #[arg(long, value_name = "URL")]
    pub pr: Option<String>,
    /// How to file it: the peek backend (default) or your own GitHub CLI.
    #[arg(long, value_enum, default_value_t = ReportVia::Backend, value_name = "backend|gh")]
    pub via: ReportVia,
    /// Attach the non-secret `peek doctor` output.
    #[arg(long)]
    pub attach_status: bool,
    /// Print exactly what would be filed (the request body, or gh's title and body) and send
    /// nothing. Allowed in testing environments too.
    #[arg(long)]
    pub dry_run: bool,
}

/// Report transports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ReportVia {
    /// POST /api/v1/reports; the backend files a GitHub issue.
    Backend,
    /// `gh issue create --repo teamofsilicons/silicon-peek`.
    Gh,
}

/// Hidden `peek __after-login`.
#[derive(Debug, Args)]
pub struct AfterLoginArgs {
    /// The store directory that just logged in.
    #[arg(long, value_name = "DIR")]
    pub store: PathBuf,
    /// The session's API URL.
    #[arg(long = "api-url", value_name = "URL")]
    pub api_url: String,
    /// The session's context (production or an environment UUID).
    #[arg(long, value_name = "CONTEXT")]
    pub context: String,
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::{Cli, Command};

    fn wait_of(args: &[&str]) -> Option<u64> {
        let mut command_line = vec!["peek", "send", "--ask", r#"{"question":"q","type":"text"}"#];
        command_line.extend_from_slice(args);
        match Cli::try_parse_from(command_line).map(|c| c.command) {
            Ok(Command::Send(a)) => a.wait,
            other => panic!("unexpected parse: {other:?}"),
        }
    }

    #[test]
    fn dump_frame_is_zero_based() {
        let parse = |n: &str| {
            Cli::try_parse_from(["peek", "register", "drawing", "x.js", "--dump-frame", n])
                .map(|_| ())
        };
        assert!(parse("0").is_ok());
        assert!(parse("89").is_ok());
        assert!(parse("90").is_err(), "there are 90 test frames, 0–89");
    }

    fn parse(args: &[&str]) -> Result<Command, clap::Error> {
        let mut command_line = vec!["peek"];
        command_line.extend_from_slice(args);
        Cli::try_parse_from(command_line).map(|c| c.command)
    }

    #[test]
    fn new_send_flags_parse() {
        let Ok(Command::Send(a)) = parse(&[
            "send",
            "--speak",
            "x",
            "--in",
            "2h",
            "--tz",
            "Asia/Kolkata",
            "--replace",
            "--expires-in",
            "15m",
            "--expires-at",
            "18:00",
            "--at",
            "2026-09-27T18:00",
        ]) else {
            panic!("send parses");
        };
        assert_eq!(a.in_.as_deref(), Some("2h"));
        assert_eq!(a.at.as_deref(), Some("2026-09-27T18:00"));
        assert_eq!(a.tz.as_deref(), Some("Asia/Kolkata"));
        assert_eq!(a.expires_in.as_deref(), Some("15m"));
        assert_eq!(a.expires_at.as_deref(), Some("18:00"));
        assert!(a.replace);
        let Ok(Command::Send(a)) = parse(&["send", "--ask", "{}", "--expires-in", "60"]) else {
            panic!("plain seconds still parse");
        };
        assert_eq!(a.expires_in.as_deref(), Some("60"));
    }

    #[test]
    fn queue_cancel_and_schedule_parse() {
        use super::{QueueCommand, ScheduleCommand};
        assert!(matches!(
            parse(&["queue"]),
            Ok(Command::Queue { command: None })
        ));
        assert!(matches!(
            parse(&["queue", "list"]),
            Ok(Command::Queue {
                command: Some(QueueCommand::List)
            })
        ));
        assert!(matches!(
            parse(&["queue", "clear", "--all"]),
            Ok(Command::Queue {
                command: Some(QueueCommand::Clear { all: true })
            })
        ));
        assert!(matches!(
            parse(&["queue", "clear"]),
            Ok(Command::Queue {
                command: Some(QueueCommand::Clear { all: false })
            })
        ));
        assert!(matches!(parse(&["cancel", "snd_1"]), Ok(Command::Cancel { id }) if id == "snd_1"));
        assert!(parse(&["cancel"]).is_err());
        assert!(matches!(
            parse(&["schedule", "list"]),
            Ok(Command::Schedule {
                command: ScheduleCommand::List
            })
        ));
        assert!(matches!(
            parse(&["schedule", "cancel", "sch_1"]),
            Ok(Command::Schedule { command: ScheduleCommand::Cancel { id } }) if id == "sch_1"
        ));
        assert!(matches!(
            parse(&["schedule", "clear"]),
            Ok(Command::Schedule {
                command: ScheduleCommand::Clear
            })
        ));
        // `peek schedule` alone behaves exactly like `peek ask` alone.
        let schedule = parse(&["schedule"])
            .err()
            .map(|e| (e.kind(), e.exit_code()));
        let ask = parse(&["ask"]).err().map(|e| (e.kind(), e.exit_code()));
        assert!(schedule.is_some());
        assert_eq!(schedule, ask);
    }

    #[test]
    fn wait_takes_a_value_with_or_without_an_equals_sign() {
        assert_eq!(wait_of(&[]), None);
        assert_eq!(wait_of(&["--wait"]), Some(120));
        assert_eq!(wait_of(&["--wait", "5"]), Some(5));
        assert_eq!(wait_of(&["--wait=7"]), Some(7));
        assert_eq!(wait_of(&["--wait", "--json"]), Some(120));
    }
}
