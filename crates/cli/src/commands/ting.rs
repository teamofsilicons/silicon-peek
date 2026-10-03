//! `peek ting enroll` (BLUEPRINT §3.4): (re)registers this Silicon as a Ting
//! recipient for peek through `POST /api/v1/ting/recipient`, and records the
//! enrollment in the session slot. peek never does this on its own (D7).

use std::time::Duration;

use serde_json::json;
use silicon_peek_client::{
    Result,
    identity::SlotKey,
    ipc::cli::Attach,
    runtime::{Store, auth_block},
};

use super::next;
use crate::{context::Globals, output::Out, service};

pub async fn enroll(g: &Globals, out: Out) -> Result<()> {
    use silicon_peek_client::runtime::authorization::{self, Action};
    let session = g.session(crate::context::store()?, false).await?;
    let stored = session.store.read_session()?;
    let before = stored.usable_slot(&session.slot_key, session.store.dir())?;
    g.session_org(before)?;
    let expected = before.context_id()?.to_owned();
    super::fresh(&session).await?;
    authorization::perform(
        &session.store,
        &session.client,
        &session.slot_key,
        &expected,
        Action::Enroll(g.idempotency_key()?),
    )
    .await?;
    let stored = session.store.read_session()?;
    let slot = stored.usable_slot(&session.slot_key, session.store.dir())?;
    if slot.context_id()? != expected {
        return Err(silicon_peek_client::Error::new(
            silicon_peek_client::ErrorCode::SessionRejected,
            "the account changed after enrollment",
        ));
    }
    let recipient = slot
        .ting
        .as_ref()
        .ok_or_else(|| silicon_peek_client::Error::internal("enrollment was not recorded"))?;
    if recipient.subscribed {
        retry_parked_deliveries(&session.store, &session.slot_key, &expected).await;
    }
    out.value(
        &json!({"subscribed": recipient.subscribed, "subscription_id": recipient.subscription_id}),
        |v| {
            format!(
                "enrolled as a Ting recipient for peek ({})",
                v["subscription_id"].as_str().unwrap_or_default()
            )
        },
    );
    next(out, &["peek login status --json", "peek status"]);
    Ok(())
}

/// Re-attaches the home to a running peekd (never starts it), which makes
/// this Silicon's deliveries parked in `authority_required` due now instead
/// of at the next ten-minute retry. Best effort: a peekd that is not running
/// retries them when it starts.
async fn retry_parked_deliveries(store: &Store, slot_key: &SlotKey, expected: &str) {
    let Ok(auth) = auth_block(store, slot_key) else {
        return;
    };
    if auth.context_id.as_deref() != Some(expected) {
        return;
    }
    let task = async {
        let mut s = service::connect_existing(Duration::from_millis(500))
            .await
            .ok()??;
        s.call(&Attach {}, Some(&auth), Vec::new(), Duration::from_secs(3))
            .await
            .ok()
    };
    let _ = tokio::time::timeout(Duration::from_secs(4), task).await;
}

/// Explicit feature approval; ordinary authentication and queued work stay independent.
pub async fn permission(g: &Globals, out: Out, command: crate::cli::TingCommand) -> Result<()> {
    use crate::cli::TingCommand;
    use silicon_peek_client::runtime::authorization::{self, Action};
    let action = match command {
        TingCommand::Authorize => Action::Start,
        TingCommand::AuthorizationStatus => Action::Status,
        TingCommand::CancelAuthorization => Action::Cancel,
        TingCommand::CompleteAuthorization { code_file } => {
            g.check_stdin(&[("--code-file", code_file.as_deref() == Some("-"))])?;
            Action::Complete(
                code_file
                    .as_deref()
                    .map(|path| crate::input::read_secret("--code-file", path))
                    .transpose()?,
            )
        }
        TingCommand::Enroll => return enroll(g, out).await,
    };
    let session = g.session(crate::context::store()?, false).await?;
    let stored = session.store.read_session()?;
    let before = stored.usable_slot(&session.slot_key, session.store.dir())?;
    g.session_org(before)?;
    let expected = before.context_id()?.to_owned();
    if !matches!(action, Action::Cancel) {
        super::fresh(&session).await?;
    }
    let result = authorization::perform(
        &session.store,
        &session.client,
        &session.slot_key,
        &expected,
        action.clone(),
    )
    .await?;
    out.value(&json!({"context_id": expected, "request":result}), |value| {
        let request = &value["request"];
        if request.is_null() { if matches!(action, Action::Cancel) { "Permission review cancelled locally. Your queued work is retained.".into() } else { "No permission review saved. Run peek ting authorize to start.".into() } }
        else if request["completed"] == true { "Ting permission approved. Run peek ting enroll explicitly to enable deliveries and retry queued answers.".into() }
        else if let Some(url) = request["authorization"]["authorization_url"].as_str() {
            format!("Review Ting permission in IAM: {url}\nThen: peek ting complete-authorization --code-file - (keep the same profile, org and testing environment)")
        } else { format!("Permission request status: {}", request["authorization"]["status"]) }
    });
    Ok(())
}
