//! Drawings (BLUEPRINT §1.9.2): validation runs inside Peek.app with the
//! same `QuickJS` runtime and renderer used live (D15); peekd stores the
//! script under `drawings/<context>/<org>/<actor>/<sha256>.js`, activates it
//! atomically (keeping the previous one for rollback), tells the UI to load
//! it, and queues a `drawing.put` so peek-server keeps a copy.

use std::{path::PathBuf, sync::Arc};

use rusqlite::{OptionalExtension as _, params};
use serde_json::json;
use silicon_peek_client::{
    Error, ErrorCode, Result,
    ipc::{
        cli::{RegisterDrawing, RegisterDrawingResult, ServerSync},
        ui::{DrawingError, DrawingLoad, DrawingValidate, DrawingValidateResult},
    },
    runtime::{PREWARM_MARGIN, RefreshPolicy},
    schema::check_drawing_bytes,
};

use crate::{
    bubbles::{now_ms, slot_of},
    db::SqlResult as _,
    outbox,
    paths::sha256_hex,
    state::{ActorKey, Caller, Shared, SharedRef},
    telemetry::Record,
};

/// A `drawings` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrawingRow {
    /// Hex SHA-256.
    pub sha256: String,
    /// Absolute path of the active script.
    pub path: String,
    /// `pending`, `synced`, `skipped` or `failed`.
    pub server_sync: String,
    /// The previous script (rollback).
    pub previous_path: Option<String>,
}

/// A script staged for validation under its original file name,
/// `<drawing dir>/.validate-<uuid>/<filename>`: the validator names the file
/// in stack traces and messages from the path's last component.
#[derive(Debug)]
struct Staged {
    dir: PathBuf,
    file: PathBuf,
}

impl Staged {
    /// Removes the staged file (if still there) and its directory.
    fn remove(&self) {
        let _ = std::fs::remove_file(&self.file);
        let _ = std::fs::remove_dir(&self.dir);
    }
}

/// The last path component of `filename` when it is a safe file name,
/// else `drawing.js`.
fn stage_name(filename: &str) -> String {
    std::path::Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| {
            !n.is_empty()
                && n.len() <= 200
                && !n.starts_with('.')
                && n.chars().all(|c| !c.is_control() && c != '/' && c != '\\')
        })
        .map_or_else(|| "drawing.js".to_owned(), str::to_owned)
}

fn validation_failed(
    filename: &str,
    r: &DrawingValidateResult,
    check_only: bool,
    previous_active: bool,
) -> Error {
    let (message, frame) = r.error.as_ref().map_or_else(
        || ("the drawing did not pass validation".to_owned(), None),
        |e| (e.message.clone(), e.frame),
    );
    let at = frame.map_or_else(String::new, |f| format!(" at test frame {f}"));
    let hint = if previous_active && !check_only {
        "fix the script and run `peek register drawing` again; the previous drawing stays active (see peek docs drawing)"
    } else {
        "fix the script and run `peek register drawing` again (see peek docs drawing)"
    };
    Error::new(
        ErrorCode::DrawingInvalid,
        format!("drawing `{filename}` failed validation{at}: {message}"),
    )
    .with_hint(hint)
    .with_details(json!({
        "error": r.error,
        "stats": r.stats,
        "warnings": r.warnings,
        "logs": r.logs,
        "check_only": check_only,
        "previous_active": previous_active,
    }))
}

fn drawing_result(
    sha256: String,
    bytes: u64,
    r: DrawingValidateResult,
    active: bool,
    slot: Option<silicon_peek_client::identity::SlotIndex>,
    server_sync: ServerSync,
) -> RegisterDrawingResult {
    RegisterDrawingResult {
        sha256,
        bytes,
        stats: r.stats,
        warnings: r.warnings,
        logs: r.logs,
        active,
        slot,
        server_sync,
        dump: r.dump,
    }
}

impl Shared {
    /// The Silicon's drawing row.
    ///
    /// # Errors
    /// Database failures.
    pub async fn drawing_row(&self, key: &ActorKey) -> Result<Option<DrawingRow>> {
        let k = key.clone();
        self.db
            .call(move |c| {
                c.query_row(
                    "SELECT sha256, path, server_sync, previous_path FROM drawings WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3",
                    params![k.context_str(), k.org.as_str(), k.actor.as_str()],
                    |r| {
                        Ok(DrawingRow {
                            sha256: r.get(0)?,
                            path: r.get(1)?,
                            server_sync: r.get(2)?,
                            previous_path: r.get(3)?,
                        })
                    },
                )
                .optional()
                .sql()
            })
            .await
    }

