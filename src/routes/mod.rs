//! HTTP handlers, one module per route family (BLUEPRINT §5.2 route table).

pub(crate) mod auth;
pub(crate) mod byo;
pub(crate) mod discovery;
pub(crate) mod drawings;
pub(crate) mod health;
pub(crate) mod participant;
pub(crate) mod obo;
pub(crate) mod reports;
pub(crate) mod speech;
pub(crate) mod telemetry;
pub(crate) mod ting;
pub(crate) mod webhooks;

use axum::http::{Method, StatusCode, Uri};
use silicon_peek_client::ErrorCode;

use crate::error::ApiError;

const ROUTES_HINT: &str =
    "the routes are listed in src/README.md of https://github.com/teamofsilicons/silicon-peek";

/// Unknown path.
#[allow(clippy::unused_async)] // axum handlers are async by contract
pub(crate) async fn not_found(uri: Uri) -> ApiError {
    ApiError::not_found(format!("peek-server has no route {}", uri.path())).with_hint(ROUTES_HINT)
}

/// Known path, wrong method.
#[allow(clippy::unused_async)] // axum handlers are async by contract
pub(crate) async fn method_not_allowed(method: Method, uri: Uri) -> ApiError {
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        ErrorCode::InvalidInput,
        format!("{} does not accept {method}", uri.path()),
    )
    .with_hint(ROUTES_HINT)
}
