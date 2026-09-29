//! The HTTP error type.
//!
//! Every failure answers with the Ting-shaped envelope plus the request ID
//! (BLUEPRINT §5.2):
//!
//! ```json
//! {"error":{"code":"…","message":"…","hint":"…","retryable":false,"request_id":"…","details":{…}}}
//! ```
//!
//! Codes come from [`silicon_peek_client::ErrorCode`], so the CLI and peekd
//! map them to exit codes and retry decisions without a second table.

use std::fmt;

use axum::{
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use silicon_peek_client::{Error, ErrorCode};

tokio::task_local! {
    /// The request ID of the request being handled (set by the request
    /// middleware around every handler).
    pub(crate) static REQUEST_ID: String;
}

/// The current request's ID, when called inside a request.
pub(crate) fn current_request_id() -> Option<String> {
    REQUEST_ID.try_with(Clone::clone).ok()
}

/// A failed request: an HTTP status plus a peek error object.
#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    error: Error,
    retry_after: Option<u64>,
}

/// Result alias for handlers and services.
pub(crate) type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    /// An error with an explicit status and code.
    pub(crate) fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            error: Error::new(code, message),
            retry_after: None,
        }
    }

    /// Wraps an error produced by the shared client library (validation of
    /// wire types). The status follows the code's exit class.
    pub(crate) fn from_client(error: Error) -> Self {
        let status = match error.exit_code() {
            silicon_peek_client::ExitCode::Usage => {
                if *error.code() == ErrorCode::PayloadTooLarge
                    || *error.code() == ErrorCode::DrawingTooLarge
                {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    StatusCode::BAD_REQUEST
                }
            }
            silicon_peek_client::ExitCode::NotAuthenticated => StatusCode::UNAUTHORIZED,
            silicon_peek_client::ExitCode::Refused => StatusCode::CONFLICT,
            silicon_peek_client::ExitCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            silicon_peek_client::ExitCode::Internal | silicon_peek_client::ExitCode::Success => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        Self {
            status,
            error,
            retry_after: None,
        }
    }

    /// `400 invalid_input`.
    pub(crate) fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ErrorCode::InvalidInput, message)
    }

    /// `401 unauthenticated`.
    pub(crate) fn unauthenticated(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthenticated,
            message,
        )
        .with_hint(
            "refresh the session and retry; if it keeps failing, log in again: peek login '<SLT>'",
        )
    }

    /// `404 not_found`.
    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, ErrorCode::NotFound, message)
    }

    /// `500 internal_error`. The message is shown to the caller, so it never
    /// carries secrets; the detail goes to the log.
    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::InternalError,
            message,
        )
        .with_hint("this is a bug in peek-server; report it with `peek report \"<what you ran>\"` and quote the request ID")
    }

    /// `503 iam_misconfigured`: the operator must fix the app secret.
    pub(crate) fn iam_misconfigured(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::IamMisconfigured,
            message,
        )
        .with_hint("this is an operator problem, not yours: peek-server's IAM app secret (PEEK_IAM_APP_SECRET) is missing or no longer valid; retry later")
    }

    /// Adds a hint.
    pub(crate) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.error = self.error.with_hint(hint);
        self
    }

    /// Adds structured details.
    pub(crate) fn with_details(mut self, details: Value) -> Self {
        self.error = self.error.with_details(details);
        self
    }

    /// Overrides retryability.
    pub(crate) fn with_retryable(mut self, retryable: bool) -> Self {
        self.error = self.error.with_retryable(retryable);
        self
    }

    /// Sets `Retry-After` (seconds) and marks the error retryable.
    pub(crate) fn with_retry_after(mut self, seconds: u64) -> Self {
        self.retry_after = Some(seconds);
        self.error = self.error.with_retryable(true);
        self
    }

    /// The HTTP status.
    #[cfg(test)]
    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }

    /// The machine code.
    pub(crate) fn code(&self) -> &ErrorCode {
        self.error.code()
    }

    /// The message.
    pub(crate) fn message(&self) -> &str {
        self.error.message()
    }

    /// `Retry-After` seconds, if set.
    #[cfg(test)]
    pub(crate) fn retry_after(&self) -> Option<u64> {
        self.retry_after
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.status.as_u16(), self.error)
    }
}

impl std::error::Error for ApiError {}

impl From<rusqlite::Error> for ApiError {
    fn from(error: rusqlite::Error) -> Self {
        tracing::error!(error = %error, "database statement failed");
        Self::internal("peek-server could not read or write its database")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let request_id = current_request_id();
        if self.status.is_server_error() {
            tracing::warn!(
                status = self.status.as_u16(),
                code = %self.error.code(),
                error_message = self.error.message(),
                request_id = request_id.as_deref().unwrap_or_default(),
                "request failed"
            );
        }
        let object = self.error.with_request_id(request_id).to_object();
        let mut response = (self.status, axum::Json(json!({ "error": object }))).into_response();
        if let Some(seconds) = self.retry_after
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn envelope_carries_the_request_id_and_retry_after()
    -> Result<(), Box<dyn std::error::Error>> {
        let error = ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::TingUnavailable,
            "Ting is busy",
        )
        .with_retry_after(7)
        .with_details(json!({"retry_after": 7}));
        let response = REQUEST_ID
            .scope("req-1".to_owned(), async move { error.into_response() })
            .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("7")
        );
        let body = axum::body::to_bytes(response.into_body(), 1 << 20).await?;
        let value: Value = serde_json::from_slice(&body)?;
        assert_eq!(value["error"]["code"], "ting_unavailable");
        assert_eq!(value["error"]["retryable"], true);
        assert_eq!(value["error"]["request_id"], "req-1");
        assert_eq!(value["error"]["details"]["retry_after"], 7);
        Ok(())
    }

    #[test]
    fn client_errors_map_to_statuses_by_exit_class() {
        let e = ApiError::from_client(Error::invalid_input("bad"));
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
        let e = ApiError::from_client(Error::new(ErrorCode::PayloadTooLarge, "big"));
        assert_eq!(e.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let e = ApiError::from_client(Error::new(ErrorCode::TestingSecretInvalid, "no"));
        assert_eq!(e.status(), StatusCode::UNAUTHORIZED);
    }
}
