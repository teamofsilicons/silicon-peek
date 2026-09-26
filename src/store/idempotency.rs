//! `idempotency`: stored responses of idempotent POSTs (logic in
//! [`crate::idempotency`]).

use rusqlite::{Connection, OptionalExtension as _, params};

/// A stored record.
pub(crate) struct Record {
    pub(crate) request_sha256: String,
    pub(crate) status: Option<u16>,
    pub(crate) response: Option<Vec<u8>>,
    pub(crate) created_at: i64,
}

/// Reads a record.
pub(crate) fn get(
    conn: &Connection,
    ctx: &str,
    scope: &str,
    key: &str,
) -> rusqlite::Result<Option<Record>> {
    conn.query_row(
        "SELECT request_sha256, status, response, created_at FROM idempotency WHERE ctx = ?1 AND scope = ?2 AND key = ?3",
        params![ctx, scope, key],
        |row| {
            let status: Option<i64> = row.get(1)?;
            Ok(Record {
                request_sha256: row.get(0)?,
                status: status.and_then(|s| u16::try_from(s).ok()),
                response: row.get(2)?,
                created_at: row.get(3)?,
            })
        },
    )
    .optional()
}

/// Reserves a key (first execution in progress).
pub(crate) fn insert_pending(
    conn: &Connection,
    ctx: &str,
    scope: &str,
    key: &str,
    request_sha256: &str,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO idempotency(ctx, scope, key, request_sha256, status, response, created_at) VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5)",
        params![ctx, scope, key, request_sha256, now],
    )?;
    Ok(())
}

/// Takes over an abandoned in-progress reservation.
pub(crate) fn renew_pending(
    conn: &Connection,
    ctx: &str,
    scope: &str,
    key: &str,
    now: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE idempotency SET created_at = ?4 WHERE ctx = ?1 AND scope = ?2 AND key = ?3 AND status IS NULL",
        params![ctx, scope, key, now],
    )
}

/// Stores the final response of a reservation.
pub(crate) fn complete(
    conn: &Connection,
    ctx: &str,
    scope: &str,
    key: &str,
    status: u16,
    response: &[u8],
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE idempotency SET status = ?4, response = ?5 WHERE ctx = ?1 AND scope = ?2 AND key = ?3 AND status IS NULL",
        params![ctx, scope, key, i64::from(status), response],
    )
}

/// Drops an in-progress reservation (the request failed).
pub(crate) fn release(
    conn: &Connection,
    ctx: &str,
    scope: &str,
    key: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM idempotency WHERE ctx = ?1 AND scope = ?2 AND key = ?3 AND status IS NULL",
        params![ctx, scope, key],
    )
}
