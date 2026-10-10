//! Queue v2 (contract §6.2–§6.7): withdrawing sends without a ting (the
//! Silicon's own `send.cancel`, `queue.clear`, `--replace`, `unregister`),
//! expiry of speak/show sends, overflow promotion, the "+X" badge
//! (`queue.state`) and the `queue.list` view.
//!
//! Everything here runs under [`Shared::core`] (the `*_locked` functions take
//! the guard), with the database written in the same critical section, as in
//! [`crate::bubbles`].

use rusqlite::{OptionalExtension as _, params};
use serde_json::json;
use silicon_peek_client::{
    Error, ErrorCode, Result,
    ids::{AskId, ScheduleId, SendId},
    ipc::{
        cli::{
            AskResult, AskState, CancelledFrom, QueueClearResult, QueueItem, QueueItemState,
            QueueListResult, SendCancelResult,
        },
        ui::{CancelReason, PeekCancel, QueueState},
    },
    schema::limits,
    timestamp::Timestamp,
    ting::{SchemaV1, SendExpired, TingData},
};
use tokio::sync::oneshot;

use crate::{
    bubbles::{SendRow, Slotted, ask_of_send, load_send, queue_ting, slot_of, take_from_queue},
    db::SqlResult as _,
    state::{ActorKey, Caller, Core, Shared, SharedRef, WaiterMsg},
    telemetry::Record,
};

/// How a send is withdrawn without a ting.
#[derive(Clone, Debug)]
pub(crate) struct Withdraw {
    /// What Peek.app is told (`peek.cancel.reason`).
    pub ui_reason: CancelReason,
    /// `sends.close_reason` of a non-ask send.
    pub close_reason: &'static str,
    /// The state an ask closes in (`cancelled` or `replaced`); its send's
    /// `close_reason` is the same word.
    pub ask_state: AskState,
    /// A ting queued in the same transaction (expiry of a speak/show).
    pub ting: Option<(SendRow, TingData)>,
}

impl Withdraw {
    /// The Silicon's own `--replace`.
    pub(crate) fn replaced() -> Self {
        Self {
            ui_reason: CancelReason::Replaced,
            close_reason: "replaced",
            ask_state: AskState::Replaced,
            ting: None,
        }
    }

    /// `peek cancel` (and `queue clear` with its own close reason).
    pub(crate) fn cancelled(close_reason: &'static str) -> Self {
        Self {
            ui_reason: CancelReason::CancelledBySilicon,
            close_reason,
            ask_state: AskState::Cancelled,
            ting: None,
        }
    }
}

/// A send taken out of its queue.
#[derive(Clone, Debug)]
pub(crate) struct Withdrawn {
    /// The send.
    pub send_id: SendId,
    /// Its ask.
    pub ask_id: Option<AskId>,
    /// Where it was.
    pub was: CancelledFrom,
    /// 0 = current, else its place in line.
    pub queue_position: u32,
    /// Whether `peek.show` had reached the UI.
    pub pushed: bool,
}

/// What [`Shared::cancel_actor_bubbles`] cancelled.
#[derive(Clone, Debug, Default)]
pub struct CancelledAll {
    /// Pending asks.
    pub asks: Vec<AskId>,
    /// Non-ask sends that were current or waiting.
    pub sends: Vec<SendId>,
    /// Scheduled sends that were not due yet.
    pub scheduled: Vec<ScheduleId>,
}

/// The `ask.result` a `--wait` connection gets for an ask the Silicon
/// withdrew itself (no answer).
pub(crate) fn cancelled_result(ask_id: &AskId, state: AskState) -> AskResult {
    AskResult {
        ask_id: ask_id.clone(),
        state,
        answer: None,
        via: None,
        transcript: None,
        answered_at: None,
    }
}

impl Shared {
    /// Where a send sits and its place in line, without changing anything.
    fn locate(core: &Core, key: &ActorKey, send_id: &SendId) -> Option<(Slotted, u32)> {
        let q = core.queues.get(key)?;
        if let Some(b) = q.current.as_ref().filter(|b| &b.send_id == send_id) {
            return Some((Slotted::Current { pushed: b.pushed }, 0));
        }
        if let Some(i) = q.waiting.iter().position(|b| &b.send_id == send_id) {
            return Some((Slotted::Waiting(i), position(i)));
        }
        q.overflow
            .iter()
            .position(|b| &b.send_id == send_id)
            .map(|i| (Slotted::Overflow(i), position(limits::QUEUE_MAX + i)))
    }

