//! Durable, explicitly requested feature approval shared by CLI and Peek.app.
//! The store lock pins the login and retry receipt through each provider request.
use super::{
    Store,
    fs::{read_private, write_atomic},
};
use crate::{
    Error, ErrorCode, Result, Secret, authorization::TingAuthorization, http::Client,
    identity::SlotKey, ids::IdempotencyKey, timestamp::unix_now,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const FILE: &str = "feature-consent.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pending {
    start_key: String,
    finish_key: String,
    request: Option<TingAuthorization>,
    code: Option<Secret>,
    code_started_at: Option<i64>,
}
impl Default for Pending {
    fn default() -> Self {
        Self {
            start_key: IdempotencyKey::generate().as_str().into(),
            finish_key: IdempotencyKey::generate().as_str().into(),
            request: None,
            code: None,
            code_started_at: None,
        }
    }
}
fn read(store: &Store) -> Result<BTreeMap<String, Pending>> {
    read_private(&store.path(FILE))?.map_or_else(
        || Ok(BTreeMap::new()),
        |bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|_| Error::internal("permission retry state is invalid"))
        },
    )
}
pub(super) fn forget_context(store: &Store, context_id: &str) -> Result<()> {
    let mut pending = read(store)?;
    pending.remove(context_id);
    save(store, &pending)
}

fn save(store: &Store, pending: &BTreeMap<String, Pending>) -> Result<()> {
    let bytes = serde_json::to_vec(pending)
        .map_err(|_| Error::internal("permission retry state could not be encoded"))?;
    write_atomic(store.dir(), FILE, &bytes)
}

/// Explicit approval action. Completing approval never enrolls or sends queued work.
#[derive(Clone, Debug)]
pub enum Action {
    /// Create a request or resume the same interrupted start.
    Start,
    /// Fetch the existing approval's state.
    Status,
    /// Complete with the supplied code, or replay a pending completion.
    Complete(Option<Secret>),
    /// Abandon this local request. Queued work and drafts remain untouched.
    Cancel,
    /// Explicitly enroll the recipient after approval; the UI explains queued delivery retries.
    Enroll(Option<IdempotencyKey>),
}

