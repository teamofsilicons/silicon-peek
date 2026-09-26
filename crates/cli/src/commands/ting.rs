//! `peek ting enroll` (BLUEPRINT §3.4): (re)registers this Silicon as a Ting
//! recipient for peek through `POST /api/v1/ting/recipient`, and records the
//! enrollment in the session slot. peek never does this on its own (D7).

use serde_json::json;
use silicon_peek_client::{
    Result, ids::IdempotencyKey, runtime::session::SlotTing, timestamp::unix_now,
};

use super::{bearer, next};
use crate::{context::Globals, output::Out};

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
