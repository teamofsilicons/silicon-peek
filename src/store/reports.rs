//! `reports`: bug reports, filed as GitHub issues when possible.

use rusqlite::{Connection, params};

/// A new report row.
pub(crate) struct NewReport<'a> {
    pub(crate) id: &'a str,
    pub(crate) ctx: &'a str,
    pub(crate) org: Option<&'a str>,
    pub(crate) actor: Option<&'a str>,
    pub(crate) message: &'a str,
    pub(crate) pr: Option<&'a str>,
    pub(crate) context: Option<&'a str>,
    pub(crate) attached_status: Option<&'a str>,
    pub(crate) created_at: i64,
}

/// Stores a report as `stored`.
pub(crate) fn insert(conn: &Connection, r: &NewReport<'_>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO reports(id, ctx, org_id, actor_id, message, pr, context, attached_status, created_at, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'stored')",
        params![
            r.id,
            r.ctx,
            r.org,
            r.actor,
            r.message,
            r.pr,
            r.context,
            r.attached_status,
            r.created_at
        ],
    )?;
    Ok(())
}

/// Marks a report filed.
pub(crate) fn mark_filed(conn: &Connection, id: &str, issue_url: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE reports SET status = 'filed', issue_url = ?2, filing_error = NULL WHERE id = ?1",
        params![id, issue_url],
    )?;
    Ok(())
}

/// Records why filing failed (the report stays `stored`).
pub(crate) fn mark_filing_error(conn: &Connection, id: &str, error: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE reports SET filing_error = ?2 WHERE id = ?1",
        params![id, error],
    )?;
    Ok(())
}
