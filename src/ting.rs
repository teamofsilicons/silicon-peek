//! Ting: OBO proofs, recipient enrollment, grant revocation and deliveries
//! (BLUEPRINT §3.4–§3.6).
//!
//! Every Ting call gets a fresh single-use proof minted with peek's app
//! secret, subject = the Silicon's own access token, bound to the exact body
//! bytes. The body is never re-serialized between hashing and sending. In a
//! testing plane the Ting test headers come **only** from that proof's
//! `testing_context`, validated against the request's environment; inbound
//! headers are never forwarded.

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{Value, json};
use silicon_iam_client::{IdempotencyKey, Mutation, api::obo::body_sha256, models};
use silicon_peek_client::{ErrorCode, Secret};
use time::OffsetDateTime;

use crate::{
    auth::{ENROLL_SCOPES, Principal, REVOKE_SCOPES},
    error::{ApiError, ApiResult},
    iam::{IamPlane, api_code, upstream},
    plane::Plane,
    state::AppState,
};

const AUDIENCE: &str = "ting";
const CATALOG_TTL: Duration = Duration::from_secs(300);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const PROOF_ERRORS: [&str; 3] = ["invalid_proof", "proof_expired", "proof_consumed"];

/// A Ting OBO endpoint peek calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Endpoint {
    /// `subscriptions.register` → `POST /v1/subscriptions`.
    Register,
    /// `tings.send` → `POST /v1/tings`.
    Send,
    /// `subscriptions.revoke` → `POST /v1/subscriptions/revoke`.
    Revoke,
}

impl Endpoint {
    fn id(self) -> &'static str {
        match self {
            Self::Register => "subscriptions.register",
            Self::Send => "tings.send",
            Self::Revoke => "subscriptions.revoke",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Register => "/v1/subscriptions",
            Self::Send => "/v1/tings",
            Self::Revoke => "/v1/subscriptions/revoke",
        }
    }
}

/// A single-use proof plus the Ting test headers it carries.
struct Proof {
    access_proof: Secret,
    testing: Option<(Secret, Secret)>,
}

fn ting_unavailable(message: impl Into<String>, retry_after: u64) -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::TingUnavailable,
        message,
    )
    .with_hint("retry with backoff; the same request and key are safe to repeat")
    .with_retry_after(retry_after)
    .with_details(json!({"retry_after": retry_after}))
}

fn ting_rejected(message: impl Into<String>, ting_code: Option<&str>) -> ApiError {
    ApiError::new(StatusCode::BAD_GATEWAY, ErrorCode::TingRejected, message)
        .with_hint("this is a peek bug or a Ting configuration problem; report it with `peek report` and quote the request ID")
        .with_details(json!({"ting_code": ting_code}))
}

async fn catalog(
    state: &AppState,
    plane: &Plane,
    iam: &dyn IamPlane,
) -> ApiResult<models::OboEndpointCatalog> {
    let ctx = plane.ctx_string();
    if let Ok(cache) = state.0.catalogs.lock()
        && let Some((at, catalog)) = cache.get(&ctx)
        && at.elapsed() < CATALOG_TTL
    {
        return Ok(catalog.clone());
    }
    let catalog = iam.obo_endpoints(AUDIENCE).await.map_err(|e| match api_code(&e) {
        Some((code, 403 | 404)) => ting_rejected(
            format!("IAM will not show peek Ting's OBO endpoints ({code}); peek's app_scope must declare Ting's endpoints"),
            None,
        ),
        _ => upstream(&e, "read Ting's OBO endpoint catalog", plane.is_testing()),
    })?;
    if let Ok(mut cache) = state.0.catalogs.lock() {
        cache.insert(ctx, (Instant::now(), catalog.clone()));
    }
    Ok(catalog)
}