/// Runs an explicitly selected feature action for one frozen login context.
/// The caller supplies a fresh, world-scoped client; this function attaches the locked session.
///
/// # Errors
/// Changed context, provider/transport errors, or a stale/declined approval.
#[allow(clippy::too_many_lines)] // Keep persistence and all provider branches visibly under the same store lock.
pub async fn perform(
    store: &Store,
    client: &Client,
    key: &SlotKey,
    expected_context: &str,
    action: Action,
) -> Result<Option<TingAuthorization>> {
    let lock = store.lock_async().await?;
    let file = store.read_session()?;
    let slot = file.usable_slot(key, store.dir())?;
    if slot.context_id()? != expected_context || client.api_url() != key.api_url() {
        return Err(Error::new(
            ErrorCode::SessionRejected,
            "the account changed; return to the original permission context",
        ));
    }
    let mut all = read(store)?;
    if matches!(action, Action::Cancel) {
        all.remove(expected_context);
        save(store, &all)?;
        return Ok(None);
    }
    if matches!(action, Action::Status) && !all.contains_key(expected_context) {
        return Ok(None);
    }
    let pending = match action {
        Action::Start => all.entry(expected_context.into()).or_default(),
        _ => all.get_mut(expected_context).ok_or_else(|| {
            Error::invalid_input("start Ting permission review for this account first")
        })?,
    };
    let old = pending.request.clone();
    if matches!(action, Action::Enroll(_)) && !old.as_ref().is_some_and(|request| request.completed)
    {
        return Err(Error::new(
            ErrorCode::ReconsentRequired,
            "complete Ting permission review before enabling deliveries",
        ));
    }
    if let Action::Complete(code) = &action {
        let request = old.as_ref().ok_or_else(|| {
            Error::invalid_input("retry the permission request before entering a code")
        })?;
        if !request.completed
            && matches!(
                request.authorization.status.as_str(),
                "declined" | "expired"
            )
        {
            all.remove(expected_context);
            save(store, &all)?;
            return Err(Error::new(
                ErrorCode::ReconsentRequired,
                "permission was declined or expired; start a fresh review",
            ));
        }
        if !request.completed
            && request.authorization.expires_at.unix() <= unix_now()
            && pending.code.is_none()
        {
            all.remove(expected_context);
            save(store, &all)?;
            return Err(Error::new(
                ErrorCode::ReconsentRequired,
                "permission review expired; start again",
            ));
        }
        if pending.code_started_at.is_some_and(|started| {
            unix_now().saturating_sub(started) > super::session::REPLAY_WINDOW_SECS
        }) {
            all.remove(expected_context);
            save(store, &all)?;
            return Err(Error::new(
                ErrorCode::ReconsentRequired,
                "the approval code recovery window expired; start a fresh review",
            ));
        }
        if let Some(code) = code {
            if code.expose().trim().is_empty() {
                return Err(Error::invalid_input("enter the IAM approval code"));
            }
            if pending.code.as_ref().is_some_and(|saved| saved != code) {
                return Err(Error::invalid_input(
                    "a different approval code is already pending; recover the same request or cancel it",
                ));
            }
            pending.code = Some(code.clone());
            pending.code_started_at.get_or_insert_with(unix_now);
        }
        if !request.completed && pending.code.is_none() {
            return Err(Error::invalid_input(
                "provide an approval code or retry a pending completion",
            ));
        }
    }
    let receipt = pending.clone();
    save(store, &all)?; // both retry keys and the exact completion code precede network I/O
    let authed = client.with_session(slot.access_token.clone(), slot.org_id.clone());
    let outcome = match action {
        Action::Start | Action::Status if receipt.request.is_none() => {
            authed
                .ting_authorization_start(&IdempotencyKey::parse(&receipt.start_key)?)
                .await
        }
        Action::Start | Action::Status => {
            authed
                .ting_authorization_status(
                    old.as_ref()
                        .ok_or_else(|| {
                            Error::invalid_input("permission start is pending; retry start")
                        })?
                        .request_id,
                )
                .await
        }
        Action::Complete(_) if old.as_ref().is_some_and(|request| request.completed) => Ok(old
            .clone()
            .ok_or_else(|| Error::internal("approval missing"))?),
        Action::Complete(_) => {
            let request = old
                .as_ref()
                .ok_or_else(|| Error::internal("approval missing"))?;
            let code = receipt
                .code
                .as_ref()
                .ok_or_else(|| Error::internal("approval code missing"))?;
            authed
                .ting_authorization_complete(
                    request.request_id,
                    code,
                    &IdempotencyKey::parse(&receipt.finish_key)?,
                )
                .await
        }
        Action::Enroll(ref override_key) => {
            let request = old
                .as_ref()
                .ok_or_else(|| Error::internal("approval missing"))?;
            let enroll_key = match override_key {
                Some(key) => key.clone(),
                None => IdempotencyKey::parse(&format!("peek-enroll-{}", request.request_id))?,
            };
            let enrolled = authed.ting_enroll(&enroll_key).await?;
            let mut file = store.read_session()?;
            let current = file.slots.get_mut(&key.as_string()).ok_or_else(|| {
                Error::new(ErrorCode::SessionRejected, "selected account disappeared")
            })?;
            current.ting = Some(super::session::SlotTing {
                subscribed: enrolled.subscribed,
                subscription_id: Some(enrolled.subscription_id),
                registered_at: Some(unix_now()),
                error: None,
            });
            store.write_session(&lock, &file)?;
            Ok(request.clone())
        }
        Action::Cancel => unreachable!(),
    };
    match outcome {
        Ok(response) => {
            response.validate(&slot.actor, &slot.org_id, old.as_ref())?;
            if matches!(action, Action::Complete(_)) && !response.completed {
                return Err(Error::new(
                    ErrorCode::UnexpectedResponse,
                    "approval was not completed; retry the same request",
                ));
            }
            if let Some(pending) = all.get_mut(expected_context) {
                pending.request = Some(response.clone());
                if response.completed {
                    pending.code = None;
                    pending.code_started_at = None;
                }
            }
            save(store, &all)?;
            Ok(Some(response))
        }
        Err(error) if error.status() == Some(412) => {
            all.remove(expected_context);
            save(store, &all)?;
            Err(Error::new(ErrorCode::ReconsentRequired, "permission terms changed; start a fresh review in IAM. Your pending work is retained").with_status(412))
        }
        Err(error) => Err(error),
    }
}
