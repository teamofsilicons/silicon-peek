//! `env_bindings` and `participant_ops`: the Honeycomb lifecycle control
//! plane (production database only).

use rusqlite::{Connection, OptionalExtension as _, Row, params};

/// A testing environment peek was prepared in.
#[derive(Clone, Debug)]
pub(crate) struct Binding {
    pub(crate) environment_id: String,
    pub(crate) org_id: String,
    pub(crate) environment_revision: i64,
    pub(crate) generation: i64,
    pub(crate) key_version: i64,
    pub(crate) testing_key_sha256: String,
    pub(crate) sealed_root_key: Vec<u8>,
    pub(crate) state: String,
    pub(crate) operation_id: String,
    pub(crate) last_activity_at: Option<i64>,
}

const BINDING_COLUMNS: &str = "environment_id, org_id, environment_revision, generation, key_version, testing_key_sha256, sealed_root_key, state, operation_id, last_activity_at";

fn binding(row: &Row<'_>) -> rusqlite::Result<Binding> {
    Ok(Binding {
        environment_id: row.get(0)?,
        org_id: row.get(1)?,
        environment_revision: row.get(2)?,
        generation: row.get(3)?,
        key_version: row.get(4)?,
        testing_key_sha256: row.get(5)?,
        sealed_root_key: row.get(6)?,
        state: row.get(7)?,
        operation_id: row.get(8)?,
        last_activity_at: row.get(9)?,
    })
}

/// The binding for an environment.
pub(crate) fn get(conn: &Connection, environment_id: &str) -> rusqlite::Result<Option<Binding>> {
    conn.query_row(
        &format!("SELECT {BINDING_COLUMNS} FROM env_bindings WHERE environment_id = ?1"),
        [environment_id],
        binding,
    )
    .optional()
}

/// Every binding that still holds a root key (for routing test webhooks).
pub(crate) fn with_root_keys(conn: &Connection) -> rusqlite::Result<Vec<Binding>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {BINDING_COLUMNS} FROM env_bindings WHERE state <> 'purged' AND length(sealed_root_key) > 0"
    ))?;
    stmt.query_map([], binding)?.collect()
}

/// Active bindings with activity Honeycomb has not heard about yet.
pub(crate) fn activity_due(conn: &Connection) -> rusqlite::Result<Vec<Binding>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {BINDING_COLUMNS} FROM env_bindings
         WHERE state = 'active' AND last_activity_at IS NOT NULL
           AND (activity_reported_at IS NULL OR last_activity_at > activity_reported_at)"
    ))?;
    stmt.query_map([], binding)?.collect()
}

/// Notes activity in an environment, at most once a minute.
pub(crate) fn touch_activity(
    conn: &Connection,
    environment_id: &str,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE env_bindings SET last_activity_at = ?2
         WHERE environment_id = ?1 AND state = 'active' AND (last_activity_at IS NULL OR last_activity_at < ?2 - 60)",
        params![environment_id, now],
    )?;
    Ok(())
}

/// Records that Honeycomb acknowledged activity up to `reported_at`.
pub(crate) fn mark_activity_reported(
    conn: &Connection,
    b: &Binding,
    reported_at: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE env_bindings SET activity_reported_at = MAX(COALESCE(activity_reported_at, 0), ?2)
         WHERE environment_id = ?1 AND generation = ?3 AND key_version = ?4 AND state = 'active'",
        params![b.environment_id, reported_at, b.generation, b.key_version],
    )?;
    Ok(())
}

/// The fields a lifecycle operation writes into the binding's pending barrier.
pub(crate) struct PendingBinding<'a> {
    pub(crate) environment_id: &'a str,
    pub(crate) org_id: &'a str,
    pub(crate) environment_revision: i64,
    pub(crate) generation: i64,
    pub(crate) key_version: i64,
    pub(crate) testing_key_sha256: &'a str,
    pub(crate) sealed_root_key: &'a [u8],
    pub(crate) operation_id: &'a str,
    pub(crate) reset_activity: bool,
}

