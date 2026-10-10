//! Positions (BLUEPRINT §1.9.1): `register.side`, `unregister`, `status`
//! and the `slots.state` table Peek.app draws from.

use std::sync::Arc;

use rusqlite::{OptionalExtension as _, params};
use serde_json::{Value, json};
use silicon_peek_client::{
    Error, ErrorCode, Result,
    identity::{AccountId, ActorId, Context, SlotIndex, SlotInfo},
    ipc::{
        cli::{
            CarbonStatus, DeliveriesStatus, DrawingStatus, QueueStatus, RegisterSideResult,
            StatusResult, UnregisterResult,
        },
        ui::{CancelReason, SlotDrawing, SlotState, SlotsState},
    },
};

use crate::{
    bubbles::{free_slots, now_ms},
    db::SqlResult as _,
    outbox,
    state::{Caller, Shared, SharedRef},
};

/// The fallback visual's initial: the first letter or digit of the display
/// name, else of the handle, uppercased.
#[must_use]
pub fn initial(display_name: Option<&str>, actor: &ActorId) -> String {
    display_name
        .and_then(|n| n.chars().find(|c| c.is_alphanumeric()))
        .or_else(|| actor.handle().chars().find(|c| c.is_alphanumeric()))
        .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect())
}

/// `(context, slot, account, actor, display_name, testing, drawing sha, drawing path)`.
type SlotRow = (
    String,
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

enum SideOutcome {
    Taken { owner: String, free: Vec<u8> },
    Registered { moved_from: Option<SlotIndex> },
}

/// The `register.side` transaction (§1.9.1 step 2).
fn register_side_tx(
    tx: &rusqlite::Connection,
    c2: &Caller,
    index: SlotIndex,
    now: i64,
) -> Result<SideOutcome> {
    let ctx = c2.key.context_str();
    let holder: Option<(String, String)> = tx
        .query_row(
            "SELECT account_id, actor_id FROM slots WHERE context = ?1 AND slot = ?2",
            params![ctx, index.get()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .sql()?;
    if let Some((account, actor)) = &holder
        && account != c2.key.account.as_str()
    {
        return Ok(SideOutcome::Taken {
            owner: actor.clone(),
            free: free_slots(tx, c2.key.context)?,
        });
    }
    let current: Option<i64> = tx
        .query_row(
            "SELECT slot FROM slots WHERE context = ?1 AND account_id = ?2",
            params![ctx, c2.key.account.as_str()],
            |r| r.get(0),
        )
        .optional()
        .sql()?;
    let moved_from = match current {
        Some(old) if old == i64::from(index.get()) => None,
        Some(old) => {
            tx.execute(
                "UPDATE slots SET slot = ?3 WHERE context = ?1 AND account_id = ?2",
                params![ctx, c2.key.account.as_str(), index.get()],
            )
            .sql()?;
            tx.execute(
                "UPDATE sends SET slot = ?3 WHERE context = ?1 AND account_id = ?2 AND closed_at IS NULL",
                params![ctx, c2.key.account.as_str(), index.get()],
            )
            .sql()?;
            SlotIndex::new(u64::try_from(old).unwrap_or(0)).ok()
        }
        None => {
            tx.execute(
                "INSERT INTO slots (context, slot, account_id, actor_id, home_path, registered_at, api_url, display_name)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    ctx,
                    index.get(),
                    c2.key.account.as_str(),
                    c2.key.actor.as_str(),
                    c2.home.home_path,
                    now,
                    c2.home.api_url.as_str(),
                    c2.display_name
                ],
            )
            .sql()?;
            None
        }
    };
    tx.execute(
        "UPDATE slots SET home_path = ?3, api_url = ?4, display_name = coalesce(?5, display_name)
         WHERE context = ?1 AND account_id = ?2",
        params![
            ctx,
            c2.key.account.as_str(),
            c2.home.home_path,
            c2.home.api_url.as_str(),
            c2.display_name
        ],
    )
    .sql()?;
    Ok(SideOutcome::Registered { moved_from })
}

impl Shared {
    /// `register.side` in one transaction: refuse a slot another Silicon
    /// holds in this context (`side_taken` with the free list), move the
    /// Silicon if it holds another one, otherwise claim it.
    ///
    /// # Errors
    /// `side_taken`; database failures.
    pub async fn register_side(
        self: &SharedRef,
        caller: &Caller,
        index: SlotIndex,
    ) -> Result<RegisterSideResult> {
        let c2 = caller.clone();
        let now = now_ms();
        let outcome = self
            .db
            .tx(move |tx| register_side_tx(tx, &c2, index, now))
            .await?;
        let moved_from = match outcome {
            SideOutcome::Taken { owner, free } => {
                let list = free.iter().map(u8::to_string).collect::<Vec<_>>().join(",");
                let hint = match free.first() {
                    Some(f) => format!("choose a free one: peek register side {f} (free: {list})"),
                    None => "every position is taken in this context; another Silicon must `peek unregister` first".to_owned(),
                };
                return Err(Error::new(
                    ErrorCode::SideTaken,
                    format!("position {index} is held by {owner}; peek gives each Silicon exactly one position."),
                )
                .with_hint(hint)
                .with_details(json!({"owner": owner, "free": free})));
            }
            SideOutcome::Registered { moved_from } => moved_from,
        };
        self.push_slots_state().await;
        let has_drawing = self.drawing_row(&caller.key).await?.is_some();
        if !has_drawing {
            let this = Arc::clone(self);
            let c = caller.clone();
            tokio::spawn(async move { this.fetch_server_drawing(c).await });
        }
        let mut warnings = Vec::new();
        if let Some(w) = self.take_fallback_warning(&caller.key).await {
            warnings.push(w);
        }
        let modifier = self.settings.get().hotkey_modifier;
        Ok(RegisterSideResult {
            slot: SlotInfo::from(index),
            moved_from,
            hotkey: Some(format!("{modifier}+{}", index.get())),
            warnings,
        })
    }

    /// `unregister`: releases the position, cancels pending asks, queued and
    /// scheduled sends, and deletes the drawing locally and on peek-server.
    ///
    /// # Errors
    /// Database failures.
    pub async fn unregister(self: &SharedRef, caller: &Caller) -> Result<UnregisterResult> {
        let cancelled = self
            .cancel_actor_bubbles(&caller.key, CancelReason::Unregistered)
            .await?;
        let k = caller.key.clone();
        let home = caller.home.clone();
        let (released, paths) = self
            .db
            .tx(move |tx| {
                let slot: Option<i64> = tx
                    .query_row(
                        "SELECT slot FROM slots WHERE context = ?1 AND account_id = ?2",
                        params![k.context_str(), k.account.as_str()],
                        |r| r.get(0),
                    )
                    .optional()
                    .sql()?;
                tx.execute(
                    "DELETE FROM slots WHERE context = ?1 AND account_id = ?2",
                    params![k.context_str(), k.account.as_str()],
                )
                .sql()?;
                let paths: Option<(String, Option<String>)> = tx
                    .query_row(
                        "SELECT path, previous_path FROM drawings WHERE context = ?1 AND account_id = ?2",
                        params![k.context_str(), k.account.as_str()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .sql()?;
                if paths.is_some() {
                    tx.execute(
                        "DELETE FROM drawings WHERE context = ?1 AND account_id = ?2",
                        params![k.context_str(), k.account.as_str()],
                    )
                    .sql()?;
                    tx.execute(
                        "UPDATE outbox SET status = 'cancelled', last_error_code = 'unregistered'
                         WHERE kind = 'drawing.put' AND status IN ('pending','authority_required')
                           AND context = ?1 AND account_id = ?2",
                        params![k.context_str(), k.account.as_str()],
                    )
                    .sql()?;
                }
                outbox::insert(
                    tx,
                    &outbox::NewRow {
                        event_id: format!("evt_{}", uuid::Uuid::now_v7().simple()),
                        key: k.clone(),
                        home,
                        kind: outbox::Kind::DrawingDelete,
                        request: Vec::new(),
                        subject_id: None,
                    },
                )?;
                Ok((slot.and_then(|s| SlotIndex::new(u64::try_from(s).unwrap_or(0)).ok()), paths))
            })
            .await?;
        if let Some((path, previous)) = paths {
            let _ = std::fs::remove_file(&path);
            if let Some(p) = previous {
                let _ = std::fs::remove_file(p);
            }
        }
        self.outbox_wake.notify_one();
        self.push_slots_state().await;
        Ok(UnregisterResult {
            released_slot: released,
            cancelled_asks: cancelled.asks,
            cancelled_sends: cancelled.sends,
            cancelled_scheduled: cancelled.scheduled,
        })
    }

    /// The full slot table.
    ///
    /// # Errors
    /// Database failures.
    pub async fn slots_state(&self) -> Result<SlotsState> {
        let rows: Vec<SlotRow> = self
            .db
            .call(|c| {
                let mut st = c
                    .prepare(
                        "SELECT s.context, s.slot, s.account_id, s.actor_id, s.display_name, d.sha256, d.path
                         FROM slots s LEFT JOIN drawings d
                           ON d.context = s.context AND d.account_id = s.account_id
                         ORDER BY s.slot, (s.context = 'production') DESC, s.registered_at",
                    )
                    .sql()?;
                let v = st
                    .query_map([], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
                    })
                    .sql()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .sql()?;
                Ok(v)
            })
            .await?;
        let mut slots = Vec::new();
        for (context, slot, account, actor, display_name, sha, path) in rows {
            let (Ok(context), Ok(index), Ok(account_id), Ok(actor_id)) = (
                Context::parse(&context),
                SlotIndex::new(u64::try_from(slot).unwrap_or(0)),
                AccountId::parse(&account),
                ActorId::parse(&actor),
            ) else {
                continue;
            };
            slots.push(SlotState {
                index,
                context,
                initial: initial(display_name.as_deref(), &actor_id),
                actor_id,
                account_id,
                display_name,
                drawing: sha
                    .zip(path)
                    .map(|(sha256, path)| SlotDrawing { sha256, path }),
                hotkey: true,
            });
        }
        Ok(SlotsState { slots })
    }

    /// Pushes `slots.state` to the UI.
    pub async fn push_slots_state(&self) {
        match self.slots_state().await {
            Ok(state) => {
                self.ui.event(&state, Vec::new());
            }
            Err(e) => tracing::error!(error = %e, "building slots.state failed"),
        }
    }

    /// The `status` view of a Silicon's queue.
    ///
    /// # Errors
    /// Database failures.
    pub async fn queue_status(&self, key: &crate::state::ActorKey) -> Result<QueueStatus> {
        let k = key.clone();
        let scheduled = self
            .db
            .call(move |c| crate::queue::scheduled_count(c, &k))
            .await?;
        let core = self.core.lock().await;
        let q = core.queues.get(key);
        let waiting = q.map_or(0, crate::state::ActorQueue::waiting_count);
        Ok(QueueStatus {
            pending: waiting,
            on_screen: q.and_then(|q| q.current.as_ref().map(|b| b.send_id.clone())),
            waiting,
            limit: u32::try_from(silicon_peek_client::schema::limits::QUEUE_MAX)
                .unwrap_or(u32::MAX),
            scheduled,
            held: self.held_reason(q.and_then(|q| q.current.as_ref())),
        })
    }

    /// `status`.
    ///
    /// # Errors
    /// Database failures.
    pub async fn status(self: &SharedRef, caller: &Caller) -> Result<StatusResult> {
        const TING: &str = "'ting'";
        const DRAWING: &str = "'drawing.put','drawing.delete'";
        let k = caller.key.clone();
        let (slot, drawing, pending_asks, deliveries, server_sync) = self
            .db
            .call(move |c| {
                let slot = crate::bubbles::slot_of(c, &k)?;
                let drawing: Option<(String, i64, Option<String>)> = c
                    .query_row(
                        "SELECT sha256, bytes, last_error FROM drawings WHERE context = ?1 AND account_id = ?2",
                        params![k.context_str(), k.account.as_str()],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()
                    .sql()?;
                let pending_asks: i64 = c
                    .query_row(
                        "SELECT count(*) FROM asks a JOIN sends s ON s.send_id = a.send_id
                         WHERE a.state = 'pending' AND s.context = ?1 AND s.account_id = ?2",
                        params![k.context_str(), k.account.as_str()],
                        |r| r.get(0),
                    )
                    .sql()?;
                // `deliveries` are Ting events only; the drawing's backend
                // copy is reported as `drawing.server_sync`.
                let counts = |kinds: &str, status: &str| -> Result<i64> {
                    c.query_row(
                        &format!("SELECT count(*) FROM outbox WHERE kind IN ({kinds}) AND status = ?3 AND context = ?1 AND account_id = ?2"),
                        params![k.context_str(), k.account.as_str(), status],
                        |r| r.get(0),
                    )
                    .sql()
                };
                let pending = counts(TING, "pending")?;
                let authority = counts(TING, "authority_required")?;
                let server_sync = if counts(DRAWING, "authority_required")? > 0 {
                    "authority_required"
                } else if counts(DRAWING, "pending")? > 0 {
                    "pending"
                } else {
                    "synced"
                };
                let last_error: Option<String> = c
                    .query_row(
                        "SELECT last_error_code FROM outbox WHERE kind = 'ting' AND context = ?1 AND account_id = ?2
                           AND status != 'accepted' AND last_error_code IS NOT NULL
                         ORDER BY next_attempt_at DESC LIMIT 1",
                        params![k.context_str(), k.account.as_str()],
                        |r| r.get(0),
                    )
                    .optional()
                    .sql()?;
                Ok((
                    slot,
                    drawing,
                    pending_asks,
                    (pending, authority, last_error),
                    server_sync,
                ))
            })
            .await?;
        let mut warnings = Vec::new();
        if let Some(w) = self.take_fallback_warning(&caller.key).await {
            warnings.push(w);
        }
        Ok(StatusResult {
            slot: slot.map(SlotInfo::from),
            drawing: drawing.map(|(sha256, bytes, last_error)| DrawingStatus {
                sha256,
                bytes: u64::try_from(bytes).unwrap_or(0),
                active: last_error.is_none(),
                last_error: last_error
                    .and_then(|e| serde_json::from_str::<Value>(&e).ok())
                    .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned)),
                server_sync: Some(server_sync.to_owned()),
            }),
            queue: self.queue_status(&caller.key).await?,
            pending_asks: u32::try_from(pending_asks).unwrap_or(u32::MAX),
            deliveries: DeliveriesStatus {
                pending: u32::try_from(deliveries.0).unwrap_or(u32::MAX),
                authority_required: u32::try_from(deliveries.1).unwrap_or(u32::MAX),
                last_error: deliveries.2,
            },
            ui_running: self.ui.is_connected(),
            carbon: Some({
                let p = self.ui.presence();
                CarbonStatus {
                    available: p.available,
                    reason: p.reason,
                    paused: p.paused,
                }
            }),
            warnings,
        })
    }
}
