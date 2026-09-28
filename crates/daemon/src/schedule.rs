//! Scheduled sends (`peek send --in/--at`, contract §6.6) and the timer loop
//! that fires them, expires sends and closes bubbles Peek.app never reported
//! done.
//!
//! A scheduled send is stored in `scheduled` with its IDs pre-assigned and
//! its images already copied. When it comes due it is *materialized* into
//! `sends` (and `asks`) with the same IDs and placed like any send: current,
//! waiting, or (five already waiting) overflow, which takes the next free
//! spot before any new send. It is never an error then. The Silicon gets
//! `peek.schedule.due` saying what happened, and `peek.send.shown` when it
//! appears.
//!
//! All comparisons use wall-clock unix milliseconds ([`Shared::now_ms`]):
//! tokio's clock stops while the Mac sleeps, so the loop never sleeps longer
//! than [`crate::config::Timings::timer_catchup_cap`] while anything is
//! scheduled or expiring, logs wall-clock jumps, and is woken on every
//! `presence` change and UI connect.

use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::{Value, json};
use silicon_peek_client::{
    Error, ErrorCode, Result,
    identity::{ActorId, ApiUrl, Context, OrgId},
    ids::{AskId, ScheduleId, SendId},
    ipc::cli::{
        CancelledFrom, HeldReason, ScheduleCancelResult, ScheduleClearResult, ScheduleListResult,
        ScheduledItem, SendCancelResult, SendOp, SendResult, SendStatus,
    },
    schema::limits,
    timestamp::Timestamp,
    ting::{AskExpired, DueOutcome, ScheduleDue, SchemaV1, SendExpired, TingData, WaitingReason},
};

use crate::{
    bubbles::{
        NewSend, Resolution, SendPayload, StoredSpeechStatus, build_payload, insert_send,
        queue_ting_for,
    },
    db::SqlResult as _,
    net::HomeRef,
    queue::{ScheduledRef, Withdraw, schedule_not_found, scheduled_count},
    speech::TtsCache,
    state::{ActorKey, Bubble, Caller, Shared, SharedRef},
    telemetry::Record,
};

/// How far the wall clock may run ahead of the monotonic clock between two
/// timer passes before peekd logs a sleep/wake jump.
const JUMP_LOG_THRESHOLD_MS: i64 = 2_000;
/// Longest the timer loop ever sleeps.
const TIMER_IDLE: Duration = Duration::from_secs(60);
/// Due scheduled sends fired per query (the pass repeats until none is due).
const FIRE_BATCH: i64 = 50;
/// How soon the timer looks again at something still overdue after a pass.
const OVERDUE_RETRY_MS: u64 = 250;

/// A `scheduled` row.
#[derive(Clone, Debug)]
struct ScheduledRow {
    schedule_id: ScheduleId,
    send_id: SendId,
    ask_id: Option<AskId>,
    key: ActorKey,
    home: HomeRef,
    isi: Option<String>,
    payload: SendPayload,
    notify: String,
    kind: String,
    replace: bool,
    due_at: i64,
    expires_at: Option<i64>,
    tz: Option<String>,
    warnings: Option<String>,
    created_at: i64,
}

const SCHEDULED_COLS: &str = "schedule_id, send_id, ask_id, context, org_id, actor_id, home_path, api_url, isi, payload, notify, kind, replace_current, due_at, expires_at, tz, warnings, created_at";

fn corrupt(what: &str, e: impl std::fmt::Display) -> Error {
    Error::new(
        ErrorCode::StoreCorrupt,
        format!("peekd.sqlite holds an unreadable scheduled {what}: {e}"),
    )
    .with_hint("move ~/Library/Application Support/Peek/peekd.sqlite aside and reopen Peek")
}

