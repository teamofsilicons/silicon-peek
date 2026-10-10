//! Signed ACCOUNTS notifications, keyed by immutable account UUIDs.
use crate::{
    error::{ApiError, ApiResult},
    extract::single_header,
    state::AppState,
};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use silicon_accounts_client::{
    DEFAULT_WEBHOOK_TOLERANCE, SIGNATURE_HEADER, TIMESTAMP_HEADER, WebhookPayload,
    verify_and_parse_webhook,
};
use silicon_peek_client::{ErrorCode, timestamp::unix_now};

pub(crate) async fn accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let stamp = single_header(&headers, TIMESTAMP_HEADER)?
        .ok_or_else(|| ApiError::invalid_input("Webhook timestamp is required"))?;
    let signature = single_header(&headers, SIGNATURE_HEADER)?
        .ok_or_else(|| ApiError::invalid_input("Webhook signature is required"))?;
    let event = state
        .0
        .config
        .accounts
        .webhook_keys
        .iter()
        .find_map(|(_, secret)| {
            verify_and_parse_webhook(
                secret.expose(),
                stamp,
                signature,
                &body,
                DEFAULT_WEBHOOK_TOLERANCE,
            )
            .ok()
        })
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                ErrorCode::Unauthenticated,
                "Webhook signature is invalid",
            )
        })?;
    if event.app_id.as_deref() != Some("peek") {
        return Err(ApiError::unauthenticated(
            "Webhook audience does not match Peek",
        ));
    }
    state.0.production_db.call(move|c|{
        let tx=c.transaction()?;
        if tx.execute("INSERT OR IGNORE INTO webhook_events(event_id,ctx,event_type,received_at) VALUES(?1,'production',?2,?3)",rusqlite::params![event.event_id,event.event_type,unix_now()])?==0{return Ok(())}
        match event.payload {
            WebhookPayload::AccountIdChanged(change)=>{
                for table in ["drawings","ting_enrollments","deliveries","reports"]{
                    tx.execute(&format!("UPDATE {table} SET actor_id=?2 WHERE account_id=?1"),rusqlite::params![change.uuid,change.new_id])?;
                }
            }
            WebhookPayload::AccountDeleted(change)|WebhookPayload::MembershipAccessRemoved(change)=>{
                for table in ["drawings","ting_enrollments","deliveries","byo_keys","reports"]{
                    tx.execute(&format!("DELETE FROM {table} WHERE account_id=?1"),[&change.uuid])?;
                }
            }
            _=>{}
        }
        tx.commit()?;Ok(())
    }).await?;
    Ok(StatusCode::NO_CONTENT)
}