    async fn require_ui(self: &SharedRef) -> Result<()> {
        if self.ui.is_connected() {
            return Ok(());
        }
        self.launch_ui_soon();
        if self
            .ui
            .wait_connected(self.cfg.timings.ui_connect_wait)
            .await
        {
            return Ok(());
        }
        Err(Error::new(
            ErrorCode::PeekServiceUnavailable,
            "Peek.app is not running; drawings are validated inside Peek.app with the same runtime that draws them",
        )
        .with_hint("open Peek (peek app install), then run the command again"))
    }

    /// Writes `bytes` to a private staging directory in the Silicon's
    /// drawing dir under the original file name and asks the UI to validate
    /// it. Frame numbers in the report are 0-based (0…89).
    async fn validate_script(
        self: &SharedRef,
        key: &ActorKey,
        filename: &str,
        bytes: &[u8],
        preview: bool,
        dump_frame: Option<u32>,
    ) -> Result<(DrawingValidateResult, Vec<Vec<u8>>, Staged)> {
        let dir = self
            .paths
            .ensure_drawing_dir(key.context, &key.org, &key.actor)?;
        let stage_dir = dir.join(format!(".validate-{}", uuid::Uuid::now_v7().simple()));
        silicon_peek_client::runtime::fs::ensure_private_dir(&stage_dir)?;
        let name = stage_name(filename);
        let staged = Staged {
            file: stage_dir.join(&name),
            dir: stage_dir,
        };
        if let Err(e) = silicon_peek_client::runtime::fs::write_atomic(&staged.dir, &name, bytes) {
            staged.remove();
            return Err(e);
        }
        if let Err(e) = self.require_ui().await {
            staged.remove();
            return Err(e);
        }
        let op = DrawingValidate {
            script_path: staged.file.to_string_lossy().into_owned(),
            preview,
            dump_frame,
        };
        match self
            .ui
            .request(&op, Vec::new(), self.cfg.timings.drawing_validate_timeout)
            .await
        {
            Ok((result, blobs)) => Ok((result, blobs, staged)),
            Err(e) => {
                staged.remove();
                Err(e)
            }
        }
    }

