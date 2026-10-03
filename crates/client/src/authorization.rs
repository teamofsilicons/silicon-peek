//! Public, secret-free feature approval contracts. Ordinary login never grants Ting access.
use crate::{
    Error, ErrorCode, Result,
    identity::{Actor, OrgId},
    timestamp::Timestamp,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// A provider-owned authorization request. Delegated tokens never leave peek-server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TingAuthorization {
    /// Peek's durable request identifier.
    pub request_id: Uuid,
    /// IAM's approval snapshot.
    pub authorization: ConsentDetail,
    /// Whether peek-server durably exchanged the approved code.
    pub completed: bool,
    /// Public endpoint bindings; never tokens.
    #[serde(default)]
    pub roots: Vec<Value>,
}

/// The IAM fields needed to display and correlate consent safely.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsentDetail {
    /// IAM request identifier.
    pub id: Uuid,
    /// Requesting application.
    pub app_id: String,
    /// The actor who owns this Peek request.
    pub actor: Actor,
    /// The Peek request's organization.
    pub org_id: OrgId,
    /// pending, approved, declined, exchanged or expired.
    pub status: String,
    /// Version of the terms presented for review.
    pub version: i64,
    /// The approval deadline.
    pub expires_at: Timestamp,
    /// Safe IAM review link, when available.
    pub authorization_url: Option<String>,
    /// Callback correlation, absent for manual code entry.
    pub state: Option<String>,
    /// Provider tree displayed by IAM.
    #[serde(default)]
    pub endpoints: Vec<Value>,
}

impl TingAuthorization {
    /// Validates actor/org ownership, identifiers, state and the review URL.
    ///
    /// # Errors
    /// `unexpected_response` if the response cannot be safely presented or accepted.
    pub fn validate(&self, actor: &Actor, org: &OrgId, previous: Option<&Self>) -> Result<()> {
        let detail = &self.authorization;
        let unsafe_url = detail.authorization_url.as_ref().is_some_and(|value| {
            url::Url::parse(value).map_or(true, |url| {
                url.scheme() != "https"
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
            })
        });
        if self.request_id.is_nil()
            || detail.id.is_nil()
            || detail.app_id != "peek"
            || &detail.actor != actor
            || &detail.org_id != org
            || detail.version < 1
            || unsafe_url
            || !matches!(
                detail.status.as_str(),
                "pending" | "approved" | "declined" | "exchanged" | "expired"
            )
            || (self.completed && !matches!(detail.status.as_str(), "approved" | "exchanged"))
            || previous.is_some_and(|old| {
                old.request_id != self.request_id
                    || old.authorization.id != detail.id
                    || old.authorization.state != detail.state
            })
        {
            return Err(Error::new(
                ErrorCode::UnexpectedResponse,
                "the permission response did not match this account and approval request",
            ));
        }
        Ok(())
    }
}