/// Writes the pending barrier for an operation.
pub(crate) fn upsert_pending(
    conn: &Connection,
    b: &PendingBinding<'_>,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO env_bindings(ctx, environment_id, org_id, environment_revision, generation, key_version,
                                  testing_key_sha256, sealed_root_key, state, operation_id, updated_at)
         VALUES ('production', ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?9)
         ON CONFLICT(environment_id) DO UPDATE SET environment_revision = excluded.environment_revision,
                generation = excluded.generation, key_version = excluded.key_version,
                testing_key_sha256 = excluded.testing_key_sha256, sealed_root_key = excluded.sealed_root_key,
                state = 'pending', operation_id = excluded.operation_id, updated_at = excluded.updated_at",
        params![
            b.environment_id,
            b.org_id,
            b.environment_revision,
            b.generation,
            b.key_version,
            b.testing_key_sha256,
            b.sealed_root_key,
            b.operation_id,
            now
        ],
    )?;
    if b.reset_activity {
        conn.execute(
            "UPDATE env_bindings SET last_activity_at = NULL, activity_reported_at = NULL WHERE environment_id = ?1",
            [b.environment_id],
        )?;
    }
    Ok(())
}

/// Moves a binding to its final state; a purge also erases the root key.
pub(crate) fn finish(
    conn: &Connection,
    environment_id: &str,
    state: &str,
    now: i64,
) -> rusqlite::Result<()> {
    if state == "purged" {
        conn.execute(
            "UPDATE env_bindings SET state = 'purged', sealed_root_key = x'', testing_key_sha256 = '', updated_at = ?2 WHERE environment_id = ?1",
            params![environment_id, now],
        )?;
    } else {
        conn.execute(
            "UPDATE env_bindings SET state = ?2, updated_at = ?3 WHERE environment_id = ?1",
            params![environment_id, state, now],
        )?;
    }
    Ok(())
}

/// A lifecycle operation record.
pub(crate) struct Operation {
    pub(crate) environment_id: String,
    pub(crate) org_id: String,
    pub(crate) request_sha256: String,
    pub(crate) state: String,
    pub(crate) target_state: String,
    pub(crate) receipt: Vec<u8>,
}

/// Reads an operation.
pub(crate) fn get_operation(
    conn: &Connection,
    operation_id: &str,
) -> rusqlite::Result<Option<Operation>> {
    conn.query_row(
        "SELECT environment_id, org_id, request_sha256, state, target_state, receipt FROM participant_ops WHERE operation_id = ?1",
        [operation_id],
        |row| {
            Ok(Operation {
                environment_id: row.get(0)?,
                org_id: row.get(1)?,
                request_sha256: row.get(2)?,
                state: row.get(3)?,
                target_state: row.get(4)?,
                receipt: row.get(5)?,
            })
        },
    )
    .optional()
}

/// The fields of a new operation record.
pub(crate) struct NewOperation<'a> {
    pub(crate) operation_id: &'a str,
    pub(crate) environment_id: &'a str,
    pub(crate) org_id: &'a str,
    pub(crate) action: &'a str,
    pub(crate) request_sha256: &'a str,
    pub(crate) target_state: &'a str,
    pub(crate) receipt: &'a [u8],
}

/// Inserts a pending operation.
pub(crate) fn insert_operation(
    conn: &Connection,
    op: &NewOperation<'_>,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO participant_ops(ctx, operation_id, environment_id, org_id, action, request_sha256, state, target_state, receipt, created_at, updated_at)
         VALUES ('production', ?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8, ?8)",
        params![
            op.operation_id,
            op.environment_id,
            op.org_id,
            op.action,
            op.request_sha256,
            op.target_state,
            op.receipt,
            now
        ],
    )?;
    Ok(())
}

/// Updates an operation's state and receipt.
pub(crate) fn update_operation(
    conn: &Connection,
    operation_id: &str,
    state: &str,
    receipt: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE participant_ops SET state = ?2, receipt = ?3, updated_at = ?4 WHERE operation_id = ?1",
        params![operation_id, state, receipt, now],
    )?;
    Ok(())
}
