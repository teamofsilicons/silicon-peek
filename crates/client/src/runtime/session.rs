//! `session.json` schema v1 (BLUEPRINT §1.7, gap-token-custody §5.2).
//!
//! One file per home holds one slot per `"<api_url>#<context>"`. The Silicon's
//! `oat_`/`ort_` pair lives only here (client custody, D5): 0600, never
//! printed, never in argv, env or URLs. Unknown fields written by a newer peek
//! are preserved on rewrite, because several peek versions share one machine.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    REQUIRED_SCOPES, Secret,
    api::{EnrollmentError, SessionResponse, TingEnrollment},
    error::{Error, ErrorCode, Result},
    identity::{Actor, ActorId, OrgId, SlotKey},
    ids::IdempotencyKey,
    timestamp::Timestamp,
};

/// The schema this build reads and writes.
pub const SESSION_SCHEMA: u32 = 1;

/// IAM's replay window for one-time secrets: a login or refresh must be
/// recovered with the same key within 10 minutes.
pub const REPLAY_WINDOW_SECS: i64 = 600;

/// IAM returns no refresh-family expiry; status reports `logged_in_at + 900 d`
/// as an estimate.
pub const FAMILY_LIFETIME_ESTIMATE_SECS: i64 = 900 * 24 * 3600;

/// The hint for every terminal session failure.
pub const RELOGIN_HINT: &str = "log in again: si auth setup peek   (or: iam silicon-login … --app-id peek …; peek login '<SLT>')";

/// The whole file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionFile {
    /// Always 1 for this build.
    pub schema: u32,
    /// Slots by `"<api_url>#<context>"`.
    #[serde(default)]
    pub slots: BTreeMap<String, SessionSlot>,
    /// An SLT exchange whose outcome is uncertain (≤ 10 minutes).
    #[serde(default)]
    pub pending_login: Option<PendingLogin>,
    /// Refresh tokens still to revoke remotely.
    #[serde(default)]
    pub pending_revocations: Vec<PendingRevocation>,
    /// Set by `peek logout`; tells peekd to cancel queued rows.
    #[serde(default)]
    pub logged_out: Option<LoggedOut>,
    /// Fields written by a newer peek.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for SessionFile {
    fn default() -> Self {
        Self {
            schema: SESSION_SCHEMA,
            slots: BTreeMap::new(),
            pending_login: None,
            pending_revocations: Vec::new(),
            logged_out: None,
            extra: BTreeMap::new(),
        }
    }
}

/// One session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSlot {
    /// The authenticated actor.
    pub actor: Actor,
    /// The selected org.
    pub org_id: OrgId,
    /// Compatibility projection: exactly the token-bound organization.
    #[serde(default)]
    pub org_ids: Vec<OrgId>,
    /// `si:<handle>[<org>]`.
    pub membership_id: String,
    /// Space-separated granted scopes.
    pub scope: String,
    /// `oat_…`.
    pub access_token: Secret,
    /// `ort_…`.
    pub refresh_token: Secret,
    /// Unix seconds; computed from the request start, never the replay time.
    pub access_expires_at: i64,
    /// When the pending refresh started (unix seconds).
    #[serde(default)]
    pub refresh_started_at: Option<i64>,
    /// `peek-refresh-<hex(blake3(refresh_token))>`, persisted before any I/O.
    #[serde(default)]
    pub pending_refresh_key: Option<String>,
    /// When the login committed (unix seconds).
    pub logged_in_at: i64,
    /// When peek-server last verified the session (unix seconds).
    #[serde(default)]
    pub verified_at: Option<i64>,
    /// Ting enrollment.
    #[serde(default)]
    pub ting: Option<SlotTing>,
    /// Display name, when disclosed.
    #[serde(default)]
    pub display_name: Option<String>,
    /// A required scope is missing.
    #[serde(default)]
    pub reconsent_required: bool,
    /// Terminal rejection; the slot is dead until the next login.
    #[serde(default)]
    pub rejected: Option<Rejection>,
    /// Fields written by a newer peek.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Ting enrollment of a slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotTing {
    /// Active recipient.
    pub subscribed: bool,
    /// Ting subscription ID.
    #[serde(default)]
    pub subscription_id: Option<String>,
    /// When enrolled (unix seconds).
    #[serde(default)]
    pub registered_at: Option<i64>,
    /// The last enrollment failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EnrollmentError>,
}

