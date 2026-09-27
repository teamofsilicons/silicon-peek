//! `peek status`: this Silicon's state on the Mac in one view (position,
//! drawing, queue, asks, deliveries) plus Peek.app and peekd.

use serde_json::{Value, json};
use silicon_peek_client::{Result, ipc::cli::StatusOp, runtime::daemon::REQUEST_TIMEOUT};

use super::{mac_session, next};
use crate::{
    context::Globals,
    output::{Out, kv},
    service::{self, require_mac},
};

pub async fn run(g: &Globals, out: Out) -> Result<()> {
    require_mac("status")?;
    let (session, auth) = mac_session(g).await?;
    let mut svc = service::ensure_service().await?;
    let (result, _) = svc
        .call(&StatusOp {}, Some(&auth), Vec::new(), REQUEST_TIMEOUT)
        .await?;
    out.warnings(&result.warnings);
    let mut value = serde_json::to_value(&result).unwrap_or(Value::Null);
    let actor = session
        .store
        .read_session()
        .ok()
        .and_then(|f| f.slot(&session.slot_key).map(|s| s.actor_id().to_string()));
    value["actor_id"] = json!(actor);
    value["context"] = json!(session.context.to_string());
    value["daemon"] = json!({
        "running": true,
        "version": svc.hello.peekd_version,
        "protocol": svc.hello.protocol,
    });
    value["app"] = json!({
        "build": svc.hello.app.as_ref().map(|a| a.build),
        "ui_running": svc.hello.app.as_ref().is_some_and(|a| a.ui_running) || result.ui_running,
    });
    out.value(&value, kv);
    let mut hints: Vec<&str> = Vec::new();
    if result.slot.is_none() {
        hints.push("peek register side <1-8>");
    }
    if result.drawing.is_none() {
        hints.push("peek register drawing ./logo.js");
    }
    if result.queue.waiting > 0 {
        hints.push("peek queue");
    }
    if result.deliveries.authority_required > 0 {
        hints.push(
            "peek login status --json (deliveries wait for a valid session or `peek ting enroll`)",
        );
    }
    next(out, &hints);
    Ok(())
}
