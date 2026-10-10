//! The peekd IPC client (BLUEPRINT §1.6): socket location, peer-uid check,
//! `hello`, request/reply with timeouts, and the event stream a
//! `send --wait` connection receives.
//!
//! Socket: `/var/tmp/silicon-peek-<uid>/peekd.sock` (directory 0700 owned by
//! the user, socket 0600), overridable only for tests with
//! `PEEK_DAEMON_SOCKET`. Both ends require the peer's uid to equal `getuid()`.
//!
//! peekd exists only on macOS; on other platforms [`request`] returns
//! `platform_unsupported` and the connection type is not compiled.

// The connection is Unix-only; its helpers and imports are unused elsewhere.
#![cfg_attr(not(unix), allow(unused_imports, dead_code))]

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{
    error::{Error, ErrorCode, Origin, Result},
    ipc::{
        AuthBlock, Event, Message, Op, Request,
        cli::{Hello, HelloResult},
        frame::{AsyncFrameReader, FrameLimits, write_frame_async},
    },
};

/// The test-only socket override.
pub const SOCKET_ENV: &str = "PEEK_DAEMON_SOCKET";

/// Per-request timeout for CLI ops.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for `register.drawing` and for a `send --wait` connection's setup.
pub const LONG_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// `/var/tmp/silicon-peek-<uid>` (36 bytes of socket path for uid 501, well
/// under macOS's 104-byte `sun_path`).
#[cfg(unix)]
#[must_use]
pub fn socket_dir() -> PathBuf {
    PathBuf::from(format!("/var/tmp/silicon-peek-{}", super::sys::uid()))
}

/// The socket path: `PEEK_DAEMON_SOCKET` when set (tests), else
/// `<socket_dir>/peekd.sock`.
#[cfg(unix)]
#[must_use]
pub fn socket_path() -> PathBuf {
    match std::env::var_os(SOCKET_ENV) {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => socket_dir().join("peekd.sock"),
    }
}

/// peekd's single-instance lock: `peekd.lock` next to [`socket_path`]
/// (`<socket_dir>/peekd.lock`, or beside a `PEEK_DAEMON_SOCKET` override).
#[cfg(unix)]
#[must_use]
pub fn lock_path() -> PathBuf {
    socket_path()
        .parent()
        .map_or_else(socket_dir, Path::to_path_buf)
        .join("peekd.lock")
}

/// For peekd: creates the socket directory (0700) or verifies an existing
/// one; refuses a symlink or a directory owned by anyone else.
///
/// # Errors
/// `daemon_identity_mismatch` naming the problem.
pub fn ensure_socket_dir(dir: &Path) -> Result<()> {
    super::fs::ensure_private_dir(dir).map_err(|e| {
        Error::new(
            ErrorCode::DaemonIdentityMismatch,
            format!("the peekd socket directory is unusable: {}", e.message()),
        )
        .with_hint(format!("remove {} as its owner, then retry", dir.display()))
    })
}

fn identity_mismatch(detail: &str) -> Error {
    Error::new(
        ErrorCode::DaemonIdentityMismatch,
        format!("the peekd socket belongs to another operating-system user ({detail})"),
    )
    .with_hint("run peek as the user logged in to this Mac's GUI session")
}

fn unavailable(path: &Path, why: &str) -> Error {
    Error::new(
        ErrorCode::DaemonUnavailable,
        format!("peekd is not answering at {}: {why}", path.display()),
    )
    .with_hint("open Peek (peek app install), or check `peek daemon status`")
    .with_origin(Origin::Transport)
}

/// Checks that the peer of a connected socket runs as this user (`getpeereid`
/// through tokio's safe wrapper). peekd uses this for every accepted client.
///
/// # Errors
/// `daemon_identity_mismatch`.
#[cfg(unix)]
pub fn verify_peer(stream: &tokio::net::UnixStream) -> Result<()> {
    let cred = stream
        .peer_cred()
        .map_err(|e| identity_mismatch(&format!("peer credentials unavailable: {e}")))?;
    let me = super::sys::uid();
    if cred.uid() == me {
        Ok(())
    } else {
        Err(identity_mismatch(&format!(
            "peer uid {}, expected {me}",
            cred.uid()
        )))
    }
}

