//! `POST /api/v1/ting/recipient` (explicit enrollment, §3.4) and
//! `POST /api/v1/deliveries` (§3.5, §3.6).

use std::time::Instant;

use axum::{
    Extension,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use silicon_peek_client::{
    ErrorCode,
    api::{DeliveryResponse, DeliveryStatus, TingRecipient},
    identity::DataContext,
    timestamp::unix_now,
    ting::{DeliveryRequest, TING_BODY_MAX_BYTES, TingData, ting_send_body},
};

use crate::{
    auth::{self, Bearer, DELIVERY_SCOPES, ENROLL_SCOPES, Principal},
    error::{ApiError, ApiResult},
    extract::{IdemKey, JsonBody, RawBody},
    idempotency::{Idempotent, request_hash},
    plane::Plane,
    state::AppState,
    store::{
        self,
        deliveries::{Begin, DeliveryKey, Outcome},
    },
    telemetry::{Event, RequestMeta},
    ting,
};

/// The enrollment body: exactly `{}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EmptyBody {}

/// `POST /api/v1/ting/recipient`: (re)enrolls the caller as a Ting recipient.
pub(crate) async fn enroll(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    plane: Plane,
    IdemKey(key): IdemKey,
    headers: HeaderMap,
    _body: JsonBody<EmptyBody>,
) -> ApiResult<Response> {
    let principal = auth::authenticate(
        &plane,
        Bearer::required(&headers)?,
        &state.0.config.iam.app_id,
    )
    .await?;
    principal.require_scopes(&ENROLL_SCOPES)?;
    let ctx = plane.ctx_string();
    let hash = request_hash(&[
        b"ting.recipient",
        ctx.as_bytes(),
        principal.org.as_str().as_bytes(),
        principal.actor.as_str().as_bytes(),
    ]);
    let run = Idempotent {
        db: plane.db.clone(),
        ctx,
        scope: "ting.recipient",
        key,
        request_sha256: hash,
    };
    let task_state = state.clone();
    let task_meta = meta.clone();
    run.run(StatusCode::OK, async move {
        let started = Instant::now();
        let result = async {
            let subscription = ting::enroll(&task_state, &plane, &principal).await?;
            let (ctx, org, actor, sub) = (
                plane.ctx_string(),
                principal.org.to_string(),
                principal.actor.to_string(),
                subscription.clone(),
            );
            plane
                .db
                .call(move |conn| {
                    Ok(store::enrollments::record(
                        conn,
                        &ctx,
                        &org,
                        &actor,
                        &sub,
                        unix_now(),
                    )?)
                })
                .await?;
            Ok(TingRecipient {
                subscribed: true,
                subscription_id: subscription,
            })
        }
        .await;
        task_state.0.telemetry.record(
            &task_meta,
            Event::new("ting.enroll", "ting.enroll")
                .actor(&principal.org, &principal.actor)
                .duration(started.elapsed())
                .outcome(&result),
        );
        result
    })
    .await
}

/// `POST /api/v1/deliveries`: validates the delivery against the verified
/// identity, builds the deterministic Ting body and sends it.
pub(crate) async fn deliver(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    plane: Plane,
    IdemKey(key): IdemKey,
    headers: HeaderMap,
    RawBody(raw): RawBody,
) -> ApiResult<Response> {
    if raw.len() > TING_BODY_MAX_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::PayloadTooLarge,
            format!(
                "the delivery is {} bytes; Ting accepts at most {TING_BODY_MAX_BYTES}",
                raw.len()
            ),
        ));
    }
    let delivery: DeliveryRequest = silicon_peek_client::json::from_slice(&raw, "the delivery")
        .map_err(ApiError::from_client)?;
    let principal = auth::authenticate(
        &plane,
        Bearer::required(&headers)?,
        &state.0.config.iam.app_id,
    )
    .await?;
    principal.require_scopes(&DELIVERY_SCOPES)?;
    let data = delivery
        .validate_for(&principal.actor)
        .map_err(ApiError::from_client)?;
    let expected = plane.data_context();
    if data_context(&data) != expected {
        return Err(ApiError::invalid_input(format!(
            "data.context is `{}` but this request is in the {} plane",
            context_name(data_context(&data)),
            context_name(expected)
        )));
    }
    let body = ting_send_body(&principal.org, &principal.actor, &delivery)
        .map_err(ApiError::from_client)?;
    let run = Idempotent {
        db: plane.db.clone(),
        ctx: plane.ctx_string(),
        scope: "deliveries",
        key,
        request_sha256: request_hash(&[&raw]),
    };
    run.run(
        StatusCode::OK,
        deliver_once(state.clone(), meta, plane, principal, delivery, body),
    )
    .await
}

/// The `context` every peek payload carries. Exhaustive on purpose: a new
/// Ting type in the client crate does not compile here until its plane check
/// is wired.
fn data_context(data: &TingData) -> DataContext {
    match data {
        TingData::AskAnswered(d) => d.context,
        TingData::AskDismissed(d) => d.context,
        TingData::AskExpired(d) => d.context,
        TingData::MessageReceived(d) => d.context,
        TingData::SpeechFinished(d) => d.context,
        TingData::ShowDismissed(d) => d.context,
        TingData::SendExpired(d) => d.context,
        TingData::ScheduleDue(d) => d.context,
        TingData::SendShown(d) => d.context,
    }
}

