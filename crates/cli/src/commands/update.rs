//! `peek update` (BLUEPRINT §4.6): Honeycomb guidance. peek never replaces
//! its own binary; Honeycomb's per-home worker updates the CLI and peekd
//! updates Peek.app.

use serde_json::{Value, json};
use silicon_peek_client::{Result, VERSION, telemetry::is_off_value};
use std::fmt::Write as _;

use crate::{context::Globals, output::Out};

#[cfg(target_os = "macos")]
async fn app_value() -> Value {
    let Ok(paths) = crate::service::paths() else {
        return Value::Null;
    };
    let (build, _, _) = crate::service::bundle_info(&paths.app).await;
    json!({"build": build, "best_offer": crate::service::best_offer(&paths).map(|(b, _, _)| b)})
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::unused_async)] // mirrors the macOS signature
async fn app_value() -> Value {
    Value::Null
}

pub async fn run(_g: &Globals, out: Out) -> Result<()> {
    let auto_update = !std::env::var("HONEYCOMB_AUTO_UPDATE").is_ok_and(|v| is_off_value(&v));
    let value = json!({
        "manager": "honeycomb",
        "app_id": "peek",
        "current_version": VERSION,
        "auto_update": auto_update,
        "can_replace_running_binary": false,
        "command": "honeycomb update 'peek'",
        "app": app_value().await,
        "message": "Honeycomb manages the peek CLI: its per-home worker installs new releases every minute. peekd updates Peek.app (newest build wins, never a downgrade). peek never replaces itself.",
    });
    out.value(&value, |v| {
        let mut s = format!(
            "peek {} is managed by Honeycomb (automatic updates {}).\nUpdate now: {}",
            v["current_version"].as_str().unwrap_or_default(),
            if v["auto_update"] == true {
                "on"
            } else {
                "off"
            },
            v["command"].as_str().unwrap_or_default()
        );
        if let Some(app) = v["app"].as_object() {
            let _ = write!(
                s,
                "\nPeek.app build {} installed; newest offered build {} (peek app update)",
                app.get("build")
                    .map_or_else(|| "-".to_owned(), ToString::to_string),
                app.get("best_offer")
                    .map_or_else(|| "-".to_owned(), ToString::to_string)
            );
        }
        s
    });
    Ok(())
}
