//! `POST /api/v1/auth/{login,refresh,logout}` and `GET /api/v1/auth/me`
//! (BLUEPRINT §2.4, §2.6). The app secret stays here; the token pair goes
//! straight back to the caller and is never stored (D5).

use std::{collections::BTreeSet, time::Instant};

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use silicon_iam_client::{Error as IamError, IdempotencyKey as IamKey, Mutation, models};
use silicon_peek_client::{
    ErrorCode, REQUIRED_SCOPES, Secret,
    api::{
        LoginRequest, LogoutRequest, Me, RefreshRequest, SessionResponse, TingEnrollment, headers,
    },
    identity::{Actor, ActorId, ActorType, OrgId},
    ids::IdempotencyKey,
    timestamp::unix_now,
};

use crate::{
    auth::{self, Bearer, Principal},
    error::{ApiError, ApiResult},
    extract::{IdemKey, JsonBody, single_header},
    iam::{IamPlane, PROFILE_TIMEOUT, TokenKind, api_code, token_shape_ok, upstream},
    plane::Plane,
    state::AppState,
    store,
    telemetry::{Event, RequestMeta},
    ting,
};

fn mutation(key: &IdempotencyKey) -> ApiResult<Mutation> {
    IamKey::parse(key.as_str())
        .map(Mutation::with_key)
        .map_err(|_| {
            ApiError::invalid_input(
                "the Idempotency-Key is not acceptable to IAM (16–255 visible ASCII)",
            )
        })
}

fn idempotency_error(code: &str) -> Option<ApiError> {
    match code {
        "idempotency_in_progress" => Some(
            ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::IdempotencyInProgress,
                "IAM is still processing the first request with this Idempotency-Key",
            )
            .with_hint("retry the same request with the same key in a moment")
            .with_retry_after(2),
        ),
        "idempotency_conflict" => Some(ApiError::new(
            StatusCode::CONFLICT,
            ErrorCode::IdempotencyConflict,
            "IAM saw this Idempotency-Key with a different request",
        )),
        "idempotency_response_expired" => Some(
            ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::IdempotencyResponseExpired,
                "IAM's 10-minute replay window for this Idempotency-Key has passed",
            )
            .with_hint(
                "the original result can no longer be replayed; log in again: peek login '<SLT>'",
            ),
        ),
        _ => None,
    }
}

fn slt_rejected(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, ErrorCode::SltRejected, message).with_hint(
        "mint a fresh SLT (it lives 2 minutes and works once): iam silicon-login --app-id peek --grant-org <org> --approve-scopes, then peek login '<SLT>'",
    )
}

fn session_rejected(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, ErrorCode::SessionRejected, message)
        .with_hint("the session is over; log in again: si auth setup peek (or iam silicon-login … --app-id peek …; peek login '<SLT>')")
}

fn login_error(e: &IamError, testing: bool) -> ApiError {
    match api_code(e) {
        Some(("private_application_organization_required", _)) => ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::PrivateApplicationOrganizationRequired,
            "peek is still private to its owning org, and this SLT does not select it",
        )
        .with_hint("mint the SLT with --grant-org tos, or wait until peek is published"),
        Some((code, 409)) if idempotency_error(code).is_some() => {
            idempotency_error(code).unwrap_or_else(|| upstream(e, "exchange the SLT", testing))
        }
        Some((code, 400 | 401 | 403 | 404 | 410 | 422)) if code != "invalid_client" => {
            slt_rejected(format!(
                "IAM rejected the SLT ({code}): it expired, was already used, or was minted for another app"
            ))
        }
        _ => upstream(e, "exchange the SLT", testing),
    }
}

fn refresh_error(e: &IamError, testing: bool) -> ApiError {
    match api_code(e) {
        Some((code, 409)) if idempotency_error(code).is_some() => {
            idempotency_error(code).unwrap_or_else(|| upstream(e, "refresh the session", testing))
        }
        Some((
            code @ ("invalid_grant" | "refresh_token_reuse" | "unauthenticated" | "invalid_token"),
            400 | 401 | 410,
        )) => session_rejected(format!("IAM ended this session ({code})")),
        Some((code, 401)) if code != "invalid_client" => {
            session_rejected(format!("IAM ended this session ({code})"))
        }
        _ => upstream(e, "refresh the session", testing),
    }
}

/// A token pair IAM issued, checked against its live authorizations.
struct Verified {
    access: Secret,
    refresh: Secret,
    expires_in: u64,
    scopes: BTreeSet<String>,
    actor: ActorId,
    org: OrgId,
    org_ids: Vec<OrgId>,
    membership_id: String,
}

