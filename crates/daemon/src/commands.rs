//! External commands peekd runs (`open`, `ditto`, `codesign`, `xattr`,
//! `plutil`, `apps`), behind [`CommandRunner`] so tests inject a fake
//! and never touch the real system.
//!
//! Children always get stdin `/dev/null` and never inherit peekd's pipes
//! (§1.8: Stemcell and ISI tools read their pipes to EOF).

use std::{
    ffi::OsString, fmt, future::Future, io, path::PathBuf, pin::Pin, process::Stdio, time::Duration,
};

/// One command invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    /// Absolute path of the program.
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<OsString>,
    /// Environment variables to set.
    pub env: Vec<(OsString, OsString)>,
    /// Environment variables to remove.
    pub env_remove: Vec<OsString>,
    /// Kill the child after this long.
    pub timeout: Duration,
}

impl CommandSpec {
    /// A command with a 60 s timeout and no environment changes.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
            timeout: Duration::from_secs(60),
        }
    }

    /// Appends an argument.
    #[must_use]
    pub fn arg(mut self, a: impl Into<OsString>) -> Self {
        self.args.push(a.into());
        self
    }

    /// Sets an environment variable.
    #[must_use]
    pub fn env(mut self, k: impl Into<OsString>, v: impl Into<OsString>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }

    /// Removes an environment variable.
    #[must_use]
    pub fn env_remove(mut self, k: impl Into<OsString>) -> Self {
        self.env_remove.push(k.into());
        self
    }

    /// Sets the timeout.
    #[must_use]
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// The program's file name, for messages.
    #[must_use]
    pub fn name(&self) -> String {
        self.program.file_name().map_or_else(
            || self.program.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }
}

impl fmt::Display for CommandSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.program.display())?;
        for a in &self.args {
            write!(f, " {}", a.to_string_lossy())?;
        }
        Ok(())
    }
}

/// What a command produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandOutput {
    /// The exit status (`None` when killed by a signal).
    pub status: Option<i32>,
    /// Captured stdout.
    pub stdout: Vec<u8>,
    /// Captured stderr.
    pub stderr: Vec<u8>,
}

impl CommandOutput {
    /// A successful, silent result.
    #[must_use]
    pub fn ok() -> Self {
        Self {
            status: Some(0),
            ..Self::default()
        }
    }

    /// A failed result with a stderr message.
    #[must_use]
    pub fn failed(code: i32, stderr: &str) -> Self {
        Self {
            status: Some(code),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// Whether the exit status is 0.
    #[must_use]
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    /// The last non-empty stderr line (or stdout), for messages.
    #[must_use]
    pub fn last_line(&self) -> String {
        let pick = |b: &[u8]| {
            String::from_utf8_lossy(b)
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().chars().take(300).collect::<String>())
        };
        pick(&self.stderr)
            .or_else(|| pick(&self.stdout))
            .unwrap_or_else(|| format!("exit status {:?}", self.status))
    }
}

/// A boxed command future.
pub type CommandFuture<'a> = Pin<Box<dyn Future<Output = io::Result<CommandOutput>> + Send + 'a>>;

/// Runs external commands.
pub trait CommandRunner: Send + Sync + fmt::Debug {
    /// Runs `spec` to completion (or its timeout) and captures its output.
    fn run(&self, spec: CommandSpec) -> CommandFuture<'_>;
}

/// The real runner: `tokio::process`, stdin `/dev/null`, output captured,
/// killed on timeout.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemCommands;

impl CommandRunner for SystemCommands {
    fn run(&self, spec: CommandSpec) -> CommandFuture<'_> {
        Box::pin(async move {
            let mut cmd = tokio::process::Command::new(&spec.program);
            cmd.args(&spec.args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            for k in &spec.env_remove {
                cmd.env_remove(k);
            }
            for (k, v) in &spec.env {
                cmd.env(k, v);
            }
            let child = cmd.spawn()?;
            match tokio::time::timeout(spec.timeout, child.wait_with_output()).await {
                Ok(out) => {
                    let out = out?;
                    Ok(CommandOutput {
                        status: out.status.code(),
                        stdout: out.stdout,
                        stderr: out.stderr,
                    })
                }
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "{} did not finish within {} s",
                        spec.name(),
                        spec.timeout.as_secs()
                    ),
                )),
            }
        })
    }
}
