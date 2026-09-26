//! Telemetry helpers shared by every peek component (BLUEPRINT §6).
//!
//! Events are relayed through peek-server's gateway; no component but the
//! backend holds a table key. These helpers decide opt-out and hash actor IDs,
//! so raw identities never leave the machine.

use sha2::{Digest, Sha256};

use crate::identity::{ActorId, OrgId};

/// Environment variables whose value `0`, `false`, `off` or `no` turns
/// telemetry off (any one of them wins).
pub const OPT_OUT_ENV: [&str; 3] = [
    "PEEK_TELEMETRY",
    "SPACE_STATION_TELEMETRY",
    "SILICON_TELEMETRY",
];

/// Whether a value means "off".
#[must_use]
pub fn is_off_value(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "off" | "no"
    )
}

/// Whether the process environment opts out of telemetry.
#[must_use]
pub fn env_opt_out() -> bool {
    OPT_OUT_ENV
        .iter()
        .any(|k| std::env::var(k).is_ok_and(|v| is_off_value(&v)))
}

/// The telemetry identity of an actor: `hex(sha256("<org>:<actor>"))[..16]`
/// (BLUEPRINT D28). Raw actor IDs are never recorded.
#[must_use]
pub fn actor_hash(org: &OrgId, actor: &ActorId) -> String {
    let digest = Sha256::digest(format!("{org}:{actor}").as_bytes());
    let mut hex = hex::encode(digest);
    hex.truncate(16);
    hex
}

/// Keys allowed in an event's `context` object (§6.4). Anything else is
/// dropped before an event is recorded.
pub const CONTEXT_KEYS: [&str; 29] = [
    "slot",
    "mode",
    "appearance",
    "speak_chars",
    "show_elements",
    "show_kinds",
    "ask_type",
    "options_count",
    "drawing_bytes",
    "drawing_sha256",
    "answer_latency_ms",
    "gesture",
    "input",
    "shortcut",
    "ting_status",
    "attempt",
    "http_route",
    "method",
    "status",
    "update_from",
    "update_to",
    "command",
    "tts_model",
    "tts_ttfb_ms",
    "stt_ms",
    "stt_language",
    "matched",
    "dg_request_id",
    "key_source",
];

/// Removes every non-allowlisted key from an event `context` object.
pub fn scrub_context(context: &mut serde_json::Map<String, serde_json::Value>) {
    context.retain(|k, _| CONTEXT_KEYS.contains(&k.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;

    #[test]
    fn off_values() {
        for v in ["0", "false", "OFF", " no "] {
            assert!(is_off_value(v), "{v}");
        }
        for v in ["1", "on", "true", ""] {
            assert!(!is_off_value(v), "{v}");
        }
    }

    #[test]
    fn actor_hash_is_stable_and_short() -> Result<()> {
        let h = actor_hash(&OrgId::parse("tos")?, &ActorId::parse("si:cleanup")?);
        assert_eq!(h.len(), 16);
        assert_eq!(h, hex::encode(Sha256::digest(b"tos:si:cleanup"))[..16]);
        Ok(())
    }

    #[test]
    fn scrubbing_keeps_only_allowlisted_keys() {
        let mut m = serde_json::Map::new();
        m.insert("slot".into(), 3.into());
        m.insert("answer_text".into(), "secret".into());
        scrub_context(&mut m);
        assert_eq!(m.len(), 1);
        assert!(m.contains_key("slot"));
    }
}
