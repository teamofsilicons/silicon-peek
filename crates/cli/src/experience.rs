//! Help, parse-error recovery and machine discovery, all derived from the one
//! grammar in `cli.rs` (the ACCOUNTS `experience.rs` pattern, notes/cli-patterns §3).
//!
//! - Globals are grouped under "Context and output" (positionals stay under
//!   "Arguments", flags under "Options"); env values are never shown.
//! - Every command gets an `after_help` with Examples, `Next:` and the
//!   Docs/Source/Rust links ([`notes`]). The root keeps BLUEPRINT §7.2's text.
//! - A missing required argument prints the leaf's full help after clap's
//!   error. With `--json` a parse error is one `{"error":{…}}` object instead.
//! - `peek commands [--json]` lists the same tree.

use clap::{Command, CommandFactory as _, FromArgMatches as _, error::ErrorKind};
use serde::Serialize;
use serde_json::{Value, json};
use std::fmt::Write as _;

use crate::cli::{Cli, ROOT_AFTER_HELP};

const LINKS: &str = "Source: https://github.com/teamofsilicons/silicon-peek · Rust: https://crates.io/crates/silicon-peek-client · Bugs: peek report --help";

/// The enriched grammar used for parsing, help and discovery.
pub fn command() -> Command {
    let mut command = enrich(Cli::command(), "peek");
    command.build();
    // clap wraps after_help to the terminal width; the root's closing block
    // is BLUEPRINT §7.2 text and must stay verbatim, so render the rest of
    // the root help and append that block unwrapped.
    let mut body = command
        .clone()
        .after_help(None::<&'static str>)
        .after_long_help(None::<&'static str>);
    let rendered = body.render_long_help().to_string();
    command.override_help(format!("{}\n\n{ROOT_AFTER_HELP}\n", rendered.trim_end()))
}

fn enrich(mut command: Command, path: &str) -> Command {
    command = command.mut_args(|arg| {
        // Positionals keep clap's "Arguments" and flags its "Options"
        // (shared with -h/--help); globals get their own section.
        let arg = arg.hide_env_values(true);
        if arg.is_global_set() {
            arg.help_heading("Context and output")
        } else {
            arg
        }
    });
    let names: Vec<String> = command
        .get_subcommands()
        .map(|c| c.get_name().to_owned())
        .collect();
    for name in names {
        let child_path = format!("{path} {name}");
        command = command.mut_subcommand(&name, |child| enrich(child, &child_path));
    }
    let notes = notes(path);
    command.after_help(notes.clone()).after_long_help(notes)
}

fn public(command: &Command) -> bool {
    !command.is_hide_set() && command.get_name() != "help"
}

/// What `main` does with the command line.
pub enum Parsed {
    /// Run this command; `path` is the subcommand chain (`["register", "side"]`).
    Run {
        /// The parsed arguments.
        cli: Box<Cli>,
        /// The subcommand chain.
        path: Vec<String>,
    },
    /// Help, version or a parse error was printed; exit with this code.
    Exit(u8),
}

/// Parses argv. On `--help`/`--version` and on errors it prints and returns
/// [`Parsed::Exit`] without touching the store, the network or peekd.
pub fn parse(args: Vec<std::ffi::OsString>) -> Parsed {
    let json = args.iter().any(|a| a == "--json");
    let mut command = command();
    let matches = match command.try_get_matches_from_mut(args) {
        Ok(m) => m,
        Err(error) => return Parsed::Exit(parse_error(&mut command, &error, json)),
    };
    let mut path = Vec::new();
    let mut selected = &matches;
    while let Some((name, child)) = selected.subcommand() {
        path.push(name.to_owned());
        selected = child;
    }
    match Cli::from_arg_matches(&matches) {
        Ok(cli) => Parsed::Run {
            cli: Box::new(cli),
            path,
        },
        Err(error) => Parsed::Exit(parse_error(&mut command, &error, json)),
    }
}

fn parse_error(command: &mut Command, error: &clap::Error, json: bool) -> u8 {
    let code = u8::try_from(error.exit_code()).unwrap_or(2);
    let informational = matches!(
        error.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
    let leaf = leaf_help(command, error);
    if json && !informational {
        let rendered = error.render().to_string();
        let message = rendered
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("invalid command line")
            .trim_start_matches("error: ")
            .to_owned();
        let usage = error
            .get(clap::error::ContextKind::Usage)
            .map(|u| u.to_string().trim().to_owned());
        let hint = leaf.as_ref().map_or_else(
            || "run peek --help, or peek commands --json for the whole tree".to_owned(),
            |(path, _)| format!("run {path} --help"),
        );
        let envelope = json!({"error":{
            "code":"invalid_input","message":message,"hint":hint,"retryable":false,
            "request_id":null,"details":{"usage":usage,"kind":format!("{:?}", error.kind())}
        }});
        eprintln!("{envelope}");
        return 2;
    }
    let _ = error.print();
    if matches!(error.kind(), ErrorKind::MissingRequiredArgument)
        && let Some((_, help)) = leaf
    {
        eprintln!("\n{help}");
    }
    code
}

/// Finds the command whose usage clap reported (clap's usage context holds
/// grammar only, never argv values) and renders its long help.
fn leaf_help(command: &mut Command, error: &clap::Error) -> Option<(String, String)> {
    let usage = error.get(clap::error::ContextKind::Usage)?.to_string();
    let mut entries = Vec::new();
    collect_help(command, &mut Vec::new(), &mut entries);
    entries.into_iter().rev().find_map(|(path, help)| {
        let full = if path.is_empty() {
            "peek".to_owned()
        } else {
            format!("peek {}", path.join(" "))
        };
        let prefix = format!("Usage: {full}");
        usage
            .lines()
            .any(|line| {
                line.trim_start()
                    .strip_prefix(&prefix)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
            })
            .then_some((full, help))
    })
}

fn collect_help(
    command: &mut Command,
    path: &mut Vec<String>,
    entries: &mut Vec<(Vec<String>, String)>,
) {
    entries.push((path.clone(), command.render_long_help().to_string()));
    let names: Vec<String> = command
        .get_subcommands()
        .filter(|c| public(c))
        .map(|c| c.get_name().to_owned())
        .collect();
    for name in names {
        if let Some(child) = command.find_subcommand_mut(&name) {
            path.push(name);
            collect_help(child, path, entries);
            path.pop();
        }
    }
}

/// Prints the long help of `path` on stderr (used when runtime validation
/// finds that a required input is missing).
pub fn print_help(path: &[String]) {
    let mut command = command();
    let mut selected = &mut command;
    for name in path {
        let Some(child) = selected.find_subcommand_mut(name) else {
            return;
        };
        selected = child;
    }
    eprintln!("\n{}", selected.render_long_help());
}

#[derive(Serialize)]
struct Parameter {
    name: String,
    long: Option<String>,
    short: Option<char>,
    positional: bool,
    required: bool,
    global: bool,
    description: String,
    possible_values: Vec<String>,
}

#[derive(Serialize)]
struct Entry {
    command: String,
    description: String,
    usage: String,
    help: String,
    group: bool,
    arguments: Vec<Parameter>,
}

/// Every public command, from the grammar the parser uses.
pub fn commands_value() -> Value {
    let mut entries = Vec::new();
    collect_entries(&mut command(), "peek", &mut entries);
    serde_json::to_value(entries).unwrap_or(Value::Array(Vec::new()))
}

/// The human listing of `peek commands`.
pub fn commands_text(value: &Value) -> String {
    let mut out = String::from("peek <command> [options]\n\n");
    for entry in value.as_array().into_iter().flatten() {
        let command = entry["command"].as_str().unwrap_or_default();
        let first = entry["description"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default();
        let short: String = first.chars().take(100).collect();
        let _ = writeln!(out, "  {command:<34} {short}");
    }
    out.push_str(
        "\nRun `peek <command> --help` for exact requirements and examples.\n\
         Run `peek docs` for offline guides, or `peek commands --json` for machine-readable help.",
    );
    out
}

fn collect_entries(command: &mut Command, path: &str, entries: &mut Vec<Entry>) {
    let names: Vec<String> = command
        .get_subcommands()
        .filter(|c| public(c))
        .map(|c| c.get_name().to_owned())
        .collect();
    for name in names {
        let Some(child) = command.find_subcommand_mut(&name) else {
            continue;
        };
        let child_path = format!("{path} {name}");
        let help = child.render_long_help().to_string();
        entries.push(Entry {
            command: child_path.clone(),
            description: child
                .get_about()
                .or_else(|| child.get_long_about())
                .map(ToString::to_string)
                .unwrap_or_default(),
            usage: child.render_usage().to_string(),
            help,
            group: child.get_subcommands().any(public),
            arguments: child
                .get_arguments()
                .filter(|arg| !arg.is_hide_set() && arg.get_id() != "help")
                .map(|arg| Parameter {
                    name: arg.get_id().to_string(),
                    long: arg.get_long().map(str::to_owned),
                    short: arg.get_short(),
                    positional: arg.is_positional(),
                    required: arg.is_required_set(),
                    global: arg.is_global_set(),
                    description: arg
                        .get_long_help()
                        .or_else(|| arg.get_help())
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                    possible_values: arg
                        .get_possible_values()
                        .into_iter()
                        .filter(|v| !v.is_hide_set())
                        .map(|v| v.get_name().to_owned())
                        .collect(),
                })
                .collect(),
        });
        collect_entries(child, &child_path, entries);
    }
}

fn docs(topic: &str) -> String {
    format!("Docs: https://peek.teamofsilicons.com/docs/{topic} · peek docs {topic}\n{LINKS}")
}

/// `after_help` for each command path: Examples, `Next:` and links.
#[allow(clippy::too_many_lines)] // a data table: one arm per command path
pub fn notes(path: &str) -> String {
    let (body, topic): (&str, &str) = match path {
        "peek" => return ROOT_AFTER_HELP.to_owned(),
        "peek accounts" => (
            "Examples:\n  peek accounts --json\n\n\
             Next:\n  silicon-accounts login --app peek --json   mint an SLT\n  \
             peek login '<SLT>'                                                  exchange it",
            "accounts",
        ),
        "peek login" => (
            "Examples:\n  peek login \"$SLT\"\n  printf %s \"$SLT\" | peek login --token-file -\n  \
             peek login --recover\n\n\
             Next:\n  peek login status --json      verify the session\n  \
             peek register side <1-8>      claim a position\n  peek logout                   end the session",
            "accounts",
        ),
        "peek login status" => (
            "Examples:\n  peek login status --json\n\n\
             Exit codes: 0 with authenticated true or false; 5 (nothing on stdout) when the backend or ACCOUNTS \
             is unreachable.\n\n\
             Next:\n  peek login '<SLT>'     when authenticated is false\n  peek ting enroll       when ting.subscribed is false",
            "accounts",
        ),
        "peek logout" => (
            "Examples:\n  peek logout\n  peek logout --json\n  peek logout --revoke-ting     also remove this Silicon's Ting grant (every home)\n\n\
             Next:\n  peek login status --json     now answers authenticated:false\n  peek login '<SLT>'           log in again",
            "accounts",
        ),
        "peek config" => (
            "Examples:\n  peek config set '{\"notify\":[\"speech_finished\"],\"voice\":\"JBFqnCBsd6RMkjVDRZzb\"}'\n  \
             peek config set '{\"position\":3,\"drawing\":\"./logo.js\"}'\n  \
             peek config show --json\n  peek config get voice\n  peek config unset voice\n  peek config telemetry off\n\n\
             Next:\n  peek send --speak \"…\"     uses configured defaults",
            "cli",
        ),
        "peek config set" => (
            "Examples:\n  peek config set '{\"telemetry\":false}'\n  \
             peek config set '{\"notify\":[\"speech_finished\",\"show_dismissed\"],\"delivery_max_age_hours\":24}'\n  \
             peek config set '{\"voice\":null}'          reset to the default\n\n\
             Next:\n  peek config show --json",
            "cli",
        ),
        "peek config show" | "peek config get" | "peek config unset" => (
            "Examples:\n  peek config show --json\n  peek config get notify\n  peek config unset voice\n\n\
             Next:\n  peek config set '<json-object>'",
            "cli",
        ),
        "peek config telemetry" => (
            "Examples:\n  peek config telemetry off\n  PEEK_TELEMETRY=0 peek send --speak \"…\"   off for one run\n\n\
             Next:\n  peek docs telemetry      what is recorded, and what never is",
            "telemetry",
        ),
        "peek config home" => (
            "Examples:\n  peek config home /Volumes/data/silicons/dj\n  peek config home \"$SILICON_HOME\"   back to the default\n\n\
             Next:\n  peek login status --json",
            "cli",
        ),
        "peek ting authorize" | "peek ting authorization-status" => (
            "Examples:\n  peek --profile work ting authorize --json\n  peek --profile work ting authorization-status --json\n\nNext:\n  Review the returned ACCOUNTS link, then peek ting complete-authorization --code-file -",
            "ting",
        ),
        "peek ting complete-authorization" => (
            "Examples:\n  peek ting complete-authorization --code-file -\n  peek ting complete-authorization   # retry a saved completion\n\nNext:\n  peek ting enroll   explicitly enable delivery and retry queued answers",
            "ting",
        ),
        "peek ting cancel-authorization" => (
            "Examples:\n  peek ting cancel-authorization\n\nNext:\n  peek ting authorize   start a fresh review; pending work stays saved",
            "ting",
        ),
        "peek ting" | "peek ting enroll" => (
            "Examples:\n  peek ting enroll --json\n\n\
             Next:\n  peek login status --json     shows ting.subscribed\n  peek status                  shows the delivery backlog",
            "ting",
        ),
        "peek register" => (
            "Examples:\n  peek register side 3\n  peek register drawing ./logo.js\n\n\
             Next:\n  peek send --speak \"Hello\"",
            "silicon",
        ),
        "peek register side" => (
            "Examples:\n  peek register side 3\n  peek register side 5 --json\n\n\
             Next:\n  peek register drawing ./logo.js    give the bubble its look\n  \
             peek send --speak \"Hello\"          say something",
            "silicon",
        ),
        "peek register drawing" => (
            "Examples:\n  peek register drawing ./cassette.js\n  peek register drawing ./cassette.js --check\n  \
             peek register drawing ./cassette.js --preview out.png\n  peek register drawing ./cassette.js --dump-frame 30 --json\n\n\
             Next:\n  peek send --speak \"Hello\"     see it live\n  peek docs drawing             the drawing API",
            "drawing",
        ),
        "peek unregister" => (
            "Examples:\n  peek unregister --json\n\n\
             Next:\n  peek register side <1-8>     claim a position again",
            "silicon",
        ),
        "peek send" => (
            "Examples:\n  peek send --speak \"Build finished in 42 s\"\n  \
             peek send --speak \"Now playing\" --show '{\"elements\":[{\"type\":\"image\",\"path\":\"./cover.jpg\",\"caption\":\"CO2\"}]}'\n  \
             peek send --ask '{\"question\":\"Delete old.zip?\",\"type\":\"single_choice\",\"options\":[\"Keep\",\"Delete\"]}'\n  \
             peek send --ask @question.json --expires-in 600 --json\n  \
             peek send --ask '{\"question\":\"Volume?\",\"type\":\"slider\",\"min\":0,\"max\":100}' --wait=60\n  \
             peek send --show '{\"elements\":[{\"type\":\"text\",\"text\":\"Build finished\"}]}' --expires-in 10m\n  \
             peek send --speak \"Stand-up\" --at 09:55\n  \
             peek send --ask @q.json --in 2h --expires-at 2026-09-27T20:00\n  \
             peek send --speak \"Correction\" --replace\n\n\
             Limits: --speak 1–2000 chars · show 1–3 elements, text ≤160, caption ≤50, images ≤10 MiB \
             (png, jpeg, heic, webp, gif) · question ≤80 · 2–6 options, labels ≤40 · 1 on screen + 5 waiting \
             per Silicon · --expires-in/--expires-at 10 s – 7 d · --in/--at up to 365 d, 500 scheduled.\n\n\
             Next:\n  peek ask get <ASK_ID>     the answer, locally\n  \
             peek queue                what is on screen and waiting\n  \
             peek schedule list        scheduled sends\n  \
             peek docs ting            route peek.ask.answered in your flow\n  peek history              recent sends",
            "show",
        ),
        "peek queue" | "peek queue list" => (
            "Examples:\n  peek queue\n  peek queue --json\n  peek queue list --json | jq '.waiting[].send_id'\n\n\
             A send that would be the sixth waiting one fails with queue_full (exit 4).\n\n\
             Next:\n  peek cancel <SEND_ID>     withdraw one\n  \
             peek queue clear          drop every waiting send\n  peek schedule list        scheduled sends",
            "cli",
        ),
        "peek queue clear" => (
            "Examples:\n  peek queue clear\n  peek queue clear --all      also the send on screen\n  peek queue clear --json\n\n\
             Next:\n  peek queue                what is left",
            "cli",
        ),
        "peek cancel" => (
            "Examples:\n  peek cancel snd_0192…\n  peek cancel ask_0192… --json\n  peek cancel sch_0192…\n\n\
             Next:\n  peek queue                what is on screen and waiting\n  \
             peek schedule list        scheduled sends",
            "cli",
        ),
        "peek schedule" | "peek schedule list" => (
            "Examples:\n  peek schedule list\n  peek schedule list --json\n  \
             peek send --speak \"Stand-up in 5\" --at 09:55 --tz Asia/Kolkata\n\n\
             Next:\n  peek schedule cancel <ID>     cancel one before it is due\n  \
             peek schedule clear           cancel all",
            "cli",
        ),
        "peek schedule cancel" | "peek schedule clear" => (
            "Examples:\n  peek schedule cancel sch_0192…\n  peek schedule cancel snd_0192… --json\n  peek schedule clear\n\n\
             A send that already fired is in the queue: withdraw it with peek cancel <SEND_ID>.\n\n\
             Next:\n  peek schedule list        what is left",
            "cli",
        ),
        "peek ask" | "peek ask get" | "peek ask list" | "peek ask cancel" => (
            "Examples:\n  peek ask get ask_0192… --json\n  peek ask list --state pending --limit 20 --json\n  \
             peek ask cancel ask_0192…\n\n\
             Next:\n  peek send --ask '{…}'     ask another question\n  peek history              recent sends",
            "ask",
        ),
        "peek history" => (
            "Examples:\n  peek history --limit 50 --json\n  peek history --before snd_0192… --json\n\n\
             Next:\n  peek ask get <ASK_ID>",
            "cli",
        ),
        "peek status" => (
            "Examples:\n  peek status\n  peek status --json\n\n\
             Next:\n  peek doctor                  exact fixes\n  peek login status --json     identity",
            "cli",
        ),
        "peek account"
        | "peek account byo"
        | "peek account byo deepgram"
        | "peek account byo deepgram set"
        | "peek account byo deepgram show"
        | "peek account byo deepgram delete" => (
            "Legacy key management only: current speech uses ElevenLabs v4 TTS through Deepgram and OpenAI transcription.\n\n\
             Examples:\n  peek --account tos account byo deepgram show --json\n  peek --account tos account byo deepgram delete\n\n\
             Stored Deepgram keys do not affect either speech provider.",
            "privacy",
        ),
        "peek app" | "peek app status" | "peek app install" | "peek app update"
        | "peek app uninstall" => (
            "Examples:\n  peek app status --json\n  peek app install\n  peek app update\n  peek app uninstall\n\n\
             Next:\n  peek daemon status     the helper inside Peek.app\n  peek doctor            exact fixes",
            "platforms",
        ),
        "peek daemon" | "peek daemon status" | "peek daemon restart" => (
            "Examples:\n  peek daemon status --json\n  peek daemon restart\n\n\
             Next:\n  peek app status        Peek.app itself\n  peek doctor            exact fixes",
            "troubleshooting",
        ),
        "peek docs" => (
            "Examples:\n  peek docs\n  peek docs ask\n  peek docs --search keyterm\n  peek docs --all --json\n  \
             peek docs ask --json | jq -r .content\n\n\
             Next:\n  peek commands --json     the whole command tree",
            "index",
        ),
        "peek commands" => (
            "Examples:\n  peek commands\n  peek commands --json | jq '.[].command'\n\n\
             Next:\n  peek <command> --help\n  peek docs cli",
            "cli",
        ),
        "peek report" => (
            "Examples:\n  peek report \"send --ask fails with invalid_input when an option label has an emoji\"\n  \
             peek report \"…\" --pr https://github.com/teamofsilicons/silicon-peek/pull/42 --attach-status\n  \
             peek report \"…\" --via gh\n\n\
             Fixes are welcome: https://github.com/teamofsilicons/silicon-peek\n\n\
             Next:\n  peek docs development     build and test peek locally",
            "development",
        ),
        "peek update" => (
            "Examples:\n  peek update --json\n  apps update 'peek'\n\n\
             Next:\n  peek app update     Peek.app and peekd",
            "versioning",
        ),
        "peek doctor" => (
            "Examples:\n  peek doctor\n  peek doctor --json | jq '.checks[] | select(.status != \"ok\")'\n\n\
             Next:\n  peek report \"…\" --attach-status     include this output in a bug report",
            "troubleshooting",
        ),
        _ => ("Explore: peek commands · peek docs", "index"),
    };
    format!("{body}\n\n{}", docs(topic))
}
