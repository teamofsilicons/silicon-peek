//! Request extractors that answer with the peek error envelope instead of
//! axum's plain-text rejections.

use std::net::{IpAddr, SocketAddr};

use axum::{
    body::Bytes,
    extract::{ConnectInfo, FromRequest, FromRequestParts, Request},
    http::{HeaderMap, StatusCode, header, request::Parts},
};
use serde::de::DeserializeOwned;
use silicon_peek_client::{ErrorCode, api::headers, ids::IdempotencyKey};

use crate::error::{ApiError, ApiResult};

/// The value of a header that may appear at most once.
pub(crate) fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> ApiResult<Option<&'a str>> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(ApiError::invalid_input(format!(
            "the {name} header was sent more than once; send it exactly once"
        )));
    }
    value
        .to_str()
        .map(Some)
        .map_err(|_| ApiError::invalid_input(format!("the {name} header is not visible ASCII")))
}

/// The raw body, with the size limit mapped to `413 payload_too_large`.
pub(crate) struct RawBody(pub(crate) Bytes);

impl<S: Send + Sync> FromRequest<S> for RawBody {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        Bytes::from_request(req, state)
            .await
            .map(Self)
            .map_err(|rejection| {
                if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    ApiError::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        ErrorCode::PayloadTooLarge,
                        "the request body exceeds this route's size limit",
                    )
                    .with_hint("send less data; limits are listed in the peek-server README")
                } else {
                    ApiError::invalid_input(format!(
                        "the request body could not be read: {rejection}"
                    ))
                }
            })
    }
}

/// A strictly parsed JSON body (duplicate keys and unknown fields refused
/// per the target type), plus its raw bytes.
pub(crate) struct JsonBody<T> {
    pub(crate) value: T,
    pub(crate) raw: Bytes,
}

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for JsonBody<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        if let Some(content_type) = single_header(req.headers(), header::CONTENT_TYPE.as_str())? {
            let mime = content_type
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if mime != "application/json" && !mime.ends_with("+json") {
                return Err(ApiError::new(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    ErrorCode::InvalidInput,
                    format!("this route takes application/json, not `{mime}`"),
                )
                .with_hint("send Content-Type: application/json"));
            }
        }
        let RawBody(raw) = RawBody::from_request(req, state).await?;
        let value = silicon_peek_client::json::from_slice::<T>(&raw, "the request body")
            .map_err(ApiError::from_client)?;
        Ok(Self { value, raw })
    }
}

/// The `Idempotency-Key` every POST must carry (except the telemetry
/// gateway and the ACCOUNTS webhook, which have their own dedupe).
pub(crate) struct IdemKey(pub(crate) IdempotencyKey);

impl<S: Send + Sync> FromRequestParts<S> for IdemKey {
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(idempotency_key(parts))
    }
}

fn idempotency_key(parts: &Parts) -> ApiResult<IdemKey> {
    let Some(value) = single_header(&parts.headers, headers::IDEMPOTENCY_KEY)? else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::IdempotencyKeyRequired,
            "this POST needs an Idempotency-Key header",
        )
        .with_hint("send Idempotency-Key: <16–255 visible ASCII>, and reuse it only to retry the exact same request"));
    };
    IdempotencyKey::parse(value)
        .map(IdemKey)
        .map_err(ApiError::from_client)
}

/// The client address for per-IP limits: the TCP peer, or — only when the
/// peer is the loopback reverse proxy (Caddy) — the last `X-Forwarded-For`
/// entry, which Caddy sets to the real client.
pub(crate) fn client_ip(parts: &Parts) -> String {
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    let forwarded = || {
        parts
            .headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .filter_map(|s| s.parse::<IpAddr>().ok())
            .next_back()
    };
    match peer {
        Some(ip) if ip.is_loopback() => forwarded().unwrap_or(ip).to_string(),
        Some(ip) => ip.to_string(),
        None => forwarded().map_or_else(|| "unknown".to_owned(), |ip| ip.to_string()),
    }
}

/// The client address (see [`client_ip`]).
pub(crate) struct ClientIp(pub(crate) String);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(Ok(Self(client_ip(parts))))
    }
}