    /// Where the current send is, for `queue.list` and `send.cancel`:
    /// `on_screen` once it reached the UI and nothing holds it, else `held`.
    fn current_state(&self, core: &Core, key: &ActorKey) -> QueueItemState {
        let current = core.queues.get(key).and_then(|q| q.current.as_ref());
        if self.held_reason(current).is_none() && current.is_some_and(|b| b.pushed || b.shown) {
            QueueItemState::OnScreen
        } else {
            QueueItemState::Held
        }
    }

    /// Takes `send_id` out of `key`'s queue and closes it without a ting
    /// (unless `how.ting` carries one): an ask closes as `how.ask_state` and
    /// a live `--wait` gets that state at once (no ting depends on its ack);
    /// a non-ask closes with `how.close_reason`. Speech stops, and Peek.app
    /// gets `peek.cancel` when it had the bubble. The caller advances the
    /// queue ([`Shared::after_removal_locked`]); `--replace` puts its own
    /// send in first.
    ///
    /// An ask whose answer (or expiry) is being recorded right now is left
    /// to it (`None`), except for `--replace`, which takes the bubble anyway
    /// and lets the answer be delivered.
    ///
    /// # Errors
    /// Database failures.
    pub(crate) async fn withdraw_locked(
        self: &SharedRef,
        core: &mut Core,
        key: &ActorKey,
        send_id: &SendId,
        how: Withdraw,
    ) -> Result<Option<Withdrawn>> {
        let Some((slotted, queue_position)) = Self::locate(core, key, send_id) else {
            return Ok(None);
        };
        let state = match slotted {
            Slotted::Current { .. } => match self.current_state(core, key) {
                QueueItemState::OnScreen => CancelledFrom::OnScreen,
                _ => CancelledFrom::Held,
            },
            Slotted::Waiting(_) => CancelledFrom::Waiting,
            Slotted::Overflow(_) => CancelledFrom::DueWaiting,
        };
        let ask_id = core
            .queues
            .get(key)
            .and_then(|q| {
                q.current
                    .iter()
                    .chain(q.waiting.iter())
                    .chain(q.overflow.iter())
                    .find(|b| &b.send_id == send_id)
            })
            .and_then(|b| b.ask_id.clone());
        let answering = ask_id.as_ref().is_some_and(|a| core.resolving.contains(a));
        if answering && how.ask_state != AskState::Replaced {
            return Ok(None);
        }
        let pushed = matches!(
            take_from_queue(core, key, send_id),
            Some(Slotted::Current { pushed: true })
        );
        if !answering {
            self.close_withdrawn(send_id, ask_id.as_ref(), &how).await?;
        }
        self.speech.cancel(send_id);
        if pushed {
            // A Peek.app older than build 1002 knows no `replaced` reason.
            let reason = if how.ui_reason == CancelReason::Replaced && !self.ui_reports_shown() {
                CancelReason::CancelledBySilicon
            } else {
                how.ui_reason
            };
            self.ui.event(
                &PeekCancel {
                    send_id: send_id.clone(),
                    reason,
                },
                Vec::new(),
            );
        }
        Ok(Some(Withdrawn {
            send_id: send_id.clone(),
            ask_id,
            was: state,
            queue_position,
            pushed,
        }))
    }

