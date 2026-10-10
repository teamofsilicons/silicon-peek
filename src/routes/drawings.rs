//! `PUT/GET/DELETE /api/v1/drawings/current`: the Silicon's drawing copy.

use axum::{
    Extension, Json,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use silicon_peek_client::{
    ErrorCode,
    api::{DrawingStored, headers},
    schema::limits::DRAWING_MAX_BYTES,
    timestamp::{Timestamp, unix_now},
};

use crate::{
    auth::{self, Bearer, Principal},
    error::{ApiError, ApiResult},
    extract::{RawBody, single_header},
    plane::Plane,
    state::AppState,
    store,
    telemetry::{Event, RequestMeta},
};

async fn principal(state: &AppState, plane: &Plane, headers: &HeaderMap) -> ApiResult<Principal> {
    auth::authenticate(
        plane,
        Bearer::required(headers)?,
        &state.0.config.accounts.app_id,
    )
    .await
}

/// `PUT`: raw JavaScript, `X-Peek-Drawing-Sha256` must match, ≤ 256 KiB.
pub(crate) async fn put(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    plane: Plane,
    headers: HeaderMap,
    RawBody(bytes): RawBody,
) -> ApiResult<Json<DrawingStored>> {
    let principal = principal(&state, &plane, &headers).await?;
    if let Some(content_type) = single_header(&headers, header::CONTENT_TYPE.as_str())? {
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if !matches!(mime.as_str(), "application/javascript" | "text/javascript") {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                ErrorCode::InvalidInput,
                format!("a drawing is sent as application/javascript, not `{mime}`"),
            ));
        }
    }
    if bytes.len() > DRAWING_MAX_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::DrawingTooLarge,
            format!(
                "the drawing is {} bytes; the limit is {DRAWING_MAX_BYTES} bytes (256 KiB)",
                bytes.len()
            ),
        )
        .with_hint("minify the script or move large data out of it"));
    }
    silicon_peek_client::schema::check_drawing_bytes("current", &bytes)
        .map_err(ApiError::from_client)?;
    let actual = hex::encode(Sha256::digest(&bytes));
    let declared = single_header(&headers, headers::DRAWING_SHA256)?.ok_or_else(|| {
        ApiError::invalid_input(
            "the X-Peek-Drawing-Sha256 header is required (hex SHA-256 of the body)",
        )
    })?;
    if !declared.eq_ignore_ascii_case(&actual) {
        return Err(ApiError::invalid_input(format!(
            "X-Peek-Drawing-Sha256 is {declared} but the body hashes to {actual}; the upload was altered or truncated"
        ))
        .with_hint("retry the upload"));
    }
    let now = unix_now();
    let (ctx, account, actor, sha, body) = (
        plane.ctx_string(),
        principal.account.to_string(),
        principal.actor.to_string(),
        actual.clone(),
        bytes.to_vec(),
    );
    plane
        .db
        .call(move |conn| {
            Ok(store::drawings::upsert(
                conn, &ctx, &account, &actor, &sha, &body, now,
            )?)
        })
        .await?;
    state.0.telemetry.record(
        &meta,
        Event::new("drawing.put", "drawing.put")
            .actor(&principal.account, &principal.actor)
            .context("drawing_bytes", bytes.len())
            .context("drawing_sha256", actual.clone()),
    );
    Ok(Json(DrawingStored {
        sha256: actual,
        bytes: bytes.len() as u64,
        updated_at: Timestamp::from_unix(now),
    }))
}

/// `GET`: the script with `ETag: "<sha256>"`, or `404 drawing_not_found`.
pub(crate) async fn get(
    State(state): State<AppState>,
    plane: Plane,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let principal = principal(&state, &plane, &headers).await?;
    let (ctx, account, actor) = (
        plane.ctx_string(),
        principal.account.to_string(),
        principal.actor.to_string(),
    );
    let drawing = plane
        .db
        .call(move |conn| Ok(store::drawings::get(conn, &ctx, &account, &actor)?))
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                ErrorCode::DrawingNotFound,
                format!("peek-server holds no drawing for {}", principal.actor),
            )
            .with_hint("register one: peek register drawing ./logo.js")
        })?;
    let etag = format!("\"{}\"", drawing.sha256);
    let not_modified = single_header(&headers, header::IF_NONE_MATCH.as_str())?.is_some_and(|v| {
        v.split(',')
            .any(|t| t.trim().trim_start_matches("W/") == etag)
    });
    let mut response = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        (StatusCode::OK, Body::from(drawing.bytes)).into_response()
    };
    let h = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    if let Ok(v) = HeaderValue::from_str(&drawing.sha256) {
        h.insert(headers::DRAWING_SHA256, v);
    }
    if let Ok(v) = HeaderValue::from_str(&Timestamp::from_unix(drawing.updated_at).to_rfc3339()) {
        h.insert("x-peek-drawing-updated-at", v);
    }
    if !not_modified {
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/javascript; charset=utf-8"),
        );
    }
    Ok(response)
}

/// `DELETE`: forgets the drawing (idempotent).
pub(crate) async fn delete(
    State(state): State<AppState>,
    plane: Plane,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let principal = principal(&state, &plane, &headers).await?;
    let (ctx, account, actor) = (
        plane.ctx_string(),
        principal.account.to_string(),
        principal.actor.to_string(),
    );
    plane
        .db
        .call(move |conn| Ok(store::drawings::delete(conn, &ctx, &account, &actor)?))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
