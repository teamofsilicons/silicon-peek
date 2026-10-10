//! `GET/PUT/DELETE /api/v1/accounts/{account}/byo/deepgram`: an account's own Deepgram
//! legacy key (unused by speech). Any member may read the status; changes need `account_role` owner or
//! admin (disclosed by `profile`). The key is never returned.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use silicon_peek_client::{
    ErrorCode, Secret,
    api::{ByoDeepgramRequest, ByoStatus},
    timestamp::{Timestamp, unix_now},
};

use crate::{
    auth::{self, Bearer, Principal},
    crypto::byo_aad,
    deepgram,
    error::{ApiError, ApiResult},
    extract::JsonBody,
    plane::Plane,
    state::AppState,
    store,
};

async fn principal_for(
    state: &AppState,
    plane: &Plane,
    headers: &HeaderMap,
    account: &str,
) -> ApiResult<Principal> {
    let principal = auth::authenticate(
        plane,
        Bearer::required(headers)?,
        &state.0.config.accounts.app_id,
    )
    .await?;
    if principal.account.as_str() != account {
        return Err(ApiError::invalid_input(format!(
            "the path names account `{account}` but the session is for account `{}` (X-Account-ID)",
            principal.account
        )));
    }
    Ok(principal)
}

async fn status(plane: &Plane, account: String) -> ApiResult<ByoStatus> {
    let ctx = plane.ctx_string();
    let row = plane
        .db
        .call(move |conn| Ok(store::byo::get(conn, &ctx, &account)?))
        .await?;
    Ok(match row {
        Some(row) => ByoStatus {
            configured: true,
            updated_at: Some(Timestamp::from_unix(row.updated_at)),
            base_url: row.base_url,
        },
        None => ByoStatus {
            configured: false,
            updated_at: None,
            base_url: None,
        },
    })
}

/// `GET`: whether the account has a key.
pub(crate) async fn get(
    State(state): State<AppState>,
    plane: Plane,
    Path(account): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<ByoStatus>> {
    principal_for(&state, &plane, &headers, &account).await?;
    status(&plane, account).await.map(Json)
}

/// `PUT {"api_key","base_url"?}`: validates the key with Deepgram, then
/// stores it sealed.
pub(crate) async fn put(
    State(state): State<AppState>,
    plane: Plane,
    Path(account): Path<String>,
    headers: HeaderMap,
    body: JsonBody<ByoDeepgramRequest>,
) -> ApiResult<Json<ByoStatus>> {
    let principal = principal_for(&state, &plane, &headers, &account).await?;
    let request = body.value;
    let key = request.api_key.expose().trim();
    if !(16..=512).contains(&key.len()) || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ApiError::invalid_input(
            "api_key must be a Deepgram API key: 16–512 visible ASCII characters",
        ));
    }
    // Validate legacy keys only on allowed HTTPS hosts (Deepgram's own by
    // default), never an IP address or localhost.
    let base_url = match request.base_url.as_deref() {
        None => None,
        Some(url) => Some(
            state
                .0
                .config
                .deepgram
                .byo_hosts
                .check_origin(url)
                .map_err(|why| {
                    ApiError::invalid_input(format!("base_url {why}"))
                        .with_hint("use a Deepgram origin such as https://api.eu.deepgram.com, or leave --base-url out for https://api.deepgram.com")
                        .with_details(serde_json::json!({"reason": "base_url_not_allowed"}))
                })?,
        ),
    };
    let effective_base = base_url
        .clone()
        .unwrap_or_else(|| state.0.config.deepgram.base_url.clone());
    let key = Secret::new(key);
    deepgram::validate_key(&state, &effective_base, &key).await?;
    let ctx = plane.ctx_string();
    let sealed = state
        .0
        .sealer
        .seal(&byo_aad(&ctx, &account), key.expose().as_bytes())?;
    let (account_db, by) = (account.clone(), principal.actor.to_string());
    plane
        .db
        .call(move |conn| {
            Ok(store::byo::put(
                conn,
                &ctx,
                &account_db,
                &sealed,
                base_url.as_deref(),
                unix_now(),
                &by,
            )?)
        })
        .await?;
    tracing::info!(account = %account, "account configured its own Deepgram key");
    status(&plane, account).await.map(Json)
}

/// `DELETE`: removes the stored legacy key; active speech routing is unchanged.
pub(crate) async fn delete(
    State(state): State<AppState>,
    plane: Plane,
    Path(account): Path<String>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let principal = principal_for(&state, &plane, &headers, &account).await?;
    let ctx = plane.ctx_string();
    plane
        .db
        .call(move |conn| Ok(store::byo::delete(conn, &ctx, &account)?))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