    /// Closes a withdrawn send's rows (and queues `how.ting`) in one
    /// transaction, then hands a live `--wait` its final state.
    async fn close_withdrawn(
        &self,
        send_id: &SendId,
        ask_id: Option<&AskId>,
        how: &Withdraw,
    ) -> Result<()> {
        let now = self.now_ms();
        let sid = send_id.as_str().to_owned();
        let aid = ask_id.map(|a| a.as_str().to_owned());
        let ask_state = enum_word(how.ask_state);
        let close_reason = how.close_reason;
        let ting = how.ting.clone();
        let queued_ting = self
            .db
            .tx(move |tx| {
                let reason = if let Some(aid) = &aid {
                    tx.execute(
                        "UPDATE asks SET state = ?2, closed_at = ?3, waiter = 0 WHERE ask_id = ?1 AND state = 'pending'",
                        params![aid, ask_state, now],
                    )
                    .sql()?;
                    ask_state.as_str()
                } else {
                    close_reason
                };
                tx.execute(
                    "UPDATE sends SET closed_at = ?2, close_reason = ?3 WHERE send_id = ?1 AND closed_at IS NULL",
                    params![sid, now, reason],
                )
                .sql()?;
                if let Some((send, data)) = &ting {
                    queue_ting(tx, send, data, send.isi.clone(), &sid)?;
                    return Ok(true);
                }
                Ok(false)
            })
            .await?;
        if queued_ting {
            self.outbox_wake.notify_one();
        }
        if let Some(a) = ask_id {
            let waiter = self.waiters.lock().ok().and_then(|mut w| w.remove(a));
            if let Some(tx) = waiter {
                // Nothing depends on the CLI's ack: no ting is sent for the
                // Silicon's own action either way.
                let (ack, _) = oneshot::channel();
                let _ = tx.send(WaiterMsg {
                    result: cancelled_result(a, how.ask_state),
                    ack,
                });
            }
            let _ = std::fs::remove_file(self.paths.recording(a.as_str()));
        }
        Ok(())
    }

    /// After a send left `key`'s queue: the next one becomes current when the
    /// current one left, else the first overflow send takes the free waiting
    /// spot; the badge follows.
    pub(crate) async fn after_removal_locked(
        self: &SharedRef,
        core: &mut Core,
        key: &ActorKey,
        from: Slotted,
    ) {
        if matches!(from, Slotted::Current { .. }) {
            self.advance_locked(core, key).await;
            return;
        }
        self.promote_overflow_locked(core, key).await;
        self.emit_queue_state_locked(core, key).await;
        if core
            .queues
            .get(key)
            .is_some_and(crate::state::ActorQueue::is_empty)
        {
            core.queues.remove(key);
        }
    }

    /// Moves due scheduled sends from overflow into free waiting spots
    /// (`sends.overflow = 0`), oldest first.
    pub(crate) async fn promote_overflow_locked(&self, core: &mut Core, key: &ActorKey) {
        let Some(q) = core.queues.get_mut(key) else {
            return;
        };
        if q.current.is_none() && q.waiting.is_empty() {
            q.current = q.overflow.pop_front();
        }
        let mut promoted = Vec::new();
        while q.current.is_some() && q.waiting.len() < limits::QUEUE_MAX {
            let Some(b) = q.overflow.pop_front() else {
                break;
            };
            promoted.push(b.send_id.as_str().to_owned());
            q.waiting.push_back(b);
        }
        debug_assert!(q.invariants_hold());
        if promoted.is_empty() {
            return;
        }
        if let Err(e) = self
            .db
            .tx(move |tx| {
                for id in promoted {
                    tx.execute("UPDATE sends SET overflow = 0 WHERE send_id = ?1", [id])
                        .sql()?;
                }
                Ok(())
            })
            .await
        {
            tracing::warn!(error = %e, "recording promoted scheduled sends failed");
        }
    }