fn scheduled_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<ScheduledRow>> {
    let schedule_id: String = r.get(0)?;
    let send_id: String = r.get(1)?;
    let ask_id: Option<String> = r.get(2)?;
    let context: String = r.get(3)?;
    let org: String = r.get(4)?;
    let actor: String = r.get(5)?;
    let home_path: String = r.get(6)?;
    let api_url: String = r.get(7)?;
    let isi: Option<String> = r.get(8)?;
    let payload: Vec<u8> = r.get(9)?;
    let notify: String = r.get(10)?;
    let kind: String = r.get(11)?;
    let replace: i64 = r.get(12)?;
    let due_at: i64 = r.get(13)?;
    let expires_at: Option<i64> = r.get(14)?;
    let tz: Option<String> = r.get(15)?;
    let warnings: Option<String> = r.get(16)?;
    let created_at: i64 = r.get(17)?;
    Ok((|| {
        let context = Context::parse(&context).map_err(|e| corrupt("context", e))?;
        Ok(ScheduledRow {
            schedule_id: ScheduleId::parse(&schedule_id).map_err(|e| corrupt("id", e))?,
            send_id: SendId::parse(&send_id).map_err(|e| corrupt("send id", e))?,
            ask_id: ask_id
                .map(|a| AskId::parse(&a))
                .transpose()
                .map_err(|e| corrupt("ask id", e))?,
            key: ActorKey {
                context,
                org: OrgId::parse(&org).map_err(|e| corrupt("org", e))?,
                actor: ActorId::parse(&actor).map_err(|e| corrupt("actor", e))?,
            },
            home: HomeRef {
                home_path,
                api_url: ApiUrl::parse(&api_url).map_err(|e| corrupt("api url", e))?,
                context,
            },
            isi,
            payload: serde_json::from_slice(&payload).map_err(|e| corrupt("payload", e))?,
            notify,
            kind,
            replace: replace != 0,
            due_at,
            expires_at,
            tz,
            warnings,
            created_at,
        })
    })())
}

fn load_scheduled(c: &Connection, schedule_id: &str) -> Result<Option<ScheduledRow>> {
    c.query_row(
        &format!("SELECT {SCHEDULED_COLS} FROM scheduled WHERE schedule_id = ?1"),
        [schedule_id],
        scheduled_from_row,
    )
    .optional()
    .sql()?
    .transpose()
}

fn schedule_full(key: &ActorKey) -> Error {
    Error::new(
        ErrorCode::ScheduleFull,
        format!(
            "{} sends are already scheduled for {}; cancel some with `peek schedule cancel <id>` or `peek schedule clear`",
            limits::SCHEDULED_MAX,
            key.actor
        ),
    )
    .with_hint("peek schedule list    lists them, soonest first")
    .with_details(json!({"scheduled": limits::SCHEDULED_MAX, "limit": limits::SCHEDULED_MAX}))
}

fn waiting_reason(held: HeldReason) -> WaitingReason {
    match held {
        HeldReason::CarbonAway => WaitingReason::CarbonAway,
        HeldReason::Paused => WaitingReason::Paused,
        HeldReason::AppNotRunning => WaitingReason::AppNotRunning,
    }
}

/// Where a fired scheduled send went.
struct Fired {
    outcome: DueOutcome,
    replaced: Option<SendId>,
    reason: Option<WaitingReason>,
    position: Option<u32>,
}

