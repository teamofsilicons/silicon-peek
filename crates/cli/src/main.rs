//! `peek`: the Peek command-line interface for Silicons and Carbons.
//!
//! Peek gives each Silicon a position on a Carbon's Mac screen and a small
//! JavaScript drawing for its bubble; with this CLI the Silicon speaks,
//! shows or asks, and the answer comes back as a Ting event. This binary
//! also implements the IAM app contract Stemcell relies on (`peek iam
//! --json`, `peek login <SLT>`, `peek login status --json`, `peek logout`,
//! `peek config set '<json>'`) on every platform.
//!
//! Layout:
//! - `cli` / `experience`: the clap grammar, help tree, leaf help on missing
//!   arguments and `peek commands`.
//! - `context`: store, config, API URL, testing environment, org, client.
//! - `commands/*`: one module per command family.
//! - `service`: Peek.app and peekd on macOS (`ensure_service`,
//!   `ensure_app`) and the exact `platform_unsupported` error elsewhere.
//! - `sys`: the only `unsafe` (`renamex_np`, `setsid`).
//! - `output`, `input`, `docs`, `telemetry`.

mod cli;
mod commands;
mod context;
mod docs;
mod experience;
mod input;
mod output;
mod service;
mod sys;
mod telemetry;

use std::{process::ExitCode, time::Instant};

use silicon_peek_client::Error;

fn main() -> ExitCode {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let started = Instant::now();
    let (cli, path) = match experience::parse(std::env::args_os().collect()) {
        experience::Parsed::Run { cli, path } => (cli, path),
        experience::Parsed::Exit(code) => return ExitCode::from(code),
    };
    let globals = context::Globals::new(&cli.global);
    let out = globals.out();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            out.error(
                &Error::internal(format!("starting the async runtime failed: {e}")),
                &path,
            );
            return ExitCode::from(1);
        }
    };
    let result = runtime
        .block_on(commands::run(*cli, &path, &globals))
        // Every input error names its field and says what to do.
        .map_err(|e| {
            let hint = if path.is_empty() {
                "run `peek --help`".to_owned()
            } else {
                format!("run `peek {} --help` for the accepted values", path.join(" "))
            };
            e.with_input_context("", &hint)
        });
    let exit = match &result {
        Ok(()) => 0,
        Err(e) => {
            out.error(e, &path);
            let missing = e
                .details()
                .and_then(|d| d.get("missing_argument"))
                .is_some();
            if missing && !out.json {
                experience::print_help(&path);
            }
            e.exit_code().code()
        }
    };
    runtime.block_on(telemetry::finish(
        &globals,
        &path,
        result.as_ref().err(),
        exit,
        started.elapsed(),
    ));
    fallback_banner(&globals);
    output::print_banner();
    ExitCode::from(exit)
}

/// When a run selected a testing environment by id but failed before the
/// session was resolved, the banner still names it (read from testing.json).
fn fallback_banner(globals: &context::Globals) {
    if output::has_banner() {
        return;
    }
    let Ok(Some(id)) = globals.test_selector() else {
        return;
    };
    let name = context::existing_store()
        .ok()
        .flatten()
        .and_then(|s| s.read_testing().ok())
        .and_then(|t| t.environments.get(&id).map(|e| e.name.clone()))
        .unwrap_or_else(|| "unsaved testing environment".to_owned());
    output::set_banner(&name, &id);
}
