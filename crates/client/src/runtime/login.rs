//! Login, recovery, logout and revocation bookkeeping (BLUEPRINT §2.4, §2.6).
//!
//! These functions do the store and backend parts; the CLI adds the peekd
//! `attach`/`detach` calls, `ensure_app()` and the printed output.

use std::time::Duration;

use super::{
    session::{PendingLogin, PendingRevocation, REPLAY_WINDOW_SECS, SessionSlot},
    store::Store,
};
use crate::{
    Secret,
    api::{SessionResponse, TingEnrollment},
    error::{Error, ErrorCode, Result},
    http::Client,
    identity::{ActorId, Context, OrgId, SlotKey},
    ids::IdempotencyKey,
    timestamp::unix_now,
};

/// In-process retries of an uncertain SLT exchange: 1 s, 3 s, 9 s.
#[must_use]
pub fn default_login_delays() -> Vec<Duration> {
    [1, 3, 9].into_iter().map(Duration::from_secs).collect()
}

/// A committed login.
#[derive(Clone, Debug)]
pub struct LoginOutcome {
    /// The slot key.
    pub slot_key: SlotKey,
    /// The new session.
    pub slot: SessionSlot,
    /// Ting enrollment as reported by the backend.
    pub ting: Option<TingEnrollment>,
    /// The replaced family's refresh token, queued for revocation.
    pub replaced: Option<PendingRevocation>,
    /// This home's IPC token (created on the first login).
    pub daemon_token: Secret,
}

fn is_uncertain(e: &Error) -> bool {
    e.is_transport()
        || e.status().is_some_and(|s| s >= 500)
        || *e.code() == ErrorCode::IdempotencyInProgress
}

/// `peek login <SLT>` steps 1–5: records `pending_login`, exchanges the SLT
/// (retrying uncertain outcomes with the same key and body), then commits the
/// session under the lock and creates `daemon-token` if absent.
///
/// # Errors
/// `slt_is_public_id` (production), `slt_rejected`, `private_application_organization_required`,
/// or the last uncertain error, with `pending_login` kept for `--recover`.
pub async fn login(
    store: &Store,
    client: &Client,
    context: Context,
    slt: &Secret,
    org_hint: Option<&OrgId>,
    delays: &[Duration],
) -> Result<LoginOutcome> {
    let slt_text = slt.expose().trim();
    if slt_text.is_empty() {
        return Err(Error::invalid_input("the SLT is empty").with_hint(
            "mint one: iam silicon-login --app-id peek --grant-org <org> --approve-scopes",
        ));
    }
    if context == Context::Production && ActorId::looks_like_public_id(slt_text) {
        return Err(Error::new(
            ErrorCode::SltIsPublicId,
            "that looks like a public ID (si:… or c:…), not a short-lived token; only testing environments accept public IDs",
        )
        .with_hint("mint an SLT: iam silicon-login --app-id peek --grant-org <org> --approve-scopes"));
    }
    let slt = Secret::new(slt_text);
    let key = SlotKey::new(client.api_url().clone(), context);
    // A real SLT is single-use, so its derived key names exactly one login.
    // A public ID (testing environments only; refused above in production)
    // repeats on every login: each attempt gets its own key.
    let idem = if ActorId::looks_like_public_id(slt.expose()) {
        IdempotencyKey::login_attempt(slt.expose())
    } else {
        IdempotencyKey::login(slt.expose())
    };
    let started_at = unix_now();
    store
        .update_session_async(|f| {
            f.pending_login = Some(PendingLogin {
                key: idem.as_str().to_owned(),
                slt: slt.clone(),
                started_at,
                slot: Some(key.as_string()),
                org_hint: org_hint.map(|o| o.as_str().to_owned()),
            });
            Ok(())
        })
        .await?;
    exchange_and_commit(
        store, client, &key, &slt, org_hint, &idem, started_at, delays,
    )
    .await
}

