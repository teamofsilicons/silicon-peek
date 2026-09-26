//! Shared fixtures for the integration tests.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use serde_json::{Value, json};
use silicon_peek_client::{
    Secret,
    http::Client,
    identity::{ApiUrl, Context, SlotKey},
    runtime::{SessionFile, Store},
    timestamp::unix_now,
};

pub const FULL_SCOPE: &str = "obo:ting:subscriptions.register obo:ting:subscriptions.revoke obo:ting:tings.send self.identity.read self.membership.read self.profile.read";

/// A login/refresh response body.
pub fn session_body(access: &str, refresh: &str, expires_in: u64) -> Value {
    json!({
        "access_token": access, "refresh_token": refresh, "token_type": "Bearer",
        "expires_in": expires_in, "scope": FULL_SCOPE,
        "actor": {"type": "silicon", "public_id": "si:cleanup"},
        "org_id": "tos", "org_ids": ["tos"], "membership_id": "si:cleanup[tos]",
        "reconsent_required": false, "display_name": "Cleanup",
        "ting": {"subscribed": true, "subscription_id": "sub_1"}
    })
}

/// An error body in the server's envelope.
pub fn error_body(code: &str, message: &str, retryable: bool) -> Value {
    json!({"error": {"code": code, "message": message, "hint": null, "retryable": retryable, "request_id": "req_1"}})
}

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub store: Store,
    pub client: Client,
    pub key: SlotKey,
}

/// A fresh store under a temp `SILICON_HOME` and a client for `api`.
pub fn fixture(api: &str) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join(".peek")).expect("store");
    let api = ApiUrl::parse(api).expect("api url");
    let client = Client::new(&api).expect("client");
    let key = SlotKey::new(api, Context::Production);
    Fixture {
        dir,
        store,
        client,
        key,
    }
}

/// Writes a slot whose access token expires at `expires_at`.
pub fn write_slot(f: &Fixture, access: &str, refresh: &str, expires_at: i64) {
    let now = unix_now();
    let slot = json!({
        "actor": {"type": "silicon", "public_id": "si:cleanup"},
        "org_id": "tos", "org_ids": ["tos"], "membership_id": "si:cleanup[tos]",
        "scope": FULL_SCOPE, "access_token": access, "refresh_token": refresh,
        "access_expires_at": expires_at, "refresh_started_at": null, "pending_refresh_key": null,
        "logged_in_at": now - 3600, "verified_at": now - 3600,
        "ting": {"subscribed": true, "subscription_id": "sub_1", "registered_at": now - 3600},
        "display_name": "Cleanup", "reconsent_required": false, "rejected": null
    });
    let mut file = SessionFile::default();
    file.slots.insert(
        f.key.as_string(),
        serde_json::from_value(slot).expect("slot"),
    );
    let lock = f.store.lock().expect("lock");
    f.store.write_session(&lock, &file).expect("write");
}

pub fn secret(s: &str) -> Secret {
    Secret::new(s)
}