fn unusable(what: &str) -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::IamUnavailable,
        format!("IAM returned an unusable session: {what}"),
    )
    .with_retryable(true)
}

async fn verify_tokens(
    iam: &dyn IamPlane,
    app_id: &str,
    tokens: models::OAuthTokenResponse,
    hint: Option<&OrgId>,
    reject: fn(String) -> ApiError,
) -> ApiResult<Verified> {
    if !token_shape_ok(&tokens.access_token, "oat_")
        || !token_shape_ok(&tokens.refresh_token, "ort_")
        || tokens.token_type.as_str() != Some("Bearer")
        || tokens.expires_in <= 0
    {
        return Err(unusable("the token pair is malformed"));
    }
    let actor_ref = tokens.actor.as_ref().ok_or_else(|| unusable("no actor"))?;
    let actor_type = match actor_ref.type_field {
        models::ActorRefType::Silicon => ActorType::Silicon,
        models::ActorRefType::Carbon => ActorType::Carbon,
        _ => {
            return Err(reject(
                "peek sessions belong to Silicons and Carbons only".to_owned(),
            ));
        }
    };
    let actor =
        ActorId::parse(&actor_ref.public_id).map_err(|_| unusable("the actor ID is malformed"))?;
    if actor.actor_type() != actor_type {
        return Err(unusable("the actor type does not match its ID"));
    }
    let org = tokens
        .org_id
        .as_deref()
        .and_then(|org| OrgId::parse(org).ok())
        .ok_or_else(|| reject("IAM 5 requires one organization; sign in again".to_owned()))?;
    if hint.is_some_and(|hint| hint != &org) {
        return Err(reject(
            "The session belongs to a different organization".to_owned(),
        ));
    }
    let inspected = iam
        .introspect(&tokens.access_token, None)
        .await
        .map_err(|e| upstream(&e, "verify the new session", iam.environment_id().is_some()))?;
    let principal = auth::verify(
        &inspected,
        &Bearer {
            token: Secret::new(tokens.access_token.clone()),
            org: org.clone(),
        },
        app_id,
        iam.environment_id(),
        unix_now(),
    )
    .map_err(|check| {
        reject(format!(
            "IAM 5 returned an invalid ordinary session ({check}); sign in again"
        ))
    })?;
    let scopes: BTreeSet<String> = tokens
        .scope
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect();
    if principal.actor != actor || principal.scopes != scopes {
        return Err(reject(
            "The token response and its verified identity do not agree".to_owned(),
        ));
    }
    let membership_id = principal.membership_id;

    Ok(Verified {
        access: Secret::new(tokens.access_token),
        refresh: Secret::new(tokens.refresh_token),
        expires_in: u64::try_from(tokens.expires_in).unwrap_or(0),
        scopes: tokens
            .scope
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect(),
        actor,
        org: org.clone(),
        org_ids: vec![org],
        membership_id,
    })
}

fn session_response(
    v: Verified,
    plane: &Plane,
    display_name: Option<String>,
    ting: Option<TingEnrollment>,
) -> SessionResponse {
    let reconsent = REQUIRED_SCOPES.iter().any(|s| !v.scopes.contains(*s));
    SessionResponse {
        access_token: v.access,
        refresh_token: v.refresh,
        token_type: "Bearer".to_owned(),
        expires_in: v.expires_in,
        scope: v.scopes.into_iter().collect::<Vec<_>>().join(" "),
        actor: Actor {
            actor_type: v.actor.actor_type(),
            public_id: v.actor,
        },
        org_id: v.org,
        org_ids: v.org_ids,
        membership_id: v.membership_id,
        reconsent_required: reconsent,
        display_name,
        ting,
        testing_environment: plane.testing_environment(),
    }
}

/// Best-effort display name (`self.profile.read`); never fails the caller.
async fn display_name(
    iam: &dyn IamPlane,
    access_token: &str,
    scopes: &BTreeSet<String>,
) -> Option<String> {
    if !scopes.contains("self.profile.read") {
        return None;
    }
    match tokio::time::timeout(PROFILE_TIMEOUT, iam.me(access_token)).await {
        Ok(Ok(me)) => me
            .get("display_name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|n| {
                !n.is_empty() && n.chars().count() <= 200 && !n.chars().any(char::is_control)
            })
            .map(str::to_owned),
        Ok(Err(e)) => {
            tracing::info!(iam_code = ?api_code(&e).map(|(c, _)| c.to_owned()), "display name unavailable");
            None
        }
        Err(_) => None,
    }
}

/// A JSON response (the request middleware adds `Cache-Control: no-store`
/// and, on auth routes, `Pragma: no-cache`).
fn json_response<T: serde::Serialize>(value: T) -> Response {
    Json(value).into_response()
}