/// `peek login --recover`: retries the pending exchange with its original key
/// and SLT within IAM's 10-minute replay window.
///
/// # Errors
/// `invalid_input` when nothing is pending or it targets another API or
/// context; `login_attempt_expired` after 10 minutes; otherwise as [`login`].
pub async fn recover_login(
    store: &Store,
    client: &Client,
    context: Context,
    delays: &[Duration],
) -> Result<LoginOutcome> {
    let key = SlotKey::new(client.api_url().clone(), context);
    let file = store.read_session()?;
    let pending = file.pending_login.clone().ok_or_else(|| {
        Error::invalid_input("there is no interrupted login to recover in this home")
            .with_hint("run peek login '<SLT>' with a fresh SLT")
    })?;
    if pending
        .slot
        .as_deref()
        .is_some_and(|s| s != key.as_string())
    {
        return Err(Error::invalid_input(format!(
            "the interrupted login targeted {}, not {key}",
            pending.slot.as_deref().unwrap_or("?")
        ))
        .with_hint("rerun --recover with the same --api and --test as the original login"));
    }
    if unix_now().saturating_sub(pending.started_at) > REPLAY_WINDOW_SECS {
        store
            .update_session_async(|f| {
                f.pending_login = None;
                Ok(())
            })
            .await?;
        return Err(Error::new(
            ErrorCode::LoginAttemptExpired,
            "the interrupted login is older than 10 minutes, so IAM can no longer replay it",
        )
        .with_hint("mint a new SLT and run peek login '<SLT>'"));
    }
    let idem = IdempotencyKey::parse(&pending.key)?;
    let org = pending.org_hint.as_deref().map(OrgId::parse).transpose()?;
    exchange_and_commit(
        store,
        client,
        &key,
        &pending.slt,
        org.as_ref(),
        &idem,
        pending.started_at,
        delays,
    )
    .await
}

#[allow(clippy::too_many_arguments)] // one call site each in login/recover
async fn exchange_and_commit(
    store: &Store,
    client: &Client,
    key: &SlotKey,
    slt: &Secret,
    org_hint: Option<&OrgId>,
    idem: &IdempotencyKey,
    started_at: i64,
    delays: &[Duration],
) -> Result<LoginOutcome> {
    let mut attempt = 0usize;
    let resp: SessionResponse = loop {
        match client.login(slt, org_hint, idem).await {
            Ok(r) => break r,
            Err(e) if is_uncertain(&e) => {
                if let Some(d) = delays.get(attempt) {
                    attempt += 1;
                    tokio::time::sleep(*d).await;
                    continue;
                }
                return Err(Error::new(
                    e.code().clone(),
                    format!(
                        "logging in failed and its outcome is uncertain: {}",
                        e.message()
                    ),
                )
                .with_hint(
                    "run `peek login --recover` within 10 minutes; it replays the same exchange",
                )
                .with_retryable(true)
                .with_origin(e.origin())
                .with_request_id(e.request_id().map(str::to_owned)));
            }
            Err(e) => {
                // A definitive answer: the SLT is spent or invalid.
                store
                    .update_session_async(|f| {
                        f.pending_login = None;
                        Ok(())
                    })
                    .await?;
                return Err(e);
            }
        }
    };
    if !resp.access_token.expose().starts_with("oat_")
        || !resp.refresh_token.expose().starts_with("ort_")
        || resp.expires_in == 0
    {
        return Err(Error::new(
            ErrorCode::UnexpectedResponse,
            "peek-server returned tokens that are not an IAM oat_/ort_ pair with a lifetime",
        )
        .with_hint(
            "check --api / PEEK_API_URL; report it with peek report if it points at peek-server",
        ));
    }
    let now = unix_now();
    let session = SessionSlot::from_login(&resp, started_at, now);
    let lock = store.lock_async().await?;
    let mut file = store.read_session()?;
    let replaced = file.record_login(key, session.clone(), now);
    store.write_session(&lock, &file)?;
    let daemon_token = store.ensure_daemon_token(&lock)?;
    drop(lock);
    Ok(LoginOutcome {
        slot_key: key.clone(),
        slot: session,
        ting: resp.ting,
        replaced,
        daemon_token,
    })
}