fn check_endpoint(catalog: &models::OboEndpointCatalog, endpoint: Endpoint) -> ApiResult<()> {
    let found = catalog
        .endpoints
        .iter()
        .find(|e| e.endpoint_id == endpoint.id())
        .ok_or_else(|| {
            ting_rejected(
                format!("Ting's OBO catalog has no `{}` endpoint", endpoint.id()),
                None,
            )
        })?;
    let metadata_empty = found
        .metadata
        .as_object()
        .is_some_and(serde_json::Map::is_empty);
    if found.path != endpoint.path()
        || !metadata_empty
        || !found.ttl_seconds.is_some_and(|t| (1..=60).contains(&t))
    {
        return Err(ting_unavailable(
            format!(
                "Ting's `{}` OBO endpoint changed shape (path, metadata or proof lifetime); peek-server refuses to call it",
                endpoint.id()
            ),
            300,
        ));
    }
    Ok(())
}

fn exchange_error(e: &silicon_iam_client::Error, testing: bool) -> ApiError {
    match api_code(e) {
        Some((code, 401 | 410)) if code != "invalid_client" => ApiError::unauthenticated(format!(
            "IAM no longer accepts the Silicon's access token for delegation to Ting ({code})"
        )),
        Some((code, 403)) => ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::ReconsentRequired,
            format!("IAM refused to delegate this Silicon's authority to Ting ({code})"),
        )
        .with_hint("log in again and approve peek's Ting scopes; if that does not help, Ting has not approved peek's critical scopes yet")
        .with_details(json!({"iam_code": code})),
        Some((code, 404 | 422)) => ting_rejected(
            format!("IAM rejected peek's Ting proof request ({code})"),
            None,
        ),
        _ => upstream(e, "mint a Ting proof", testing),
    }
}

async fn validate_testing(
    iam: &dyn IamPlane,
    plane: &Plane,
    context: Option<&models::OboTestingContext>,
) -> ApiResult<Option<(Secret, Secret)>> {
    let expected = plane.testing.as_ref().map(|t| t.environment_id);
    match (expected, context) {
        (None, None) => Ok(None),
        (Some(expected), Some(tc)) if tc.app_id == AUDIENCE => {
            let audience = iam
                .audience_testing_context(AUDIENCE, &tc.app_secret, &tc.iam_test_key)
                .await
                .map_err(|e| match api_code(&e) {
                    Some((code, _)) => ting_rejected(
                        format!("IAM did not validate the Ting testing credential it issued ({code}); refusing to send"),
                        None,
                    ),
                    None => upstream(&e, "validate Ting's testing credential", true),
                })?;
            if audience.environment_id != expected || audience.application.app_id != AUDIENCE {
                return Err(ting_rejected(
                    "IAM's Ting testing credential names another testing environment; refusing to send",
                    None,
                ));
            }
            Ok(Some((
                Secret::new(tc.app_secret.clone()),
                Secret::new(tc.iam_test_key.clone()),
            )))
        }
        (None, Some(_)) => Err(ting_rejected(
            "IAM attached a testing context to a production proof; refusing to send",
            None,
        )),
        _ => Err(ting_rejected(
            "IAM returned a Ting proof without the testing context this environment requires; refusing to send",
            None,
        )),
    }
}

async fn mint_proof(
    state: &AppState,
    plane: &Plane,
    principal: &Principal,
    endpoint: Endpoint,
    body: &[u8],
) -> ApiResult<Proof> {
    let iam = plane.iam()?;
    let catalog = catalog(state, plane, iam.as_ref()).await?;
    check_endpoint(&catalog, endpoint)?;
    let request = models::OboExchangeRequest {
        org_id: Some(principal.org.as_str().to_owned()),
        subject_token: principal.access_token.expose().to_owned(),
        audience: AUDIENCE.to_owned(),
        endpoint_id: endpoint.id().to_owned(),
        metadata: json!({}),
        request: models::OboExchangeRequestBinding {
            method: "POST".to_owned(),
            body_sha256: body_sha256(body),
        },
    };
    let proof = iam
        .obo_exchange(
            &request,
            &catalog,
            &Mutation::with_key(IdempotencyKey::generate()),
        )
        .await
        .map_err(|e| exchange_error(&e, plane.is_testing()))?;
    if !(1..=60).contains(&proof.expires_in)
        || proof.expires_at <= OffsetDateTime::now_utc()
        || proof.access_proof.is_empty()
    {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::IamUnavailable,
            "IAM issued a Ting proof that is already expired or malformed",
        )
        .with_retryable(true));
    }
    let testing = validate_testing(iam.as_ref(), plane, proof.testing_context.as_ref()).await?;
    Ok(Proof {
        access_proof: Secret::new(proof.access_proof),
        testing,
    })
}

