//! The one Peek.app connection (role `ui`): events and requests from peekd to
//! the app, with replies correlated by request id.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use silicon_peek_client::{
    Error, ErrorCode, Result,
    error::Origin,
    ipc::{
        Event, EventBody, Message, Op, Reply, Request,
        frame::Frame,
        ui::{Presence, UiStatusReport},
    },
};
use tokio::sync::{Notify, mpsc, oneshot};

/// A connected Peek.app.
#[derive(Debug)]
pub struct UiLink {
    /// Connection number (distinguishes a reconnect from the old link).
    pub conn_id: u64,
    /// Outbound frames, written in order by the connection's writer task.
    tx: mpsc::UnboundedSender<Frame>,
    /// peekd→UI requests waiting for a reply.
    pending: Mutex<HashMap<String, oneshot::Sender<Reply>>>,
    /// Set once the connection is gone (or replaced): no request may wait
    /// on it any more.
    closed: AtomicBool,
    /// The app's `CFBundleVersion`.
    pub build: u64,
    /// The app's version string.
    pub version: String,
    /// The app's PID (from `LOCAL_PEERPID`).
    pub pid: Option<i32>,
    /// The latest `ui.status` this connection pushed (microphone and
    /// hotkey state), for `peek doctor`.
    status: Mutex<Option<UiStatusReport>>,
    /// The Carbon's presence as this connection last reported it (an app
    /// that never sends `presence` counts as available).
    presence: Mutex<Presence>,
}

impl UiLink {
    /// A link writing to `tx`.
    #[must_use]
    pub fn new(
        conn_id: u64,
        tx: mpsc::UnboundedSender<Frame>,
        build: u64,
        version: String,
        pid: Option<i32>,
    ) -> Self {
        Self {
            conn_id,
            tx,
            pending: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            build,
            version,
            pid,
            status: Mutex::new(None),
            presence: Mutex::new(Presence::default()),
        }
    }

    /// Records a `presence` report; returns the previous one.
    pub fn set_presence(&self, presence: Presence) -> Presence {
        std::mem::replace(
            &mut *self.presence.lock().unwrap_or_else(PoisonError::into_inner),
            presence,
        )
    }

    /// The Carbon's presence as this connection reported it.
    #[must_use]
    pub fn presence(&self) -> Presence {
        *self.presence.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps the app's latest `ui.status` push.
    pub fn set_status(&self, report: UiStatusReport) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = Some(report);
    }

    /// The app's latest `ui.status` push, if it sent one on this connection.
    #[must_use]
    pub fn status(&self) -> Option<UiStatusReport> {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Queues a message; false when the connection is gone.
    fn send_message(&self, m: Message) -> bool {
        match m.into_frame() {
            Ok(frame) => self.tx.send(frame).is_ok(),
            Err(e) => {
                tracing::error!(error = %e, "an outbound UI frame could not be built");
                false
            }
        }
    }

    /// Sends a reply to one of the app's requests; false when the connection
    /// is gone.
    pub fn send_reply(&self, reply: Reply) -> bool {
        self.send_message(Message::Reply(reply))
    }

    /// Marks the connection gone and fails every request still waiting on
    /// it at once ("disconnected before answering"), instead of letting each
    /// run to its timeout: a waiting request holds its own reference to the
    /// link, so its reply sender would otherwise never be dropped.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let waiting =
            std::mem::take(&mut *self.pending.lock().unwrap_or_else(PoisonError::into_inner));
        drop(waiting);
    }

    /// Whether [`UiLink::close`] ran.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Delivers a reply from the app to its waiting request.
    pub fn complete(&self, reply: Reply) {
        let waiter = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&reply.id);
        if let Some(tx) = waiter {
            let _ = tx.send(reply);
        } else {
            tracing::warn!(id = %reply.id, "Peek.app answered a request peekd is not waiting for");
        }
    }
}

/// Holds the current UI link, if any.
#[derive(Debug, Default)]
pub struct UiHub {
    current: Mutex<Option<Arc<UiLink>>>,
    connected: Notify,
    next_id: AtomicU64,
}

impl UiHub {
    /// A fresh connection number.
    pub fn next_conn_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// The current link.
    #[must_use]
    pub fn current(&self) -> Option<Arc<UiLink>> {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether Peek.app is connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.current().is_some()
    }

    /// The connected app's `presence` report; available when no app is
    /// connected or it never reported one.
    #[must_use]
    pub fn presence(&self) -> Presence {
        self.current().map(|l| l.presence()).unwrap_or_default()
    }

    /// Whether the Carbon can see bubbles right now (see [`Presence`]).
    #[must_use]
    pub fn carbon_available(&self) -> bool {
        self.presence().available
    }

    /// Installs a new link (newest wins) and returns the replaced one.
    pub fn install(&self, link: Arc<UiLink>) -> Option<Arc<UiLink>> {
        let old = self
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(link);
        self.connected.notify_waiters();
        old
    }