    /// Sends `queue.state` when the Silicon's current bubble is pushed and
    /// its waiting count differs from what Peek.app last got.
    pub(crate) async fn emit_queue_state_locked(&self, core: &mut Core, key: &ActorKey) {
        let Some(q) = core.queues.get(key) else {
            return;
        };
        let Some(current) = q.current.as_ref().filter(|b| b.pushed) else {
            return;
        };
        let waiting = q.waiting_count();
        if q.last_badge == Some(waiting) {
            return;
        }
        let send_id = current.send_id.clone();
        let k = key.clone();
        let slot = match self.db.call(move |c| slot_of(c, &k)).await {
            Ok(Some(slot)) => slot,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, "reading a slot for queue.state failed");
                return;
            }
        };
        let sent = self.ui.event(
            &QueueState {
                slot,
                context: key.context,
                send_id,
                waiting,
            },
            Vec::new(),
        );
        if sent && let Some(q) = core.queues.get_mut(key) {
            q.last_badge = Some(waiting);
        }
    }

    /// A speak/show send passed its deadline (contract §6.5): it leaves the
    /// queue (sliding away if it was on screen) and the Silicon gets
    /// `peek.send.expired` saying whether it was shown.
    ///
    /// # Errors
    /// Database failures.
    pub(crate) async fn expire_send(self: &SharedRef, send_id: &SendId) -> Result<()> {
        let mut core = self.core.lock().await;
        let sid = send_id.as_str().to_owned();
        let (send, slot) = self
            .db
            .call(move |c| {
                let send = load_send(c, &sid)?;
                let slot = match &send {
                    Some(s) => slot_of(c, &s.key)?,
                    None => None,
                };
                Ok((send, slot))
            })
            .await?;
        let Some(send) = send.filter(|s| s.closed_at.is_none()) else {
            return Ok(());
        };
        let Some(expires_at) = send.expires_at else {
            return Ok(());
        };
        let now = self.now_ms();
        let data = TingData::SendExpired(SendExpired {
            schema: SchemaV1,
            send_id: send.send_id.clone(),
            kind: send.kind.clone(),
            created_at: Timestamp::from_unix_ms(send.created_at),
            expires_at: Timestamp::from_unix_ms(expires_at),
            expired_at: Timestamp::from_unix_ms(now.max(expires_at).max(send.created_at)),
            shown: send.shown_at.is_some(),
            shown_at: send.shown_at.map(Timestamp::from_unix_ms),
            scheduled: send.schedule_id.is_some(),
            schedule_id: send.schedule_id.clone(),
            slot: slot.unwrap_or(send.slot),
            context: send.key.context.data_context(),
        });
        let key = send.key.clone();
        let shown = send.shown_at.is_some();
        let how = Withdraw {
            ui_reason: CancelReason::Expired,
            close_reason: "expired",
            ask_state: AskState::Expired,
            ting: Some((send.clone(), data.clone())),
        };
        let from = Self::locate(&core, &key, send_id).map(|(s, _)| s);
        if self
            .withdraw_locked(&mut core, &key, send_id, how)
            .await?
            .is_none()
        {
            // Not in any queue (a row the queue lost): close it with its ting.
            let sid = send_id.as_str().to_owned();
            self.db
                .tx(move |tx| {
                    let n = tx
                        .execute(
                            "UPDATE sends SET closed_at = ?2, close_reason = 'expired' WHERE send_id = ?1 AND closed_at IS NULL",
                            params![sid, now],
                        )
                        .sql()?;
                    if n == 1 {
                        queue_ting(tx, &send, &data, send.isi.clone(), &sid)?;
                    }
                    Ok(())
                })
                .await?;
            self.outbox_wake.notify_one();
        }
        if let Some(from) = from {
            self.after_removal_locked(&mut core, &key, from).await;
        }
        let mut rec = Record::new("send.expired", "ok")
            .with("status", if shown { "shown" } else { "not_shown" });
        if let Some(slot) = slot {
            rec = rec.with("slot", slot.get());
        }
        rec.actor = Some((key.account.clone(), key.actor.clone()));

        self.record(rec);
        Ok(())
    }

    // ------------------------------------------------------------- CLI ops

    /// `queue.list`: the Silicon's current send and the ones waiting.
    ///
    /// # Errors
    /// Database failures.
    pub async fn queue_list(&self, caller: &Caller) -> Result<QueueListResult> {
        let key = caller.key.clone();
        let k = key.clone();
        let (slot, scheduled) = self
            .db
            .call(move |c| {
                let slot = slot_of(c, &k)?;
                let scheduled = scheduled_count(c, &k)?;
                Ok((slot, scheduled))
            })
            .await?;
        let scheduled_limit = u32::try_from(limits::SCHEDULED_MAX).unwrap_or(u32::MAX);
        let limit = u32::try_from(limits::QUEUE_MAX).unwrap_or(u32::MAX);
        let Some(slot) = slot else {
            return Ok(QueueListResult {
                slot: None,
                limit,
                on_screen: None,
                waiting: Vec::new(),
                held: None,
                scheduled,
                scheduled_limit,
            });
        };
        let core = self.core.lock().await;
        let Some(q) = core.queues.get(&key) else {
            return Ok(QueueListResult {
                slot: Some(slot),
                limit,
                on_screen: None,
                waiting: Vec::new(),
                held: None,
                scheduled,
                scheduled_limit,
            });
        };
        let held = self.held_reason(q.current.as_ref());
        let current_state = self.current_state(&core, &key);
        let mut entries: Vec<(SendId, Option<AskId>, QueueItemState, u32)> = Vec::new();
        if let Some(b) = &q.current {
            entries.push((b.send_id.clone(), b.ask_id.clone(), current_state, 0));
        }
        for (i, b) in q.waiting.iter().enumerate() {
            entries.push((
                b.send_id.clone(),
                b.ask_id.clone(),
                QueueItemState::Waiting,
                position(i),
            ));
        }
        for (i, b) in q.overflow.iter().enumerate() {
            entries.push((
                b.send_id.clone(),
                b.ask_id.clone(),
                QueueItemState::DueWaiting,
                position(limits::QUEUE_MAX + i),
            ));
        }
        drop(core);
        let items = self.queue_items(entries).await?;
        let mut items = items.into_iter();
        let mut on_screen = None;
        let mut waiting = Vec::new();
        for item in items.by_ref() {
            if item.queue_position == 0 {
                on_screen = Some(item);
            } else {
                waiting.push(item);
            }
        }
        Ok(QueueListResult {
            slot: Some(slot),
            limit,
            on_screen,
            waiting,
            held,
            scheduled,
            scheduled_limit,
        })
    }

    /// The `queue.list` rows of `entries` (send, ask, state, position).
    async fn queue_items(
        &self,
        entries: Vec<(SendId, Option<AskId>, QueueItemState, u32)>,
    ) -> Result<Vec<QueueItem>> {
        let now = self.now_ms();
        self.db
            .call(move |c| {
                let mut out = Vec::new();
                for (send_id, ask_id, state, pos) in entries {
                    let Some(row) = load_send(c, send_id.as_str())? else {
                        continue;
                    };
                    out.push(QueueItem {
                        send_id,
                        ask_id,
                        kind: row.kind.clone(),
                        summary: row.payload.summary(),
                        state,
                        queue_position: pos,
                        created_at: Timestamp::from_unix_ms(row.created_at),
                        queued_at: Timestamp::from_unix_ms(row.queued_at),
                        age_ms: u64::try_from(now.saturating_sub(row.created_at)).unwrap_or(0),
                        expires_at: row.expires_at.map(Timestamp::from_unix_ms),
                        shown_at: row.shown_at.map(Timestamp::from_unix_ms),
                        schedule_id: row.schedule_id.clone(),
                        due_at: row.due_at.map(Timestamp::from_unix_ms),
                    });
                }
                Ok(out)
            })
            .await
    }

    /// `queue.clear`: every waiting and overflow send is withdrawn in order
    /// (asks close `cancelled`, others `cleared`); with `all` the current one
    /// too. One `queue.state` at the end. No tings.
    ///
    /// # Errors
    /// Database failures.
    pub async fn queue_clear(
        self: &SharedRef,
        caller: &Caller,
        all: bool,
    ) -> Result<QueueClearResult> {
        let key = caller.key.clone();
        let mut core = self.core.lock().await;
        let (current, rest): (Option<SendId>, Vec<SendId>) =
            core.queues.get(&key).map_or((None, Vec::new()), |q| {
                (
                    q.current.as_ref().map(|b| b.send_id.clone()),
                    q.waiting
                        .iter()
                        .chain(q.overflow.iter())
                        .map(|b| b.send_id.clone())
                        .collect(),
                )
            });
        let mut cancelled = Vec::new();
        let mut current_gone = false;
        if all
            && let Some(c) = &current
            && self
                .withdraw_locked(&mut core, &key, c, Withdraw::cancelled("cleared"))
                .await?
                .is_some()
        {
            cancelled.push(c.clone());
            current_gone = true;
        }
        for id in rest {
            if self
                .withdraw_locked(&mut core, &key, &id, Withdraw::cancelled("cleared"))
                .await?
                .is_some()
            {
                cancelled.push(id);
            }
        }
        if current_gone {
            self.advance_locked(&mut core, &key).await;
        } else {
            self.after_removal_locked(&mut core, &key, Slotted::Waiting(0))
                .await;
        }
        drop(core);
        let slot = self
            .db
            .call({
                let k = key.clone();
                move |c| slot_of(c, &k)
            })
            .await
            .ok()
            .flatten();
        let mut rec = Record::new("queue.cleared", "ok").with("queue_waiting", cancelled.len());
        if let Some(slot) = slot {
            rec = rec.with("slot", slot.get());
        }
        rec.actor = Some((key.account.clone(), key.actor.clone()));

        self.record(rec);
        Ok(QueueClearResult {
            cancelled,
            on_screen: current,
            on_screen_cancelled: current_gone,
        })
    }

    /// `send.cancel` (contract §6.7): `sch_…` (or a still-scheduled `snd_…`)
    /// deletes the scheduled send; `ask_…` means its send; an open send of
    /// the caller is withdrawn wherever it is; a closed one reports how it
    /// closed. No tings.
    ///
    /// # Errors
    /// `invalid_input` for anything but `snd_…`, `ask_…` or `sch_…`;
    /// `send_not_found` / `schedule_not_found` when it is not the caller's.
    pub async fn send_cancel(
        self: &SharedRef,
        caller: &Caller,
        target: &str,
    ) -> Result<SendCancelResult> {
        let key = caller.key.clone();
        let target = target.trim();
        let send_id = match self.cancel_target(&key, target).await? {
            Target::Send(s) => s,
            Target::Done(r) => return Ok(r),
        };
        if let Some(r) = self
            .delete_scheduled(&key, ScheduledRef::Send(&send_id))
            .await?
        {
            return Ok(r);
        }
        let sid = send_id.as_str().to_owned();
        let (row, ask) = self
            .db
            .call(move |c| Ok((load_send(c, &sid)?, ask_of_send(c, &sid)?)))
            .await?;
        let Some(row) = row.filter(|r| r.key == key) else {
            return Err(send_not_found(target, &key));
        };
        let closed = |state: String| SendCancelResult {
            send_id: row.send_id.clone(),
            ask_id: ask.as_ref().map(|a| a.ask_id.clone()),
            schedule_id: row.schedule_id.clone(),
            was: CancelledFrom::Closed,
            queue_position: None,
            state,
            due_at: row.due_at.map(Timestamp::from_unix_ms),
            tz: None,
        };
        if row.closed_at.is_some() {
            let state = ask.as_ref().map_or_else(
                || {
                    row.close_reason
                        .clone()
                        .unwrap_or_else(|| "closed".to_owned())
                },
                |a| enum_word(a.state),
            );
            return Ok(closed(state));
        }
        let mut core = self.core.lock().await;
        let from = Self::locate(&core, &key, &send_id).map(|(s, _)| s);
        let withdrawn = self
            .withdraw_locked(&mut core, &key, &send_id, Withdraw::cancelled("cancelled"))
            .await?;
        if let Some(from) = from
            && withdrawn.is_some()
        {
            self.after_removal_locked(&mut core, &key, from).await;
        }
        drop(core);
        let Some(w) = withdrawn else {
            // Being answered or expired right now, or already closed: report
            // the state it ends in.
            let sid = send_id.as_str().to_owned();
            let (row, ask) = self
                .db
                .call(move |c| Ok((load_send(c, &sid)?, ask_of_send(c, &sid)?)))
                .await?;
            let state = ask.map_or_else(
                || {
                    row.and_then(|r| r.close_reason)
                        .unwrap_or_else(|| "pending".to_owned())
                },
                |a| enum_word(a.state),
            );
            return Ok(closed(state));
        };
        let mut rec = Record::new("send.cancelled", "ok")
            .with("status", enum_word(w.was))
            .with("slot", row.slot.get());
        rec.actor = Some((row.key.account.clone(), row.key.actor.clone()));

        self.record(rec);
        Ok(SendCancelResult {
            send_id: w.send_id,
            ask_id: w.ask_id,
            schedule_id: row.schedule_id.clone(),
            was: w.was,
            queue_position: Some(w.queue_position),
            state: "cancelled".to_owned(),
            due_at: row.due_at.map(Timestamp::from_unix_ms),
            tz: None,
        })
    }
}

