//! `peekd run [--launchd | --parent-ui | --headless]`: the per-user Peek
//! daemon. launchd starts it from Peek.app's agent plist
//! (`ProgramArguments [peekd, run, --launchd]`); Peek.app spawns it with
//! `--parent-ui` when the background item still needs approval.

use std::process::ExitCode;

use silicon_peek_client::Error;
use silicon_peek_daemon::{DaemonConfig, logging, restrict_umask, start};

const USAGE: &str = "\
peekd: the per-user Peek daemon (slots, queue, Deepgram, delivery outbox)

Usage:
  peekd run [--launchd | --parent-ui | --headless]
  peekd --version | --help

peekd is started by Peek.app (a launchd agent, ai.tos.peek.daemon); you do not
normally run it yourself. It listens on /var/tmp/silicon-peek-<uid>/peekd.sock
and keeps its state in ~/Library/Application Support/Peek/.

Isolated runs (development and tests; see docs/development.md):
  PEEK_SUPPORT_DIR    replaces ~/Library/Application Support/Peek
  PEEK_CACHES_DIR     replaces ~/Library/Caches/Peek
  PEEK_DAEMON_SOCKET  replaces the socket path (the lock sits next to it)
  PEEK_NO_SERVICES=1  never launch Peek.app, never run the updater or watchdog
  PEEK_API_URL        peek-server origin for telemetry (loopback http allowed)
  The install hooks PEEK_INSTALL_SUPPORT_DIR, PEEK_INSTALL_APPLICATIONS_DIR and
  PEEK_INSTALL_NO_LAUNCH=1 are honoured as the peek CLI honours them.

Flags:
  --launchd     started by launchd (the default mode)
  --parent-ui   spawned by Peek.app itself; never launches Peek.app
  --headless    no GUI session; never launches Peek.app (sends wait queued)

Check it with `peek daemon status`; restart it with `peek daemon restart`.
Logs: ~/Library/Application Support/Peek/peekd.log
";

fn fail(e: &Error) -> ExitCode {
    eprintln!("peekd: error: {}", e.message());
    if let Some(h) = e.hint() {
        eprintln!("peekd: hint: {h}");
    }
    ExitCode::from(e.exit_code().code())
}

fn main() -> ExitCode {
    let _ = rustls::crypto::ring::default_provider().install_default();
    restrict_umask();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut launch_ui = true;
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("peekd {}", silicon_peek_client::VERSION);
            return ExitCode::SUCCESS;
        }
        Some("--help" | "-h" | "help") => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some("run") => {}
        None => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
        Some(other) => {
            eprintln!("peekd: unknown command `{other}`\n");
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    }
    for flag in &args[1..] {
        match flag.as_str() {
            "--launchd" => {}
            "--parent-ui" | "--headless" => launch_ui = false,
            other => {
                eprintln!("peekd: unknown flag `{other}`\n");
                eprint!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let cfg = match DaemonConfig::from_env(launch_ui) {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    if let Err(e) = std::fs::create_dir_all(&cfg.support_dir) {
        return fail(&Error::internal(format!(
            "creating {} failed: {e}",
            cfg.support_dir.display()
        )));
    }
    let log_path = cfg.support_dir.join("peekd.log");
    if let Err(e) = logging::init(cfg.log_to_file.then_some(log_path.as_path())) {
        eprintln!("peekd: warning: cannot write {}: {e}", log_path.display());
        let _ = logging::init(None);
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            return fail(&Error::internal(format!(
                "starting the async runtime failed: {e}"
            )));
        }
    };
    runtime.block_on(async move {
        let handle = match start(cfg).await {
            Ok(h) => h,
            Err(e) => {
                tracing::error!(code = %e.code(), error = %e.message(), "peekd could not start");
                return fail(&e);
            }
        };
        let signals = async {
            use tokio::signal::unix::{SignalKind, signal};
            match (
                signal(SignalKind::terminate()),
                signal(SignalKind::interrupt()),
            ) {
                (Ok(mut term), Ok(mut int)) => {
                    tokio::select! {
                        _ = term.recv() => tracing::info!("SIGTERM received"),
                        _ = int.recv() => tracing::info!("SIGINT received"),
                    }
                }
                _ => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            () = signals => {}
            () = handle.exit_requested() => {}
        }
        let code = handle.shutdown().await;
        ExitCode::from(u8::try_from(code).unwrap_or(1))
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn usage_mentions_every_mode() {
        for m in ["--launchd", "--parent-ui", "--headless", "peekd run"] {
            assert!(super::USAGE.contains(m));
        }
    }
}
