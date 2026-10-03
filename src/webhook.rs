//! The IAM webhook receiver (BLUEPRINT §2.10).
//!
//! 1. The raw body is verified (HMAC over `{timestamp}.{raw}`) before anything
//!    is parsed.
//! 2. Test envelopes are routed only to the testing environment whose stored
//!    root key they carry; production events only to production.
//! 3. Events are deduplicated on `event_id` in the same transaction that
//!    applies them.
//! 4. `organization.membership.removed.v1` and `organization.silicon.removed.v1`
//!    delete that actor's drawing and Ting enrollment. Everything else is
//!    acknowledged and logged. Webhooks are notifications, never authority.

use axum::http::{HeaderMap, StatusCode};
use serde_json::Value;
use silicon_iam_client::{EnvironmentKey, VerifiedWebhook, WebhookError, models::WebhookEvent};
use silicon_peek_client::{
    ErrorCode,
    identity::{ActorId, Context, OrgId},
    timestamp::unix_now,
};
use uuid::Uuid;

use crate::{
    crypto::root_key_aad,
    error::{ApiError, ApiResult},
    state::AppState,
    store,
};

/// Event types that remove an actor from an org.
const REMOVALS: [&str; 2] = [
    "organization.membership.removed.v1",
    "organization.silicon.removed.v1",
];

/// What happened to a delivery.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Handled {
    /// Applied; `removed` actors were cleaned up.
    Applied { removed: usize },
    /// Already processed.
    Duplicate,
    /// A test event for an environment peek does not know (acknowledged).
    Unroutable,
}

fn verification_error(error: &WebhookError) -> ApiError {
    match error {
        WebhookError::BodyTooLarge { .. } => ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::PayloadTooLarge,
            error.to_string(),
        ),
        WebhookError::InvalidPayload | WebhookError::EventIdMismatch => {
            ApiError::invalid_input(format!("the webhook is authentic but malformed: {error}"))
        }
        _ => ApiError::unauthenticated(format!("webhook verification failed: {error}")),
    }
}

/// The `(org, actor)` pairs a removal event names.
pub(crate) fn removed_members(event: &WebhookEvent) -> Vec<(OrgId, ActorId)> {
    let members = event
        .data
        .pointer("/current/members")
        .or_else(|| event.data.get("members"))
        .and_then(Value::as_array);
    let mut out = Vec::new();
    for member in members.into_iter().flatten() {
        let resource = member.get("resource").unwrap_or(member);
        let from_membership = resource
            .get("membership_id")
            .and_then(Value::as_str)
            .and_then(|m| {
                let (actor, rest) = m.split_once('[')?;
                let org = rest.strip_suffix(']')?;
                Some((org.to_owned(), actor.to_owned()))
            });
        let from_parts = || {
            let actor = resource
                .get("principal_id")
                .or_else(|| member.pointer("/principal/public_id"))
                .and_then(Value::as_str)?;
            let org = member
                .pointer("/organization/org_id")
                .and_then(Value::as_str)?;
            Some((org.to_owned(), actor.to_owned()))
        };
        let Some((org, actor)) = from_membership.or_else(from_parts) else {
            continue;
        };
        if let (Ok(org), Ok(actor)) = (OrgId::parse(&org), ActorId::parse(&actor))
            && !out.contains(&(org.clone(), actor.clone()))
        {
            out.push((org, actor));
        }
    }
    out
}

async fn route(state: &AppState, verified: &VerifiedWebhook) -> ApiResult<Option<Context>> {
    if !verified.is_testing() {
        return Ok(Some(Context::Production));
    }
    let bindings = state
        .0
        .production_db
        .call(|conn| Ok(store::bindings::with_root_keys(conn)?))
        .await?;
    for binding in bindings {
        let Ok(root) = state.0.sealer.open_string(
            &root_key_aad(&binding.environment_id),
            &binding.sealed_root_key,
        ) else {
            continue;
        };
        let Ok(key) = EnvironmentKey::new(root) else {
            continue;
        };
        if verified.verify_testing_environment(&key).is_ok() {
            let id = Uuid::parse_str(&binding.environment_id)
                .map_err(|_| ApiError::internal("a stored environment ID is not a UUID"))?;
            return Ok(Some(Context::Testing(id)));
        }
    }
    Ok(None)
}