    /// Makes a validated temp script the active drawing. Returns the final
    /// path and whether it differs from the previous active one.
    async fn activate(
        self: &SharedRef,
        key: &ActorKey,
        sha: &str,
        bytes: u64,
        temp: &std::path::Path,
        server_sync: &'static str,
    ) -> Result<(PathBuf, bool)> {
        let dir = self
            .paths
            .ensure_drawing_dir(key.context, &key.org, &key.actor)?;
        let final_path = dir.join(format!("{sha}.js"));
        if final_path.exists() {
            let _ = std::fs::remove_file(temp);
        } else {
            std::fs::rename(temp, &final_path).map_err(|e| {
                Error::internal(format!("activating {} failed: {e}", final_path.display()))
            })?;
        }
        let k = key.clone();
        let sha_s = sha.to_owned();
        let path_s = final_path.to_string_lossy().into_owned();
        let now = now_ms();
        let (changed, stale) = self
            .db
            .tx(move |tx| {
                let old: Option<(String, String, Option<String>)> = tx
                    .query_row(
                        "SELECT sha256, path, previous_path FROM drawings WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3",
                        params![k.context_str(), k.org.as_str(), k.actor.as_str()],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()
                    .sql()?;
                let (changed, previous, stale) = match &old {
                    Some((old_sha, _, prev)) if *old_sha == sha_s => (false, prev.clone(), None),
                    Some((_, old_path, prev)) => (true, Some(old_path.clone()), prev.clone().filter(|p| *p != path_s)),
                    None => (true, None, None),
                };
                tx.execute(
                    "INSERT INTO drawings (context, org_id, actor_id, sha256, path, bytes, active_since, server_sync, last_error, error_pending, previous_path)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, 0, ?9)
                     ON CONFLICT(context, org_id, actor_id) DO UPDATE SET sha256 = excluded.sha256, path = excluded.path,
                        bytes = excluded.bytes, active_since = excluded.active_since,
                        server_sync = CASE WHEN ?10 THEN excluded.server_sync ELSE drawings.server_sync END,
                        last_error = NULL, error_pending = 0, previous_path = excluded.previous_path",
                    params![
                        k.context_str(),
                        k.org.as_str(),
                        k.actor.as_str(),
                        sha_s,
                        path_s,
                        i64::try_from(bytes).unwrap_or(i64::MAX),
                        now,
                        server_sync,
                        previous,
                        changed
                    ],
                )
                .sql()?;
                Ok((changed, stale))
            })
            .await?;
        if let Some(p) = stale {
            let _ = std::fs::remove_file(p);
        }
        Ok((final_path, changed))
    }

    /// Tells the UI to load the Silicon's active drawing, if it holds a slot.
    async fn load_in_ui(&self, key: &ActorKey, path: &std::path::Path, sha: &str) {
        let k = key.clone();
        let slot = self.db.call(move |c| slot_of(c, &k)).await.ok().flatten();
        self.push_slots_state().await;
        let Some(slot) = slot else { return };
        let op = DrawingLoad {
            context: key.context,
            org_id: key.org.clone(),
            actor_id: key.actor.clone(),
            slot,
            script_path: path.to_string_lossy().into_owned(),
            sha256: sha.to_owned(),
        };
        match self
            .ui
            .request(&op, Vec::new(), self.cfg.timings.ui_request_timeout)
            .await
        {
            Ok((r, _)) if r.ok => {}
            Ok(_) => tracing::warn!(actor = %key.actor, "Peek.app could not load the new drawing"),
            Err(e) => tracing::debug!(error = %e, "drawing.load was not delivered"),
        }
    }

    /// `register.drawing` (§1.9.2).
    ///
    /// # Errors
    /// `drawing_too_large`, `drawing_invalid` (with the A9 error in
    /// `details`), `peek_service_unavailable`.
    pub async fn register_drawing(
        self: &SharedRef,
        caller: &Caller,
        op: RegisterDrawing,
        blobs: Vec<Vec<u8>>,
    ) -> Result<(RegisterDrawingResult, Vec<Vec<u8>>)> {
        let [bytes] = <[Vec<u8>; 1]>::try_from(blobs).map_err(|b| {
            Error::invalid_input(format!(
                "register.drawing carries exactly one blob (the script), not {}",
                b.len()
            ))
        })?;
        check_drawing_bytes(&op.filename, &bytes)?;
        let sha = sha256_hex(&bytes);
        let key = caller.key.clone();
        let (result, reply_blobs, staged) = self
            .validate_script(&key, &op.filename, &bytes, op.preview, op.dump_frame)
            .await?;
        let mut rec = Record::new("drawing.validate", if result.ok { "ok" } else { "error" })
            .with("drawing_bytes", bytes.len())
            .with("drawing_sha256", sha.clone());
        rec.actor = Some((key.org.clone(), key.actor.clone()));
        rec.testing = key.context.is_testing();
        self.record(rec);
        if !result.ok {
            staged.remove();
            let previous_active = self.drawing_row(&key).await.is_ok_and(|r| r.is_some());
            return Err(validation_failed(
                &op.filename,
                &result,
                op.check_only,
                previous_active,
            ));
        }
        let preview_blobs = if op.preview {
            reply_blobs.into_iter().take(1).collect()
        } else {
            Vec::new()
        };
        let k = key.clone();
        let slot = self.db.call(move |c| slot_of(c, &k)).await?;
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if op.check_only {
            staged.remove();
            return Ok((
                drawing_result(sha, len, result, false, slot, ServerSync::Skipped),
                preview_blobs,
            ));
        }
        let previous = self.drawing_row(&key).await?;
        let unchanged_and_synced = previous
            .as_ref()
            .is_some_and(|p| p.sha256 == sha && p.server_sync == "synced");
        let activated = self
            .activate(
                &key,
                &sha,
                len,
                &staged.file,
                if unchanged_and_synced {
                    "synced"
                } else {
                    "pending"
                },
            )
            .await;
        staged.remove();
        let (path, _changed) = activated?;
        let server_sync = if unchanged_and_synced {
            ServerSync::Skipped
        } else {
            self.queue_drawing_put(caller, bytes).await?;
            ServerSync::Pending
        };
        self.load_in_ui(&key, &path, &sha).await;
        Ok((
            drawing_result(sha, len, result, true, slot, server_sync),
            preview_blobs,
        ))
    }

    /// Queues the upload of the active script, superseding any upload still
    /// waiting.
    async fn queue_drawing_put(&self, caller: &Caller, bytes: Vec<u8>) -> Result<()> {
        let row = outbox::NewRow {
            event_id: silicon_peek_client::ids::EventId::generate()
                .as_str()
                .to_owned(),
            key: caller.key.clone(),
            home: caller.home.clone(),
            kind: outbox::Kind::DrawingPut,
            request: bytes,
            subject_id: None,
        };
        let k = caller.key.clone();
        self.db
            .tx(move |tx| {
                // The new upload replaces the server copy, so an older upload
                // and an older delete (after an unregister) still waiting
                // must not run after it.
                tx.execute(
                    "UPDATE outbox SET status = 'cancelled', last_error_code = 'superseded'
                     WHERE kind IN ('drawing.put','drawing.delete') AND status IN ('pending','authority_required')
                       AND context = ?1 AND org_id = ?2 AND actor_id = ?3",
                    params![k.context_str(), k.org.as_str(), k.actor.as_str()],
                )
                .sql()?;
                outbox::insert(tx, &row)
            })
            .await?;
        self.outbox_wake.notify_one();
        Ok(())
    }

    /// Whether the Silicon's server copy is being deleted (an `unregister`
    /// whose `drawing.delete` has not been accepted yet).
    async fn drawing_delete_pending(&self, key: &ActorKey) -> bool {
        let k = key.clone();
        self.db
            .call(move |c| {
                c.query_row(
                    "SELECT count(*) FROM outbox WHERE kind = 'drawing.delete'
                       AND status IN ('pending','authority_required')
                       AND context = ?1 AND org_id = ?2 AND actor_id = ?3",
                    params![k.context_str(), k.org.as_str(), k.actor.as_str()],
                    |r| r.get::<_, i64>(0),
                )
                .sql()
            })
            .await
            // Unknown counts as pending: never resurrect on a guess.
            .map_or(true, |n| n > 0)
    }

    /// §1.9.2 step 6: a Silicon registered a side without a local drawing;
    /// fetch its server copy, validate it in the UI, then activate it.
    ///
    /// Not while an `unregister`'s delete of that copy is still pending:
    /// the Silicon released its drawing, and the server's copy is on its
    /// way out (checked again right before activating).
    pub async fn fetch_server_drawing(self: Arc<Self>, caller: Caller) {
        if self.drawing_delete_pending(&caller.key).await {
            tracing::debug!(actor = %caller.key.actor, "the server drawing is being deleted; not fetching it");
            return;
        }
        let policy = RefreshPolicy::with_delays(self.cfg.timings.refresh_retry.clone());
        let fetched = match self
            .net
            .session(&caller.home, PREWARM_MARGIN, &policy)
            .await
        {
            Ok((client, slot, _))
                if slot.actor.public_id == caller.key.actor && slot.org_id == caller.key.org =>
            {
                client.get_drawing().await
            }
            Ok(_) => return,
            Err(e) => {
                tracing::debug!(error = %e, "no session to fetch the server drawing");
                return;
            }
        };
        let drawing = match fetched {
            Ok(Some(d)) => d,
            Ok(None) => return,
            Err(e) => {
                tracing::info!(error = %e, "fetching the server copy of a drawing failed");
                return;
            }
        };
        if check_drawing_bytes("server drawing", &drawing.bytes).is_err() {
            return;
        }
        let key = caller.key.clone();
        match self
            .validate_script(&key, "drawing.js", &drawing.bytes, false, None)
            .await
        {
            Ok((r, _, staged)) if r.ok => {
                if self.drawing_row(&key).await.ok().flatten().is_some() {
                    // The Silicon registered one meanwhile; it wins.
                    staged.remove();
                    return;
                }
                if self.drawing_delete_pending(&key).await {
                    // Unregistered while the copy was fetched: it stays gone.
                    staged.remove();
                    return;
                }
                let len = u64::try_from(drawing.bytes.len()).unwrap_or(0);
                let activated = self
                    .activate(&key, &drawing.sha256, len, &staged.file, "synced")
                    .await;
                staged.remove();
                match activated {
                    Ok((path, _)) => self.load_in_ui(&key, &path, &drawing.sha256).await,
                    Err(e) => tracing::warn!(error = %e, "activating the server drawing failed"),
                }
            }
            Ok((_, _, staged)) => {
                staged.remove();
                tracing::info!(actor = %key.actor, "the server copy of a drawing no longer validates; not activating it");
            }
            Err(e) => tracing::debug!(error = %e, "validating the server drawing failed"),
        }
    }

    /// `drawing.error`: the fallback visual replaced a drawing; the error is
    /// attached to the Silicon's next CLI result.
    ///
    /// # Errors
    /// Database failures.
    pub async fn ui_drawing_error(&self, op: DrawingError) -> Result<()> {
        let body = json!({
            "reason": op.reason,
            "message": op.message,
            "stack": op.stack,
            "at": silicon_peek_client::timestamp::Timestamp::now(),
        })
        .to_string();
        let k = ActorKey {
            context: op.context,
            org: op.org_id,
            actor: op.actor_id,
        };
        let mut rec = Record::new("drawing.fallback", "error");
        rec.actor = Some((k.org.clone(), k.actor.clone()));
        rec.testing = k.context.is_testing();
        self.record(rec);
        self.db
            .call(move |c| {
                c.execute(
                    "UPDATE drawings SET last_error = ?4, error_pending = 1 WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3",
                    params![k.context_str(), k.org.as_str(), k.actor.as_str(), body],
                )
                .sql()
                .map(|_| ())
            })
            .await
    }
}

#[cfg(test)]
mod staging_tests {
    use super::stage_name;

    #[test]
    fn staged_names_keep_the_original_file_name() {
        assert_eq!(stage_name("./art/cassette.js"), "cassette.js");
        assert_eq!(stage_name("/abs/path/logo.js"), "logo.js");
        assert_eq!(stage_name(""), "drawing.js");
        assert_eq!(stage_name(".."), "drawing.js");
        assert_eq!(stage_name(".hidden.js"), "drawing.js");
    }
}
