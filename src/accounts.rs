//! ACCOUNTS app credentials and live token verification.

use axum::http::StatusCode;
use silicon_accounts_client::{AccountsClient, AppClient, Error};
use silicon_peek_client::{ErrorCode, Secret};

use crate::{config::AccountsConfig, error::ApiError};

pub(crate) struct Accounts {
    client: AccountsClient,
    app_id: String,
    secret: Secret,
}

impl Accounts {
    pub(crate) fn new(config: &AccountsConfig) -> Result<Option<Self>, Error> {
        let Some(secret) = config.app_secret.clone() else {
            return Ok(None);
        };
        let client = AccountsClient::builder()
            .base_url(&config.base_url)
            .timeout(config.request_timeout)
            .max_retries(0)
            .telemetry(config.sdk_telemetry)
            .build()?;
        Ok(Some(Self {
            client,
            app_id: config.app_id.clone(),
            secret,
        }))
    }

    pub(crate) fn app(&self) -> AppClient<'_> {
        self.client.as_app(&self.app_id, self.secret.expose())
    }
}

pub(crate) fn api_code(error: &Error) -> Option<(&str, u16)> {
    match error {
        Error::Api(e) => Some((&e.code, e.status)),
        Error::OAuth(e) => Some((&e.error, e.status)),
        _ => None,
    }
}

pub(crate) fn upstream(error: &Error, operation: &str, _testing: bool) -> ApiError {
    if api_code(error).is_some_and(|(code, _)| code == "invalid_client") {
        return ApiError::accounts_misconfigured("ACCOUNTS rejected Peek's app credentials");
    }
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::AccountsUnavailable,
        format!("ACCOUNTS could not {operation}"),
    )
    .with_details(serde_json::json!({"accounts_code": api_code(error).map(|(code,_)|code)}))
    .with_retry_after(2)
}

pub(crate) fn token_shape_ok(value: &str, prefix: &str) -> bool {
    let shape = if prefix == "jwt" {
        value.split('.').count() == 3
    } else {
        value.starts_with(prefix)
    };
    shape && (16..=16384).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_graphic())
}
