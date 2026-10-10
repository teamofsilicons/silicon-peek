//! `byo_keys`: account-wide Deepgram keys (sealed).

use rusqlite::{Connection, OptionalExtension as _, params};

/// A stored BYO key.
pub(crate) struct ByoKey {
    pub(crate) base_url: Option<String>,
    pub(crate) updated_at: i64,
}

/// Stores (or replaces) the account's key.
pub(crate) fn put(
    conn: &Connection,
    ctx: &str,
    account: &str,
    sealed: &[u8],
    base_url: Option<&str>,
    now: i64,
    updated_by: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO byo_keys(ctx, account_id, provider, sealed, base_url, updated_at, updated_by) VALUES (?1, ?2, 'deepgram', ?3, ?4, ?5, ?6)
         ON CONFLICT(ctx, account_id, provider) DO UPDATE SET sealed = excluded.sealed, base_url = excluded.base_url,
                updated_at = excluded.updated_at, updated_by = excluded.updated_by",
        params![ctx, account, sealed, base_url, now, updated_by],
    )?;
    Ok(())
}

/// The account's key, if configured.
pub(crate) fn get(conn: &Connection, ctx: &str, account: &str) -> rusqlite::Result<Option<ByoKey>> {
    conn.query_row(
        "SELECT base_url, updated_at FROM byo_keys WHERE ctx = ?1 AND account_id = ?2 AND provider = 'deepgram'",
        params![ctx, account],
        |row| {
            Ok(ByoKey {
                base_url: row.get(0)?,
                updated_at: row.get(1)?,
            })
        },
    )
    .optional()
}

/// Removes the account's key.
pub(crate) fn delete(conn: &Connection, ctx: &str, account: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM byo_keys WHERE ctx = ?1 AND account_id = ?2 AND provider = 'deepgram'",
        params![ctx, account],
    )
}
