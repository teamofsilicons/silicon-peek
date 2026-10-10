//! SQL for every table (BLUEPRINT §5.2). Functions take a borrowed
//! connection or transaction and run inside [`crate::db::Db::call`].
//!
//! Every data row is keyed by `ctx`, and callers only ever pass the `ctx` of
//! the plane they resolved from a validated credential.

pub(crate) mod byo;
pub(crate) mod deliveries;
pub(crate) mod drawings;
pub(crate) mod enrollments;
pub(crate) mod idempotency;
pub(crate) mod reports;
pub(crate) mod webhooks;

use rusqlite::{Connection, params};

/// Idempotency records are kept 24 hours; webhook dedupe records 30 days
/// (ACCOUNTS retries for far less than either).
pub(crate) fn gc(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    let idem = conn.execute(
        "DELETE FROM idempotency WHERE created_at < ?1",
        params![now - 24 * 3600],
    )?;
    let hooks = conn.execute(
        "DELETE FROM webhook_events WHERE received_at < ?1",
        params![now - 30 * 24 * 3600],
    )?;
    // Keep a tombstone so an old credential is never submitted upstream twice.
    let tokens = conn.execute(
        "UPDATE account_token_exchanges SET state='uncertain',response=NULL WHERE response IS NOT NULL AND expires_at <= ?1",
        [now],
    )?;
    Ok(idem + hooks + tokens)
}