impl SlotTing {
    /// From a backend enrollment result.
    #[must_use]
    pub fn from_enrollment(e: &TingEnrollment, now: i64) -> Self {
        Self {
            subscribed: e.subscribed,
            subscription_id: e.subscription_id.clone(),
            registered_at: e.subscribed.then_some(now),
            error: e.error.clone(),
        }
    }
}

/// Why a slot was rejected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    /// The server's code (`session_rejected`, `idempotency_response_expired`, …).
    pub code: String,
    /// When (unix seconds).
    pub at: i64,
    /// The server request ID.
    #[serde(default)]
    pub request_id: Option<String>,
}

/// An uncertain SLT exchange.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingLogin {
    /// `peek-login-<hex(blake3(slt))>`, or a per-attempt key for a testing
    /// public ID ([`crate::ids::IdempotencyKey::login_attempt`]).
    pub key: String,
    /// The SLT (kept only until the exchange commits).
    pub slt: Secret,
    /// Unix seconds.
    pub started_at: i64,
    /// The slot key the login targets.
    #[serde(default)]
    pub slot: Option<String>,
    /// The `X-Org-ID` hint that was sent.
    #[serde(default)]
    pub org_hint: Option<String>,
}

/// A refresh token to revoke.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingRevocation {
    /// `ort_…`.
    pub token: Secret,
    /// `peek-revoke-<hex(blake3(token))>`.
    pub key: String,
    /// Unix seconds.
    pub since: i64,
    /// The slot key it belonged to (which backend to call).
    #[serde(default)]
    pub slot: Option<String>,
}

impl PendingRevocation {
    /// A revocation for `token` from `slot`.
    #[must_use]
    pub fn new(token: Secret, slot: &str, now: i64) -> Self {
        let key = IdempotencyKey::revoke(token.expose()).as_str().to_owned();
        Self {
            token,
            key,
            since: now,
            slot: Some(slot.to_owned()),
        }
    }
}

/// The logout tombstone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedOut {
    /// Exact login context whose work may be cancelled.
    #[serde(default)]
    pub context_id: Option<String>,
    /// Organization of that login.
    #[serde(default)]
    pub org_id: Option<OrgId>,
    /// The actor that logged out.
    pub actor: String,
    /// Unix seconds.
    pub at: i64,
    /// The slot key.
    #[serde(default)]
    pub slot: Option<String>,
}

impl SessionSlot {
    /// Builds a slot from a login response. `started_at` is when the exchange
    /// began: expiry is measured from there, never from a replay.
    #[must_use]
    pub fn from_login(resp: &SessionResponse, started_at: i64, now: i64) -> Self {
        let mut slot = Self {
            actor: resp.actor.clone(),
            org_id: resp.org_id.clone(),
            org_ids: if resp.org_ids.is_empty() {
                vec![resp.org_id.clone()]
            } else {
                resp.org_ids.clone()
            },
            membership_id: resp.membership_id.clone(),
            scope: resp.scope.clone(),
            access_token: resp.access_token.clone(),
            refresh_token: resp.refresh_token.clone(),
            access_expires_at: started_at
                .saturating_add(i64::try_from(resp.expires_in).unwrap_or(i64::MAX)),
            refresh_started_at: None,
            pending_refresh_key: None,
            logged_in_at: started_at,
            verified_at: Some(now),
            ting: resp
                .ting
                .as_ref()
                .map(|t| SlotTing::from_enrollment(t, now)),
            display_name: resp.display_name.clone(),
            reconsent_required: resp.reconsent_required,
            rejected: None,
            extra: BTreeMap::from([(
                "context_id".into(),
                Value::String(uuid::Uuid::new_v4().to_string()),
            )]),
        };
        slot.reconsent_required |= !slot.has_required_scopes();
        slot
    }

    /// Applies a refresh response: both tokens, scope and expiry
    /// (`refresh_started_at + expires_in`), and clears the pending state.
    pub fn apply_refresh(&mut self, resp: &SessionResponse, started_at: i64) {
        self.access_token = resp.access_token.clone();
        self.refresh_token = resp.refresh_token.clone();
        self.scope.clone_from(&resp.scope);
        self.access_expires_at =
            started_at.saturating_add(i64::try_from(resp.expires_in).unwrap_or(i64::MAX));
        self.refresh_started_at = None;
        self.pending_refresh_key = None;
        self.reconsent_required = resp.reconsent_required || !self.has_required_scopes();
        if resp.display_name.is_some() {
            self.display_name.clone_from(&resp.display_name);
        }
    }