/// Ting's answer.
struct Reply {
    status: u16,
    body: Bytes,
    retry_after: Option<u64>,
}

impl Reply {
    fn error_code(&self) -> Option<String> {
        serde_json::from_slice::<Value>(&self.body)
            .ok()?
            .pointer("/error/code")?
            .as_str()
            .map(str::to_owned)
    }
}

async fn post(
    state: &AppState,
    endpoint: Endpoint,
    body: &[u8],
    proof: &Proof,
) -> ApiResult<Reply> {
    let url = format!("{}{}", state.0.config.ting.base_url, endpoint.path());
    let mut request = state
        .0
        .http
        .post(url)
        .timeout(state.0.config.ting.request_timeout)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .bearer_auth(proof.access_proof.expose())
        .body(body.to_vec());
    if let Some((app_secret, key)) = &proof.testing {
        request = request
            .header("IAM_TEST_APP_SECRET", app_secret.expose())
            .header("X-Testing-Environment-Key", key.expose());
    }
    let transport = |e: &reqwest::Error| {
        let why = if e.is_timeout() {
            "timed out"
        } else if e.is_connect() {
            "could not connect"
        } else {
            "the connection failed"
        };
        ting_unavailable(format!("peek-server could not reach Ting: {why}"), 5)
    };
    let mut response = request.send().await.map_err(|e| transport(&e))?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|s| s.min(3600));
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| transport(&e))? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(ting_unavailable("Ting answered with an oversized body", 5));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Reply {
        status,
        body: Bytes::from(body),
        retry_after,
    })
}

/// Classifies a failed Ting answer that is not a proof error.
fn classify_failure(reply: &Reply) -> (ApiError, &'static str) {
    let code = reply.error_code();
    let code_str = code.as_deref();
    match reply.status {
        403 if code_str == Some("recipient_not_registered") => (
            ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::RecipientNotRegistered,
                "Ting has no active peek grant for this Silicon (it was revoked, or never enrolled)",
            )
            .with_hint("the Silicon must re-enroll explicitly: peek ting enroll (peek never re-registers on its own)"),
            "recipient_not_registered",
        ),
        404 => (
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                ErrorCode::TingTypeMissing,
                "Ting does not know this peek type in this data context; an operator must register it",
            )
            .with_hint("operators: `ting --org tos types register --type <type> --description …` (BLUEPRINT §3.1); the delivery stays queued")
            .with_retry_after(900)
            .with_details(json!({"ting_code": code_str})),
            "ting_type_missing",
        ),
        409 => (
            ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::TingKeyConflict,
                "Ting already holds a different ting under this delivery key",
            )
            .with_hint("this is a peek bug (a changed body under the same key); report it with `peek report`")
            .with_details(json!({"ting_code": code_str})),
            "rejected",
        ),
        429 => (
            ting_unavailable(
                "Ting is rate limiting deliveries",
                reply.retry_after.unwrap_or(30),
            ),
            "unavailable",
        ),
        500..=599 => (
            ting_unavailable(
                format!(
                    "Ting could not accept the delivery right now ({})",
                    code_str.unwrap_or("server error")
                ),
                reply.retry_after.unwrap_or(5),
            ),
            "unavailable",
        ),
        status => {
            tracing::error!(status, ting_code = code_str, "Ting rejected a peek request; this is a bug or a configuration problem");
            (
                ting_rejected(
                    format!(
                        "Ting rejected the request with HTTP {status} {}",
                        code_str.unwrap_or("(no error code)")
                    ),
                    code_str,
                ),
                "rejected",
            )
        }
    }
}

