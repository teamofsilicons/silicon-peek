//! `fresh_session(home, margin)`: the one refresh function the CLI and peekd
//! share (BLUEPRINT §2.5, gap-token-custody §5.4). It is load-bearing:
//!
//! 1. Read without the lock. If `access_expires_at > now + margin` and nothing
//!    is pending or rejected, return the slot.
//! 2. Take `session.lock` (async: `try_lock` + 50 ms sleeps). Re-read and
//!    re-check step 1: another process may have refreshed meanwhile.
//! 3. If no refresh is pending, set `pending_refresh_key =
//!    "peek-refresh-" + hex(blake3(refresh_token))` and `refresh_started_at`,
//!    and write atomically **before any network I/O**.
//! 4. `POST /api/v1/auth/refresh` with that key and exactly
//!    `{"refresh_token":"<stored>"}`.
//! 5. On 200: `access_expires_at = refresh_started_at + expires_in`; replace
//!    both tokens and the scope; clear the pending state; write atomically.
//! 6. On a transport error, 5xx, 429 or `409 idempotency_in_progress`: keep
//!    the pending state, release the lock, and retry with the same key and
//!    body at 1, 2, 5, 10, 30, 60, 120 and 240 s, within 10 minutes of
//!    `refresh_started_at` (IAM's replay window).
//! 7. On `401 session_rejected` or `409 idempotency_response_expired`: set
//!    `rejected = {code, at, request_id}`. Terminal.
//! 8. `503 iam_misconfigured` is an outage, never a rejection.
//!
//! The deterministic key makes a lost response and two processes holding the
//! same token safe: both derive the same key and IAM replays one rotation.

use std::time::Duration;

use super::{
    session::{RELOGIN_HINT, REPLAY_WINDOW_SECS, Rejection, SessionFile, SessionSlot},
    store::{Store, StoreLock},
};
use crate::{
    Secret,
    api::SessionResponse,
    error::{Error, ErrorCode, Result},
    http::Client,
    identity::{Context, SlotKey},
    ids::IdempotencyKey,
    timestamp::unix_now,
};

/// The CLI's margin.
pub const CLI_MARGIN: Duration = Duration::from_secs(60);
/// peekd's margin before a delivery.
pub const DELIVERY_MARGIN: Duration = Duration::from_secs(120);
/// peekd's margin when pre-warming on `focus`.
pub const PREWARM_MARGIN: Duration = Duration::from_secs(300);

/// When to retry a refresh whose outcome is uncertain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshPolicy {
    /// Delays before each retry (the first attempt is immediate).
    pub delays: Vec<Duration>,
    /// Stop scheduling retries this long after `refresh_started_at`.
    pub window: Duration,
}

impl Default for RefreshPolicy {
    /// 1, 2, 5, 10, 30, 60, 120 and 240 s, within 10 minutes.
    fn default() -> Self {
        Self {
            delays: [1, 2, 5, 10, 30, 60, 120, 240]
                .into_iter()
                .map(Duration::from_secs)
                .collect(),
            window: Duration::from_secs(u64::try_from(REPLAY_WINDOW_SECS).unwrap_or(600)),
        }
    }
}

impl RefreshPolicy {
    /// One attempt, no retries (the caller reports `backend_unavailable` and
    /// the next run resumes with the same key).
    #[must_use]
    pub fn single_attempt() -> Self {
        Self {
            delays: Vec::new(),
            ..Self::default()
        }
    }

    /// Explicit delays (tests, or a short CLI budget).
    #[must_use]
    pub fn with_delays(delays: Vec<Duration>) -> Self {
        Self {
            delays,
            ..Self::default()
        }
    }
}

fn secs(d: Duration) -> i64 {
    i64::try_from(d.as_secs()).unwrap_or(i64::MAX)
}

/// Whether a refresh failure is terminal for the family (step 7).
#[must_use]
pub fn is_terminal(e: &Error) -> bool {
    if e.is_transport() {
        return false;
    }
    match e.code() {
        ErrorCode::SessionRejected | ErrorCode::IdempotencyResponseExpired => true,
        ErrorCode::IamMisconfigured => false,
        ErrorCode::Other(code) => {
            e.status() == Some(401)
                || (e.status() == Some(400)
                    && matches!(
                        code.as_str(),
                        "invalid_grant"
                            | "refresh_token_reuse"
                            | "invalid_token"
                            | "unauthenticated"
                    ))
        }
        _ => e.status() == Some(401),
    }
}

