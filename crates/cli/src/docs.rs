//! `peek docs [TOPIC] [--search TEXT] [--all]`: offline manuals (BLUEPRINT
//! §7.6, the DM shape).
//!
//! The manuals are `crates/cli/docs/*.md`, embedded by `build.rs`. Each of
//! the 16 advertised topics maps to `<topic>.md`; when a file is absent (for
//! example in a partial checkout) the topic still resolves to a short
//! built-in text that points at the online page, so `peek docs <topic>` never
//! fails for an advertised topic.

use serde_json::{Value, json};
use silicon_peek_client::{Error, Result};
use std::fmt::Write as _;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_docs.rs"));
}

/// Search excerpts are cut at this many characters (BLUEPRINT §7.6).
const EXCERPT_MAX: usize = 320;

/// One advertised topic: name, fallback title, fallback summary.
struct Topic {
    name: &'static str,
    title: &'static str,
    summary: &'static str,
}

const TOPICS: &[Topic] = &[
    Topic {
        name: "start",
        title: "Start here",
        summary: "Install peek with `curl -fsSL https://peek.teamofsilicons.com/install.sh | sh` (Silicon Apps, the peek CLI, then Peek.app). A Silicon then runs `peek accounts --json`, `peek login '<SLT>'`, `peek register side <1-8>`, `peek register drawing ./logo.js`, and `peek send --speak \"…\"`.",
    },
    Topic {
        name: "carbon",
        title: "Peek for Carbons",
        summary: "Peek.app shows each Silicon's bubble at its position. Press ctrl+cmd+<position> to focus a bubble, answer an ask by voice (`\\`), keyboard or click, and speak or type to a Silicon with no pending ask. Esc reaches a new peek for 3 s (or while hovered): once hides a show (twice also stops the audio) or folds an ask to compact; the down arrow always dismisses. \"+N\" shows how many of that Silicon's peeks wait. Settings: compact mode, hotkey modifier, display, telemetry.",
    },
    Topic {
        name: "silicon",
        title: "Peek for Silicons",
        summary: "Log in with an SLT (`peek login`), claim one of 8 positions (`peek register side`), register a drawing (`peek register drawing`), then `peek send` with --speak, --show or --ask. Answers arrive as Ting events (peek.ask.answered); inspect them locally with `peek ask get`.",
    },
    Topic {
        name: "cli",
        title: "CLI reference",
        summary: "The full command tree is in `peek --help`, `peek <command> --help` and `peek commands --json`. --json prints one JSON value; errors are {\"error\":{code,message,hint,retryable,request_id,details}} on stderr. Exit codes: 0 ok, 1 internal, 2 invalid input, 3 not authenticated, 4 refused, 5 unavailable.",
    },
    Topic {
        name: "show",
        title: "Speak and show",
        summary: "`peek send --speak \"…\"` streams 1–2000 characters with ElevenLabs v4 TTS through Deepgram; --voice-instructions or config voice_instructions customizes accent, style and delivery. `--show '{\"elements\":[…]}'` shows 1–3 elements: text (≤160 characters) or image (png, jpeg, heic, webp, gif; ≤10 MiB; caption ≤50). Image paths are relative to the current directory; the CLI reads the bytes. Sends queue (1 on screen + 5 waiting; `peek queue`); --replace takes over; --expires-in/--expires-at drop a late send; --in/--at schedule it.",
    },
    Topic {
        name: "ask",
        title: "Ask a question",
        summary: "`peek send --ask '{\"question\":\"…\",\"type\":\"single_choice\",\"options\":[\"Yes\",\"No\"]}'` asks one question (≤80 characters). Types: text, single_choice, multiple_choice (2–6 options, labels ≤40), slider, range. Add --expires-in/--expires-at or --wait[=SECS]; it queues behind the Silicon's other sends, --replace takes over, --in/--at schedule it (no --wait then). The answer arrives as peek.ask.answered.",
    },
    Topic {
        name: "drawing",
        title: "Drawing the visual",
        summary: "A drawing is one JavaScript file (≤256 KiB) drawing into a 100×100 unit square with a canvas-like ctx: `peek.frame((ctx, input) => { … })`. `peek register drawing ./x.js` validates it for 90 offscreen frames inside Peek.app; --preview writes a PNG grid and --dump-frame N prints a display list.",
    },
    Topic {
        name: "ting",
        title: "Ting events",
        summary: "peek sends nine Ting types to the asking Silicon: peek.ask.answered, peek.ask.dismissed, peek.ask.expired, peek.message.received, peek.send.expired, peek.schedule.due, and (opt-in, always for scheduled sends) peek.send.shown, plus (opt-in) peek.speech.finished and peek.show.dismissed. metadata.isi carries the ISI of the send. Route them in your Stemcell flow; delivery is at least once, so dedupe by ting id.",
    },
    Topic {
        name: "accounts",
        title: "ACCOUNTS and sessions",
        summary: "peek is the ACCOUNTS app `peek` owned by `si:tos`. Mint an SLT with `silicon-accounts login --app peek --json`, then `peek login '<SLT>'`. Tokens stay in $SILICON_HOME/.peek/session.json (0600); `peek login status --json` verifies them live; `peek logout` revokes them.",
    },
    Topic {
        name: "telemetry",
        title: "Telemetry",
        summary: "peek records anonymous usage events (command, outcome, durations; actor ids hashed) through its backend into Space Station. It never records speak/show/ask text, answers, transcripts, paths, drawings or tokens. Turn it off with `peek config telemetry off`, --no-telemetry (this process only), or PEEK_TELEMETRY=0 (also forwarded to the helper for this home).",
    },
    Topic {
        name: "privacy",
        title: "Privacy",
        summary: "History, asks and answers stay on the Mac. Text and voice instructions go directly from the Mac to Deepgram for ElevenLabs v4 TTS; audio streams back to the Mac. The backend issues short-lived speech tokens. Completed microphone recordings go through the backend to OpenAI gpt-transcribe; Peek shows only the final transcript. Provider API keys stay on the backend; local speech audio is cached on the Mac.",
    },
    Topic {
        name: "platforms",
        title: "Platforms",
        summary: "Full support: macOS 26+ on Apple silicon and Intel (Peek.app). On Linux and Windows the same CLI implements accounts, login, login status, logout, config, docs, commands, report, update and doctor; commands that need the Mac exit 4 with platform_unsupported.",
    },
    Topic {
        name: "development",
        title: "Development",
        summary: "peek is open source: https://github.com/teamofsilicons/silicon-peek. `cargo check --workspace --locked` verifies the Rust workspace. File bugs with `peek report \"…\" --pr <url>`.",
    },
    Topic {
        name: "versioning",
        title: "Versioning and compatibility",
        summary: "One version for the CLI, peekd, Peek.app and the backend. Silicon Apps updates the CLI every minute; peekd updates Peek.app (newest build wins, never downgrade). `peek update` prints the guidance; `apps update 'peek'` updates by hand.",
    },
    Topic {
        name: "troubleshooting",
        title: "Troubleshooting",
        summary: "Run `peek doctor` for every check with its exact fix, `peek status` for this Silicon's state on the Mac, and `peek login status --json` for identity. Errors carry a code, a hint and, from the backend, a request id to quote in `peek report`.",
    },
];