    /// Removes the link if it is still `conn_id`; true when it was current.
    pub fn remove(&self, conn_id: u64) -> bool {
        let mut cur = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if cur.as_ref().is_some_and(|l| l.conn_id == conn_id) {
            *cur = None;
            true
        } else {
            false
        }
    }

    /// Waits until a UI is connected (or the timeout passes).
    pub async fn wait_connected(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.connected.notified();
            if self.is_connected() {
                return true;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.is_connected();
            }
        }
    }

    /// Sends an event to the UI; false when none is connected.
    pub fn event<E: EventBody>(&self, body: &E, blobs: Vec<Vec<u8>>) -> bool {
        let Some(link) = self.current() else {
            return false;
        };
        match Event::new(body, blobs) {
            Ok(e) => link.send_message(Message::Event(e)),
            Err(e) => {
                tracing::error!(event = E::NAME, error = %e, "a UI event could not be built");
                false
            }
        }
    }

    /// Sends a request to the UI and waits for its reply.
    ///
    /// # Errors
    /// `peek_service_unavailable` when no UI is connected, it disconnects or
    /// does not answer in time; the app's own error reply otherwise.
    pub async fn request<O: Op>(
        &self,
        op: &O,
        blobs: Vec<Vec<u8>>,
        timeout: Duration,
    ) -> Result<(O::Output, Vec<Vec<u8>>)> {
        let link = self.current().ok_or_else(|| {
            Error::new(
                ErrorCode::PeekServiceUnavailable,
                format!(
                    "Peek.app is not connected to peekd, so `{}` cannot run",
                    O::NAME
                ),
            )
            .with_hint("open Peek (peek app install), then retry")
        })?;
        let request = Request::new(op, None, blobs)?;
        let id = request.id.clone();
        let (tx, rx) = oneshot::channel();
        link.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), tx);
        // Checked after inserting: a close() that ran before the insert is
        // seen here; one that runs after it drops our sender.
        if link.is_closed() || !link.send_message(Message::Request(request)) {
            link.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            return Err(gone(O::NAME));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => reply.into_result::<O::Output>().map_err(|e| {
                if e.origin() == Origin::Daemon {
                    e
                } else {
                    Error::new(
                        ErrorCode::UnexpectedResponse,
                        format!(
                            "Peek.app answered `{}` with an unexpected shape: {}",
                            O::NAME,
                            e.message()
                        ),
                    )
                    .with_hint(
                        "Peek.app and peekd ship together; reinstall Peek (peek app install)",
                    )
                }
            }),
            Ok(Err(_)) => Err(gone(O::NAME)),
            Err(_) => {
                link.pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id);
                Err(Error::new(
                    ErrorCode::PeekServiceUnavailable,
                    format!(
                        "Peek.app did not answer `{}` within {} s",
                        O::NAME,
                        timeout.as_secs()
                    ),
                )
                .with_hint("check that Peek is responsive (its menu bar icon), then retry"))
            }
        }
    }
}

fn gone(op: &str) -> Error {
    Error::new(
        ErrorCode::PeekServiceUnavailable,
        format!("Peek.app disconnected before answering `{op}`"),
    )
    .with_hint("reopen Peek, then retry")
}

#[cfg(test)]
mod tests {
    use super::*;
    use silicon_peek_client::ipc::ui::AppQuit;

    #[tokio::test]
    async fn a_disconnect_fails_waiting_requests_at_once() {
        let hub = Arc::new(UiHub::default());
        let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
        let link = Arc::new(UiLink::new(hub.next_conn_id(), tx, 1, "1".into(), None));
        hub.install(Arc::clone(&link));
        let waiting = {
            let hub = Arc::clone(&hub);
            tokio::spawn(async move {
                hub.request(&AppQuit { build: 2 }, Vec::new(), Duration::from_secs(90))
                    .await
            })
        };
        // The request reached the app, which then goes away without answering.
        assert!(rx.recv().await.is_some());
        let started = std::time::Instant::now();
        link.close();
        hub.remove(link.conn_id);
        let Ok(Ok(result)) = tokio::time::timeout(Duration::from_secs(5), waiting).await else {
            panic!("the waiting request must end promptly");
        };
        let Err(e) = result else {
            panic!("a disconnected app cannot answer");
        };
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(*e.code(), ErrorCode::PeekServiceUnavailable);
        assert!(
            e.message().contains("disconnected before answering"),
            "{}",
            e.message()
        );
        // A request on a link closed before it was sent fails the same way.
        let (tx, _rx) = mpsc::unbounded_channel::<Frame>();
        let late = Arc::new(UiLink::new(hub.next_conn_id(), tx, 1, "1".into(), None));
        hub.install(Arc::clone(&late));
        late.close();
        let Err(e) = hub
            .request(&AppQuit { build: 2 }, Vec::new(), Duration::from_secs(90))
            .await
        else {
            panic!("a closed link cannot answer");
        };
        assert!(e.message().contains("disconnected before answering"));
    }
}
