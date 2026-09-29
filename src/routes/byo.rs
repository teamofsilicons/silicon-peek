//! `GET/PUT/DELETE /api/v1/orgs/{org}/byo/deepgram`: an org's own Deepgram
//! legacy key (unused by speech). Any member may read the status; changes need `org_role` owner or
//! admin (disclosed by `self.membership.read`). The key is never returned.

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
    org: &str,
) -> ApiResult<Principal> {
    let principal = auth::authenticate(
        plane,
        Bearer::required(headers)?,
        &state.0.config.iam.app_id,
    )
    .await?;
    if principal.org.as_str() != org {
        return Err(ApiError::invalid_input(format!(
            "the path names org `{org}` but the session is for org `{}` (X-Org-ID)",
            principal.org
        )));
    }
    Ok(principal)
}

fn require_admin(principal: &Principal) -> ApiResult<()> {
    if principal.is_org_admin() {
        return Ok(());
    }
    let (message, hint) = match principal.org_role.as_deref() {
        None => (
            format!(
                "changing org `{}`'s Deepgram key needs an owner or admin, and IAM did not disclose this session's org role",
                principal.org
            ),
            "log in again approving self.membership.read so peek can see your role",
        ),
        Some(role) => (
            format!(
                "changing org `{}`'s Deepgram key needs an owner or admin; this session's role is `{role}`",
                principal.org
            ),
            "ask an org owner or admin to set it",
        ),
    };
    Err(ApiError::new(StatusCode::FORBIDDEN, ErrorCode::NotOrgAdmin, message).with_hint(hint))
}

async fn status(plane: &Plane, org: String) -> ApiResult<ByoStatus> {
    let ctx = plane.ctx_string();
    let row = plane
        .db
        .call(move |conn| Ok(store::byo::get(conn, &ctx, &org)?))
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

/// `GET`: whether the org has a key.
pub(crate) async fn get(
    State(state): State<AppState>,
    plane: Plane,
    Path(org): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<ByoStatus>> {
    principal_for(&state, &plane, &headers, &org).await?;
    status(&plane, org).await.map(Json)
}

/// `PUT {"api_key","base_url"?}`: validates the key with Deepgram, then
/// stores it sealed.
pub(crate) async fn put(
    State(state): State<AppState>,
    plane: Plane,
    Path(org): Path<String>,
    headers: HeaderMap,
    body: JsonBody<ByoDeepgramRequest>,
) -> ApiResult<Json<ByoStatus>> {
    let principal = principal_for(&state, &plane, &headers, &org).await?;
    require_admin(&principal)?;
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
        .seal(&byo_aad(&ctx, &org), key.expose().as_bytes())?;
    let (org_db, by) = (org.clone(), principal.actor.to_string());
    plane
        .db
        .call(move |conn| {
            Ok(store::byo::put(
                conn,
                &ctx,
                &org_db,
                &sealed,
                base_url.as_deref(),
                unix_now(),
                &by,
            )?)
        })
        .await?;
    tracing::info!(org = %org, "org configured its own Deepgram key");
    status(&plane, org).await.map(Json)
}

/// `DELETE`: removes the stored legacy key; active speech routing is unchanged.
pub(crate) async fn delete(
    State(state): State<AppState>,
    plane: Plane,
    Path(org): Path<String>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let principal = principal_for(&state, &plane, &headers, &org).await?;
    require_admin(&principal)?;
    let ctx = plane.ctx_string();
    plane
        .db
        .call(move |conn| Ok(store::byo::delete(conn, &ctx, &org)?))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
