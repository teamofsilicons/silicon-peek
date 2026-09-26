//! The HTTP router and the request middleware.
//!
//! Every response carries `X-Request-ID` and `Cache-Control: no-store`
//! (`Pragma: no-cache` too on auth routes); every error — including axum's
//! own rejections, 404/405 and timeouts — uses the peek error envelope with
//! that request ID. Requests are logged as one JSON line each (never headers
//! or bodies) and recorded as `http.completed` telemetry.

use std::time::{Duration, Instant};

use axum::{
    Router,
    extract::{DefaultBodyLimit, MatchedPath, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use silicon_peek_client::{
    ErrorCode,
    api::{headers, routes as paths},
    telemetry::is_off_value,
};
use tower_http::timeout::TimeoutLayer;
use uuid::Uuid;

use crate::{
    error::{ApiError, REQUEST_ID},
    routes,
    state::AppState,
    telemetry::{Event, RequestMeta},
};

/// The largest request body a route accepts, except the speech proxy's
/// `POST /api/v1/speech/listen` (4 MiB of recorded audio).
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Server-side limit for one request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Builds the router with every route of BLUEPRINT §5.2.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(paths::HEALTHZ, get(routes::health::healthz))
        .route(paths::READYZ, get(routes::health::readyz))
        .route(paths::IAM, get(routes::discovery::iam))
        .route(paths::LOGIN, post(routes::auth::login))
        .route(paths::REFRESH, post(routes::auth::refresh))
        .route(paths::LOGOUT, post(routes::auth::logout))
        .route(paths::ME, get(routes::auth::me))
        .route(paths::TING_RECIPIENT, post(routes::ting::enroll))
        .route(paths::DELIVERIES, post(routes::ting::deliver))
        .route(paths::SPEECH_TOKEN, post(routes::speech::token))
        .route(paths::SPEECH_SPEAK, post(routes::speech::speak))
        .route(
            paths::SPEECH_LISTEN,
            post(routes::speech::listen)
                .layer(DefaultBodyLimit::max(silicon_peek_client::api::LISTEN_MAX_BYTES)),
        )
        .route(
            paths::DRAWING,
            put(routes::drawings::put)
                .get(routes::drawings::get)
                .delete(routes::drawings::delete),
        )
        .route(
            "/api/v1/orgs/{org}/byo/deepgram",
            get(routes::byo::get)
                .put(routes::byo::put)
                .delete(routes::byo::delete),
        )
        .route(paths::REPORTS, post(routes::reports::create))
        .route(paths::WEB_TELEMETRY, post(routes::telemetry::ingest))
        .route(paths::IAM_WEBHOOK, post(routes::webhooks::iam))
        .route(
            "/internal/honeycomb/organizations/{org}/testing-environments/{environment}/operations/{operation}",
            put(routes::participant::apply).get(routes::participant::receipt),
        )
        .fallback(routes::not_found)
        .method_not_allowed_fallback(routes::method_not_allowed)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            REQUEST_TIMEOUT,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), request_context))
        .with_state(state)
}

fn trace_id(value: Option<&HeaderValue>) -> Option<String> {
    let v = value?.to_str().ok()?;
    ((1..=64).contains(&v.len())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then(|| v.to_owned())
}

/// Turns a non-JSON error response (an axum rejection, the timeout layer)
/// into the peek envelope.
async fn normalize(response: Response) -> Response {
    let status = response.status();
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !(status.is_client_error() || status.is_server_error()) || is_json {
        return response;
    }
    let text = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .ok()
        .and_then(|b| String::from_utf8(b.to_vec()).ok())
        .map(|t| t.trim().to_owned())
        .unwrap_or_default();
    let error = match status {
        StatusCode::SERVICE_UNAVAILABLE | StatusCode::REQUEST_TIMEOUT => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::BackendUnavailable,
            format!(
                "peek-server did not finish the request within {} s",
                REQUEST_TIMEOUT.as_secs()
            ),
        )
        .with_hint("retry the same request; idempotent requests replay their original result")
        .with_retry_after(5),
        StatusCode::PAYLOAD_TOO_LARGE => ApiError::new(
            status,
            ErrorCode::PayloadTooLarge,
            "the request body is too large",
        ),
        s if s.is_client_error() => ApiError::new(
            s,
            ErrorCode::InvalidInput,
            if text.is_empty() {
                format!("the request was refused with HTTP {s}")
            } else {
                format!("the request was refused: {text}")
            },
        ),
        s => ApiError::new(
            s,
            ErrorCode::InternalError,
            format!("peek-server failed with HTTP {s}"),
        ),
    };
    error.into_response()
}

async fn request_context(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let request_id = Uuid::now_v7().to_string();
    let started = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned());
    let meta = RequestMeta {
        request_id: request_id.clone(),
        telemetry: !req
            .headers()
            .get(headers::TELEMETRY)
            .and_then(|v| v.to_str().ok())
            .is_some_and(is_off_value),
        testing: req.headers().contains_key(headers::TESTING_KEY),
        trace_id: trace_id(req.headers().get(headers::TRACE_ID)),
    };
    req.extensions_mut().insert(meta.clone());
    let mut response = REQUEST_ID
        .scope(request_id.clone(), async move {
            let response = next.run(req).await;
            normalize(response).await
        })
        .await;
    let h = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&request_id) {
        h.insert(headers::REQUEST_ID, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if path.starts_with("/api/v1/auth/") {
        h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    }
    let status = response.status().as_u16();
    let elapsed = started.elapsed();
    let route_label = route.as_deref().unwrap_or("(unmatched)");
    tracing::info!(
        request_id = %request_id,
        method = %method,
        route = route_label,
        status,
        duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        "request"
    );
    let skip = matches!(
        route.as_deref(),
        Some(paths::WEB_TELEMETRY | paths::HEALTHZ | paths::READYZ) | None
    );
    if !skip {
        state.0.telemetry.record(
            &meta,
            Event::new("http.completed", "http")
                .duration(elapsed)
                .context("http_route", route_label)
                .context("method", method.as_str())
                .context("status", status),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_ids_are_validated() {
        assert_eq!(
            trace_id(Some(&HeaderValue::from_static("0192-abc_D"))),
            Some("0192-abc_D".to_owned())
        );
        assert_eq!(trace_id(Some(&HeaderValue::from_static("a b"))), None);
        assert_eq!(trace_id(None), None);
    }

    #[tokio::test]
    async fn plain_rejections_become_envelopes() -> Result<(), Box<dyn std::error::Error>> {
        let plain = (StatusCode::BAD_REQUEST, "Invalid URL: bad uuid").into_response();
        let response = normalize(plain).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), 1 << 16).await?;
        let v: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(v["error"]["code"], "invalid_input");
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("bad uuid")
        );
        let timeout = normalize(StatusCode::SERVICE_UNAVAILABLE.into_response()).await;
        let body = axum::body::to_bytes(timeout.into_body(), 1 << 16).await?;
        let v: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(v["error"]["code"], "backend_unavailable");
        assert_eq!(v["error"]["retryable"], true);
        Ok(())
    }
}
