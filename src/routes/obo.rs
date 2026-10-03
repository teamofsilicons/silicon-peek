//! Explicit Ting permission, separate from ordinary login. Manual-code flow only.
use crate::{
    auth::{self, Bearer},
    error::ApiResult,
    extract::{IdemKey, JsonBody},
    obo,
    plane::Plane,
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Complete {
    code: String,
}

pub(crate) async fn start(
    State(state): State<AppState>,
    plane: Plane,
    IdemKey(key): IdemKey,
    headers: HeaderMap,
    _body: JsonBody<Empty>,
) -> ApiResult<Json<Value>> {
    let principal = auth::authenticate(
        &plane,
        Bearer::required(&headers)?,
        &state.0.config.iam.app_id,
    )
    .await?;
    principal.require_scopes(&["self.identity.read"])?;
    Ok(Json(
        obo::start(&state, &plane, &principal, key.as_str()).await?,
    ))
}
pub(crate) async fn status(
    State(state): State<AppState>,
    plane: Plane,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let principal = auth::authenticate(
        &plane,
        Bearer::required(&headers)?,
        &state.0.config.iam.app_id,
    )
    .await?;
    Ok(Json(obo::status(&state, &plane, &principal, id).await?))
}
pub(crate) async fn complete(
    State(state): State<AppState>,
    plane: Plane,
    Path(id): Path<Uuid>,
    IdemKey(_key): IdemKey,
    headers: HeaderMap,
    body: JsonBody<Complete>,
) -> ApiResult<Json<Value>> {
    let principal = auth::authenticate(
        &plane,
        Bearer::required(&headers)?,
        &state.0.config.iam.app_id,
    )
    .await?;
    Ok(Json(
        obo::complete(&state, &plane, &principal, id, &body.value.code).await?,
    ))
}
