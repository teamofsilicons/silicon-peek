//! Short-lived ACCOUNTS user-verification proofs for Ting.
use crate::{
    accounts::upstream,
    auth::Principal,
    error::{ApiError, ApiResult},
    plane::Plane,
    state::AppState,
    store,
};
use axum::http::StatusCode;
use silicon_accounts_client::{IssueUserVerification, IssuedProof, ProofKind};
use silicon_peek_client::ErrorCode;

pub(crate) fn required() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        ErrorCode::ReconsentRequired,
        "Enable Ting delivery for this account first",
    )
    .with_hint("Run `peek ting enroll` or enable Ting delivery in Peek settings")
}

pub(crate) async fn access(
    _state: &AppState,
    plane: &Plane,
    principal: &Principal,
    endpoint: &str,
    _operation: &str,
    _body: &[u8],
    _force_refresh: bool,
) -> ApiResult<IssuedProof> {
    if endpoint == "tings.send" {
        let (ctx, account, actor) = (
            plane.ctx_string(),
            principal.account.to_string(),
            principal.actor.to_string(),
        );
        let enrolled = plane
            .db
            .call(move |c| Ok(store::enrollments::get(c, &ctx, &account, &actor)?))
            .await?;
        if enrolled.is_none_or(|e| !e.active()) {
            return Err(required());
        }
    }
    let accounts = plane.accounts()?;
    let request = IssueUserVerification {
        subject_token: principal.access_token.expose().to_owned(),
        receiving_app: "ting".to_owned(),
        scopes: vec![endpoint.to_owned()],
        access_ttl_seconds: Some(600),
    };
    let proof = accounts
        .app()
        .issue_user_verification(&request, None)
        .await
        .map_err(|e| upstream(&e, "authorize Ting delivery", false))?;
    if proof.kind != Some(ProofKind::UserVerification)
        || proof.issuing_app.as_deref() != Some("peek")
        || proof.receiving_app.as_deref() != Some("ting")
        || proof
            .user
            .as_ref()
            .is_none_or(|u| u.uuid != principal.account.as_str())
        || !proof.scopes.iter().any(|s| s == endpoint)
    {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            ErrorCode::AccountsUnavailable,
            "Accounts returned a proof for a different account or app",
        )
        .with_retry_after(30));
    }
    Ok(proof)
}