/// Mints a proof and posts, retrying once with a fresh proof when Ting
/// refuses the proof itself. Returns the reply and the attempts made.
async fn call(
    state: &AppState,
    plane: &Plane,
    principal: &Principal,
    endpoint: Endpoint,
    body: &[u8],
) -> (ApiResult<Reply>, u32) {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let result = async {
            let proof = mint_proof(state, plane, principal, endpoint, body).await?;
            post(state, endpoint, body, &proof).await
        }
        .await;
        match result {
            Ok(reply)
                if reply.status == 401
                    && reply
                        .error_code()
                        .is_some_and(|c| PROOF_ERRORS.contains(&c.as_str())) =>
            {
                if attempts >= 2 {
                    let code = reply.error_code().unwrap_or_default();
                    return (
                        Err(ting_unavailable(
                            format!("Ting refused two fresh proofs in a row ({code})"),
                            5,
                        )),
                        attempts,
                    );
                }
            }
            other => return (other, attempts),
        }
    }
}

#[derive(Deserialize)]
struct Subscription {
    id: String,
    app_id: String,
    #[serde(rename = "for")]
    recipient: String,
    active: bool,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 255 && !id.chars().any(char::is_control)
}

/// Enrolls the principal as a Ting recipient for peek and returns the
/// subscription ID. Only called at login and on explicit `peek ting enroll`
/// (never from the delivery path, D7).
pub(crate) async fn enroll(
    state: &AppState,
    plane: &Plane,
    principal: &Principal,
) -> ApiResult<String> {
    principal.require_scopes(&ENROLL_SCOPES)?;
    let body =
        silicon_peek_client::ting::subscription_register_body(&principal.org, &principal.actor)
            .map_err(ApiError::from_client)?;
    let (reply, _) = call(state, plane, principal, Endpoint::Register, &body).await;
    let reply = reply?;
    match reply.status {
        200 | 201 => {
            let sub: Subscription = serde_json::from_slice(&reply.body).map_err(|_| {
                ting_unavailable("Ting answered the enrollment with an unexpected body", 5)
            })?;
            if !valid_id(&sub.id)
                || sub.app_id != silicon_peek_client::APP_ID
                || sub.recipient != principal.actor.as_str()
                || !sub.active
            {
                return Err(ting_rejected(
                    "Ting's enrollment answer does not match the request (app, recipient or active flag)",
                    None,
                ));
            }
            Ok(sub.id)
        }
        403 => {
            let code = reply.error_code();
            Err(ting_rejected(
                format!(
                    "Ting refused the enrollment ({})",
                    code.as_deref().unwrap_or("forbidden")
                ),
                code.as_deref(),
            ))
        }
        _ => Err(classify_failure(&reply).0),
    }
}

/// Revokes the principal's Ting grant (logout). A grant Ting no longer knows
/// counts as revoked.
pub(crate) async fn revoke(
    state: &AppState,
    plane: &Plane,
    principal: &Principal,
    subscription_id: &str,
) -> ApiResult<()> {
    principal.require_scopes(&REVOKE_SCOPES)?;
    let body = silicon_peek_client::ting::subscription_revoke_body(&principal.org, subscription_id)
        .map_err(ApiError::from_client)?;
    let (reply, _) = call(state, plane, principal, Endpoint::Revoke, &body).await;
    let reply = reply?;
    match reply.status {
        200..=299 | 404 => Ok(()),
        _ => Err(classify_failure(&reply).0),
    }
}

/// A delivery Ting accepted.
pub(crate) struct Accepted {
    pub(crate) ting_id: String,
    pub(crate) silent: bool,
    pub(crate) replayed: bool,
}

/// A delivery that failed, with the `deliveries.status` to record.
pub(crate) struct Failed {
    pub(crate) error: ApiError,
    pub(crate) status: &'static str,
}

#[derive(Deserialize)]
struct SendAnswer {
    id: String,
    status: String,
    key: String,
    #[serde(default)]
    silent: bool,
}

