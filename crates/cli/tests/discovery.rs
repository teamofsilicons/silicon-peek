//! Offline contracts: `peek iam --json`, help, leaf help after a missing
//! argument, `peek commands`, `peek docs`, `peek update`, `peek doctor`, and
//! the JSON/human output split.

mod common;

use common::{DEAD_API, Env};
use serde_json::json;

#[tokio::test]
async fn iam_json_is_static_and_side_effect_free() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["iam", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(
        run.stderr.is_empty(),
        "stderr must stay empty: {}",
        run.stderr
    );
    let v = run.json();
    assert_eq!(v["app_id"], "peek");
    assert_eq!(v["org_id"], "tos");
    assert_eq!(v["name"], "Peek");
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["api_version"], "v1");
    assert_eq!(v["api_url"], DEAD_API);
    assert_eq!(v["iam_url"], "https://backend.iam.teamofsilicons.com");
    assert_eq!(v["auth_url"], "https://auth.iam.teamofsilicons.com");
    assert_eq!(v["login_method"], "short_lived_token");
    assert_eq!(v["credential_issuer"], false);
    assert_eq!(v["docs_url"], "https://peek.teamofsilicons.com/docs");
    assert_eq!(
        v["repository_url"],
        "https://github.com/teamofsilicons/silicon-peek"
    );
    assert_eq!(v["rust_package"], "silicon-peek-client");
    assert_eq!(v["cli_package"], "silicon-peek-cli");
    assert_eq!(v["install"], "honeycomb install 'peek'");
    assert_eq!(
        v["platforms"]["full"],
        json!(["macos-aarch64", "macos-x86_64"])
    );
    assert_eq!(
        v["platforms"]["iam_only"],
        json!([
            "linux-x86_64",
            "linux-aarch64",
            "windows-x86_64",
            "windows-aarch64"
        ])
    );
    assert!(
        v["login"]
            .as_str()
            .is_some_and(|l| l.contains("--app-id peek"))
    );
    assert!(v.get("testing").is_none());
    assert!(
        !env.store_dir().exists(),
        "peek iam must not create the store"
    );
}

#[tokio::test]
async fn iam_answers_even_with_a_broken_silicon_home() {
    let mut env = Env::new(DEAD_API);
    let missing = env.home.join("missing");
    env.var("SILICON_HOME", missing.to_str().unwrap_or_default());
    let run = env.run(&["iam", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["app_id"], "peek");
}

#[tokio::test]
async fn iam_human_mode_is_readable() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["iam"]).await;
    assert_eq!(run.code, 0);
    assert!(run.stdout.contains("IAM app id `peek`"));
    assert!(run.stdout.contains("honeycomb install 'peek'"));
    assert!(serde_json::from_str::<serde_json::Value>(&run.stdout).is_err());
}

#[tokio::test]
async fn root_help_ends_with_the_blueprint_block() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["--help"]).await;
    assert_eq!(run.code, 0);
    let block = "Start (Silicon):  peek iam --json · peek login <SLT> · peek register side <1-8> (custom drawing optional)\n\
Then:             peek send --speak \"…\" --show '{…}'   |   peek send --speak \"…\" --ask '{…}'\n\
Answers arrive as Ting events of type peek.ask.answered (route them in your flow; see peek docs ting).\n\
State: $SILICON_HOME/.peek (else ~/.peek). Test mode: peek --test <env-uuid> <command>.\n\
Explore: peek commands · peek <command> --help · peek docs <topic>\n\
Docs: https://peek.teamofsilicons.com/docs · Source: https://github.com/teamofsilicons/silicon-peek\n\
Rust: https://crates.io/crates/silicon-peek-client · Bugs: peek report --help";
    assert!(run.stdout.trim_end().ends_with(block), "{}", run.stdout);
}

#[tokio::test]
async fn logout_help_exits_zero_for_stemcell() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["logout", "--help"]).await;
    assert_eq!(run.code, 0);
    assert!(run.stdout.contains("remote_revocation"));
    assert!(run.stdout.contains("Examples:"));
    assert!(
        run.stdout
            .contains("Docs: https://peek.teamofsilicons.com/docs/iam")
    );
}

#[tokio::test]
async fn a_missing_argument_prints_the_leaf_help() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["register", "side"]).await;
    assert_eq!(run.code, 2);
    assert!(run.stderr.contains("no default position configured"));
    assert!(
        run.stderr.contains("Claims a position on the screen"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("peek register side 5 --json"));
    assert!(run.stdout.is_empty());
}