fn context_name(c: DataContext) -> &'static str {
    match c {
        DataContext::Production => "production",
        DataContext::Testing => "testing",
    }
}

async fn deliver_once(
    state: AppState,
    meta: RequestMeta,
    plane: Plane,
    principal: Principal,
    delivery: DeliveryRequest,
    body: Vec<u8>,
) -> ApiResult<DeliveryResponse> {
    let event_id = delivery.event_id.to_string();
    if let Some(recorded) = begin(&plane, &principal, &delivery).await? {
        return Ok(recorded);
    }
    let started = Instant::now();
    let (outcome, attempts) = ting::send(&state, &plane, &principal, &body, &delivery.key).await;
    let event = Event::new("ting.send", delivery.ting_type.as_str())
        .actor(&principal.org, &principal.actor)
        .duration(started.elapsed())
        .context("attempt", attempts);
    match outcome {
        Ok(accepted) => {
            record(
                &plane,
                &event_id,
                Recorded::Accepted(&accepted),
                attempts,
                &principal,
            )
            .await?;
            state.0.telemetry.record(
                &meta,
                event.context(
                    "ting_status",
                    if accepted.silent {
                        "silent"
                    } else {
                        "accepted"
                    },
                ),
            );
            Ok(DeliveryResponse {
                event_id,
                ting_id: accepted.ting_id,
                status: DeliveryStatus::Accepted,
                silent: accepted.silent,
                replayed: accepted.replayed,
            })
        }
        Err(failed) => {
            if let Err(e) = record(
                &plane,
                &event_id,
                Recorded::Failed(&failed),
                attempts,
                &principal,
            )
            .await
            {
                tracing::warn!(code = %e.code(), "could not record a failed delivery");
            }
            state.0.telemetry.record(
                &meta,
                event
                    .context("ting_status", failed.error.code().as_str())
                    .failed(failed.error.code()),
            );
            Err(failed.error)
        }
    }
}

/// Records the first sight of a delivery; `Some` when Ting already accepted
/// this event (answered from the record, never resent).
async fn begin(
    plane: &Plane,
    principal: &Principal,
    delivery: &DeliveryRequest,
) -> ApiResult<Option<DeliveryResponse>> {
    let ctx = plane.ctx_string();
    let event_id = delivery.event_id.to_string();
    let begin = {
        let (ctx, event_id, org, actor, ting_type, ting_key) = (
            ctx.clone(),
            event_id.clone(),
            principal.org.to_string(),
            principal.actor.to_string(),
            delivery.ting_type.as_str().to_owned(),
            delivery.key.clone(),
        );
        plane
            .db
            .call(move |conn| {
                let key = DeliveryKey {
                    ctx: &ctx,
                    event_id: &event_id,
                    org: &org,
                    actor: &actor,
                    ting_type: &ting_type,
                    ting_key: &ting_key,
                };
                Ok(store::deliveries::begin(conn, &key, unix_now())?)
            })
            .await?
    };
    match begin {
        Begin::Conflict => Err(ApiError::new(
            StatusCode::CONFLICT,
            ErrorCode::IdempotencyConflict,
            format!("event_id {event_id} was already used for a different delivery"),
        )
        .with_hint("every delivery needs its own evt_… ID; this is a peekd bug")),
        Begin::Accepted(accepted) => Ok(Some(DeliveryResponse {
            event_id,
            ting_id: accepted.ting_id,
            status: DeliveryStatus::Accepted,
            silent: accepted.silent,
            replayed: true,
        })),
        Begin::Send => Ok(None),
    }
}

enum Recorded<'a> {
    Accepted(&'a ting::Accepted),
    Failed(&'a ting::Failed),
}

/// Records an attempt's outcome; `recipient_not_registered` also marks the
/// enrollment revoked, so `login status` shows `ting.subscribed:false`.
async fn record(
    plane: &Plane,
    event_id: &str,
    outcome: Recorded<'_>,
    attempts: u32,
    principal: &Principal,
) -> ApiResult<()> {
    let (ctx, event_id) = (plane.ctx_string(), event_id.to_owned());
    let (org, actor) = (principal.org.to_string(), principal.actor.to_string());
    let (ting_id, silent, failed) = match outcome {
        Recorded::Accepted(a) => (Some(a.ting_id.clone()), a.silent, None),
        Recorded::Failed(f) => (
            None,
            false,
            Some((f.status, f.error.code().as_str().to_owned())),
        ),
    };
    plane
        .db
        .call(move |conn| {
            let now = unix_now();
            match (&ting_id, &failed) {
                (Some(ting_id), _) => store::deliveries::record(
                    conn,
                    &ctx,
                    &event_id,
                    &Outcome::Accepted { ting_id, silent },
                    attempts,
                    now,
                )?,
                (None, Some((status, code))) => {
                    store::deliveries::record(
                        conn,
                        &ctx,
                        &event_id,
                        &Outcome::Failed {
                            status,
                            error: code,
                        },
                        attempts,
                        now,
                    )?;
                    if *status == "recipient_not_registered" {
                        store::enrollments::mark_revoked(conn, &ctx, &org, &actor, now)?;
                    }
                }
                (None, None) => {}
            }
            Ok(())
        })
        .await
}
