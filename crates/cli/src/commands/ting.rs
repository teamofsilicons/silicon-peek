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
    g.session_account(before)?;
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