/// Whether a refresh failure may succeed on retry with the same key (step 6).
#[must_use]
pub fn is_retryable(e: &Error) -> bool {
    e.is_transport()
        || *e.code() == ErrorCode::IdempotencyInProgress
        || e.status().is_some_and(|s| s == 429 || s >= 500)
}

/// Returns a session usable for at least `margin`, refreshing it if needed
/// with the default retry policy (§2.5). `client` must target the slot's API
/// (and carry the testing secret in a testing context); `context` selects the
/// slot `"<api_url>#<context>"`.
///
/// # Errors
/// `not_logged_in`, `session_rejected` (terminal; the slot is marked), or the
/// last retryable error (`backend_unavailable`, `iam_misconfigured`, …) with
/// the refresh left pending for the next call.
pub async fn fresh_session(
    store: &Store,
    client: &Client,
    context: Context,
    margin: Duration,
) -> Result<SessionSlot> {
    fresh_session_with(store, client, context, margin, &RefreshPolicy::default()).await
}

/// [`fresh_session`] with an explicit retry policy.
///
/// # Errors
/// As [`fresh_session`].
pub async fn fresh_session_with(
    store: &Store,
    client: &Client,
    context: Context,
    margin: Duration,
    policy: &RefreshPolicy,
) -> Result<SessionSlot> {
    let key = SlotKey::new(client.api_url().clone(), context);
    let margin = secs(margin);

    // Step 1: lock-free fast path.
    {
        let file = store.read_session()?;
        let slot = file.usable_slot(&key, store.dir())?;
        if slot.is_fresh(unix_now(), margin) {
            return Ok(slot.clone());
        }
    }
    refresh_locked(
        store,
        client,
        &key,
        policy,
        |slot, now| slot.is_fresh(now, margin),
        margin,
    )
    .await
}

/// Forces one rotation after peek-server answered 401 for `rejected_access`
/// (the `/me` rule of §2.6). If another process already rotated the family,
/// the newer session is returned without a network call.
///
/// # Errors
/// As [`fresh_session`].
pub async fn force_refresh(
    store: &Store,
    client: &Client,
    context: Context,
    rejected_access: &Secret,
    policy: &RefreshPolicy,
) -> Result<SessionSlot> {
    let key = SlotKey::new(client.api_url().clone(), context);
    let rejected = rejected_access.clone();
    refresh_locked(
        store,
        client,
        &key,
        policy,
        move |slot, now| {
            slot.access_token != rejected
                && slot.pending_refresh_key.is_none()
                && slot.access_expires_at > now
        },
        0,
    )
    .await
}

/// Step 3: returns the pending key and start time, persisting a new
/// deterministic key (before any network I/O) when none is pending.
fn persist_pending(
    store: &Store,
    lock: &StoreLock,
    file: &mut SessionFile,
    slot_key: &str,
    slot: &SessionSlot,
    now: i64,
) -> Result<(IdempotencyKey, i64)> {
    if let (Some(k), Some(started)) = (&slot.pending_refresh_key, slot.refresh_started_at) {
        return Ok((IdempotencyKey::parse(k)?, started));
    }
    let k = IdempotencyKey::refresh(slot.refresh_token.expose());
    if let Some(s) = file.slots.get_mut(slot_key) {
        s.pending_refresh_key = Some(k.as_str().to_owned());
        s.refresh_started_at = Some(now);
    }
    store.write_session(lock, file)?;
    Ok((k, now))
}

/// Step 5, under the lock: applies the rotation to the family that was sent.
fn commit_rotation(
    store: &Store,
    lock: &StoreLock,
    slot_key: &str,
    sent: &SessionSlot,
    resp: &SessionResponse,
    started_at: i64,
) -> Result<SessionSlot> {
    let mut file = store.read_session()?;
    let updated = match file.slots.get_mut(slot_key) {
        Some(s) if s.refresh_token == sent.refresh_token => {
            s.apply_refresh(resp, started_at);
            s.clone()
        }
        _ => {
            return Err(Error::internal(
                "the session changed while its refresh was in flight despite the lock",
            ));
        }
    };
    store.write_session(lock, &file)?;
    Ok(updated)
}