impl Shared {
    /// `send` with `due_at` (contract §6.6): preconditions, images copied
    /// now, the voice chosen now, then stored (at most 500 per Silicon,
    /// counted in the inserting transaction). Nothing is shown yet.
    ///
    /// # Errors
    /// `side_not_registered`, `schedule_full`,
    /// image errors.
    pub(crate) async fn schedule_send(
        self: &SharedRef,
        caller: &Caller,
        op: SendOp,
        blobs: Vec<Vec<u8>>,
    ) -> Result<SendResult> {
        let Some(due) = op.due_at else {
            return Err(Error::internal("schedule_send needs due_at"));
        };
        let key = caller.key.clone();
        let slot = self.send_preconditions(&key).await?;
        let (show, ask) = self
            .store_images(op.show.clone(), op.ask.clone(), blobs)
            .await?;
        let (speech_info, stored_speech, mut warnings_out) = self.plan_speech(caller, &op).await;
        let now = self.now_ms();
        let payload = build_payload(&op, show, ask, stored_speech, now);
        let schedule_id = ScheduleId::generate();
        let send_id = SendId::generate();
        let ask_id = payload.ask.as_ref().map(|_| AskId::generate());
        if let Some(w) = self.ting_warning(caller).await {
            warnings_out.push(w);
        }
        if let Some(w) = self.take_fallback_warning(&key).await {
            warnings_out.push(w);
        }
        let stored_warnings: Vec<Value> = warnings_out
            .iter()
            .filter_map(|w| serde_json::to_value(w).ok())
            .collect();
        let notify = serde_json::to_string(&op.notify)
            .map_err(|e| Error::internal(format!("serializing notify failed: {e}")))?;
        let payload_bytes = serde_json::to_vec(&payload)
            .map_err(|e| Error::internal(format!("serializing a send failed: {e}")))?;
        let (k, home) = (key.clone(), caller.home.clone());
        let (sch, sid, aid) = (
            schedule_id.as_str().to_owned(),
            send_id.as_str().to_owned(),
            ask_id.as_ref().map(|a| a.as_str().to_owned()),
        );
        let (isi, kind, replace, tz) = (op.isi.clone(), op.kind(), op.replace, op.tz.clone());
        let expires_at = payload.expires_at;
        let due_ms = due.unix_ms();
        let warnings_json =
            (!stored_warnings.is_empty()).then(|| Value::Array(stored_warnings).to_string());
        self.db
            .tx(move |tx| {
                let n = scheduled_count(tx, &k)?;
                if usize::try_from(n).unwrap_or(usize::MAX) >= limits::SCHEDULED_MAX {
                    return Err(schedule_full(&k));
                }
                tx.execute(
                    &format!(
                        "INSERT INTO scheduled ({SCHEDULED_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)"
                    ),
                    params![
                        sch,
                        sid,
                        aid,
                        k.context_str(),
                        k.org.as_str(),
                        k.actor.as_str(),
                        home.home_path,
                        home.api_url.as_str(),
                        isi,
                        payload_bytes,
                        notify,
                        kind,
                        i64::from(replace),
                        due_ms,
                        expires_at,
                        tz,
                        warnings_json,
                        now
                    ],
                )
                .sql()?;
                Ok(())
            })
            .await?;
        self.timers_wake.notify_one();
        let mut rec = Record::new("send.scheduled", "ok")
            .with("slot", slot.get())
            .with("scheduled", true);
        rec.actor = Some((key.org.clone(), key.actor.clone()));
        rec.isi.clone_from(&op.isi);
        rec.testing = key.context.is_testing();
        self.record(rec);
        Ok(SendResult {
            send_id,
            ask_id,
            slot,
            status: SendStatus::Scheduled,
            speech: speech_info,
            warnings: warnings_out,
            queue_position: None,
            waiting: None,
            expires_at: expires_at.map(Timestamp::from_unix_ms),
            schedule_id: Some(schedule_id),
            due_at: Some(due),
            tz: op.tz,
            replaced_send_id: None,
        })
    }

