//! `drawings`: each Silicon's drawing copy.

use rusqlite::{Connection, OptionalExtension as _, params};

/// A stored drawing.
pub(crate) struct Drawing {
    pub(crate) sha256: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) updated_at: i64,
}

/// Stores (or replaces) the actor's drawing.
pub(crate) fn upsert(
    conn: &Connection,
    ctx: &str,
    account: &str,
    actor: &str,
    sha256: &str,
    bytes: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO drawings(ctx, account_id, actor_id, sha256, bytes, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT DO UPDATE SET actor_id = excluded.actor_id, sha256 = excluded.sha256, bytes = excluded.bytes, updated_at = excluded.updated_at",
        params![ctx, account, actor, sha256, bytes, now],
    )?;
    Ok(())
}

/// The actor's drawing, if any.
pub(crate) fn get(
    conn: &Connection,
    ctx: &str,
    account: &str,
    actor: &str,
) -> rusqlite::Result<Option<Drawing>> {
    conn.query_row(
        "SELECT sha256, bytes, updated_at FROM drawings WHERE ctx = ?1 AND account_id = ?2",
        params![ctx, account],
        |row| {
            Ok(Drawing {
                sha256: row.get(0)?,
                bytes: row.get(1)?,
                updated_at: row.get(2)?,
            })
        },
    )
    .optional()
}

/// Deletes the actor's drawing; returns how many rows went away.
pub(crate) fn delete(
    conn: &Connection,
    ctx: &str,
    account: &str,
    actor: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM drawings WHERE ctx = ?1 AND account_id = ?2",
        params![ctx, account],
    )
}
