//! `peek ting enroll` (BLUEPRINT §3.4): (re)registers this Silicon as a Ting
//! recipient for peek through `POST /api/v1/ting/recipient`, and records the
//! enrollment in the session slot. peek never does this on its own (D7).

use std::time::Duration;

use serde_json::json;
use silicon_peek_client::{
    Result,
    identity::SlotKey,
    ids::IdempotencyKey,
    ipc::cli::Attach,
    runtime::{Store, auth_block, session::SlotTing},
    timestamp::unix_now,
};

use super::{bearer, next};
use crate::{context::Globals, output::Out, service};

pub async fn enroll(g: &Globals, out: Out) -> Result<()> {
    let key = g
        .idempotency_key()?
        .unwrap_or_else(IdempotencyKey::generate);
    let session = g.session(crate::context::store()?, false).await?;
    let recipient = bearer(g, &session, |client, _org| {
        let key = key.clone();
        async move { client.ting_enroll(&key).await }
    })
    .await?;
    let slot_key = session.slot_key.as_string();
    let now = unix_now();
    let enrolled = recipient.clone();
    session
        .store
        .update_session_async(|f| {
            if let Some(s) = f.slots.get_mut(&slot_key) {
                s.ting = Some(SlotTing {
                    subscribed: enrolled.subscribed,
                    subscription_id: Some(enrolled.subscription_id.clone()),
                    registered_at: Some(now),
                    error: None,
                });
            }
            Ok(())
        })
        .await?;
    if recipient.subscribed {
        retry_parked_deliveries(&session.store, &session.slot_key).await;
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
async fn retry_parked_deliveries(store: &Store, slot_key: &SlotKey) {
    let Ok(auth) = auth_block(store, slot_key) else {
        return;
    };
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