    /// Fires one due scheduled send (contract §6.6 steps 1–5): deletes and
    /// materializes it in one transaction, expires it when its deadline has
    /// passed, else replaces (`--replace`) or queues it, then tells the
    /// Silicon with `peek.schedule.due`.
    ///
    /// # Errors
    /// Database failures.
    #[allow(clippy::too_many_lines)] // one algorithm, step by step
    pub(crate) async fn fire_scheduled(self: &SharedRef, schedule_id: &str) -> Result<()> {
        let mut core = self.core.lock().await;
        let sch = schedule_id.to_owned();
        let Some(row) = self.db.call(move |c| load_scheduled(c, &sch)).await? else {
            return Ok(());
        };
        let now = self.now_ms();
        let mut payload = row.payload.clone();
        // The TTS cache may have gained (or lost) this text since.
        if let (Some(text), Some(speech)) = (&payload.speak, payload.speech.as_mut())
            && let Some(model) = &speech.model
            && matches!(
                speech.status,
                StoredSpeechStatus::Pending | StoredSpeechStatus::Cached
            )
        {
            let cached = self
                .speech
                .cache
                .lookup(&TtsCache::key(model, text))
                .is_some();
            speech.status = if cached {
                StoredSpeechStatus::Cached
            } else {
                StoredSpeechStatus::Pending
            };
        }
        let payload_bytes = serde_json::to_vec(&payload)
            .map_err(|e| Error::internal(format!("serializing a send failed: {e}")))?;
        let expired = row.expires_at.is_some_and(|e| e <= now);
        let r = row.clone();
        let slot = self
            .db
            .tx(move |tx| {
                tx.execute(
                    "DELETE FROM scheduled WHERE schedule_id = ?1",
                    [r.schedule_id.as_str()],
                )
                .sql()?;
                let Some(slot) = crate::bubbles::slot_of(tx, &r.key)? else {
                    return Ok(None);
                };
                insert_send(
                    tx,
                    &NewSend {
                        send_id: r.send_id.as_str().to_owned(),
                        ask_id: r.ask_id.as_ref().map(|a| a.as_str().to_owned()),
                        key: r.key.clone(),
                        home: r.home.clone(),
                        slot,
                        isi: r.isi.clone(),
                        payload: payload_bytes,
                        notify: r.notify.clone(),
                        kind: r.kind.clone(),
                        expires_at: r.expires_at,
                        waiter: 0,
                        created_at: r.created_at,
                        queued_at: now,
                        schedule_id: Some(r.schedule_id.as_str().to_owned()),
                        due_at: Some(r.due_at),
                        warnings: r.warnings.clone(),
                    },
                )?;
                if expired {
                    let expires_at = r.expires_at.unwrap_or(now);
                    tx.execute(
                        "UPDATE sends SET closed_at = ?2, close_reason = 'expired' WHERE send_id = ?1",
                        params![r.send_id.as_str(), now],
                    )
                    .sql()?;
                    let context = r.key.context.data_context();
                    let due = TingData::ScheduleDue(ScheduleDue {
                        schema: SchemaV1,
                        schedule_id: r.schedule_id.clone(),
                        send_id: r.send_id.clone(),
                        ask_id: r.ask_id.clone(),
                        kind: r.kind.clone(),
                        due_at: Timestamp::from_unix_ms(r.due_at),
                        fired_at: Timestamp::from_unix_ms(now.max(r.due_at)),
                        outcome: DueOutcome::Expired,
                        replaced_send_id: None,
                        waiting_reason: None,
                        queue_position: None,
                        expires_at: r.expires_at.map(Timestamp::from_unix_ms),
                        slot,
                        context,
                    });
                    queue_ting_for(tx, &r.key, &r.home, &due, r.isi.clone(), r.schedule_id.as_str())?;
                    if let (Some(ask_id), Some(q)) = (&r.ask_id, &r.payload.ask) {
                        tx.execute(
                            "UPDATE asks SET state = 'expired', closed_at = ?2, delivered_via = 'ting' WHERE ask_id = ?1",
                            params![ask_id.as_str(), now],
                        )
                        .sql()?;
                        let data = TingData::AskExpired(AskExpired {
                            schema: SchemaV1,
                            ask_id: ask_id.clone(),
                            send_id: r.send_id.clone(),
                            question: q.question.clone(),
                            ask_type: q.ask_type(),
                            asked_at: Timestamp::from_unix_ms(r.created_at),
                            expired_at: Timestamp::from_unix_ms(now.max(r.created_at)),
                            slot,
                            context,
                            shown: Some(false),
                        });
                        let ev = queue_ting_for(tx, &r.key, &r.home, &data, r.isi.clone(), ask_id.as_str())?;
                        tx.execute(
                            "UPDATE asks SET event_id = ?2 WHERE ask_id = ?1",
                            params![ask_id.as_str(), ev],
                        )
                        .sql()?;
                    } else {
                        let data = TingData::SendExpired(SendExpired {
                            schema: SchemaV1,
                            send_id: r.send_id.clone(),
                            kind: r.kind.clone(),
                            created_at: Timestamp::from_unix_ms(r.created_at),
                            expires_at: Timestamp::from_unix_ms(expires_at),
                            expired_at: Timestamp::from_unix_ms(now.max(expires_at).max(r.created_at)),
                            shown: false,
                            shown_at: None,
                            scheduled: true,
                            schedule_id: Some(r.schedule_id.clone()),
                            slot,
                            context,
                        });
                        queue_ting_for(tx, &r.key, &r.home, &data, r.isi.clone(), r.send_id.as_str())?;
                    }
                }
                Ok(Some(slot))
            })
            .await?;
        let Some(slot) = slot else {
            tracing::error!(
                schedule = %row.schedule_id,
                actor = %row.key.actor,
                "a scheduled send came due but its Silicon holds no position; dropped"
            );
            return Ok(());
        };
        let key = row.key.clone();
        let fired = if expired {
            self.outbox_wake.notify_one();
            Fired {
                outcome: DueOutcome::Expired,
                replaced: None,
                reason: None,
                position: None,
            }
        } else {
            let mut bubble = Bubble::new(
                row.send_id.clone(),
                row.ask_id.clone(),
                payload.show.is_some(),
                payload.speak.is_some(),
            );
            let mut replaced = None;
            if row.replace
                && let Some(current) = core
                    .queues
                    .get(&key)
                    .and_then(|q| q.current.as_ref().map(|b| b.send_id.clone()))
                && let Some(w) = self
                    .withdraw_locked(&mut core, &key, &current, Withdraw::replaced())
                    .await?
            {
                if w.pushed {
                    bubble.replaces = Some(w.send_id.clone());
                }
                replaced = Some(w.send_id);
            }
            let queue = core.queues.entry(key.clone()).or_default();
            let position = if queue.current.is_none() {
                queue.current = Some(bubble);
                0
            } else if queue.waiting.len() < limits::QUEUE_MAX {
                queue.waiting.push_back(bubble);
                u32::try_from(queue.waiting.len()).unwrap_or(u32::MAX)
            } else {
                queue.overflow.push_back(bubble);
                u32::try_from(limits::QUEUE_MAX + queue.overflow.len()).unwrap_or(u32::MAX)
            };
            debug_assert!(queue.invariants_hold());
            if position > u32::try_from(limits::QUEUE_MAX).unwrap_or(u32::MAX) {
                let sid = row.send_id.as_str().to_owned();
                self.db
                    .call(move |c| {
                        c.execute("UPDATE sends SET overflow = 1 WHERE send_id = ?1", [sid])
                            .sql()
                    })
                    .await?;
            }
            if position == 0 {
                self.push_current_locked(&mut core, &key).await;
                if !self.ui.is_connected() && self.ui.carbon_available() {
                    self.launch_ui_soon();
                }
            } else {
                self.emit_queue_state_locked(&mut core, &key).await;
            }
            if row.expires_at.is_some() {
                self.timers_wake.notify_one();
            }
            let held = self.held_reason(
                core.queues
                    .get(&key)
                    .and_then(|q| q.current.as_ref())
                    .filter(|b| b.send_id == row.send_id),
            );
            match (position, replaced) {
                (0, Some(old)) => Fired {
                    outcome: DueOutcome::Replaced,
                    replaced: Some(old),
                    reason: held.map(waiting_reason),
                    position: None,
                },
                (0, None) if held.is_none() => Fired {
                    outcome: DueOutcome::Shown,
                    replaced: None,
                    reason: None,
                    position: None,
                },
                (0, None) => Fired {
                    outcome: DueOutcome::Queued,
                    replaced: None,
                    reason: held.map(waiting_reason),
                    position: Some(0),
                },
                (p, _) if p <= u32::try_from(limits::QUEUE_MAX).unwrap_or(u32::MAX) => Fired {
                    outcome: DueOutcome::Queued,
                    replaced: None,
                    reason: Some(WaitingReason::BehindOthers),
                    position: Some(p),
                },
                (p, _) => Fired {
                    outcome: DueOutcome::Queued,
                    replaced: None,
                    reason: Some(WaitingReason::QueueFull),
                    position: Some(p),
                },
            }
        };
        drop(core);
        if !expired {
            let data = TingData::ScheduleDue(ScheduleDue {
                schema: SchemaV1,
                schedule_id: row.schedule_id.clone(),
                send_id: row.send_id.clone(),
                ask_id: row.ask_id.clone(),
                kind: row.kind.clone(),
                due_at: Timestamp::from_unix_ms(row.due_at),
                fired_at: Timestamp::from_unix_ms(now.max(row.due_at)),
                outcome: fired.outcome,
                replaced_send_id: fired.replaced.clone(),
                waiting_reason: fired.reason,
                queue_position: fired.position,
                expires_at: row.expires_at.map(Timestamp::from_unix_ms),
                slot,
                context: key.context.data_context(),
            });
            let (k, home, isi, subject) = (
                key.clone(),
                row.home.clone(),
                row.isi.clone(),
                row.schedule_id.as_str().to_owned(),
            );
            self.db
                .tx(move |tx| queue_ting_for(tx, &k, &home, &data, isi, &subject).map(|_| ()))
                .await?;
            self.outbox_wake.notify_one();
        }
        tracing::info!(
            schedule = %row.schedule_id,
            send = %row.send_id,
            outcome = %crate::queue::enum_word(fired.outcome),
            late_ms = now.saturating_sub(row.due_at),
            "a scheduled send came due"
        );
        let mut rec = Record::new("schedule.fired", "ok")
            .with("slot", slot.get())
            .with("status", crate::queue::enum_word(fired.outcome))
            .with("scheduled", true);
        rec.actor = Some((key.org.clone(), key.actor.clone()));
        rec.isi.clone_from(&row.isi);
        rec.testing = key.context.is_testing();
        self.record(rec);
        Ok(())
    }