/// Step 7: marks the slot rejected so nobody retries, and builds the error.
fn mark_rejected(
    store: &Store,
    lock: &StoreLock,
    slot_key: &str,
    sent: &SessionSlot,
    e: &Error,
) -> Result<Error> {
    let code = match e.code() {
        ErrorCode::Other(c) => c.clone(),
        c => c.as_str().to_owned(),
    };
    let mut file = store.read_session()?;
    if let Some(s) = file.slots.get_mut(slot_key)
        && s.refresh_token == sent.refresh_token
    {
        s.rejected = Some(Rejection {
            code: code.clone(),
            at: unix_now(),
            request_id: e.request_id().map(str::to_owned),
        });
        s.pending_refresh_key = None;
        s.refresh_started_at = None;
    }
    store.write_session(lock, &file)?;
    Ok(Error::new(
        ErrorCode::SessionRejected,
        format!(
            "IAM rejected the refresh of {}'s peek session ({code}): {}",
            sent.actor.public_id,
            e.message()
        ),
    )
    .with_hint(RELOGIN_HINT)
    .with_request_id(e.request_id().map(str::to_owned))
    .with_status(e.status().unwrap_or(401)))
}

/// Step 6: the error returned once no retry fits the policy and window. The
/// pending key stays, so the next call resumes with the same key and body.
fn give_up(e: &Error, policy: &RefreshPolicy, elapsed: i64) -> Error {
    let remaining = secs(policy.window).saturating_sub(elapsed).max(0);
    let hint = if remaining > 0 {
        format!(
            "retry within {remaining} s: the refresh is pending and the next run resumes it with the same key"
        )
    } else {
        "the refresh stayed uncertain for 10 minutes; the next run tries once more, then a new login may be needed".to_owned()
    };
    let mut out = Error::new(
        e.code().clone(),
        format!("refreshing the peek session failed: {}", e.message()),
    )
    .with_hint(hint)
    .with_retryable(true)
    .with_origin(e.origin())
    .with_request_id(e.request_id().map(str::to_owned))
    .with_retry_after(e.retry_after());
    if let Some(s) = e.status() {
        out = out.with_status(s);
    }
    out
}

async fn refresh_locked(
    store: &Store,
    client: &Client,
    key: &SlotKey,
    policy: &RefreshPolicy,
    good_enough: impl Fn(&SessionSlot, i64) -> bool,
    margin: i64,
) -> Result<SessionSlot> {
    let slot_key = key.as_string();
    let mut retries = 0usize;
    let mut rotations = 0usize;
    loop {
        // Step 2: exclusive lock, then re-read.
        let lock = store.lock_async().await?;
        let mut file = store.read_session()?;
        let slot = file.usable_slot(key, store.dir())?.clone();
        let now = unix_now();
        if good_enough(&slot, now) {
            return Ok(slot);
        }
        let (pending, started_at) =
            persist_pending(store, &lock, &mut file, &slot_key, &slot, now)?;

        // Step 4: the exact body, the persisted key.
        match client.refresh(&slot.refresh_token, &pending).await {
            Ok(resp) => {
                let updated = commit_rotation(store, &lock, &slot_key, &slot, &resp, started_at)?;
                drop(lock);
                rotations += 1;
                let now = unix_now();
                if updated.is_fresh(now, margin) || good_enough(&updated, now) {
                    return Ok(updated);
                }
                // An old replay can come back nearly expired; its new token
                // derives a new key, so one more rotation is safe.
                if rotations >= 2 {
                    if updated.access_expires_at > now {
                        return Ok(updated);
                    }
                    return Err(Error::new(
                        ErrorCode::BackendUnavailable,
                        "peek-server returned an access token that is already expired",
                    )
                    .with_hint("check this machine's clock (peek measures expiry from when the refresh started)"));
                }
            }
            Err(e) if is_terminal(&e) => {
                return Err(mark_rejected(store, &lock, &slot_key, &slot, &e)?);
            }
            Err(e) if is_retryable(&e) => {
                // Step 6: keep the pending key, release the lock, retry.
                drop(lock);
                let elapsed = unix_now().saturating_sub(started_at);
                match policy.delays.get(retries).copied() {
                    Some(delay) if elapsed.saturating_add(secs(delay)) < secs(policy.window) => {
                        retries += 1;
                        tokio::time::sleep(delay).await;
                    }
                    _ => return Err(give_up(&e, policy, elapsed)),
                }
            }
            // Neither terminal nor retryable (a 4xx bug): keep the pending
            // state and surface the error.
            Err(e) => return Err(e),
        }
    }
}