/// What a `send.cancel` target names.
enum Target {
    /// This send (open, closed, or still scheduled).
    Send(SendId),
    /// A scheduled send that was deleted.
    Done(SendCancelResult),
}

impl Shared {
    /// Resolves a `send.cancel` target to the caller's send: `sch_…` deletes
    /// a still-scheduled send (or names the send it fired as), `ask_…` names
    /// its send (or its scheduled send), `snd_…` is itself.
    async fn cancel_target(&self, key: &ActorKey, target: &str) -> Result<Target> {
        if target.starts_with(ScheduleId::PREFIX) {
            let sch = ScheduleId::parse(target).map_err(|e| bad_id(target, &e))?;
            if let Some(r) = self
                .delete_scheduled(key, ScheduledRef::Schedule(&sch))
                .await?
            {
                return Ok(Target::Done(r));
            }
            let (k, s) = (key.clone(), sch.as_str().to_owned());
            let fired: Option<String> = self
                .db
                .call(move |c| {
                    c.query_row(
                        "SELECT send_id FROM sends WHERE schedule_id = ?1 AND context = ?2 AND account_id = ?3",
                        params![s, k.context_str(), k.account.as_str()],
                        |r| r.get(0),
                    )
                    .optional()
                    .sql()
                })
                .await?;
            return fired
                .and_then(|s| SendId::parse(&s).ok())
                .map(Target::Send)
                .ok_or_else(|| schedule_not_found(&sch, key));
        }
        if target.starts_with(AskId::PREFIX) {
            let ask = AskId::parse(target).map_err(|e| bad_id(target, &e))?;
            let (k, a) = (key.clone(), ask.as_str().to_owned());
            // A fired ask's send, else a scheduled ask's pre-assigned send.
            let send: Option<String> = self
                .db
                .call(move |c| {
                    let fired: Option<String> = c
                        .query_row(
                            "SELECT a.send_id FROM asks a JOIN sends s ON s.send_id = a.send_id
                             WHERE a.ask_id = ?1 AND s.context = ?2 AND s.account_id = ?3",
                            params![a, k.context_str(), k.account.as_str()],
                            |r| r.get(0),
                        )
                        .optional()
                        .sql()?;
                    if fired.is_some() {
                        return Ok(fired);
                    }
                    c.query_row(
                        "SELECT send_id FROM scheduled WHERE ask_id = ?1 AND context = ?2 AND account_id = ?3",
                        params![a, k.context_str(), k.account.as_str()],
                        |r| r.get(0),
                    )
                    .optional()
                    .sql()
                })
                .await?;
            return send
                .and_then(|s| SendId::parse(&s).ok())
                .map(Target::Send)
                .ok_or_else(|| send_not_found(target, key));
        }
        if target.starts_with(SendId::PREFIX) {
            return SendId::parse(target)
                .map(Target::Send)
                .map_err(|e| bad_id(target, &e));
        }
        Err(bad_id(target, &Error::invalid_input("unknown prefix")))
    }
}

