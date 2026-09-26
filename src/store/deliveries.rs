//! `deliveries`: one row per delivery event (BLUEPRINT §3.5 step 6).

use rusqlite::{Connection, OptionalExtension as _, params};

/// The identity of a delivery; a reused `event_id` must match it exactly.
pub(crate) struct DeliveryKey<'a> {
    pub(crate) ctx: &'a str,
    pub(crate) event_id: &'a str,
    pub(crate) org: &'a str,
    pub(crate) actor: &'a str,
    pub(crate) ting_type: &'a str,
    pub(crate) ting_key: &'a str,
}

/// An accepted delivery recorded earlier.
pub(crate) struct Accepted {
    pub(crate) ting_id: String,
    pub(crate) silent: bool,
}

/// What [`begin`] found.
pub(crate) enum Begin {
    /// New or still pending: send it.
    Send,
    /// Already accepted by Ting.
    Accepted(Accepted),
    /// The event ID was used for a different delivery.
    Conflict,
}

/// Records the first sight of a delivery, or checks a repeat against it.
pub(crate) fn begin(conn: &Connection, key: &DeliveryKey<'_>, now: i64) -> rusqlite::Result<Begin> {
    let existing = conn
        .query_row(
            "SELECT org_id, actor_id, type, ting_key, status, ting_id, silent FROM deliveries WHERE ctx = ?1 AND event_id = ?2",
            params![key.ctx, key.event_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ))
            },
        )
        .optional()?;
    match existing {
        None => {
            conn.execute(
                "INSERT INTO deliveries(ctx, event_id, org_id, actor_id, type, ting_key, status, attempts, first_seen_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', 0, ?7, ?7)",
                params![key.ctx, key.event_id, key.org, key.actor, key.ting_type, key.ting_key, now],
            )?;
            Ok(Begin::Send)
        }
        Some((org, actor, ting_type, ting_key, ..))
            if org != key.org
                || actor != key.actor
                || ting_type != key.ting_type
                || ting_key != key.ting_key =>
        {
            Ok(Begin::Conflict)
        }
        Some((_, _, _, _, status, Some(ting_id), silent)) if status == "accepted" => {
            Ok(Begin::Accepted(Accepted {
                ting_id,
                silent: silent == Some(1),
            }))
        }
        Some(_) => Ok(Begin::Send),
    }
}

/// The outcome of one delivery attempt.
pub(crate) enum Outcome<'a> {
    /// Ting accepted it.
    Accepted { ting_id: &'a str, silent: bool },
    /// It failed with a stable status and error code.
    Failed {
        status: &'static str,
        error: &'a str,
    },
}

/// Records one attempt's outcome.
pub(crate) fn record(
    conn: &Connection,
    ctx: &str,
    event_id: &str,
    outcome: &Outcome<'_>,
    attempts: u32,
    now: i64,
) -> rusqlite::Result<()> {
    match outcome {
        Outcome::Accepted { ting_id, silent } => conn.execute(
            "UPDATE deliveries SET status = 'accepted', ting_id = ?3, silent = ?4, last_error = NULL,
                    attempts = attempts + ?5, updated_at = ?6, accepted_at = COALESCE(accepted_at, ?6)
             WHERE ctx = ?1 AND event_id = ?2",
            params![ctx, event_id, ting_id, i64::from(*silent), attempts, now],
        )?,
        Outcome::Failed { status, error } => conn.execute(
            "UPDATE deliveries SET status = ?3, last_error = ?4, attempts = attempts + ?5, updated_at = ?6
             WHERE ctx = ?1 AND event_id = ?2 AND status <> 'accepted'",
            params![ctx, event_id, status, error, attempts, now],
        )?,
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::memory;

    fn key<'a>(event: &'a str, ting_key: &'a str) -> DeliveryKey<'a> {
        DeliveryKey {
            ctx: "production",
            event_id: event,
            org: "tos",
            actor: "si:cleanup",
            ting_type: "peek.ask.answered",
            ting_key,
        }
    }

    #[test]
    fn begin_record_and_conflict() -> rusqlite::Result<()> {
        let conn = memory()?;
        assert!(matches!(begin(&conn, &key("evt_1", "k"), 1)?, Begin::Send));
        record(
            &conn,
            "production",
            "evt_1",
            &Outcome::Failed {
                status: "unavailable",
                error: "ting_unavailable",
            },
            1,
            2,
        )?;
        assert!(matches!(begin(&conn, &key("evt_1", "k"), 3)?, Begin::Send));
        record(
            &conn,
            "production",
            "evt_1",
            &Outcome::Accepted {
                ting_id: "msg_1",
                silent: true,
            },
            1,
            4,
        )?;
        match begin(&conn, &key("evt_1", "k"), 5)? {
            Begin::Accepted(a) => {
                assert_eq!(a.ting_id, "msg_1");
                assert!(a.silent);
            }
            _ => panic!("expected accepted"),
        }
        assert!(matches!(
            begin(&conn, &key("evt_1", "other"), 6)?,
            Begin::Conflict
        ));
        let attempts: i64 = conn.query_row(
            "SELECT attempts FROM deliveries WHERE event_id = 'evt_1'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(attempts, 2);
        Ok(())
    }
}