    /// Stable identity of this login family. Older sessions require a fresh IAM 5 login.
    ///
    /// # Errors
    /// `session_rejected` for legacy or invalid saved context metadata.
    pub fn context_id(&self) -> Result<&str> {
        self.extra
            .get("context_id")
            .and_then(Value::as_str)
            .filter(|id| uuid::Uuid::parse_str(id).is_ok_and(|id| !id.is_nil()))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::SessionRejected,
                    "this saved session predates IAM 5; sign in again",
                )
                .with_hint(RELOGIN_HINT)
            })
    }

    /// Rejects legacy multi-organization or delegated credentials before use.
    ///
    /// # Errors
    /// `session_rejected` when the saved authority is not one ordinary context.
    pub fn validate_context(&self) -> Result<()> {
        self.context_id()?;
        if self.org_ids != [self.org_id.clone()]
            || self.actor.actor_type != self.actor.public_id.actor_type()
            || self.membership_id != format!("{}[{}]", self.actor.public_id, self.org_id)
            || self
                .scope
                .split_ascii_whitespace()
                .any(|scope| scope.starts_with("obo:"))
        {
            return Err(Error::new(
                ErrorCode::SessionRejected,
                "this saved session is not an IAM 5 account and organization context",
            )
            .with_hint(RELOGIN_HINT));
        }
        Ok(())
    }

    /// The actor ID.
    #[must_use]
    pub fn actor_id(&self) -> &ActorId {
        &self.actor.public_id
    }

    /// Granted scopes, sorted.
    #[must_use]
    pub fn scopes(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .scope
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Whether every required scope is granted.
    #[must_use]
    pub fn has_required_scopes(&self) -> bool {
        let scopes = self.scopes();
        REQUIRED_SCOPES
            .iter()
            .all(|s| scopes.iter().any(|g| g == s))
    }

    /// `fresh_session` step 1: usable for at least `margin_secs` more, with no
    /// refresh pending and no rejection.
    #[must_use]
    pub fn is_fresh(&self, now: i64, margin_secs: i64) -> bool {
        self.access_expires_at > now.saturating_add(margin_secs)
            && self.pending_refresh_key.is_none()
            && self.rejected.is_none()
    }

    /// `logged_in_at + 900 days`, labelled an estimate.
    #[must_use]
    pub fn family_expires_at_estimate(&self) -> Timestamp {
        Timestamp::from_unix(
            self.logged_in_at
                .saturating_add(FAMILY_LIFETIME_ESTIMATE_SECS),
        )
    }
}

impl SessionFile {
    /// The slot for `key`, if any.
    #[must_use]
    pub fn slot(&self, key: &SlotKey) -> Option<&SessionSlot> {
        self.slots.get(&key.as_string())
    }

    /// A usable slot: present, not logged out, not rejected.
    ///
    /// # Errors
    /// `not_logged_in` or `session_rejected`, with the fix.
    pub fn usable_slot(&self, key: &SlotKey, store: &std::path::Path) -> Result<&SessionSlot> {
        let k = key.as_string();
        let Some(slot) = self.slots.get(&k) else {
            let logged_out = self
                .logged_out
                .as_ref()
                .is_some_and(|l| l.slot.as_deref().is_none_or(|s| s == k));
            let msg = if logged_out {
                format!(
                    "this home logged out of peek ({k}); there is no session in {}",
                    store.display()
                )
            } else {
                format!("there is no peek session for {k} in {}", store.display())
            };
            return Err(Error::new(ErrorCode::NotLoggedIn, msg).with_hint(RELOGIN_HINT));
        };
        slot.validate_context()?;
        if let Some(r) = &slot.rejected {
            return Err(Error::new(
                ErrorCode::SessionRejected,
                format!(
                    "IAM rejected this home's peek session for {} ({}, at {}); it cannot be refreshed",
                    slot.actor.public_id,
                    r.code,
                    Timestamp::from_unix(r.at)
                ),
            )
            .with_hint(RELOGIN_HINT)
            .with_request_id(r.request_id.clone())
            .with_details(serde_json::json!({"rejection": r})));
        }
        Ok(slot)
    }

