//! Bearer authentication (BLUEPRINT §2.7).
//!
//! Every app route takes `Authorization: Bearer oat_…` and `X-Org-ID`. The
//! token is introspected live on every request (no cache) and must be active,
//! issued to `peek`, bound to that org, belong to a Silicon or Carbon with a
//! disclosed public ID, and match the request's data plane. Unknown, expired
//! or revoked tokens and a wrong org are `401 unauthenticated`; a missing
//! per-route scope is `403 reconsent_required`.

use std::collections::BTreeSet;

use axum::http::{HeaderMap, StatusCode, header};
use serde_json::json;
use silicon_iam_client::models::{
    ApplicationAuthorizationActorType, TokenIntrospection, TokenIntrospectionActorType,
};
use silicon_peek_client::{
    ErrorCode, REQUIRED_SCOPES, Secret,
    api::headers,
    identity::{Actor, ActorId, ActorType, OrgId},
    timestamp::unix_now,
};
use uuid::Uuid;

use crate::{
    error::{ApiError, ApiResult},
    extract::single_header,
    iam::{token_shape_ok, upstream},
    plane::Plane,
};

/// The scopes a delivery (`tings.send` proof) needs.
pub(crate) const DELIVERY_SCOPES: [&str; 2] = ["obo:ting:tings.send", "self.identity.read"];
/// The scopes a Ting enrollment needs.
pub(crate) const ENROLL_SCOPES: [&str; 2] =
    ["obo:ting:subscriptions.register", "self.identity.read"];
/// The scopes a Ting grant revocation needs.
pub(crate) const REVOKE_SCOPES: [&str; 2] = ["obo:ting:subscriptions.revoke", "self.identity.read"];

/// Bearer credentials presented with a request.
pub(crate) struct Bearer {
    pub(crate) token: Secret,
    pub(crate) org: OrgId,
}

impl Bearer {
    /// Reads `Authorization` and `X-Org-ID`; `Ok(None)` when no
    /// `Authorization` header was sent.
    pub(crate) fn from_headers(headers: &HeaderMap) -> ApiResult<Option<Self>> {
        let Some(value) = single_header(headers, header::AUTHORIZATION.as_str())? else {
            return Ok(None);
        };
        let token = value
            .strip_prefix("Bearer ")
            .filter(|t| token_shape_ok(t, "oat_"))
            .ok_or_else(|| {
                ApiError::unauthenticated(
                    "the Authorization header must be `Bearer oat_…` (a peek access token)",
                )
            })?;
        let org = single_header(headers, headers::ORG_ID)?.ok_or_else(|| {
            ApiError::invalid_input(
                "this route needs an X-Org-ID header naming the org of the session",
            )
            .with_hint("send X-Org-ID: <org handle>, for example X-Org-ID: tos")
        })?;
        let org = OrgId::parse(org).map_err(ApiError::from_client)?;
        Ok(Some(Self {
            token: Secret::new(token),
            org,
        }))
    }

    /// As [`Bearer::from_headers`], but the credentials are required.
    pub(crate) fn required(headers: &HeaderMap) -> ApiResult<Self> {
        Self::from_headers(headers)?.ok_or_else(|| {
            ApiError::unauthenticated("this route needs Authorization: Bearer oat_… and X-Org-ID")
                .with_hint("log in first: peek login '<SLT>'")
        })
    }
}

/// A verified caller.
#[derive(Clone, Debug)]
pub(crate) struct Principal {
    pub(crate) actor: ActorId,
    pub(crate) org: OrgId,
    pub(crate) membership_id: String,
    pub(crate) scopes: BTreeSet<String>,
    pub(crate) org_role: Option<String>,
    pub(crate) access_token: Secret,
}

impl Principal {
    /// The actor object of API responses.
    pub(crate) fn actor_object(&self) -> Actor {
        Actor {
            actor_type: self.actor.actor_type(),
            public_id: self.actor.clone(),
        }
    }

    /// Whether the token holds `scope`.
    pub(crate) fn has_scope(&self, scope: &str) -> bool {
        self.scopes.contains(scope)
    }

    /// `403 reconsent_required` unless every scope is held.
    pub(crate) fn require_scopes(&self, scopes: &[&str]) -> ApiResult<()> {
        let missing: Vec<&str> = scopes
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
            format!(
                "this session was granted without {}, which this action needs",
                missing.join(", ")
            ),
        )
        .with_hint("log in again so IAM asks for consent to peek's current scopes (a refresh never adds scopes)")
        .with_details(json!({"missing_scopes": missing})))
    }

    /// Whether a required scope is missing.
    pub(crate) fn reconsent_required(&self) -> bool {
        REQUIRED_SCOPES.iter().any(|s| !self.has_scope(s))
    }

    /// Whether the disclosed org role is owner or admin.
    pub(crate) fn is_org_admin(&self) -> bool {
        matches!(self.org_role.as_deref(), Some("owner" | "admin"))
    }

    /// Sorted scopes.
    pub(crate) fn scope_list(&self) -> Vec<String> {
        self.scopes.iter().cloned().collect()
    }
}