/// Whether the backend confirmed the remote revocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRevocation {
    /// The backend answered 2xx.
    Confirmed,
    /// Kept in `pending_revocations`; retried by the next CLI run or peekd.
    Pending,
}

/// A finished logout.
#[derive(Clone, Debug)]
pub struct LogoutOutcome {
    /// The actor that was logged out, if a session existed.
    pub actor: Option<ActorId>,
    /// Remote revocation state.
    pub remote_revocation: RemoteRevocation,
    /// The backend error, when revocation stayed pending.
    pub error: Option<String>,
}

/// `peek logout` steps 1–3: tombstone + queue the refresh token, revoke it
/// remotely, then delete the slot. The Silicon's Ting grant is revoked too
/// only with `revoke_ting` (`peek logout --revoke-ting`): other homes of the
/// same Silicon share it. Local logout always completes; the outcome never
/// claims a remote revocation it does not have.
///
/// # Errors
/// Store failures only; backend failures become `remote_revocation: pending`.
pub async fn logout(
    store: &Store,
    client: &Client,
    context: Context,
    revoke_ting: bool,
) -> Result<LogoutOutcome> {
    let key = SlotKey::new(client.api_url().clone(), context);
    let now = unix_now();
    // With no slot there is nothing to revoke; `login status` already
    // reports `authenticated:false` (`no_session`).
    let begun = store
        .update_session_async(|f| Ok(f.begin_logout(&key, now)))
        .await?;
    let Some((slot, revocation)) = begun else {
        return Ok(LogoutOutcome {
            actor: None,
            remote_revocation: RemoteRevocation::Confirmed,
            error: None,
        });
    };
    let idem = IdempotencyKey::parse(&revocation.key)?;
    let result = client
        .with_session(slot.access_token.clone(), slot.org_id.clone())
        .logout(&revocation.token, &idem, revoke_ting)
        .await;
    let (remote, error) = match &result {
        Ok(()) => (RemoteRevocation::Confirmed, None),
        Err(e) => (RemoteRevocation::Pending, Some(e.message().to_owned())),
    };
    store
        .update_session_async(|f| {
            if remote == RemoteRevocation::Confirmed {
                f.complete_revocation(&revocation.key);
            }
            f.finish_logout(&key, &slot.refresh_token);
            Ok(())
        })
        .await?;
    Ok(LogoutOutcome {
        actor: Some(slot.actor.public_id.clone()),
        remote_revocation: remote,
        error,
    })
}

/// Counts from [`revoke_pending`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RevocationSweep {
    /// Confirmed and removed.
    pub confirmed: usize,
    /// Still pending.
    pub pending: usize,
}

/// Best-effort revocation of every queued refresh token (login step 6; also
/// run by later CLI invocations and peekd). `client_for` builds a client for
/// a slot key (the right API and testing secret), or `None` to skip it.
///
/// # Errors
/// Store failures only.
pub async fn revoke_pending(
    store: &Store,
    client_for: impl Fn(&SlotKey) -> Option<Client>,
) -> Result<RevocationSweep> {
    let queued = store.read_session()?.pending_revocations;
    let mut sweep = RevocationSweep::default();
    let mut done = Vec::new();
    for r in queued {
        let client = r
            .slot
            .as_deref()
            .and_then(|s| SlotKey::parse(s).ok())
            .and_then(|k| client_for(&k));
        let Some(client) = client else {
            sweep.pending += 1;
            continue;
        };
        let Ok(idem) = IdempotencyKey::parse(&r.key) else {
            sweep.pending += 1;
            continue;
        };
        match client
            .without_session()
            .logout(&r.token, &idem, false)
            .await
        {
            Ok(()) => {
                sweep.confirmed += 1;
                done.push(r.key);
            }
            Err(_) => sweep.pending += 1,
        }
    }
    if !done.is_empty() {
        store
            .update_session_async(|f| {
                for k in &done {
                    f.complete_revocation(k);
                }
                Ok(())
            })
            .await?;
    }
    Ok(sweep)
}
