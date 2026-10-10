//! `POST /api/v1/reports`: bug reports (`peek report`). Stored first, then
//! filed as a GitHub issue with the server-held token; if that fails (or no
//! token is configured, or this is a testing context) the report stays
//! `stored` for operators. 10 per hour per client IP.

use axum::{
    Extension,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use silicon_peek_client::{
    ErrorCode,
    api::{ReportContext, ReportRequest, ReportResponse, ReportStatus},
    timestamp::unix_now,
};
use uuid::Uuid;

use crate::{
    auth::{self, Bearer, Principal},
    error::{ApiError, ApiResult},
    extract::{ClientIp, IdemKey, JsonBody},
    github,
    idempotency::{Idempotent, request_hash},
    plane::Plane,
    state::AppState,
    store::{self, reports::NewReport},
    telemetry::{Event, RequestMeta},
};

const MESSAGE_MAX_CHARS: usize = 10_000;
const CONTEXT_FIELD_MAX_CHARS: usize = 200;
const STATUS_MAX_BYTES: usize = 64 * 1024;

fn validate(report: &ReportRequest) -> ApiResult<()> {
    let message = report.message.trim();
    if message.is_empty() {
        return Err(ApiError::invalid_input(
            "the report message is empty; describe what you ran, what happened and what you expected",
        ));
    }
    if report.message.chars().count() > MESSAGE_MAX_CHARS {
        return Err(ApiError::invalid_input(format!(
            "the report message is longer than {MESSAGE_MAX_CHARS} characters"
        )));
    }
    if let Some(pr) = &report.pr
        && !ReportRequest::pr_is_valid(pr)
    {
        return Err(ApiError::invalid_input(format!(
            "pr `{pr}` is not a pull request of teamofsilicons/silicon-peek"
        ))
        .with_hint("pass https://github.com/teamofsilicons/silicon-peek/pull/<number>"));
    }
    if let Some(ReportContext {
        cli_version,
        platform,
        command,
        error_code,
    }) = &report.context
    {
        for (name, value) in [
            ("cli_version", cli_version),
            ("platform", platform),
            ("command", command),
            ("error_code", error_code),
        ] {
            if let Some(v) = value
                && (v.chars().count() > CONTEXT_FIELD_MAX_CHARS || v.chars().any(char::is_control))
            {
                return Err(ApiError::invalid_input(format!(
                    "context.{name} must be at most {CONTEXT_FIELD_MAX_CHARS} characters without control characters"
                )));
            }
        }
    }
    if let Some(status) = &report.status {
        let size = serde_json::to_vec(status).map_or(usize::MAX, |v| v.len());
        if size > STATUS_MAX_BYTES {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::PayloadTooLarge,
                format!(
                    "the attached status is {size} bytes; at most {STATUS_MAX_BYTES} are accepted"
                ),
            ));
        }
    }
    Ok(())
}

/// Creates a report.
pub(crate) async fn create(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    plane: Plane,
    IdemKey(key): IdemKey,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    body: JsonBody<ReportRequest>,
) -> ApiResult<Response> {
    let report = body.value;
    validate(&report)?;
    // A bad bearer never blocks a report (it may be the very thing reported).
    let principal = match Bearer::from_headers(&headers) {
        Ok(Some(bearer)) => auth::authenticate(&plane, bearer, &state.0.config.accounts.app_id)
            .await
            .ok(),
        _ => None,
    };
    let actor = principal
        .as_ref()
        .map(|p| format!("{}|{}", p.account, p.actor))
        .unwrap_or_default();
    let run = Idempotent {
        db: plane.db.clone(),
        ctx: plane.ctx_string(),
        scope: "reports",
        key,
        request_sha256: request_hash(&[&body.raw, actor.as_bytes()]),
    };
    run.run(
        StatusCode::OK,
        store_and_file(state.clone(), meta, plane, principal, report, ip),
    )
    .await
}

async fn store_and_file(
    state: AppState,
    meta: RequestMeta,
    plane: Plane,
    principal: Option<Principal>,
    report: ReportRequest,
    ip: String,
) -> ApiResult<ReportResponse> {
    let limits = &state.0.limits;
    limits.reports_ip.take(&ip, 1).map_err(|s| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::RateLimited,
            format!(
                "too many reports from this address (limit {} per hour)",
                limits.reports_ip.limit()
            ),
        )
        .with_retry_after(s)
    })?;
    let id = format!("rep_{}", Uuid::now_v7().simple());
    let (ctx, id_db) = (plane.ctx_string(), id.clone());
    let (account, actor) = (
        principal.as_ref().map(|p| p.account.to_string()),
        principal.as_ref().map(|p| p.actor.to_string()),
    );
    let context = report
        .context
        .as_ref()
        .and_then(|c| serde_json::to_string(c).ok());
    let status = report.status.as_ref().map(ToString::to_string);
    let (message, pr) = (report.message.clone(), report.pr.clone());
    plane
        .db
        .call(move |conn| {
            Ok(store::reports::insert(
                conn,
                &NewReport {
                    id: &id_db,
                    ctx: &ctx,
                    account: account.as_deref(),
                    actor: actor.as_deref(),
                    message: &message,
                    pr: pr.as_deref(),
                    context: context.as_deref(),
                    attached_status: status.as_deref(),
                    created_at: unix_now(),
                },
            )?)
        })
        .await?;
    let filed = { github::file_issue(&state, &report).await };
    let id_db = id.clone();
    let response = match filed {
        Ok(url) => {
            let url_db = url.clone();
            plane
                .db
                .call(move |conn| Ok(store::reports::mark_filed(conn, &id_db, &url_db)?))
                .await?;
            ReportResponse {
                id,
                status: ReportStatus::Filed,
                issue_url: Some(url),
            }
        }
        Err(why) => {
            tracing::info!(report = %id, reason = %why, "report stored without a GitHub issue");
            if let Err(e) = plane
                .db
                .call(move |conn| Ok(store::reports::mark_filing_error(conn, &id_db, &why)?))
                .await
            {
                tracing::warn!(code = %e.code(), "could not record why a report was not filed");
            }
            ReportResponse {
                id,
                status: ReportStatus::Stored,
                issue_url: None,
            }
        }
    };
    state.0.telemetry.record(
        &meta,
        Event::new("report.created", "report").context(
            "status",
            match response.status {
                ReportStatus::Filed => "filed",
                ReportStatus::Stored => "stored",
            },
        ),
    );
    Ok(response)
}
