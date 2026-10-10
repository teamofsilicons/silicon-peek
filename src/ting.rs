//! Ting enrollment uses a user proof; delivery and revocation use app proofs.
//! Every proof is short-lived, addressed to Ting, and scoped to one operation.
//! Retry bytes and the account UUID remain bound to the original action.

use axum::http::StatusCode;
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{Value, json};
use silicon_accounts_client::IssuedProof;
use silicon_peek_client::ErrorCode;

use crate::{
    auth::{ENROLL_SCOPES, Principal, REVOKE_SCOPES},
    error::{ApiError, ApiResult},
    obo,
    plane::Plane,
    state::AppState,
};

const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const TOKEN_ERRORS: [&str; 2] = ["invalid_obo_token", "invalid_proof"];

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
    authority: &IssuedProof,
) -> ApiResult<Reply> {
    let url = format!("{}{}", state.0.config.ting.base_url, endpoint.path());
    let mut request = state
        .0
        .http
        .post(url)
        .timeout(state.0.config.ting.request_timeout)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", authority.proof_token.expose()),
        )
        .body(body.to_vec());
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
        401 | 403 => (
            ting_unavailable("Ting could not verify Peek's Silicon Accounts proof; delivery remains queued", 300),
            "unavailable",
        ),
        404 => (
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                ErrorCode::TingTypeMissing,
                "Ting does not know this peek type in this data context; an operator must register it",
            )
            .with_hint("operators: `ting --account tos types register --type <type> --description …` (BLUEPRINT §3.1); the delivery stays queued")
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

/// Authorize and send with at most one dedicated-family refresh. The operation
/// stays bound to its original provider account, account and body hash.
async fn call(
    state: &AppState,
    plane: &Plane,
    principal: &Principal,
    endpoint: Endpoint,
    body: &[u8],
    operation: &str,
) -> (ApiResult<Reply>, u32) {
    let original: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return (Err(ApiError::internal("Invalid Ting payload")), 0),
    };

    for attempt in 1..=2 {
        let result = async {
            let authority = obo::access(
                state,
                plane,
                principal,
                endpoint.id(),
                operation,
                body,
                attempt == 2,
            )
            .await?;
            let mut outbound = original.clone();
            outbound.as_object_mut().map(|o| o.remove("account_id"));
            if outbound.get("for").is_some() {
                outbound["for"] = json!(principal.account);
            }
            // The authenticated account UUID is stable across handle changes
            // and remains identical when a retry receives a fresh proof.
            let body = if outbound == original {
                body.to_vec()
            } else {
                serde_json::to_vec(&outbound)
                    .map_err(|_| ApiError::internal("Could not build Ting request"))?
            };
            let reply = post(state, endpoint, &body, &authority).await?;
            if endpoint == Endpoint::Register && matches!(reply.status, 200 | 201) {
                let value: Value = serde_json::from_slice(&reply.body)
                    .map_err(|_| ting_rejected("Ting returned an unreadable enrollment", None))?;
                if value.get("for").and_then(Value::as_str) != Some(principal.account.as_str()) {
                    return Err(ting_rejected(
                        "Ting enrolled a different provider account",
                        None,
                    ));
                }
            }
            Ok(reply)
        }
        .await;
        match result {
            Ok(reply)
                if reply.status == 401
                    && reply
                        .error_code()
                        .is_some_and(|c| TOKEN_ERRORS.contains(&c.as_str())) =>
            {
                if attempt == 2 {
                    return (
                        Err(ting_unavailable(
                            "Ting could not verify Peek's Silicon Accounts proof; delivery remains queued",
                            300,
                        )),
                        attempt,
                    );
                }
            }
            other => return (other, attempt),
        }
    }
    (
        Err(ting_unavailable(
            "Ting proof verification is unavailable",
            300,
        )),
        2,
    )
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
/// subscription ID. Only called on explicit `peek ting enroll`
/// (never from the delivery path, D7).
pub(crate) async fn enroll(
    state: &AppState,
    plane: &Plane,
    principal: &Principal,
    operation: &str,
) -> ApiResult<String> {
    principal.require_scopes(&ENROLL_SCOPES)?;
    let body =
        silicon_peek_client::ting::subscription_register_body(&principal.account, &principal.actor)
            .map_err(ApiError::from_client)?;
    let (reply, _) = call(
        state,
        plane,
        principal,
        Endpoint::Register,
        &body,
        operation,
    )
    .await;
    let reply = reply?;
    match reply.status {
        200 | 201 => {
            let sub: Subscription = serde_json::from_slice(&reply.body).map_err(|_| {
                ting_unavailable("Ting answered the enrollment with an unexpected body", 5)
            })?;
            if !valid_id(&sub.id)
                || sub.app_id != silicon_peek_client::APP_ID
                || sub.recipient != principal.account.as_str()
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
    let body =
        silicon_peek_client::ting::subscription_revoke_body(&principal.account, subscription_id)
            .map_err(ApiError::from_client)?;
    let (reply, _) = call(
        state,
        plane,
        principal,
        Endpoint::Revoke,
        &body,
        subscription_id,
    )
    .await;
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
    let (reply, attempts) = call(state, plane, principal, Endpoint::Send, body, ting_key).await;
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
        _ => {
            let (error, status) = classify_failure(&reply);
            Err(Failed { error, status })
        }
    };
    (outcome, attempts)
}