    /// `schedule.list`: the Silicon's scheduled sends, soonest first.
    ///
    /// # Errors
    /// Database failures.
    pub async fn schedule_list(&self, caller: &Caller) -> Result<ScheduleListResult> {
        let k = caller.key.clone();
        let (slot, rows) = self
            .db
            .call(move |c| {
                let slot = crate::bubbles::slot_of(c, &k)?;
                let mut st = c
                    .prepare(&format!(
                        "SELECT {SCHEDULED_COLS} FROM scheduled WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3
                         ORDER BY due_at, schedule_id"
                    ))
                    .sql()?;
                let rows = st
                    .query_map(
                        params![k.context_str(), k.org.as_str(), k.actor.as_str()],
                        scheduled_from_row,
                    )
                    .sql()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .sql()?;
                Ok((slot, rows.into_iter().collect::<Result<Vec<_>>>()?))
            })
            .await?;
        Ok(ScheduleListResult {
            slot,
            scheduled: rows
                .into_iter()
                .map(|r| ScheduledItem {
                    summary: r.payload.summary(),
                    schedule_id: r.schedule_id,
                    send_id: r.send_id,
                    ask_id: r.ask_id,
                    kind: r.kind,
                    due_at: Timestamp::from_unix_ms(r.due_at),
                    tz: r.tz,
                    expires_at: r.expires_at.map(Timestamp::from_unix_ms),
                    replace: r.replace,
                    created_at: Timestamp::from_unix_ms(r.created_at),
                })
                .collect(),
            limit: u32::try_from(limits::SCHEDULED_MAX).unwrap_or(u32::MAX),
        })
    }

