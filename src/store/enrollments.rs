//! `ting_enrollments`: the Ting recipient grants peek created.

use rusqlite::{Connection, OptionalExtension as _, params};

/// A recorded enrollment.
pub(crate) struct Enrollment {
    pub(crate) subscription_id: String,
    pub(crate) revoked_at: Option<i64>,
}

impl Enrollment {
    /// Whether the grant is (as far as peek knows) active.
    pub(crate) fn active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// Records a (re)activated grant.
pub(crate) fn record(
    conn: &Connection,
    ctx: &str,
    org: &str,
    actor: &str,
    subscription_id: &str,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO ting_enrollments(ctx, org_id, actor_id, subscription_id, registered_at, revoked_at) VALUES (?1, ?2, ?3, ?4, ?5, NULL)
         ON CONFLICT(ctx, org_id, actor_id) DO UPDATE SET subscription_id = excluded.subscription_id, registered_at = excluded.registered_at, revoked_at = NULL",
        params![ctx, org, actor, subscription_id, now],
    )?;
    Ok(())
}

/// The actor's enrollment, if any.
pub(crate) fn get(
    conn: &Connection,
    ctx: &str,
    org: &str,
    actor: &str,
) -> rusqlite::Result<Option<Enrollment>> {
    conn.query_row(
        "SELECT subscription_id, revoked_at FROM ting_enrollments WHERE ctx = ?1 AND org_id = ?2 AND actor_id = ?3",
        params![ctx, org, actor],
        |row| {
            Ok(Enrollment {
                subscription_id: row.get(0)?,
                revoked_at: row.get(1)?,
            })
        },
    )
    .optional()
}

/// Marks the grant revoked (logout, or Ting said `recipient_not_registered`).
pub(crate) fn mark_revoked(
    conn: &Connection,
    ctx: &str,
    org: &str,
    actor: &str,
    now: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE ting_enrollments SET revoked_at = ?4 WHERE ctx = ?1 AND org_id = ?2 AND actor_id = ?3 AND revoked_at IS NULL",
        params![ctx, org, actor, now],
    )
}

/// Forgets the actor's enrollment (membership removed).
pub(crate) fn delete(
    conn: &Connection,
    ctx: &str,
    org: &str,
    actor: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM ting_enrollments WHERE ctx = ?1 AND org_id = ?2 AND actor_id = ?3",
        params![ctx, org, actor],
    )
}