    /// Records a committed login (§2.4 step 5): the replaced slot's refresh
    /// token moves to `pending_revocations`, the new slot is written, and
    /// `pending_login` and `logged_out` are cleared. Returns the revocation
    /// queued for the old family, if any.
    pub fn record_login(
        &mut self,
        key: &SlotKey,
        slot: SessionSlot,
        now: i64,
    ) -> Option<PendingRevocation> {
        let k = key.as_string();
        let replaced = self.slots.insert(k.clone(), slot).and_then(|old| {
            let same = self
                .slots
                .get(&k)
                .is_some_and(|new| new.refresh_token == old.refresh_token);
            (!same).then(|| PendingRevocation::new(old.refresh_token, &k, now))
        });
        if let Some(r) = &replaced {
            self.push_revocation(r.clone());
        }
        self.pending_login = None;
        self.logged_out = None;
        replaced
    }

    /// Queues a revocation (deduplicated by key).
    pub fn push_revocation(&mut self, r: PendingRevocation) {
        if !self.pending_revocations.iter().any(|p| p.key == r.key) {
            self.pending_revocations.push(r);
        }
    }

    /// Drops a revocation once the backend confirmed it.
    pub fn complete_revocation(&mut self, key: &str) {
        self.pending_revocations.retain(|p| p.key != key);
    }

    /// §2.6 logout step 1: writes the tombstone and queues the slot's refresh
    /// token for revocation. The slot stays until [`SessionFile::finish_logout`]
    /// so its access token can authenticate the backend call.
    pub fn begin_logout(
        &mut self,
        key: &SlotKey,
        now: i64,
    ) -> Option<(SessionSlot, PendingRevocation)> {
        let k = key.as_string();
        let slot = self.slots.get(&k)?.clone();
        self.logged_out = Some(LoggedOut {
            context_id: slot.context_id().ok().map(str::to_owned),
            org_id: Some(slot.org_id.clone()),
            actor: slot.actor.public_id.to_string(),
            at: now,
            slot: Some(k.clone()),
        });
        let r = PendingRevocation::new(slot.refresh_token.clone(), &k, now);
        self.push_revocation(r.clone());
        Some((slot, r))
    }

    /// §2.6 logout step 3: deletes the slot (if it is still the family that
    /// was logged out).
    pub fn finish_logout(&mut self, key: &SlotKey, refresh_token: &Secret) {
        let k = key.as_string();
        if self
            .slots
            .get(&k)
            .is_some_and(|s| &s.refresh_token == refresh_token)
        {
            self.slots.remove(&k);
        }
    }

    /// Whether `pending_login` is still inside IAM's replay window.
    #[must_use]
    pub fn pending_login_live(&self, now: i64) -> bool {
        self.pending_login
            .as_ref()
            .is_some_and(|p| now.saturating_sub(p.started_at) <= REPLAY_WINDOW_SECS)
    }
}