#[tokio::test]
async fn a_missing_argument_with_json_is_one_error_object() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["config", "set", "--json"]).await;
    assert_eq!(run.code, 2);
    assert!(run.stdout.is_empty());
    let e = run.error();
    assert_eq!(e["code"], "invalid_input");
    assert_eq!(e["hint"], "run peek config set --help");
    assert_eq!(run.stderr.lines().count(), 1, "{}", run.stderr);
}

#[tokio::test]
async fn login_without_an_slt_shows_the_login_help() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["login"]).await;
    assert_eq!(run.code, 2);
    assert!(run.stderr.starts_with("error: peek login needs an SLT"));
    assert!(run.stderr.contains("Exchanges an IAM short-lived token"));
}

#[tokio::test]
async fn version_flag() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["--version"]).await;
    assert_eq!(run.code, 0);
    assert_eq!(
        run.stdout.trim(),
        format!("peek {}", env!("CARGO_PKG_VERSION"))
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one table of cases
async fn commands_json_is_the_whole_tree() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["commands", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    let entries = v.as_array().cloned().unwrap_or_default();
    let names: Vec<&str> = entries
        .iter()
        .filter_map(|e| e["command"].as_str())
        .collect();
    for expected in [
        "peek iam",
        "peek login",
        "peek login status",
        "peek logout",
        "peek config set",
        "peek config home",
        "peek ting enroll",
        "peek register side",
        "peek register drawing",
        "peek unregister",
        "peek send",
        "peek ask get",
        "peek ask list",
        "peek ask cancel",
        "peek history",
        "peek status",
        "peek org byo deepgram set",
        "peek app status",
        "peek app uninstall",
        "peek daemon restart",
        "peek docs",
        "peek commands",
        "peek report",
        "peek update",
        "peek doctor",
    ] {
        assert!(names.contains(&expected), "missing {expected}");
    }
    assert!(
        !names.iter().any(|n| n.contains("__")),
        "hidden commands leak"
    );
    let send = entries
        .iter()
        .find(|e| e["command"] == "peek send")
        .cloned()
        .unwrap_or_default();
    assert_eq!(send["group"], false);
    assert!(
        send["usage"]
            .as_str()
            .is_some_and(|u| u.starts_with("Usage: peek send"))
    );
    assert!(
        send["help"]
            .as_str()
            .is_some_and(|h| h.contains("Examples:"))
    );
    let wait = send["arguments"]
        .as_array()
        .and_then(|a| a.iter().find(|p| p["name"] == "wait"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(wait["long"], "wait");
    assert_eq!(wait["positional"], false);
    let config = entries
        .iter()
        .find(|e| e["command"] == "peek config")
        .cloned()
        .unwrap_or_default();
    assert_eq!(config["group"], true);
    let state = entries
        .iter()
        .find(|e| e["command"] == "peek ask list")
        .and_then(|e| e["arguments"].as_array().cloned())
        .and_then(|a| a.into_iter().find(|p| p["name"] == "state"))
        .unwrap_or_default();
    assert_eq!(
        state["possible_values"],
        json!([
            "pending",
            "answered",
            "dismissed",
            "expired",
            "cancelled",
            "replaced"
        ])
    );
    // The seven 0.1.2 command paths (plus `peek queue list`) are listed, each
    // with its own notes.
    for path in [
        "peek queue",
        "peek queue list",
        "peek queue clear",
        "peek cancel",
        "peek schedule",
        "peek schedule list",
        "peek schedule cancel",
        "peek schedule clear",
    ] {
        let entry = entries
            .iter()
            .find(|e| e["command"] == path)
            .unwrap_or_else(|| panic!("{path} is listed"));
        assert!(
            entry["help"]
                .as_str()
                .is_some_and(|h| h.contains("Examples:")),
            "{path} has examples"
        );
    }
    let send_args: Vec<String> = send["arguments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["long"].as_str().map(str::to_owned))
        .collect();
    for flag in ["expires-in", "expires-at", "in", "at", "tz", "replace"] {
        assert!(send_args.iter().any(|a| a == flag), "--{flag}");
    }
}

#[tokio::test]
async fn docs_topics_resolve() {
    let env = Env::new(DEAD_API);
    let index = env.run(&["docs", "--json"]).await;
    assert_eq!(index.code, 0);
    let topics = index.json()["topics"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(topics.len(), 16);
    for t in &topics {
        let name = t["topic"].as_str().unwrap_or_default();
        let run = env.run(&["docs", name, "--json"]).await;
        assert_eq!(run.code, 0, "{name}: {}", run.stderr);
        let v = run.json();
        assert_eq!(v["topic"], name);
        assert_eq!(v["format"], "markdown");
        assert_eq!(v["command"], format!("peek docs {name}"));
        assert_eq!(v["embedded"], true, "{name} is bundled");
        assert!(v["content"].as_str().is_some_and(|c| c.starts_with('#')));
    }
    let human = env.run(&["docs", "ask"]).await;
    assert!(human.stdout.starts_with("# "), "raw Markdown in human mode");
    let search = env.run(&["docs", "--search", "keyterm", "--json"]).await;
    assert_eq!(search.code, 0);
    let s = search.json();
    assert_eq!(s["query"], "keyterm");
    for r in s["results"].as_array().into_iter().flatten() {
        for m in r["matches"].as_array().into_iter().flatten() {
            assert!(
                m["excerpt"]
                    .as_str()
                    .is_some_and(|e| e.chars().count() <= 320)
            );
            assert!(m["line"].as_u64().is_some());
        }
    }
    let bad = env.run(&["docs", "nope", "--json"]).await;
    assert_eq!(bad.code, 2);
    assert_eq!(bad.error()["code"], "invalid_input");
    assert_eq!(
        bad.error()["details"]["topics"].as_array().map(Vec::len),
        Some(16)
    );
}

#[tokio::test]
async fn update_prints_honeycomb_guidance() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["update", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    assert_eq!(v["manager"], "honeycomb");
    assert_eq!(v["app_id"], "peek");
    assert_eq!(v["current_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["auto_update"], true);
    assert_eq!(v["can_replace_running_binary"], false);
    assert_eq!(v["command"], "honeycomb update 'peek'");
    assert!(v.get("app").is_some());
}

#[tokio::test]
async fn doctor_degrades_gracefully() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["doctor", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    let checks = v["checks"].as_array().cloned().unwrap_or_default();
    let names: Vec<&str> = checks.iter().filter_map(|c| c["name"].as_str()).collect();
    for n in ["store", "backend", "honeycomb"] {
        assert!(names.contains(&n), "missing check {n}: {names:?}");
    }
    for c in &checks {
        assert!(matches!(c["status"].as_str(), Some("ok" | "warn" | "fail")));
        assert!(c["detail"].is_string());
    }
    let backend = checks
        .iter()
        .find(|c| c["name"] == "backend")
        .cloned()
        .unwrap_or_default();
    assert_eq!(backend["status"], "fail", "nothing listens on {DEAD_API}");
    assert!(
        !env.store_dir().exists(),
        "doctor must not create the store"
    );
    let human = env.run(&["doctor"]).await;
    assert!(human.stdout.contains("[fail] backend"));
    assert!(human.stdout.contains("fix: "));
}

#[tokio::test]
async fn json_and_human_modes_differ_only_in_rendering() {
    let env = Env::new(DEAD_API);
    let json_run = env.run(&["config", "show", "--json"]).await;
    let human_run = env.run(&["config", "show"]).await;
    assert_eq!(json_run.code, 0);
    assert_eq!(human_run.code, 0);
    assert_eq!(json_run.json()["delivery_max_age_hours"], 168);
    assert!(human_run.stdout.contains("delivery_max_age_hours: 168"));
    assert!(json_run.stderr.is_empty());
}

#[tokio::test]
async fn empty_environment_values_are_errors() {
    let mut env = Env::new(DEAD_API);
    env.var("SILICON_ORG", "");
    env.login_as("oat_a", "ort_a", i64::MAX / 4);
    let run = env.run(&["ting", "enroll", "--json"]).await;
    assert_eq!(run.code, 2, "{}", run.stderr);
    assert!(
        run.error()["message"]
            .as_str()
            .is_some_and(|m| m.contains("SILICON_ORG"))
    );
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn mac_bound_commands_are_platform_unsupported() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["send", "--speak", "hi", "--json"]).await;
    assert_eq!(run.code, 4);
    let e = run.error();
    assert_eq!(e["code"], "platform_unsupported");
    assert_eq!(
        e["hint"],
        "Run this Silicon on macOS 26+ with Peek.app installed, or use dm for text conversations."
    );
    assert_eq!(
        e["details"]["supported_platforms"],
        json!(["macos-aarch64", "macos-x86_64"])
    );
    let app = env.run(&["app", "status", "--json"]).await;
    assert_eq!(app.code, 0);
    assert_eq!(app.json()["supported"], false);
}