fn org_hint(headers: &HeaderMap) -> ApiResult<Option<OrgId>> {
    single_header(headers, headers::ORG_ID)?
        .map(|h| OrgId::parse(h).map_err(ApiError::from_client))
        .transpose()
}

/// `POST /api/v1/auth/login` `{"slt"}`.
pub(crate) async fn login(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    plane: Plane,
    IdemKey(key): IdemKey,
    headers: HeaderMap,
    body: JsonBody<LoginRequest>,
) -> ApiResult<Response> {
    let started = Instant::now();
    let result = login_inner(&state, &plane, &key, &headers, &body.value).await;
    let mut event = Event::new("auth.login", "auth.login").duration(started.elapsed());
    if let Ok(s) = &result {
        event = event.actor(&s.org_id, &s.actor.public_id);
    }
    state.0.telemetry.record(&meta, event.outcome(&result));
    result.map(json_response)
}

async fn login_inner(
    state: &AppState,
    plane: &Plane,
    key: &IdempotencyKey,
    headers: &HeaderMap,
    request: &LoginRequest,
) -> ApiResult<SessionResponse> {
    let slt = request.slt.expose();
    if !plane.is_testing() && ActorId::looks_like_public_id(slt) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::SltIsPublicId,
            "that is a public ID, not an SLT; only testing environments accept a public ID in place of an SLT",
        )
        .with_hint("mint an SLT: iam silicon-login --app-id peek --grant-org <org> --approve-scopes"));
    }
    if slt.is_empty() || slt.len() > 8192 || !slt.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(slt_rejected(
            "the SLT is empty or contains characters an SLT never has",
        ));
    }
    let hint = org_hint(headers)?;
    if plane.is_testing() && ActorId::looks_like_public_id(slt) && hint.is_none() {
        return Err(ApiError::invalid_input(
            "Testing actor login requires X-Org-ID; choose the organization explicitly",
        ));
    }
    let iam = plane.iam()?;
    let tokens = iam
        .login(slt, hint.as_ref().map(OrgId::as_str), &mutation(key)?)
        .await
        .map_err(|e| login_error(&e, plane.is_testing()))?;
    let verified = verify_tokens(
        iam.as_ref(),
        &state.0.config.iam.app_id,
        tokens,
        hint.as_ref(),
        slt_rejected_owned,
    )
    .await?;
    let name = display_name(iam.as_ref(), verified.access.expose(), &verified.scopes).await;
    // Login remains usable when Ting permission is declined or absent.
    // Provider token families belong to the feature, never to this session.
    Ok(session_response(verified, plane, name, None))
}

fn slt_rejected_owned(message: String) -> ApiError {
    slt_rejected(message)
}

fn session_rejected_owned(message: String) -> ApiError {
    session_rejected(message)
}

/// `POST /api/v1/auth/refresh` `{"refresh_token"}`.
pub(crate) async fn refresh(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    plane: Plane,
    IdemKey(key): IdemKey,
    headers: HeaderMap,
    body: JsonBody<RefreshRequest>,
) -> ApiResult<Response> {
    let started = Instant::now();
    let result = async {
        let token = body.value.refresh_token.expose();
        if !token_shape_ok(token, "ort_") {
            return Err(session_rejected(
                "the refresh token is not a peek refresh token (ort_…)",
            ));
        }
        let hint = org_hint(&headers)?;
        let iam = plane.iam()?;
        let tokens = iam
            .refresh(token, &mutation(&key)?)
            .await
            .map_err(|e| refresh_error(&e, plane.is_testing()))?;
        let verified = verify_tokens(
            iam.as_ref(),
            &state.0.config.iam.app_id,
            tokens,
            hint.as_ref(),
            session_rejected_owned,
        )
        .await?;
        Ok(session_response(verified, &plane, None, None))
    }
    .await;
    let mut event = Event::new("auth.refresh", "auth.refresh").duration(started.elapsed());
    if let Ok(s) = &result {
        event = event.actor(&s.org_id, &s.actor.public_id);
    }
    state.0.telemetry.record(&meta, event.outcome(&result));
    result.map(json_response)
}

/// `POST /api/v1/auth/logout` `{"token","revoke_ting"?}`. The Silicon's Ting
/// grant is revoked too only with `"revoke_ting":true` and the bearer: it is
/// shared by every home of the Silicon, so a plain logout (including every
/// 0.1.0 CLI, which always sent the bearer) leaves it alone. Unknown tokens
/// succeed (no oracle).
pub(crate) async fn logout(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    plane: Plane,
    IdemKey(key): IdemKey,
    headers: HeaderMap,
    body: JsonBody<LogoutRequest>,
) -> ApiResult<Response> {
    let result = logout_inner(&state, &plane, &key, &headers, &body.value).await;
    state.0.telemetry.record(
        &meta,
        Event::new("auth.logout", "auth.logout").outcome(&result),
    );
    result.map(|()| StatusCode::NO_CONTENT.into_response())
}