/// Validates the ordinary login/refresh response against its selected world.
///
/// # Errors
/// `unexpected_response` when identity, scope, token, or world bindings are invalid.
pub fn validate_response(resp: &SessionResponse, context: crate::identity::Context) -> Result<()> {
    resp.validate()?;
    if resp.testing_environment.as_ref().map(|world| world.id) != context.testing_id() {
        return Err(Error::new(
            ErrorCode::UnexpectedResponse,
            "the backend returned a different IAM world",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        identity::{ApiUrl, Context},
        json,
    };

    fn response(access: &str, refresh: &str, scope: &str) -> Result<SessionResponse> {
        json::from_value(
            serde_json::json!({"access_token":access,"refresh_token":refresh,"token_type":"Bearer","expires_in":1800,
                "scope":scope,"actor":{"type":"silicon","public_id":"si:cleanup"},"org_id":"tos","org_ids":["tos"],
                "membership_id":"si:cleanup[tos]","reconsent_required":false,"display_name":"Cleanup",
                "ting":{"subscribed":true,"subscription_id":"sub_1"}}),
            "session",
        )
    }

    const FULL: &str = "obo:ting:subscriptions.register obo:ting:subscriptions.revoke obo:ting:tings.send self.identity.read self.membership.read self.profile.read";

    #[test]
    fn blueprint_example_parses_and_round_trips() -> Result<()> {
        let text = r#"{"schema":1,"slots":{"https://backend.peek.teamofsilicons.com#production":{
            "actor":{"type":"silicon","public_id":"si:cleanup"},"org_id":"tos","org_ids":["tos"],"membership_id":"si:cleanup[tos]",
            "scope":"obo:ting:subscriptions.register obo:ting:subscriptions.revoke obo:ting:tings.send self.identity.read self.membership.read self.profile.read",
            "access_token":"oat_a","refresh_token":"ort_b","access_expires_at":1790001800,"refresh_started_at":null,"pending_refresh_key":null,
            "logged_in_at":1790000000,"verified_at":1790000000,"ting":{"subscribed":true,"subscription_id":"sub_1","registered_at":1790000001},
            "display_name":"DJ","reconsent_required":false,"rejected":null,"future_slot_field":[1]}},
            "pending_login":null,"pending_revocations":[],"logged_out":null,"future_top":true}"#;
        let f: SessionFile = json::from_slice(text.as_bytes(), "session.json")?;
        let key = SlotKey::new(ApiUrl::production(), Context::Production);
        let slot = f.slot(&key).ok_or_else(|| Error::internal("slot"))?;
        assert!(slot.has_required_scopes());
        assert_eq!(slot.scopes().len(), 6);
        assert_eq!(
            slot.family_expires_at_estimate().unix(),
            1_790_000_000 + 900 * 86_400
        );
        let back = serde_json::to_value(&f).map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(back["future_top"], true);
        assert_eq!(
            back["slots"]["https://backend.peek.teamofsilicons.com#production"]["future_slot_field"]
                [0],
            1
        );
        assert_eq!(
            back["slots"]["https://backend.peek.teamofsilicons.com#production"]["refresh_token"],
            "ort_b"
        );
        Ok(())
    }

    #[test]
    fn login_logout_bookkeeping() -> Result<()> {
        let key = SlotKey::new(ApiUrl::production(), Context::Production);
        let mut f = SessionFile::default();
        let s1 = SessionSlot::from_login(&response("oat_1", "ort_1", FULL)?, 100, 101);
        assert_eq!(s1.access_expires_at, 1900);
        assert_eq!(s1.ting.as_ref().and_then(|t| t.registered_at), Some(101));
        assert!(f.record_login(&key, s1, 101).is_none());
        let s2 = SessionSlot::from_login(&response("oat_2", "ort_2", FULL)?, 200, 201);
        let replaced = f
            .record_login(&key, s2, 201)
            .ok_or_else(|| Error::internal("replaced"))?;
        assert_eq!(replaced.token.expose(), "ort_1");
        assert_eq!(f.pending_revocations.len(), 1);
        assert_eq!(replaced.key, IdempotencyKey::revoke("ort_1").as_str());

        let (slot, r) = f
            .begin_logout(&key, 300)
            .ok_or_else(|| Error::internal("logout"))?;
        assert_eq!(r.token.expose(), "ort_2");
        assert!(f.logged_out.is_some());
        assert!(f.slot(&key).is_some(), "slot kept until the backend call");
        f.finish_logout(&key, &slot.refresh_token);
        assert!(f.slot(&key).is_none());
        let e = f.usable_slot(&key, std::path::Path::new("/x")).err();
        assert!(e.is_some_and(
            |e| *e.code() == ErrorCode::NotLoggedIn && e.message().contains("logged out")
        ));
        f.complete_revocation(&r.key);
        assert_eq!(f.pending_revocations.len(), 1);
        Ok(())
    }

    #[test]
    fn reconsent_and_freshness() -> Result<()> {
        let s = SessionSlot::from_login(&response("oat", "ort", "self.profile.read")?, 0, 0);
        assert!(s.reconsent_required);
        let mut s = SessionSlot::from_login(&response("oat", "ort", FULL)?, 1000, 1000);
        assert!(s.is_fresh(1000, 60));
        assert!(!s.is_fresh(2741, 60));
        s.pending_refresh_key = Some("k".into());
        assert!(!s.is_fresh(1000, 60));
        s.pending_refresh_key = None;
        s.rejected = Some(Rejection {
            code: "session_rejected".into(),
            at: 1,
            request_id: None,
        });
        assert!(!s.is_fresh(1000, 60));
        let mut f = SessionFile::default();
        let key = SlotKey::new(ApiUrl::production(), Context::Production);
        f.slots.insert(key.as_string(), s);
        let e = f.usable_slot(&key, std::path::Path::new("/x")).err();
        assert!(
            e.is_some_and(|e| *e.code() == ErrorCode::SessionRejected && e.exit_code().code() == 3)
        );
        Ok(())
    }

    #[test]
    fn debug_never_prints_tokens() -> Result<()> {
        let s = SessionSlot::from_login(&response("oat_secret", "ort_secret", FULL)?, 0, 0);
        let d = format!("{s:?}");
        assert!(!d.contains("oat_secret") && !d.contains("ort_secret"));
        Ok(())
    }
}
