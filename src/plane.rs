//! One production data store, keyed by the verified Carbon or Silicon account.
use crate::{
    accounts::Accounts,
    db::Db,
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{extract::FromRequestParts, http::request::Parts};
use silicon_peek_client::identity::{Context, DataContext};
use std::sync::Arc;

pub(crate) struct Plane {
    pub(crate) ctx: Context,
    pub(crate) db: Db,
    accounts: Option<Arc<Accounts>>,
}
impl Plane {
    pub(crate) fn accounts(&self) -> ApiResult<Arc<Accounts>> {
        self.accounts.clone().ok_or_else(|| {
            ApiError::accounts_misconfigured(
                "PEEK_ACCOUNTS_APP_SECRET is required to sign in through ACCOUNTS",
            )
        })
    }
    pub(crate) fn ctx_string(&self) -> String {
        self.ctx.as_string()
    }
    pub(crate) fn data_context(&self) -> DataContext {
        DataContext::Production
    }
}
impl FromRequestParts<AppState> for Plane {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        if parts.headers.contains_key("x-testing-environment-key")
            || parts
                .headers
                .contains_key("x-testing-environment-generation")
        {
            return Err(ApiError::invalid_input(
                "Testing environments are no longer supported",
            ));
        }
        Ok(Self {
            ctx: Context::Production,
            db: state.0.production_db.clone(),
            accounts: state.0.accounts.clone(),
        })
    }
}
