//! Explicit Ting enrollment shared by the CLI and native settings.
use super::Store;
use crate::{
    Error, ErrorCode, Result, http::Client, identity::SlotKey, ids::IdempotencyKey,
    timestamp::unix_now,
};

pub(super) fn forget_context(_store: &Store, _context_id: &str) -> Result<()> {
    Ok(())
}

/// Explicit local permission action.
#[derive(Clone, Debug)]
pub enum Action {
    /// Enable Ting delivery.
    Enroll(Option<IdempotencyKey>),
}

/// Apply an enrollment action while the selected login is locked.
pub async fn perform(
    store: &Store,
    client: &Client,
    key: &SlotKey,
    expected_context: &str,
    action: Action,
) -> Result<()> {
    let lock = store.lock_async().await?;
    let mut file = store.read_session()?;
    let slot = file.usable_slot(key, store.dir())?;
    if slot.context_id()? != expected_context || client.api_url() != key.api_url() {
        return Err(Error::new(
            ErrorCode::SessionRejected,
            "The selected account changed",
        ));
    }
    let enroll_key = match action {
        Action::Enroll(key) => key.unwrap_or(IdempotencyKey::parse(&format!(
            "peek-enroll-{expected_context}"
        ))?),
    };
    let enrolled = client
        .with_session(slot.access_token.clone(), slot.account_id.clone())
        .ting_enroll(&enroll_key)
        .await?;
    let current = file.slots.get_mut(&key.as_string()).ok_or_else(|| {
        Error::new(
            ErrorCode::SessionRejected,
            "The selected account disappeared",
        )
    })?;
    current.ting = Some(super::session::SlotTing {
        subscribed: enrolled.subscribed,
        subscription_id: Some(enrolled.subscription_id),
        registered_at: Some(unix_now()),
        error: None,
    });
    store.write_session(&lock, &file)?;
    Ok(())
}
