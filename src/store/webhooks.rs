//! `webhook_events`: ACCOUNTS webhook dedupe.

use rusqlite::{Connection, params};

/// Records an event ID; `false` when it was already processed.
pub(crate) fn insert_if_new(
    conn: &Connection,
    event_id: &str,
    ctx: &str,
    event_type: &str,
    now: i64,
) -> rusqlite::Result<bool> {
    let inserted = conn.execute(
        "INSERT INTO webhook_events(event_id, ctx, event_type, received_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(event_id) DO NOTHING",
        params![event_id, ctx, event_type, now],
    )?;
    Ok(inserted == 1)
}