/// Sends the exact `body` bytes to `POST /v1/tings` (§3.5 steps 4–5, §3.6).
pub(crate) async fn send(
    state: &AppState,
    plane: &Plane,
    principal: &Principal,
    body: &[u8],
    ting_key: &str,
) -> (Result<Accepted, Failed>, u32) {
    let (reply, attempts) = call(state, plane, principal, Endpoint::Send, body).await;
    let reply = match reply {
        Ok(reply) => reply,
        Err(error) => {
            let status = if *error.code() == ErrorCode::TingRejected {
                "rejected"
            } else {
                "unavailable"
            };
            return (Err(Failed { error, status }), attempts);
        }
    };
    let outcome = match reply.status {
        200 | 202 => match serde_json::from_slice::<SendAnswer>(&reply.body) {
            Ok(answer)
                if valid_id(&answer.id)
                    && answer.status == "accepted"
                    && answer.key == ting_key =>
            {
                Ok(Accepted {
                    ting_id: answer.id,
                    silent: answer.silent,
                    replayed: reply.status == 200,
                })
            }
            _ => Err(Failed {
                error: ting_unavailable(
                    "Ting accepted the delivery but its answer does not match the request",
                    5,
                ),
                status: "unavailable",
            }),
        },
        401 => Err(Failed {
            error: ting_rejected(
                "Ting refused peek's credentials for the delivery",
                reply.error_code().as_deref(),
            ),
            status: "rejected",
        }),
        _ => {
            let (error, status) = classify_failure(&reply);
            Err(Failed { error, status })
        }
    };
    (outcome, attempts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u16, code: &str) -> Reply {
        Reply {
            status,
            body: Bytes::from(json!({"error": {"code": code, "message": "m"}}).to_string()),
            retry_after: Some(9),
        }
    }

    #[test]
    fn error_matrix() {
        let cases = [
            (
                403,
                "recipient_not_registered",
                ErrorCode::RecipientNotRegistered,
                409,
                "recipient_not_registered",
            ),
            (
                403,
                "permission_denied",
                ErrorCode::TingRejected,
                502,
                "rejected",
            ),
            (
                403,
                "test_context_mismatch",
                ErrorCode::TingRejected,
                502,
                "rejected",
            ),
            (
                404,
                "not_found",
                ErrorCode::TingTypeMissing,
                502,
                "ting_type_missing",
            ),
            (
                409,
                "idempotency_conflict",
                ErrorCode::TingKeyConflict,
                409,
                "rejected",
            ),
            (
                429,
                "temporarily_rate_limited",
                ErrorCode::TingUnavailable,
                503,
                "unavailable",
            ),
            (
                503,
                "proof_verification_uncertain",
                ErrorCode::TingUnavailable,
                503,
                "unavailable",
            ),
            (
                400,
                "invalid_input",
                ErrorCode::TingRejected,
                502,
                "rejected",
            ),
        ];
        for (status, code, expected, http, db) in cases {
            let (e, s) = classify_failure(&reply(status, code));
            assert_eq!(*e.code(), expected, "{status} {code}");
            assert_eq!(e.status().as_u16(), http, "{status} {code}");
            assert_eq!(s, db, "{status} {code}");
        }
        let (e, _) = classify_failure(&reply(429, "temporarily_rate_limited"));
        assert_eq!(e.retry_after(), Some(9), "Retry-After is passed on");
    }

    #[test]
    fn endpoint_catalog_must_match() -> Result<(), serde_json::Error> {
        let catalog: models::OboEndpointCatalog = serde_json::from_value(json!({
            "application": {"app_id": "ting", "org_id": "tos"},
            "endpoints": [
                {"endpoint_id": "tings.send", "path": "/v1/tings", "critical": true, "metadata": {}, "ttl_seconds": 60},
                {"endpoint_id": "subscriptions.register", "path": "/v1/other", "critical": true, "metadata": {}, "ttl_seconds": 60}
            ]
        }))?;
        assert!(check_endpoint(&catalog, Endpoint::Send).is_ok());
        assert!(check_endpoint(&catalog, Endpoint::Register).is_err());
        assert!(check_endpoint(&catalog, Endpoint::Revoke).is_err());
        Ok(())
    }
}