    /// Deletes one of `key`'s scheduled sends (under the core lock, so it
    /// never races its firing). `None` when no such row is scheduled.
    pub(crate) async fn delete_scheduled(
        &self,
        key: &ActorKey,
        which: ScheduledRef<'_>,
    ) -> Result<Option<SendCancelResult>> {
        let _core = self.core.lock().await;
        let k = key.clone();
        let (column, value) = match which {
            ScheduledRef::Schedule(s) => ("schedule_id", s.as_str().to_owned()),
            ScheduledRef::Send(s) => ("send_id", s.as_str().to_owned()),
        };
        let row = self
            .db
            .tx(move |tx| {
                let row: Option<ScheduledRow> = tx
                    .query_row(
                        &format!(
                            "SELECT {SCHEDULED_COLS} FROM scheduled WHERE {column} = ?1 AND context = ?2 AND org_id = ?3 AND actor_id = ?4"
                        ),
                        params![value, k.context_str(), k.org.as_str(), k.actor.as_str()],
                        scheduled_from_row,
                    )
                    .optional()
                    .sql()?
                    .transpose()?;
                if let Some(r) = &row {
                    tx.execute(
                        "DELETE FROM scheduled WHERE schedule_id = ?1",
                        [r.schedule_id.as_str()],
                    )
                    .sql()?;
                }
                Ok(row)
            })
            .await?;
        Ok(row.map(|r| SendCancelResult {
            send_id: r.send_id,
            ask_id: r.ask_id,
            schedule_id: Some(r.schedule_id),
            was: CancelledFrom::Scheduled,
            queue_position: None,
            state: "cancelled".to_owned(),
            due_at: Some(Timestamp::from_unix_ms(r.due_at)),
            tz: r.tz,
        }))
    }

