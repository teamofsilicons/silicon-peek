//! Stateful pieces shared by the CLI and peekd (feature `runtime`).
//!
//! - [`store`] holds the per-home store `$SILICON_HOME/.peek` (0700
//!   directory, 0600 files, `O_EXCL`, symlink refusal, atomic writes,
//!   `session.lock`).
//! - [`session`] is the `session.json` schema v1.
//! - [`refresh`] has [`fresh_session`], the single refresh path.
//! - [`login`] does login, recovery, logout and revocation bookkeeping.
//! - [`testing`] is the `testing.json` schema.
//! - [`daemon`] is the peekd IPC client with its socket helpers.
//! - [`authenticate_home`] runs peekd's three checks on a CLI `auth` block.

pub mod authorization;
pub mod daemon;
pub mod fs;
pub mod login;
pub mod refresh;
pub mod session;
pub mod store;
pub mod sys;

use std::path::Path;

pub use refresh::{
    CLI_MARGIN, DELIVERY_MARGIN, PREWARM_MARGIN, RefreshPolicy, force_refresh, fresh_session,
    fresh_session_with,
};
pub use session::{SessionFile, SessionSlot};
pub use store::{Store, StoreLock};

use crate::{
    error::{Error, ErrorCode, Result},
    identity::{AccountId, ActorId, SlotKey},
    ipc::AuthBlock,
};

/// A CLI home that passed peekd's checks.
#[derive(Clone, Debug)]
pub struct VerifiedHome {
    /// The store.
    pub store: Store,
    /// The verified slot.
    pub slot_key: SlotKey,
    /// The Silicon, from the store slot (never from request fields).
    pub actor_id: ActorId,
    /// Its account, from the store slot.
    pub account_id: AccountId,
    /// Its display name, when known.
    pub display_name: Option<String>,
    /// Whether the slot records an active Ting enrollment (`None` when it
    /// records none).
    pub ting_subscribed: Option<bool>,
}

/// peekd's authentication of a CLI request (BLUEPRINT §1.6), in order:
///
/// 1. `realpath(home)` is a directory owned by this user with `mode & 077 == 0`;
/// 2. `<home>/daemon-token` equals `home_token` (constant-time);
/// 3. `<home>/session.json` has an unrejected slot `"<api_url>#<context>"`.
///
/// Identity comes from that slot. Naming another home does not authenticate.
///
/// # Errors
/// `invalid_silicon_home`, `home_token_mismatch`, `not_logged_in` or
/// `session_rejected`.
pub fn authenticate_home(auth: &AuthBlock) -> Result<VerifiedHome> {
    let store = Store::open_existing(Path::new(&auth.home))?;
    let token = store.daemon_token()?.ok_or_else(|| {
        Error::new(
            ErrorCode::NotLoggedIn,
            format!(
                "{} has no daemon-token; this home never logged in",
                store.dir().display()
            ),
        )
        .with_hint(session::RELOGIN_HINT)
    })?;
    if !token.ct_eq(auth.home_token.expose()) {
        return Err(Error::new(
            ErrorCode::HomeTokenMismatch,
            format!(
                "the presented home token does not match {}",
                store.path(store::DAEMON_TOKEN_FILE).display()
            ),
        )
        .with_hint("run the command from the Silicon's own home (SILICON_HOME), or log in again"));
    }
    let slot_key = SlotKey::new(auth.api_url.clone(), auth.context);
    let file = store.read_session()?;
    let slot = file.usable_slot(&slot_key, store.dir())?;
    if auth.context_id.as_deref() != Some(slot.context_id()?) {
        return Err(Error::new(
            ErrorCode::SessionRejected,
            "the login context changed; retry from the selected account",
        ));
    }
    Ok(VerifiedHome {
        actor_id: slot.actor.public_id.clone(),
        account_id: slot.account_id.clone(),
        display_name: slot.display_name.clone(),
        ting_subscribed: slot.ting.as_ref().map(|t| t.subscribed),
        slot_key,
        store,
    })
}

/// Builds the `auth` block a CLI sends for `store` and `slot_key`.
///
/// # Errors
/// `not_logged_in` when the home has no daemon-token yet.
pub fn auth_block(store: &Store, slot_key: &SlotKey) -> Result<AuthBlock> {
    let token = store.daemon_token()?.ok_or_else(|| {
        Error::new(
            ErrorCode::NotLoggedIn,
            format!(
                "{} has no daemon-token; log in first",
                store.dir().display()
            ),
        )
        .with_hint(session::RELOGIN_HINT)
    })?;
    let file = store.read_session()?;
    let context_id = file
        .usable_slot(slot_key, store.dir())?
        .context_id()?
        .to_owned();
    Ok(AuthBlock {
        home: store.dir().to_string_lossy().into_owned(),
        home_token: token,
        api_url: slot_key.api_url().clone(),
        context: slot_key.context(),
        context_id: Some(context_id),
    })
}
