//! SQL for every table (BLUEPRINT §5.2). Functions take a borrowed
//! connection or transaction and run inside [`crate::db::Db::call`].
//!
//! Every data row is keyed by `ctx`, and callers only ever pass the `ctx` of
//! the plane they resolved from a validated credential.

pub(crate) mod bindings;
pub(crate) mod byo;
pub(crate) mod deliveries;
pub(crate) mod drawings;
pub(crate) mod enrollments;
pub(crate) mod idempotency;
pub(crate) mod reports;
pub(crate) mod webhooks;

use rusqlite::{Connection, params};

/// Tables holding per-context data (everything a clean must erase).
const DATA_TABLES: [&str; 7] = [
    "drawings",
    "ting_enrollments",
    "deliveries",
    "byo_keys",
    "reports",
    "idempotency",
    "webhook_events",
];

/// Deletes every row of `ctx` from every data table, atomically.
pub(crate) fn wipe_context(conn: &mut Connection, ctx: &str) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
    let mut removed = 0;
    for table in DATA_TABLES {
        removed += tx.execute(&format!("DELETE FROM {table} WHERE ctx = ?1"), [ctx])?;
    }
    tx.commit()?;
    Ok(removed)
}

/// Idempotency records are kept 24 hours; webhook dedupe records 30 days
/// (IAM retries for far less than either).
pub(crate) fn gc(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    let idem = conn.execute(
        "DELETE FROM idempotency WHERE created_at < ?1",
        params![now - 24 * 3600],
    )?;
    let hooks = conn.execute(
        "DELETE FROM webhook_events WHERE received_at < ?1",
        params![now - 30 * 24 * 3600],
    )?;
    Ok(idem + hooks)
}

#[cfg(test)]
pub(crate) mod testing {
    use rusqlite::Connection;

    /// An in-memory database with the schema applied.
    pub(crate) fn memory() -> rusqlite::Result<Connection> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(include_str!("../../migrations/0001_initial.sql"))?;
        Ok(conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wipe_only_touches_one_context() -> rusqlite::Result<()> {
        let mut conn = testing::memory()?;
        let env = "0192f2d2-7c9e-7cc0-8b2e-6f3a2b1c0d9e";
        for ctx in ["production", env] {
            drawings::upsert(&conn, ctx, "tos", "si:cleanup", &"a".repeat(64), b"x", 1)?;
            enrollments::record(&conn, ctx, "tos", "si:cleanup", "sub_1", 1)?;
        }
        assert_eq!(wipe_context(&mut conn, env)?, 2);
        assert!(drawings::get(&conn, "production", "tos", "si:cleanup")?.is_some());
        assert!(drawings::get(&conn, env, "tos", "si:cleanup")?.is_none());
        Ok(())
    }

    #[test]
    fn gc_drops_old_rows() -> rusqlite::Result<()> {
        let conn = testing::memory()?;
        idempotency::insert_pending(&conn, "production", "s", "k", "h", 1)?;
        webhooks::insert_if_new(&conn, "e1", "production", "t", 1)?;
        idempotency::insert_pending(&conn, "production", "s", "k2", "h", 10_000_000)?;
        assert_eq!(gc(&conn, 10_000_000)?, 2);
        Ok(())
    }
}