/// Verifies, routes, dedupes and applies one delivery.
pub(crate) async fn handle(
    state: &AppState,
    headers: &HeaderMap,
    body: &[u8],
) -> ApiResult<Handled> {
    let verified = state
        .0
        .webhooks
        .verify(headers, body)
        .map_err(|e| verification_error(&e))?;
    let event = verified.event().clone();
    let Some(ctx) = route(state, &verified).await? else {
        tracing::warn!(event_id = %event.event_id, event_type = %event.event_type, "dropped an authentic test webhook for an environment peek is not prepared in");
        return Ok(Handled::Unroutable);
    };
    let db = if ctx.is_testing() {
        state.0.testing_db.clone()
    } else {
        state.0.production_db.clone()
    };
    let removed = if REMOVALS.contains(&event.event_type.as_str()) {
        removed_members(&event)
    } else {
        Vec::new()
    };
    let (event_id, event_type, ctx_s) = (
        event.event_id.to_string(),
        event.event_type.clone(),
        ctx.as_string(),
    );
    let handled = db
        .call(move |conn| {
            let tx = conn.transaction()?;
            if !store::webhooks::insert_if_new(&tx, &event_id, &ctx_s, &event_type, unix_now())? {
                return Ok(Handled::Duplicate);
            }
            for (org, actor) in &removed {
                store::drawings::delete(&tx, &ctx_s, org.as_str(), actor.as_str())?;
                store::enrollments::delete(&tx, &ctx_s, org.as_str(), actor.as_str())?;
                for table in ["obo_roots", "obo_requests", "obo_operations", "obo_locks"] {
                    tx.execute(
                        &format!("DELETE FROM {table} WHERE ctx=?1 AND org_id=?2 AND actor_id=?3"),
                        rusqlite::params![ctx_s, org.as_str(), actor.as_str()],
                    )?;
                }
            }
            tx.commit()?;
            Ok(Handled::Applied {
                removed: removed.len(),
            })
        })
        .await?;
    tracing::info!(event_id = %event.event_id, event_type = %event.event_type, ctx = %ctx, outcome = ?handled, "IAM webhook");
    Ok(handled)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn event(data: &Value) -> Result<WebhookEvent, serde_json::Error> {
        serde_json::from_value(json!({
            "spec_version": "1.0", "event_id": Uuid::now_v7(), "event_type": "organization.membership.removed.v1",
            "occurred_at": "2026-09-26T10:00:00Z", "organization_id": null,
            "aggregate": {"type": "organization_membership", "id": Uuid::now_v7(), "version": 2},
            "data": data
        }))
    }

    #[test]
    fn members_are_read_from_tombstones_and_full_rows() -> Result<(), Box<dyn std::error::Error>> {
        let e = event(&json!({"current": {"members": [
            {"resource": {"type": "organization_membership", "id": Uuid::now_v7(), "principal_id": "si:cleanup",
                          "principal_type": "silicon", "membership_id": "si:cleanup[tos]", "version": 2, "status": "removed"},
             "authorization": "removed"},
            {"resource": {"id": Uuid::now_v7(), "version": 3, "principal_id": "c:alice"},
             "organization": {"org_id": "acme"}},
            {"resource": {"membership_id": "garbage"}}
        ]}}))?;
        let got = removed_members(&e);
        assert_eq!(
            got,
            vec![
                (OrgId::parse("tos")?, ActorId::parse("si:cleanup")?),
                (OrgId::parse("acme")?, ActorId::parse("c:alice")?),
            ]
        );
        Ok(())
    }
}