    /// `schedule.cancel` (`sch_…` or its `snd_…`): `cancelled`, or `fired`
    /// when it already came due (then it is in the queue or closed).
    ///
    /// # Errors
    /// `invalid_input` for another kind of ID; `schedule_not_found`.
    pub async fn schedule_cancel(
        &self,
        caller: &Caller,
        target: &str,
    ) -> Result<ScheduleCancelResult> {
        let key = caller.key.clone();
        let target = target.trim();
        let which_sch;
        let which_snd;
        let which = if target.starts_with(ScheduleId::PREFIX) {
            which_sch = ScheduleId::parse(target).map_err(|e| bad_schedule_id(target, &e))?;
            ScheduledRef::Schedule(&which_sch)
        } else if target.starts_with(SendId::PREFIX) {
            which_snd = SendId::parse(target).map_err(|e| bad_schedule_id(target, &e))?;
            ScheduledRef::Send(&which_snd)
        } else {
            return Err(bad_schedule_id(
                target,
                &Error::invalid_input("unknown prefix"),
            ));
        };
        if let Some(r) = self.delete_scheduled(&key, which).await? {
            let schedule_id = r
                .schedule_id
                .ok_or_else(|| Error::internal("a scheduled row without its ID"))?;
            return Ok(ScheduleCancelResult {
                schedule_id,
                send_id: r.send_id,
                state: "cancelled".to_owned(),
                due_at: r.due_at,
                tz: r.tz,
            });
        }
        let (k, t) = (key.clone(), target.to_owned());
        let fired: Option<(String, String, Option<i64>)> = self
            .db
            .call(move |c| {
                c.query_row(
                    "SELECT schedule_id, send_id, due_at FROM sends
                     WHERE (schedule_id = ?1 OR send_id = ?1) AND schedule_id IS NOT NULL
                       AND context = ?2 AND org_id = ?3 AND actor_id = ?4",
                    params![t, k.context_str(), k.org.as_str(), k.actor.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .sql()
            })
            .await?;
        if let Some((sch, snd, due_at)) = fired
            && let (Ok(schedule_id), Ok(send_id)) = (ScheduleId::parse(&sch), SendId::parse(&snd))
        {
            return Ok(ScheduleCancelResult {
                schedule_id,
                send_id,
                state: "fired".to_owned(),
                due_at: due_at.map(Timestamp::from_unix_ms),
                tz: None,
            });
        }
        match which {
            ScheduledRef::Schedule(s) => Err(schedule_not_found(s, &key)),
            ScheduledRef::Send(s) => Err(Error::new(
                ErrorCode::ScheduleNotFound,
                format!("no scheduled send of {} has the send ID {s}", key.actor),
            )
            .with_hint("peek schedule list    lists this Silicon's scheduled sends")
            .with_details(json!({"id": s}))),
        }
    }

    /// `schedule.clear`: every scheduled send of the Silicon is cancelled.
    ///
    /// # Errors
    /// Database failures.
    pub async fn schedule_clear(&self, caller: &Caller) -> Result<ScheduleClearResult> {
        let _core = self.core.lock().await;
        let k = caller.key.clone();
        let ids: Vec<String> = self
            .db
            .tx(move |tx| {
                let mut st = tx
                    .prepare(
                        "SELECT schedule_id FROM scheduled WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3
                         ORDER BY due_at, schedule_id",
                    )
                    .sql()?;
                let ids: Vec<String> = st
                    .query_map(params![k.context_str(), k.org.as_str(), k.actor.as_str()], |r| r.get(0))
                    .sql()?
                    .collect::<rusqlite::Result<_>>()
                    .sql()?;
                tx.execute(
                    "DELETE FROM scheduled WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3",
                    params![k.context_str(), k.org.as_str(), k.actor.as_str()],
                )
                .sql()?;
                Ok(ids)
            })
            .await?;
        Ok(ScheduleClearResult {
            cancelled: ids
                .iter()
                .filter_map(|s| ScheduleId::parse(s).ok())
                .collect(),
        })
    }

    // -------------------------------------------------------------- timers

    /// One timer pass: fires every due scheduled send (oldest `due_at`
    /// first), expires every send past its deadline, and closes non-ask
    /// bubbles Peek.app never reported done.
    pub async fn timer_pass(self: &SharedRef) {
        loop {
            let now = self.now_ms();
            let due: Vec<String> = self
                .db
                .call(move |c| {
                    let mut st = c
                        .prepare(
                            "SELECT schedule_id FROM scheduled WHERE due_at <= ?1 ORDER BY due_at, schedule_id LIMIT ?2",
                        )
                        .sql()?;
                    let v = st
                        .query_map(params![now, FIRE_BATCH], |r| r.get(0))
                        .sql()?
                        .collect::<rusqlite::Result<Vec<String>>>()
                        .sql()?;
                    Ok(v)
                })
                .await
                .unwrap_or_default();
            if due.is_empty() {
                break;
            }
            let mut failed = false;
            for id in &due {
                if let Err(e) = self.fire_scheduled(id).await {
                    tracing::error!(schedule = %id, error = %e, "firing a scheduled send failed");
                    failed = true;
                }
            }
            if failed || due.len() < usize::try_from(FIRE_BATCH).unwrap_or(usize::MAX) {
                break;
            }
        }
        let now = self.now_ms();
        let expired: Vec<(String, Option<String>)> = self
            .db
            .call(move |c| {
                let mut st = c
                    .prepare(
                        "SELECT s.send_id, a.ask_id FROM sends s
                         LEFT JOIN asks a ON a.send_id = s.send_id AND a.state = 'pending'
                         WHERE s.closed_at IS NULL AND s.expires_at IS NOT NULL AND s.expires_at <= ?1
                         ORDER BY s.expires_at, s.send_id",
                    )
                    .sql()?;
                let v = st
                    .query_map([now], |r| Ok((r.get(0)?, r.get(1)?)))
                    .sql()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .sql()?;
                Ok(v)
            })
            .await
            .unwrap_or_default();
        for (send, ask) in expired {
            match ask.map(|a| AskId::parse(&a)) {
                Some(Ok(id)) => {
                    if let Err(e) = self.resolve_ask(&id, Resolution::Expired).await {
                        tracing::debug!(ask = %id, error = %e, "expiring an ask failed");
                    }
                }
                Some(Err(_)) => {}
                None => {
                    if let Ok(id) = SendId::parse(&send)
                        && let Err(e) = self.expire_send(&id).await
                    {
                        tracing::warn!(send = %send, error = %e, "expiring a send failed");
                    }
                }
            }
        }
        let overdue: Vec<SendId> = {
            let core = self.core.lock().await;
            let now_i = Instant::now();
            core.queues
                .values()
                .filter_map(|q| q.current.as_ref())
                .filter(|b| b.ask_id.is_none() && b.deadline.is_some_and(|d| d <= now_i))
                .map(|b| b.send_id.clone())
                .collect()
        };
        for s in overdue {
            tracing::info!(send = %s, "closing a bubble Peek.app never reported done");
            let _ = self.close_send(&s, "timeout").await;
        }
    }

    /// How long the timer loop may sleep: until the next deadline, expiry or
    /// due time (at most a minute), and at most the catch-up cap while
    /// anything is scheduled or expiring.
    async fn timer_wait(&self) -> Duration {
        let next: (Option<i64>, Option<i64>) = self
            .db
            .call(|c| {
                let expiry: Option<i64> = c
                    .query_row(
                        "SELECT min(expires_at) FROM sends WHERE closed_at IS NULL AND expires_at IS NOT NULL",
                        [],
                        |r| r.get(0),
                    )
                    .sql()?;
                let due: Option<i64> = c
                    .query_row("SELECT min(due_at) FROM scheduled", [], |r| r.get(0))
                    .sql()?;
                Ok((expiry, due))
            })
            .await
            .unwrap_or((None, None));
        let next_deadline = {
            let core = self.core.lock().await;
            core.queues
                .values()
                .filter_map(|q| q.current.as_ref().and_then(|b| b.deadline))
                .min()
        };
        let now = self.now_ms();
        let mut wait = TIMER_IDLE;
        for at in [next.0, next.1].into_iter().flatten() {
            // Anything still overdue after the pass is being handled by
            // another path (an answer racing the expiry) or failed: retry it
            // shortly instead of spinning.
            let ms = match u64::try_from(at.saturating_sub(now)) {
                Ok(ms) if ms > 0 => ms,
                _ => OVERDUE_RETRY_MS,
            };
            wait = wait.min(Duration::from_millis(ms));
        }
        if next.0.is_some() || next.1.is_some() {
            wait = wait.min(self.cfg.timings.timer_catchup_cap);
        }
        if let Some(d) = next_deadline {
            wait = wait.min(d.saturating_duration_since(Instant::now()));
        }
        wait
    }

    /// The expiry, scheduling and watchdog loop. Runs until shutdown; its
    /// first pass (right after start) fires everything that came due while
    /// peekd was not running.
    pub async fn run_timers(self: SharedRef) {
        let mut shutdown = self.shutdown.subscribe();
        let mut images_pruned = Instant::now();
        let mut last: Option<(Instant, i64)> = None;
        loop {
            if *shutdown.borrow() {
                return;
            }
            if images_pruned.elapsed() >= crate::bubbles::IMAGE_CACHE_PRUNE_EVERY {
                images_pruned = Instant::now();
                self.prune_image_cache().await;
            }
            let wall = self.now_ms();
            if let Some((mono, before)) = last {
                let mono_ms = i64::try_from(mono.elapsed().as_millis()).unwrap_or(i64::MAX);
                let jump = wall.saturating_sub(before).saturating_sub(mono_ms);
                if jump > JUMP_LOG_THRESHOLD_MS {
                    tracing::info!(
                        "wall clock jumped {} s ahead of the monotonic clock (sleep/wake); catching up",
                        jump / 1000
                    );
                }
            }
            last = Some((Instant::now(), wall));
            self.timer_pass().await;
            let wait = self.timer_wait().await;
            tokio::select! {
                () = tokio::time::sleep(wait.max(Duration::from_millis(20))) => {}
                () = self.timers_wake.notified() => {}
                _ = shutdown.changed() => {}
            }
        }
    }
}

fn bad_schedule_id(target: &str, e: &Error) -> Error {
    Error::invalid_input(format!(
        "`{target}` is not a schedule ID; expected sch_… (or the snd_… it will send as)"
    ))
    .with_hint("peek schedule list    lists the scheduled sends and their IDs")
    .with_details(json!({"field": "id", "value": target, "reason": e.message()}))
}