/// Every advertised topic name, in order.
pub fn topic_names() -> Vec<&'static str> {
    TOPICS.iter().map(|t| t.name).collect()
}

struct Guide {
    topic: &'static str,
    title: String,
    path: String,
    content: String,
    embedded: bool,
}

fn title_of(markdown: &str) -> Option<String> {
    markdown
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

fn guide(topic: &Topic) -> Guide {
    match embedded::EMBEDDED
        .iter()
        .find(|(stem, _, _)| *stem == topic.name)
    {
        Some((_, path, content)) => Guide {
            topic: topic.name,
            title: title_of(content).unwrap_or_else(|| topic.title.to_owned()),
            path: (*path).to_owned(),
            content: (*content).to_owned(),
            embedded: true,
        },
        None => Guide {
            topic: topic.name,
            title: topic.title.to_owned(),
            path: format!("docs/{}.md", topic.name),
            content: format!(
                "# {}\n\n{}\n\nThis build does not bundle the full page. Read it at \
                 https://peek.teamofsilicons.com/docs/{} or run `peek docs --search <text>`.\n",
                topic.title, topic.summary, topic.name
            ),
            embedded: false,
        },
    }
}

fn entry(g: &Guide) -> Value {
    json!({"topic": g.topic, "title": g.title, "path": g.path, "format": "markdown",
           "command": format!("peek docs {}", g.topic)})
}

fn document(g: &Guide) -> Value {
    let mut v = entry(g);
    v["content"] = json!(g.content);
    v["package_version"] = json!(silicon_peek_client::VERSION);
    v["embedded"] = json!(g.embedded);
    v
}

/// Resolves one topic by name.
///
/// # Errors
/// `invalid_input` naming every topic when `name` is not one of them.
pub fn topic(name: &str) -> Result<Value> {
    let wanted = name.trim().to_ascii_lowercase();
    if wanted == "index" {
        return Ok(index());
    }
    TOPICS
        .iter()
        .find(|t| t.name == wanted)
        .map(|t| document(&guide(t)))
        .ok_or_else(|| {
            Error::invalid_input(format!("`{name}` is not a peek docs topic"))
                .with_hint(format!("topics: {}", topic_names().join(", ")))
                .with_details(json!({"topics": topic_names()}))
        })
}

/// The topic index.
pub fn index() -> Value {
    let index_content = embedded::EMBEDDED
        .iter()
        .find(|(stem, _, _)| *stem == "index")
        .map(|(_, _, c)| *c);
    json!({
        "embedded": true,
        "package_version": silicon_peek_client::VERSION,
        "topics": TOPICS.iter().map(|t| entry(&guide(t))).collect::<Vec<_>>(),
        "online": silicon_peek_client::DOCS_URL,
        "repository": silicon_peek_client::REPOSITORY_URL,
        "help": "peek docs TOPIC; peek docs --search TEXT; peek docs --all; peek --help; peek COMMAND --help",
        "read_as_markdown": "peek docs cli --json | jq -r .content",
        "index_content": index_content,
    })
}

/// Every topic in one value.
pub fn all() -> Value {
    json!({
        "embedded": true,
        "package_version": silicon_peek_client::VERSION,
        "documents": TOPICS.iter().map(|t| document(&guide(t))).collect::<Vec<_>>(),
    })
}

/// Line matches across every topic.
///
/// # Errors
/// `invalid_input` for an empty query.
pub fn search(query: &str) -> Result<Value> {
    let q = query.trim();
    if q.is_empty() {
        return Err(Error::invalid_input(
            "documentation search needs text; run `peek docs` for the topic list",
        ));
    }
    let needle = q.to_lowercase();
    let results: Vec<Value> = TOPICS
        .iter()
        .map(guide)
        .filter_map(|g| {
            let matches: Vec<Value> = g
                .content
                .lines()
                .enumerate()
                .filter(|(_, line)| line.to_lowercase().contains(&needle))
                .map(|(n, line)| {
                    let count = line.chars().count();
                    json!({"line": n + 1,
                           "excerpt": line.chars().take(EXCERPT_MAX).collect::<String>(),
                           "truncated": count > EXCERPT_MAX})
                })
                .collect();
            if matches.is_empty() {
                return None;
            }
            let mut item = entry(&g);
            item["matches"] = json!(matches);
            Some(item)
        })
        .collect();
    Ok(json!({"query": q, "embedded": true, "results": results}))
}

/// Human rendering of any docs value.
pub fn human(value: &Value) -> String {
    if let Some(content) = value.get("content").and_then(Value::as_str) {
        return content.trim_end().to_owned();
    }
    if let Some(docs) = value.get("documents").and_then(Value::as_array) {
        return docs
            .iter()
            .filter_map(|d| d.get("content").and_then(Value::as_str))
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");
    }
    if let Some(results) = value.get("results").and_then(Value::as_array) {
        if results.is_empty() {
            return format!(
                "no matches for `{}`",
                value["query"].as_str().unwrap_or_default()
            );
        }
        let mut out = Vec::new();
        for r in results {
            let topic = r["topic"].as_str().unwrap_or_default();
            for m in r["matches"].as_array().into_iter().flatten() {
                out.push(format!(
                    "{topic}:{}: {}",
                    m["line"],
                    m["excerpt"].as_str().unwrap_or_default().trim()
                ));
            }
        }
        return out.join("\n");
    }
    let mut out = String::from("peek docs <topic>\n\n");
    for t in value["topics"].as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "  {:<16} {}",
            t["topic"].as_str().unwrap_or_default(),
            t["title"].as_str().unwrap_or_default()
        );
    }
    let _ = write!(
        out,
        "\nSearch: peek docs --search <text> · Everything: peek docs --all · Online: {}",
        silicon_peek_client::DOCS_URL
    );
    out
}