/// 1-based place in line from a 0-based waiting index.
fn position(i: usize) -> u32 {
    u32::try_from(i + 1).unwrap_or(u32::MAX)
}

/// An enum's wire word (`on_screen`, `cancelled`, …).
pub(crate) fn enum_word<T: serde::Serialize>(v: T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Not-yet-due scheduled sends of a Silicon.
pub(crate) fn scheduled_count(c: &rusqlite::Connection, key: &ActorKey) -> Result<u32> {
    let n: i64 = c
        .query_row(
            "SELECT count(*) FROM scheduled WHERE context = ?1 AND account_id = ?2",
            params![key.context_str(), key.account.as_str()],
            |r| r.get(0),
        )
        .sql()?;
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

fn bad_id(target: &str, e: &Error) -> Error {
    Error::invalid_input(format!(
        "`{target}` is not a send, ask or schedule ID; expected snd_…, ask_… or sch_…"
    ))
    .with_hint(
        "peek queue    lists the IDs on screen and waiting; peek schedule list the scheduled ones",
    )
    .with_details(json!({"field": "id", "value": target, "reason": e.message()}))
}

fn send_not_found(target: &str, key: &ActorKey) -> Error {
    Error::new(
        ErrorCode::SendNotFound,
        format!("no send {target} belongs to {} in this context", key.actor),
    )
    .with_hint("peek queue    lists this Silicon's sends on screen and waiting")
    .with_details(json!({"id": target}))
}

pub(crate) fn schedule_not_found(id: &ScheduleId, key: &ActorKey) -> Error {
    Error::new(
        ErrorCode::ScheduleNotFound,
        format!(
            "no scheduled send {id} belongs to {} in this context",
            key.actor
        ),
    )
    .with_hint("peek schedule list    lists this Silicon's scheduled sends")
    .with_details(json!({"id": id}))
}

/// A scheduled send, by either of its IDs.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ScheduledRef<'a> {
    /// `sch_…`.
    Schedule(&'a ScheduleId),
    /// Its pre-assigned `snd_…`.
    Send(&'a SendId),
}