/// How waiting for an event ended.
#[derive(Clone, Debug, PartialEq)]
pub enum EventWait {
    /// An event arrived.
    Event(Event),
    /// peekd closed the connection.
    Closed,
    /// The timeout elapsed.
    TimedOut,
}

/// One connection to peekd.
#[cfg(unix)]
#[derive(Debug)]
pub struct DaemonConnection {
    reader: AsyncFrameReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
    events: VecDeque<Event>,
    limits: FrameLimits,
}

#[cfg(unix)]
impl DaemonConnection {
    /// Connects to `path` and verifies the peer's uid.
    ///
    /// # Errors
    /// `daemon_unavailable` (not running), `daemon_identity_mismatch`.
    pub async fn connect(path: &Path, timeout: Duration) -> Result<Self> {
        let stream = tokio::time::timeout(timeout, tokio::net::UnixStream::connect(path))
            .await
            .map_err(|_| unavailable(path, "connecting timed out"))?
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::PermissionDenied => {
                    identity_mismatch(&format!("connecting to {} was denied", path.display()))
                }
                _ => unavailable(path, &e.to_string()),
            })?;
        verify_peer(&stream)?;
        let (r, w) = stream.into_split();
        Ok(Self {
            reader: AsyncFrameReader::new(r),
            writer: w,
            events: VecDeque::new(),
            limits: FrameLimits::V1,
        })
    }

    /// Connects to [`socket_path`].
    ///
    /// # Errors
    /// As [`DaemonConnection::connect`].
    pub async fn connect_default() -> Result<Self> {
        Self::connect(&socket_path(), Duration::from_secs(5)).await
    }

    async fn send(&mut self, request: Request) -> Result<String> {
        let id = request.id.clone();
        let frame = Message::Request(request).into_frame()?;
        write_frame_async(&mut self.writer, &frame, &self.limits).await?;
        Ok(id)
    }

    async fn read_message(&mut self) -> Result<Option<Message>> {
        match self.reader.read_frame().await? {
            Some(frame) => Message::from_frame(frame).map(Some),
            None => Ok(None),
        }
    }

    /// Sends one op and waits for its reply. Events that arrive first are
    /// buffered for [`DaemonConnection::next_event`].
    ///
    /// # Errors
    /// peekd's error reply (origin daemon), `daemon_unavailable` on timeout or
    /// disconnect, `protocol_error` for a mismatched reply.
    pub async fn call<O: Op>(
        &mut self,
        op: &O,
        auth: Option<&AuthBlock>,
        blobs: Vec<Vec<u8>>,
        timeout: Duration,
    ) -> Result<(O::Output, Vec<Vec<u8>>)> {
        let request = Request::new(op, auth.cloned(), blobs)?;
        let task = async {
            let id = self.send(request).await?;
            loop {
                match self.read_message().await? {
                    Some(Message::Reply(r)) if r.id == id => return r.into_result::<O::Output>(),
                    Some(Message::Reply(r)) => {
                        return Err(Error::new(
                            ErrorCode::ProtocolError,
                            format!("peekd answered request {} while {id} was pending", r.id),
                        ));
                    }
                    Some(Message::Event(e)) => self.events.push_back(e),
                    Some(Message::Request(r)) => {
                        return Err(Error::new(
                            ErrorCode::ProtocolError,
                            format!("peekd sent a `{}` request to a CLI connection", r.op),
                        ));
                    }
                    None => {
                        return Err(Error::new(
                            ErrorCode::DaemonUnavailable,
                            format!("peekd closed the connection before answering `{}`", O::NAME),
                        )
                        .with_hint("peekd may have restarted; retry the command")
                        .with_origin(Origin::Transport));
                    }
                }
            }
        };
        tokio::time::timeout(timeout, task).await.map_err(|_| {
            Error::new(
                ErrorCode::DaemonUnavailable,
                format!(
                    "peekd did not answer `{}` within {} s",
                    O::NAME,
                    timeout.as_secs()
                ),
            )
            .with_hint("check `peek daemon status`; restart it with `peek daemon restart`")
            .with_origin(Origin::Transport)
        })?
    }

    /// The handshake. Maps a protocol mismatch to `cli_outdated` (peekd is
    /// newer) or `app_update_pending` (peekd is older).
    ///
    /// # Errors
    /// As [`DaemonConnection::call`].
    pub async fn hello(&mut self, hello: &Hello, timeout: Duration) -> Result<HelloResult> {
        let (result, _) = self.call(hello, None, Vec::new(), timeout).await?;
        if !crate::ipc::SUPPORTED_PROTOCOLS.contains(&result.protocol) {
            return Err(Error::new(
                ErrorCode::AppUpdatePending,
                format!(
                    "peekd {} chose protocol {}, which this peek ({}) does not speak",
                    result.peekd_version,
                    result.protocol,
                    crate::VERSION
                ),
            )
            .with_hint("Peek.app updates itself; retry in a minute, or run `peek app update`"));
        }
        Ok(result)
    }

    /// Acknowledges a `send --wait` result (`ask.result.ack`) without
    /// waiting for a reply: from here on peekd sends no ting for it.
    ///
    /// # Errors
    /// Framing or write failures (peekd then sends the ting).
    pub async fn acknowledge_result(&mut self, ask_id: &crate::ids::AskId) -> Result<()> {
        let request = Request::new(
            &crate::ipc::cli::AskResultAck {
                ask_id: ask_id.clone(),
            },
            None,
            Vec::new(),
        )?;
        self.send(request).await.map(|_| ())
    }

    /// Waits for the next event (buffered first).
    ///
    /// # Errors
    /// Framing or protocol failures.
    pub async fn next_event(&mut self, timeout: Duration) -> Result<EventWait> {
        if let Some(e) = self.events.pop_front() {
            return Ok(EventWait::Event(e));
        }
        let wait = async {
            match self.read_message().await? {
                Some(Message::Event(e)) => Ok(EventWait::Event(e)),
                Some(Message::Reply(r)) => Err(Error::new(
                    ErrorCode::ProtocolError,
                    format!("unexpected reply {} while waiting for an event", r.id),
                )),
                Some(Message::Request(r)) => Err(Error::new(
                    ErrorCode::ProtocolError,
                    format!("peekd sent a `{}` request to a CLI connection", r.op),
                )),
                None => Ok(EventWait::Closed),
            }
        };
        match tokio::time::timeout(timeout, wait).await {
            Ok(r) => r,
            Err(_) => Ok(EventWait::TimedOut),
        }
    }
}

/// Connects to the default socket, sends `hello`, then one op, and closes.
///
/// # Errors
/// As [`DaemonConnection::connect`], [`DaemonConnection::hello`] and
/// [`DaemonConnection::call`].
#[cfg(unix)]
pub async fn request<O: Op>(
    hello: &Hello,
    op: &O,
    auth: Option<&AuthBlock>,
    blobs: Vec<Vec<u8>>,
    timeout: Duration,
) -> Result<(O::Output, Vec<Vec<u8>>)> {
    let mut conn = DaemonConnection::connect_default().await?;
    conn.hello(hello, REQUEST_TIMEOUT).await?;
    conn.call(op, auth, blobs, timeout).await
}

/// On platforms without peekd every daemon call is `platform_unsupported`.
///
/// # Errors
/// Always `platform_unsupported`.
#[cfg(not(unix))]
#[allow(clippy::unused_async)] // the same async signature as the Unix version
pub async fn request<O: Op>(
    _hello: &Hello,
    _op: &O,
    _auth: Option<&AuthBlock>,
    _blobs: Vec<Vec<u8>>,
    _timeout: Duration,
) -> Result<(O::Output, Vec<Vec<u8>>)> {
    Err(crate::platform_unsupported(O::NAME))
}