async fn logout_inner(
    state: &AppState,
    plane: &Plane,
    key: &IdempotencyKey,
    headers: &HeaderMap,
    request: &LogoutRequest,
) -> ApiResult<()> {
    let token = request.token.expose();
    let kind = if token_shape_ok(token, "ort_") {
        TokenKind::Refresh
    } else if token_shape_ok(token, "oat_") {
        TokenKind::Access
    } else {
        return Err(ApiError::invalid_input(
            "`token` must be a peek refresh token (ort_…) or access token (oat_…)",
        ));
    };
    let iam = plane.iam()?;
    if request.revoke_ting
        && let Ok(Some(bearer)) = Bearer::from_headers(headers)
        && let Ok(principal) = auth::authenticate(plane, bearer, &state.0.config.iam.app_id).await
    {
        revoke_ting_grant(state, plane, &principal).await;
    }
    match iam.revoke(token, kind, &mutation(key)?).await {
        Ok(()) => Ok(()),
        Err(e) => match api_code(&e) {
            Some((code, 409)) if idempotency_error(code).is_some() => Err(idempotency_error(code)
                .unwrap_or_else(|| upstream(&e, "revoke the session", plane.is_testing()))),
            // IAM treats unknown tokens as revoked; any other refusal of this
            // token means there is nothing left to revoke.
            Some((code, 400 | 401 | 404 | 410 | 422)) if code != "invalid_client" => Ok(()),
            _ => Err(upstream(&e, "revoke the session", plane.is_testing())),
        },
    }
}

/// Explicitly removes the Ting enrollment; ordinary logout preserves consent.
async fn revoke_ting_grant(state: &AppState, plane: &Plane, principal: &Principal) {
    let (ctx, org, actor) = (
        plane.ctx_string(),
        principal.org.to_string(),
        principal.actor.to_string(),
    );
    let enrollment = {
        let (ctx, org, actor) = (ctx.clone(), org.clone(), actor.clone());
        plane
            .db
            .call(move |conn| Ok(store::enrollments::get(conn, &ctx, &org, &actor)?))
            .await
    };
    let Ok(Some(enrollment)) = enrollment else {
        return;
    };
    if !enrollment.active() {
        return;
    }
    match ting::revoke(state, plane, principal, &enrollment.subscription_id).await {
        Ok(()) => {
            if let Err(e) = plane
                .db
                .call(move |conn| {
                    Ok(store::enrollments::mark_revoked(
                        conn,
                        &ctx,
                        &org,
                        &actor,
                        unix_now(),
                    )?)
                })
                .await
            {
                tracing::warn!(code = %e.code(), "could not record a revoked Ting grant");
            }
        }
        Err(e) => {
            tracing::warn!(code = %e.code(), error_message = e.message(), "revoking the Ting grant at logout failed; the IAM revocation still ends the session");
        }
    }
}

/// `GET /api/v1/auth/me`: the live-introspected identity plus peek's own
/// Ting enrollment record.
pub(crate) async fn me(
    State(state): State<AppState>,
    plane: Plane,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let principal = auth::authenticate(
        &plane,
        Bearer::required(&headers)?,
        &state.0.config.iam.app_id,
    )
    .await?;
    let iam = plane.iam()?;
    let name = display_name(
        iam.as_ref(),
        principal.access_token.expose(),
        &principal.scopes,
    )
    .await;
    let (ctx, org, actor) = (
        plane.ctx_string(),
        principal.org.to_string(),
        principal.actor.to_string(),
    );
    let enrollment = plane
        .db
        .call(move |conn| Ok(store::enrollments::get(conn, &ctx, &org, &actor)?))
        .await?;
    let ting = match enrollment {
        Some(e) if e.active() => TingEnrollment {
            subscribed: true,
            subscription_id: Some(e.subscription_id),
            error: None,
        },
        _ => TingEnrollment {
            subscribed: false,
            subscription_id: None,
            error: None,
        },
    };
    let me = Me {
        authenticated: true,
        actor: principal.actor_object(),
        display_name: name,
        org_id: principal.org.clone(),
        membership_id: principal.membership_id.clone(),
        org_role: principal.org_role.clone(),
        scopes: principal.scope_list(),
        reconsent_required: principal.reconsent_required(),
        ting,
    };
    Ok(json_response(me))
}
