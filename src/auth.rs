//! Every bearer is checked live with ACCOUNTS; authority comes from the token.
use crate::{
    accounts::{token_shape_ok, upstream},
    error::{ApiError, ApiResult},
    extract::single_header,
    plane::Plane,
};
use axum::http::{HeaderMap, StatusCode, header};
use silicon_accounts_client::{AccountKind, Introspection};
use silicon_peek_client::{
    ErrorCode, REQUIRED_SCOPES, Secret,
    identity::{AccountId, Actor, ActorId, ActorType},
    timestamp::unix_now,
};
use std::collections::BTreeSet;

pub(crate) const DELIVERY_SCOPES: [&str; 1] = ["profile"];
pub(crate) const ENROLL_SCOPES: [&str; 1] = ["profile"];
pub(crate) const REVOKE_SCOPES: [&str; 1] = ["profile"];

pub(crate) struct Bearer {
    pub(crate) token: Secret,
}
impl Bearer {
    pub(crate) fn from_headers(headers: &HeaderMap) -> ApiResult<Option<Self>> {
        let Some(value) = single_header(headers, header::AUTHORIZATION.as_str())? else {
            return Ok(None);
        };
        let token = value
            .strip_prefix("Bearer ")
            .filter(|t| token_shape_ok(t, "jwt"))
            .ok_or_else(|| {
                ApiError::unauthenticated("Authorization must contain a ACCOUNTS access token")
            })?;
        Ok(Some(Self {
            token: Secret::new(token),
        }))
    }
    pub(crate) fn required(headers: &HeaderMap) -> ApiResult<Self> {
        Self::from_headers(headers)?
            .ok_or_else(|| ApiError::unauthenticated("Sign in to Peek first"))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Principal {
    pub(crate) actor: ActorId,
    pub(crate) account: AccountId,
    pub(crate) membership_id: String,
    pub(crate) scopes: BTreeSet<String>,
    pub(crate) access_token: Secret,
}
impl Principal {
    pub(crate) fn actor_object(&self) -> Actor {
        Actor {
            actor_type: self.actor.actor_type(),
            public_id: self.actor.clone(),
        }
    }
    pub(crate) fn has_scope(&self, scope: &str) -> bool {
        self.scopes.contains(scope)
    }
    pub(crate) fn require_scopes(&self, scopes: &[&str]) -> ApiResult<()> {
        let missing: Vec<_> = scopes
            .iter()
            .copied()
            .filter(|s| !self.has_scope(s))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::ReconsentRequired,
            "Sign in again to approve Peek's permissions",
        )
        .with_details(serde_json::json!({"missing_scopes":missing})))
    }
    pub(crate) fn reconsent_required(&self) -> bool {
        REQUIRED_SCOPES.iter().any(|s| !self.has_scope(s))
    }
    pub(crate) fn scope_list(&self) -> Vec<String> {
        self.scopes.iter().cloned().collect()
    }
}

pub(crate) async fn authenticate(
    plane: &Plane,
    bearer: Bearer,
    app_id: &str,
) -> ApiResult<Principal> {
    let accounts = plane.accounts()?;
    let inspected = accounts
        .app()
        .introspect(bearer.token.expose())
        .await
        .map_err(|e| match crate::accounts::api_code(&e) {
            Some((code, 400 | 401 | 403 | 404)) if code != "invalid_client" => {
                ApiError::unauthenticated("This session expired or was signed out")
            }
            _ => upstream(&e, "verify this session", false),
        })?;
    verify(&inspected, &bearer, app_id, unix_now())
        .map_err(|check| ApiError::unauthenticated(format!("This session is invalid ({check})")))
}

pub(crate) fn verify(
    inspected: &Introspection,
    bearer: &Bearer,
    app_id: &str,
    now: i64,
) -> Result<Principal, &'static str> {
    if !inspected.active {
        return Err("inactive");
    }
    let audience = inspected.aud.as_ref().is_some_and(|v| {
        v.as_str() == Some(app_id)
            || v.as_array()
                .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(app_id)))
    });
    if !audience
        || inspected
            .client_id
            .as_deref()
            .is_some_and(|id| id != app_id)
    {
        return Err("audience");
    }
    if inspected.exp.is_none_or(|e| e <= now) {
        return Err("expired");
    }
    let account =
        AccountId::parse(inspected.sub.as_deref().ok_or("account")?).map_err(|_| "account")?;
    let actor = ActorId::parse(
        inspected
            .username
            .as_deref()
            .or(inspected.id.as_deref())
            .ok_or("actor")?,
    )
    .map_err(|_| "actor")?;
    let kind = match inspected.kind {
        Some(AccountKind::Carbon) => ActorType::Carbon,
        Some(AccountKind::Silicon) => ActorType::Silicon,
        _ => return Err("kind"),
    };
    if actor.actor_type() != kind {
        return Err("kind");
    }
    let membership_id = format!("{app_id}:{account}");
    if inspected
        .membership_id
        .as_deref()
        .is_some_and(|m| m != membership_id)
    {
        return Err("membership");
    }
    Ok(Principal {
        actor,
        account,
        membership_id,
        scopes: inspected
            .scope
            .as_deref()
            .unwrap_or("")
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect(),
        access_token: bearer.token.clone(),
    })
}