/// Verifies `bearer` live against IAM.
pub(crate) async fn authenticate(
    plane: &Plane,
    bearer: Bearer,
    app_id: &str,
) -> ApiResult<Principal> {
    let iam = plane.iam()?;
    let inspected = iam
        .introspect(bearer.token.expose(), Some(bearer.org.as_str()))
        .await
        .map_err(|e| match crate::iam::api_code(&e) {
            Some((code, 400 | 401 | 403 | 404)) if code != "invalid_client" => {
                ApiError::unauthenticated(format!(
                    "IAM refused to introspect the access token ({code})"
                ))
            }
            _ => upstream(&e, "verify the access token", plane.is_testing()),
        })?;
    verify(&inspected, &bearer, app_id, iam.environment_id(), unix_now()).map_err(|check| {
        if check == "active" {
            ApiError::unauthenticated(format!(
                "IAM reports this access token inactive for org `{}`: it expired, was revoked, belongs to another org or another data plane",
                bearer.org
            ))
        } else {
            ApiError::unauthenticated(format!(
                "IAM's authorization for this access token does not match this request (check `{check}` failed for app `{app_id}`, org `{}`)",
                bearer.org
            ))
            .with_details(json!({"failed_check": check}))
        }
    })
}

/// The §2.7 checks, as a pure function. Returns the name of the first check
/// that failed.
pub(crate) fn verify(
    inspected: &TokenIntrospection,
    bearer: &Bearer,
    app_id: &str,
    environment: Option<Uuid>,
    now: i64,
) -> Result<Principal, &'static str> {
    let org = bearer.org.as_str();
    if !inspected.active {
        return Err("active");
    }
    if inspected.client_id.as_deref() != Some(app_id) {
        return Err("client_id");
    }
    if inspected.audience.as_deref().is_some_and(|a| a != app_id) {
        return Err("audience");
    }
    if inspected.org_id.as_deref() != Some(org) {
        return Err("org_id");
    }
    if inspected.expires_at.is_some_and(|e| e <= now) {
        return Err("expires_at");
    }
    let actor_type = match inspected.actor_type {
        Some(TokenIntrospectionActorType::Silicon) => ActorType::Silicon,
        Some(TokenIntrospectionActorType::Carbon) => ActorType::Carbon,
        _ => return Err("actor_type"),
    };
    let snapshot = inspected.authorization.as_ref().ok_or("authorization")?;
    if snapshot.org_id != org
        || snapshot.audience != app_id
        || snapshot.testing_environment_id != environment
    {
        return Err("authorization");
    }
    let snapshot_type = match snapshot.actor_type {
        Some(ApplicationAuthorizationActorType::Silicon) => Some(ActorType::Silicon),
        Some(ApplicationAuthorizationActorType::Carbon) => Some(ActorType::Carbon),
        Some(ApplicationAuthorizationActorType::Other(_)) => return Err("actor_type"),
        None => None,
    };
    if snapshot_type.is_some_and(|t| t != actor_type) {
        return Err("actor_type");
    }
    let public_id = match (
        inspected.public_id.as_deref(),
        snapshot.public_id.as_deref(),
    ) {
        (Some(a), Some(b)) if a != b => return Err("public_id"),
        (Some(id), _) | (None, Some(id)) => id,
        (None, None) => return Err("public_id"),
    };
    let actor = ActorId::parse(public_id).map_err(|_| "public_id")?;
    if actor.actor_type() != actor_type {
        return Err("public_id");
    }
    if inspected
        .membership_id
        .as_ref()
        .is_some_and(|m| *m != snapshot.membership_id)
    {
        return Err("membership_id");
    }
    let canonical = format!("{actor}[{org}]");
    let legacy_uuid = Uuid::parse_str(&snapshot.membership_id).is_ok_and(|u| !u.is_nil());
    if snapshot.membership_id != canonical && !legacy_uuid {
        return Err("membership_id");
    }
    let snapshot_scopes: BTreeSet<String> = snapshot.scopes.iter().cloned().collect();
    let scopes: BTreeSet<String> = match inspected.scope.as_deref() {
        Some(scope) => scope.split_ascii_whitespace().map(str::to_owned).collect(),
        None => snapshot_scopes.clone(),
    };
    if !snapshot.scopes.is_empty() && scopes != snapshot_scopes {
        return Err("scope");
    }
    Ok(Principal {
        actor,
        org: bearer.org.clone(),
        membership_id: snapshot.membership_id.clone(),
        scopes,
        org_role: snapshot.org_role.clone(),
        access_token: bearer.token.clone(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn inspected() -> Value {
        json!({
            "active": true, "public_id": "si:cleanup", "actor_type": "silicon", "client_id": "peek",
            "org_id": "tos", "membership_id": "si:cleanup[tos]", "scope": "self.identity.read obo:ting:tings.send",
            "audience": "peek", "expires_at": 2_000_000_000_i64, "authorization_epoch": 1,
            "authorization": {"actor_type": "silicon", "public_id": "si:cleanup", "organization_id": Uuid::now_v7(),
                "org_id": "tos", "membership_id": "si:cleanup[tos]", "membership_version": 1,
                "authorization_epoch": 1, "audience": "peek", "testing_environment_id": null,
                "scopes": ["obo:ting:tings.send", "self.identity.read"], "org_role": "admin", "tags": null}
        })
    }

    fn bearer() -> Result<Bearer, silicon_peek_client::Error> {
        Ok(Bearer {
            token: Secret::new("oat_x"),
            org: OrgId::parse("tos")?,
        })
    }

    fn check(v: Value, env: Option<Uuid>) -> Result<Principal, &'static str> {
        let inspected: TokenIntrospection = serde_json::from_value(v).map_err(|_| "fixture")?;
        let bearer = bearer().map_err(|_| "fixture")?;
        verify(&inspected, &bearer, "peek", env, 1_900_000_000)
    }

    #[test]
    fn a_valid_token_yields_the_principal() {
        let p = check(inspected(), None);
        let Ok(p) = p else {
            panic!("expected success, got {p:?}");
        };
        assert_eq!(p.actor.as_str(), "si:cleanup");
        assert_eq!(p.membership_id, "si:cleanup[tos]");
        assert!(p.is_org_admin());
        assert!(p.require_scopes(&DELIVERY_SCOPES).is_ok());
        assert!(p.require_scopes(&ENROLL_SCOPES).is_err());
        assert!(p.reconsent_required());
    }

    #[test]
    fn every_check_rejects_its_mismatch() {
        type Mutate = fn(&mut Value);
        let cases: Vec<(&str, Mutate)> = vec![
            ("active", |v| v["active"] = json!(false)),
            ("client_id", |v| v["client_id"] = json!("dm")),
            ("audience", |v| v["audience"] = json!("dm")),
            ("org_id", |v| v["org_id"] = json!("other")),
            ("expires_at", |v| v["expires_at"] = json!(1)),
            ("actor_type", |v| v["actor_type"] = json!("application")),
            ("authorization", |v| {
                if let Some(o) = v.as_object_mut() {
                    o.remove("authorization");
                }
            }),
            ("authorization", |v| {
                v["authorization"]["org_id"] = json!("other");
            }),
            ("authorization", |v| {
                v["authorization"]["testing_environment_id"] = json!(Uuid::now_v7());
            }),
            ("public_id", |v| v["public_id"] = json!("si:other")),
            ("public_id", |v| {
                if let Some(o) = v.as_object_mut() {
                    o.remove("public_id");
                }
                v["authorization"]["public_id"] = Value::Null;
            }),
            ("public_id", |v| {
                v["public_id"] = json!("c:cleanup");
                v["authorization"]["public_id"] = json!("c:cleanup");
            }),
            ("membership_id", |v| v["membership_id"] = json!("si:x[tos]")),
            ("membership_id", |v| {
                v["membership_id"] = json!("si:cleanup[other]");
                v["authorization"]["membership_id"] = json!("si:cleanup[other]");
            }),
            ("scope", |v| v["scope"] = json!("self.identity.read")),
        ];
        for (expected, mutate) in cases {
            let mut v = inspected();
            mutate(&mut v);
            assert_eq!(check(v, None).err(), Some(expected), "{expected}");
        }
    }

    #[test]
    fn the_plane_must_match() {
        let env = Uuid::now_v7();
        let mut v = inspected();
        v["authorization"]["testing_environment_id"] = json!(env);
        assert!(check(v.clone(), Some(env)).is_ok());
        assert_eq!(check(v, None).err(), Some("authorization"));
        assert_eq!(check(inspected(), Some(env)).err(), Some("authorization"));
    }
}
