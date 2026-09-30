//! Queue v2, expiry, `--replace`, `shown`, cancel/clear and scheduling
//! (0.1.2 contract §6, test list §12.3), end to end over the real socket.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::many_single_char_names
)]

mod common;

use std::time::Duration;

use common::{DaemonCli, Harness, Home, eventually, pcm, query_one};
use serde_json::{Value, json};
use silicon_peek_client::{
    ErrorCode,
    ids::{AskId, ScheduleId, SendId},
    ipc::{
        cli::{
            AskGet, AskList, AskState, CancelledFrom, HeldReason, History, QueueClear,
            QueueItemState, QueueList, ScheduleCancel, ScheduleClear, ScheduleList, SendCancel,
            SendOp, SendStatus, StatusOp, Unregister, features,
        },
        ui::{Dismissed, Presence, PresenceReason, Shown, ShownDone, ShownReason, SpeechDone},
    },
    schema::{ask::Ask, send::Notify},
    timestamp::Timestamp,
    ting::Gesture,
};

fn send_op() -> SendOp {
    SendOp {
        isi: Some("queue-test".into()),
        speak: None,
        show: None,
        ask: None,
        voice: None,
        voice_instructions: None,
        lang: None,
        notify: vec![],
        duration_ms: None,
        expires_in_s: None,
        wait: false,
        expires_at: None,
        due_at: None,
        tz: None,
        replace: false,
    }
}

fn show(text: &str) -> SendOp {
    let mut s = send_op();
    s.show =
        Some(serde_json::from_value(json!({"elements":[{"type":"text","text":text}]})).unwrap());
    s
}

fn speak(text: &str) -> SendOp {
    let mut s = send_op();
    s.speak = Some(text.into());
    s
}

fn ask(question: &str) -> SendOp {
    let mut s = send_op();
    s.ask = Some(
        Ask::from_input(&json!({"question": question, "type": "single_choice",
            "options": [{"id":"keep","label":"Keep"},{"id":"delete","label":"Delete"}]}))
        .unwrap(),
    );
    s
}

async fn ready(h: &Harness, actor: &str, side: u64, build: u64) -> (Home, common::FakeUi) {
    let ui = h.ui_build(build).await;
    let home = h.home(actor);
    h.register(&home, side, &ui).await;
    (home, ui)
}

/// Every queued ting, oldest first: `(type, key, data)`.
fn tings(h: &Harness) -> Vec<(String, String, Value)> {
    let db = h.db();
    let mut st = db
        .prepare("SELECT request FROM outbox WHERE kind = 'ting' ORDER BY created_at, event_id")
        .unwrap();
    st.query_map([], |r| r.get::<_, Vec<u8>>(0))
        .unwrap()
        .map(|b| {
            let v: Value = serde_json::from_slice(&b.unwrap()).unwrap();
            (
                v["type"].as_str().unwrap().to_owned(),
                v["key"].as_str().unwrap().to_owned(),
                v["data"].clone(),
            )
        })
        .collect()
}

fn tings_of(h: &Harness, ting_type: &str) -> Vec<Value> {
    tings(h)
        .into_iter()
        .filter(|(t, _, _)| t == ting_type)
        .map(|(_, _, d)| d)
        .collect()
}

async fn wait_ting(h: &Harness, ting_type: &str, n: usize) -> Vec<Value> {
    eventually(10, &format!("{n} {ting_type} ting(s)"), || {
        tings_of(h, ting_type).len() >= n
    })
    .await;
    tings_of(h, ting_type)
}

fn close_reason(h: &Harness, send_id: &SendId) -> Option<String> {
    query_one(
        &h.db(),
        "SELECT close_reason FROM sends WHERE send_id = ?1",
        &[&send_id.as_str()],
    )
}

async fn shown_done(ui: &common::FakeUi, send_id: &SendId) {
    ui.request(
        &ShownDone {
            send_id: send_id.clone(),
            visible_ms: 4000,
            reason: ShownReason::Auto,
        },
        vec![],
    )
    .await
    .unwrap();
}

async fn problems(h: &Harness) {
    let p = h.handle().check_queues().await.unwrap();
    assert!(p.is_empty(), "queue invariants: {p:?}");
}

/// peekd's clock, in unix ms (the tests move it forward).
fn peekd_now(offset: Duration) -> Timestamp {
    Timestamp::now().plus(offset)
}

// ------------------------------------------------------------------ FIFO

#[tokio::test]
async fn fifo_order_survives_a_restart_and_badges_follow() {
    let mut h = Harness::start().await;
    let (home, ui) = ready(&h, "si:fifo", 2, 1000).await;
    let (a, _) = h.call(&home, &show("A"), vec![]).await.unwrap();
    let (b, _) = h.call(&home, &show("B"), vec![]).await.unwrap();
    let (c, _) = h.call(&home, &show("C"), vec![]).await.unwrap();
    assert_eq!(a.status, SendStatus::Showing);
    assert_eq!(
        (b.status, b.queue_position, b.waiting),
        (SendStatus::Queued, Some(1), Some(1))
    );
    assert_eq!(
        (c.status, c.queue_position, c.waiting),
        (SendStatus::Queued, Some(2), Some(2))
    );
    let first = ui.expect("peek.show").await;
    assert_eq!(first.fields["send_id"], a.send_id.as_str());
    assert_eq!(first.fields["queued_behind"], 0);
    for want in [1, 2] {
        let badge = ui.expect("queue.state").await;
        assert_eq!(badge.fields["waiting"], want);
        assert_eq!(badge.fields["send_id"], a.send_id.as_str());
    }
    problems(&h).await;
    shown_done(&ui, &a.send_id).await;
    let next = ui.expect("peek.show").await;
    assert_eq!(next.fields["send_id"], b.send_id.as_str());
    assert_eq!(next.fields["queued_behind"], 1);
    assert!(
        ui.try_expect("queue.state", Duration::from_millis(200))
            .await
            .is_none(),
        "peek.show carries the badge; no duplicate queue.state"
    );
    // A restart keeps the order (B was shown: it closes as daemon_restarted,
    // so C is next, then D sent after the restart).
    ui.close();
    h.restart().await;
    let (d, _) = h.call(&home, &show("D"), vec![]).await.unwrap();
    assert_eq!(d.queue_position, Some(1));
    let ui = h.ui().await;
    let next = ui.expect("peek.show").await;
    assert_eq!(next.fields["send_id"], c.send_id.as_str());
    assert_eq!(next.fields["queued_behind"], 1);
    assert!(
        matches!(
            close_reason(&h, &b.send_id).as_deref(),
            Some("daemon_restarted" | "ui_disconnected")
        ),
        "a shown show does not survive a restart"
    );
    shown_done(&ui, &c.send_id).await;
    let next = ui.expect("peek.show").await;
    assert_eq!(next.fields["send_id"], d.send_id.as_str());
    problems(&h).await;
}

#[tokio::test]
async fn queue_full_while_away_and_paused_names_the_hold() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:full", 3, 1000).await;
    ui.request(
        &Presence {
            available: false,
            reason: PresenceReason::Locked,
            paused: false,
        },
        vec![],
    )
    .await
    .unwrap();
    for i in 0..6 {
        let (r, _) = h
            .call(&home, &show(&format!("n{i}")), vec![])
            .await
            .unwrap();
        assert_eq!(r.status, SendStatus::Queued, "held while away");
        assert!(r.warnings.iter().any(|w| w.code == "carbon_away"));
        assert_eq!(r.queue_position, Some(i));
    }
    let e = h.call(&home, &show("seventh"), vec![]).await.err().unwrap();
    assert_eq!(*e.code(), ErrorCode::QueueFull);
    assert!(e.message().ends_with(
        " (the Carbon's screen is locked or asleep, so nothing moves until they are back)"
    ));
    assert_eq!(e.details().unwrap()["held"], "carbon_away");
    let (st, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    assert_eq!(st.queue.held, Some(HeldReason::CarbonAway));
    // Back but paused: bubbles are pushed (the summon finds them) and senders
    // hear `carbon_paused`.
    ui.request(
        &Presence {
            available: true,
            reason: PresenceReason::Ok,
            paused: true,
        },
        vec![],
    )
    .await
    .unwrap();
    let _ = ui.expect("peek.show").await;
    let e = h.call(&home, &show("eighth"), vec![]).await.err().unwrap();
    assert_eq!(*e.code(), ErrorCode::QueueFull);
    assert!(
        e.message()
            .ends_with(" (the Carbon paused Peek, so nothing moves until they resume)")
    );
    assert_eq!(e.details().unwrap()["held"], "paused");
    let (st, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    assert_eq!(st.queue.held, Some(HeldReason::Paused));
    assert!(st.carbon.unwrap().paused);
    let (l, _) = h.call(&home, &QueueList {}, vec![]).await.unwrap();
    assert_eq!(l.held, Some(HeldReason::Paused));
    assert_eq!(
        l.on_screen.as_ref().map(|i| i.state),
        Some(QueueItemState::Held)
    );
    assert_eq!(l.waiting.len(), 5);
    problems(&h).await;
}

#[tokio::test]
async fn a_paused_carbon_hears_carbon_paused_on_a_new_current_send() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:paused", 4, 1000).await;
    ui.request(
        &Presence {
            available: true,
            reason: PresenceReason::Ok,
            paused: true,
        },
        vec![],
    )
    .await
    .unwrap();
    let (r, _) = h.call(&home, &show("while paused"), vec![]).await.unwrap();
    assert_eq!(r.status, SendStatus::Queued);
    assert_eq!(r.queue_position, Some(0));
    let w = r
        .warnings
        .iter()
        .find(|w| w.code == "carbon_paused")
        .unwrap();
    assert_eq!(
        w.message,
        "Peek is paused by the Carbon; this send waits in position 4's queue and is shown when they resume"
    );
    // peekd still pushes it (Peek.app holds it while paused).
    let _ = ui.expect("peek.show").await;
}

// --------------------------------------------------------------- replace

#[tokio::test]
async fn replace_takes_over_a_show_and_keeps_the_waiting_order() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:replace", 1, 1002).await;
    let (a, _) = h.call(&home, &show("A"), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let mut waiting = Vec::new();
    for t in ["B", "C", "D", "E", "F"] {
        waiting.push(h.call(&home, &show(t), vec![]).await.unwrap().0.send_id);
    }
    // Allowed while the queue is full: it does not add to the queue.
    let mut r = speak("Correction");
    r.replace = true;
    let (new, _) = h.call(&home, &r, vec![]).await.unwrap();
    assert_eq!(new.status, SendStatus::Showing);
    assert_eq!(new.queue_position, Some(0));
    assert_eq!(new.waiting, Some(5));
    assert_eq!(new.replaced_send_id.as_ref(), Some(&a.send_id));
    let cancel = ui.expect("peek.cancel").await;
    assert_eq!(cancel.fields["send_id"], a.send_id.as_str());
    assert_eq!(cancel.fields["reason"], "replaced");
    let show_ev = ui.expect("peek.show").await;
    assert_eq!(show_ev.fields["send_id"], new.send_id.as_str());
    assert_eq!(show_ev.fields["replaces"], a.send_id.as_str());
    assert_eq!(show_ev.fields["queued_behind"], 5);
    assert_eq!(close_reason(&h, &a.send_id).as_deref(), Some("replaced"));
    let (l, _) = h.call(&home, &QueueList {}, vec![]).await.unwrap();
    assert_eq!(
        l.waiting
            .iter()
            .map(|i| i.send_id.clone())
            .collect::<Vec<_>>(),
        waiting
    );
    assert!(
        tings(&h).is_empty(),
        "no ting for the Silicon's own replace"
    );
    // Nothing current: a replace is just shown.
    let other = h.home("si:alone");
    h.register(&other, 7, &ui).await;
    let mut r = show("first");
    r.replace = true;
    let (alone, _) = h.call(&other, &r, vec![]).await.unwrap();
    assert_eq!(alone.status, SendStatus::Showing);
    assert_eq!(alone.replaced_send_id, None);
    let ev = ui.expect("peek.show").await;
    assert_eq!(ev.fields["send_id"], alone.send_id.as_str());
    assert!(ev.fields.get("replaces").is_none());
    problems(&h).await;
}

#[tokio::test]
async fn an_older_app_hears_cancelled_by_silicon_for_a_replace() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:oldreplace", 2, 1001).await;
    let (a, _) = h.call(&home, &show("A"), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let mut r = show("B");
    r.replace = true;
    h.call(&home, &r, vec![]).await.unwrap();
    let cancel = ui.expect("peek.cancel").await;
    assert_eq!(cancel.fields["send_id"], a.send_id.as_str());
    assert_eq!(
        cancel.fields["reason"], "cancelled_by_silicon",
        "build 1001 does not know `replaced`"
    );
    assert_eq!(close_reason(&h, &a.send_id).as_deref(), Some("replaced"));
}

#[tokio::test]
async fn replace_cancels_an_ask_and_its_waiter_without_a_ting() {
    let h = Harness::start().await;
    h.accept_deliveries().await;
    let (home, ui) = ready(&h, "si:replask", 2, 1000).await;
    // A current 0.1.2 CLI waiting on the ask gets `replaced`.
    let mut waiting_ask = ask("Keep old.zip?");
    waiting_ask.wait = true;
    let mut cli = DaemonCli::legacy(&h, "0.1.2").await;
    let (asked, _) = cli
        .conn()
        .call(
            &waiting_ask,
            Some(&home.auth),
            vec![],
            Duration::from_secs(10),
        )
        .await
        .unwrap();
    let _ = ui.expect("peek.show").await;
    let mut r = show("Never mind");
    r.replace = true;
    let (new, _) = h.call(&home, &r, vec![]).await.unwrap();
    assert_eq!(new.replaced_send_id.as_ref(), Some(&asked.send_id));
    let silicon_peek_client::runtime::daemon::EventWait::Event(ev) =
        cli.conn().next_event(Duration::from_secs(5)).await.unwrap()
    else {
        panic!("the waiter gets its result");
    };
    assert_eq!(ev.event, "ask.result");
    assert_eq!(ev.fields["state"], "replaced");
    let ask_id = asked.ask_id.unwrap();
    let (info, _) = h
        .call(
            &home,
            &AskGet {
                ask_id: ask_id.clone(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(info.state, AskState::Replaced);
    assert_eq!(
        close_reason(&h, &asked.send_id).as_deref(),
        Some("replaced")
    );
    let n: i64 = query_one(
        &h.db(),
        "SELECT count(*) FROM outbox WHERE subject_id = ?1",
        &[&ask_id.as_str()],
    )
    .unwrap();
    assert_eq!(n, 0, "no outbox row for a replaced ask");
    // A legacy CLI reads `replaced` as `cancelled` everywhere.
    let mut legacy = DaemonCli::legacy(&h, "0.1.1").await;
    let info = legacy
        .call(
            &home,
            &AskGet {
                ask_id: ask_id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(info.state, AskState::Cancelled);
    let list = legacy
        .call(
            &home,
            &AskList {
                state: Some(AskState::Cancelled),
                limit: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(list.asks.len(), 1);
    assert_eq!(list.asks[0].state, AskState::Cancelled);
    let hist = legacy
        .call(
            &home,
            &History {
                limit: Some(10),
                before: None,
            },
        )
        .await
        .unwrap();
    let item = hist
        .items
        .iter()
        .find(|i| i.send_id == asked.send_id)
        .unwrap();
    assert_eq!(item.ask_state, Some(AskState::Cancelled));
    assert_eq!(item.close_reason.as_deref(), Some("replaced"));
    // A current CLI sees the real state.
    let (list, _) = h
        .call(
            &home,
            &AskList {
                state: Some(AskState::Replaced),
                limit: None,
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(list.asks.len(), 1);
}

#[tokio::test]
async fn a_legacy_waiter_hears_cancelled_for_a_replaced_ask() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:legwait", 6, 1000).await;
    let mut waiting_ask = ask("Ship it?");
    waiting_ask.wait = true;
    let mut cli = DaemonCli::legacy(&h, "0.1.1").await;
    let (asked, _) = cli
        .conn()
        .call(
            &waiting_ask,
            Some(&home.auth),
            vec![],
            Duration::from_secs(10),
        )
        .await
        .unwrap();
    let _ = ui.expect("peek.show").await;
    let mut r = speak("Replacing");
    r.replace = true;
    h.call(&home, &r, vec![]).await.unwrap();
    let silicon_peek_client::runtime::daemon::EventWait::Event(ev) =
        cli.conn().next_event(Duration::from_secs(5)).await.unwrap()
    else {
        panic!("the waiter gets its result");
    };
    assert_eq!(ev.fields["state"], "cancelled");
    assert_eq!(ev.fields["ask_id"], asked.ask_id.unwrap().as_str());
}

// ---------------------------------------------------------------- expiry

#[tokio::test]
async fn a_waiting_show_expires_unseen_and_the_silicon_is_told() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:expire", 5, 1000).await;
    let (a, _) = h.call(&home, &show("current"), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let mut op = show("expires while waiting");
    op.expires_in_s = Some(10);
    let (b, _) = h.call(&home, &op, vec![]).await.unwrap();
    assert!(b.expires_at.is_some());
    let _ = ui.expect("queue.state").await;
    h.handle().advance_wall_clock(Duration::from_secs(11));
    let expired = wait_ting(&h, "peek.send.expired", 1).await;
    let d = &expired[0];
    assert_eq!(d["send_id"], b.send_id.as_str());
    assert_eq!(d["shown"], false);
    assert_eq!(d["shown_at"], Value::Null);
    assert_eq!(d["scheduled"], false);
    assert_eq!(d["kind"], "show");
    assert_eq!(d["slot"], 5);
    let badge = ui.expect("queue.state").await;
    assert_eq!(badge.fields["waiting"], 0);
    assert_eq!(close_reason(&h, &b.send_id).as_deref(), Some("expired"));
    let (_, key, _) = tings(&h)
        .into_iter()
        .find(|t| t.0 == "peek.send.expired")
        .unwrap();
    assert_eq!(key, format!("si:expire/{}/send_expired", b.send_id));
    // It is never shown: A's end shows nothing else.
    shown_done(&ui, &a.send_id).await;
    assert!(
        ui.try_expect("peek.show", Duration::from_millis(300))
            .await
            .is_none()
    );
    problems(&h).await;
}

#[tokio::test]
async fn an_on_screen_speak_expires_after_shown() {
    let h = Harness::start().await;
    h.agent.audio(pcm(48_000));
    let (home, ui) = ready(&h, "si:expspk", 3, 1002).await;
    let mut op = speak("A long announcement that outlives its deadline.");
    op.expires_in_s = Some(10);
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    ui.request(
        &Shown {
            send_id: r.send_id.clone(),
        },
        vec![],
    )
    .await
    .unwrap();
    let _ = ui.expect("tts.begin").await;
    h.handle().advance_wall_clock(Duration::from_secs(11));
    let cancel = ui.expect("peek.cancel").await;
    assert_eq!(cancel.fields["send_id"], r.send_id.as_str());
    assert_eq!(cancel.fields["reason"], "expired");
    let d = wait_ting(&h, "peek.send.expired", 1).await.remove(0);
    assert_eq!(d["shown"], true);
    assert!(d["shown_at"].is_string());
    assert_eq!(d["kind"], "speak");
    problems(&h).await;
}

#[tokio::test]
async fn asks_report_whether_they_were_shown_when_they_expire() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:expask", 8, 1000).await;
    let mut shown_ask = ask("Seen?");
    shown_ask.expires_in_s = Some(10);
    let (seen, _) = h.call(&home, &shown_ask, vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let mut waiting_ask = ask("Unseen?");
    waiting_ask.expires_in_s = Some(10);
    let (unseen, _) = h.call(&home, &waiting_ask, vec![]).await.unwrap();
    assert_eq!(unseen.status, SendStatus::Queued);
    h.handle().advance_wall_clock(Duration::from_secs(11));
    let expired = wait_ting(&h, "peek.ask.expired", 2).await;
    let by = |id: &AskId| {
        expired
            .iter()
            .find(|d| d["ask_id"] == id.as_str())
            .unwrap()
            .clone()
    };
    assert_eq!(by(seen.ask_id.as_ref().unwrap())["shown"], true);
    assert_eq!(by(unseen.ask_id.as_ref().unwrap())["shown"], false);
    let cancel = ui.expect("peek.cancel").await;
    assert_eq!(cancel.fields["send_id"], seen.send_id.as_str());
    assert!(
        ui.try_expect("peek.show", Duration::from_millis(300))
            .await
            .is_none()
    );
    problems(&h).await;
}

#[tokio::test]
async fn expiry_keeps_running_while_the_carbon_is_away() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:expaway", 1, 1000).await;
    ui.request(
        &Presence {
            available: false,
            reason: PresenceReason::Asleep,
            paused: false,
        },
        vec![],
    )
    .await
    .unwrap();
    let mut op = show("while you slept");
    op.expires_in_s = Some(10);
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    assert_eq!(r.status, SendStatus::Queued);
    h.handle().advance_wall_clock(Duration::from_secs(20));
    let d = wait_ting(&h, "peek.send.expired", 1).await.remove(0);
    assert_eq!(d["shown"], false);
    ui.request(&Presence::default(), vec![]).await.unwrap();
    assert!(
        ui.try_expect("peek.show", Duration::from_millis(300))
            .await
            .is_none(),
        "an expired send is never shown"
    );
}

// ----------------------------------------------------------------- shown

#[tokio::test]
async fn a_1002_app_reports_shown_before_speech_and_notifications() {
    let h = Harness::start().await;
    h.agent.audio(pcm(24_000));
    let (home, ui) = ready(&h, "si:shown", 2, 1002).await;
    let mut op = speak("Build finished");
    op.notify = vec![Notify::Shown];
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    assert_eq!(r.status, SendStatus::Showing);
    let _ = ui.expect("peek.show").await;
    let shown_at: Option<i64> = query_one(
        &h.db(),
        "SELECT shown_at FROM sends WHERE send_id = ?1",
        &[&r.send_id.as_str()],
    );
    assert!(shown_at.is_none(), "not shown until the app says so");
    assert!(
        ui.try_expect("tts.begin", Duration::from_millis(300))
            .await
            .is_none(),
        "speech waits for shown"
    );
    ui.request(
        &Shown {
            send_id: r.send_id.clone(),
        },
        vec![],
    )
    .await
    .unwrap();
    let _ = ui.expect("tts.begin").await;
    let shown_at: Option<i64> = query_one(
        &h.db(),
        "SELECT shown_at FROM sends WHERE send_id = ?1",
        &[&r.send_id.as_str()],
    );
    assert!(shown_at.is_some());
    // A duplicate `shown` and one for an unknown send change nothing.
    ui.request(
        &Shown {
            send_id: r.send_id.clone(),
        },
        vec![],
    )
    .await
    .unwrap();
    ui.request(
        &Shown {
            send_id: SendId::generate(),
        },
        vec![],
    )
    .await
    .unwrap();
    let shown = wait_ting(&h, "peek.send.shown", 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(tings_of(&h, "peek.send.shown").len(), 1, "sent once");
    let d = &shown[0];
    assert_eq!(d["send_id"], r.send_id.as_str());
    assert_eq!(d["scheduled"], false);
    assert_eq!(d["schedule_id"], Value::Null);
    assert_eq!(d["kind"], "speak");
    let (_, key, _) = tings(&h)
        .into_iter()
        .find(|t| t.0 == "peek.send.shown")
        .unwrap();
    assert_eq!(key, format!("si:shown/{}/shown", r.send_id));
    // Without --notify shown, a regular send sends no peek.send.shown.
    let (s2, _) = h.call(&home, &show("quiet"), vec![]).await.unwrap();
    ui.request(
        &Shown {
            send_id: s2.send_id.clone(),
        },
        vec![],
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(tings_of(&h, "peek.send.shown").len(), 1);
}

#[tokio::test]
async fn an_older_app_is_shown_at_push_time() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:oldapp", 3, 1001).await;
    let mut op = show("pushed");
    op.notify = vec![Notify::Shown];
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let shown_at: Option<i64> = query_one(
        &h.db(),
        "SELECT shown_at FROM sends WHERE send_id = ?1",
        &[&r.send_id.as_str()],
    );
    assert!(
        shown_at.is_some(),
        "push time counts as shown for build < 1002"
    );
    let _ = wait_ting(&h, "peek.send.shown", 1).await;
}

#[tokio::test]
async fn speak_only_closes_on_shown_done_or_the_watchdog() {
    let h = Harness::start_with(|c| {
        c.timings.bubble_grace = Duration::from_millis(300);
    })
    .await;
    let (home, ui) = ready(&h, "si:speakonly", 4, 1002).await;
    let (r, _) = h.call(&home, &speak("hi"), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    ui.request(
        &Shown {
            send_id: r.send_id.clone(),
        },
        vec![],
    )
    .await
    .unwrap();
    ui.request(
        &SpeechDone {
            send_id: r.send_id.clone(),
            stopped_by_user: false,
            played_ms: 500,
            total_ms: 500,
        },
        vec![],
    )
    .await
    .unwrap();
    // No 5 s fallback: speech.done never closes it.
    h.handle().advance_wall_clock(Duration::from_secs(10));
    h.handle().timers_now().await;
    assert_eq!(close_reason(&h, &r.send_id), None);
    ui.request(
        &ShownDone {
            send_id: r.send_id.clone(),
            visible_ms: 2000,
            reason: ShownReason::SpeechDone,
        },
        vec![],
    )
    .await
    .unwrap();
    assert_eq!(close_reason(&h, &r.send_id).as_deref(), Some("speech_done"));
    // A lost shown.done: the watchdog armed at `shown` closes it.
    let (r2, _) = h.call(&home, &speak("hello again"), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.handle().timers_now().await;
    assert_eq!(
        close_reason(&h, &r2.send_id),
        None,
        "no watchdog before shown"
    );
    ui.request(
        &Shown {
            send_id: r2.send_id.clone(),
        },
        vec![],
    )
    .await
    .unwrap();
    eventually(10, "the watchdog", || {
        close_reason(&h, &r2.send_id).as_deref() == Some("timeout")
    })
    .await;
}

#[tokio::test]
async fn esc_double_stops_speech_and_is_reported_on_a_show() {
    let h = Harness::start().await;
    h.accept_deliveries().await;
    let (home, ui) = ready(&h, "si:esc", 5, 1000).await;
    let mut op = show("Esc me");
    op.notify = vec![Notify::ShowDismissed];
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    ui.request(
        &Dismissed {
            send_id: r.send_id.clone(),
            gesture: Gesture::EscDouble,
        },
        vec![],
    )
    .await
    .unwrap();
    let d = wait_ting(&h, "peek.show.dismissed", 1).await.remove(0);
    assert_eq!(d["gesture"], "esc_double");
    assert_eq!(close_reason(&h, &r.send_id).as_deref(), Some("dismissed"));
}

// ------------------------------------------------------- cancel and clear

#[tokio::test]
async fn cancel_reports_where_each_send_was() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:cancel", 6, 1000).await;
    let (a, _) = h.call(&home, &ask("Current ask?"), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let (b, _) = h.call(&home, &show("B"), vec![]).await.unwrap();
    let (c, _) = h.call(&home, &ask("Waiting ask?"), vec![]).await.unwrap();
    let (d, _) = h.call(&home, &show("D"), vec![]).await.unwrap();
    // Waiting, by send ID.
    let (r, _) = h
        .call(
            &home,
            &SendCancel {
                target: b.send_id.to_string(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.was, CancelledFrom::Waiting);
    assert_eq!(r.queue_position, Some(1));
    assert_eq!(r.state, "cancelled");
    assert_eq!(close_reason(&h, &b.send_id).as_deref(), Some("cancelled"));
    // Waiting ask, by ask ID.
    let (r, _) = h
        .call(
            &home,
            &SendCancel {
                target: c.ask_id.clone().unwrap().to_string(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.send_id, c.send_id);
    assert_eq!(r.was, CancelledFrom::Waiting);
    assert_eq!(r.queue_position, Some(1));
    // On screen.
    let (r, _) = h
        .call(
            &home,
            &SendCancel {
                target: a.send_id.to_string(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.was, CancelledFrom::OnScreen);
    assert_eq!(r.queue_position, Some(0));
    let cancel = ui.expect("peek.cancel").await;
    assert_eq!(cancel.fields["reason"], "cancelled_by_silicon");
    let next = ui.expect("peek.show").await;
    assert_eq!(next.fields["send_id"], d.send_id.as_str());
    // Closed.
    let (r, _) = h
        .call(
            &home,
            &SendCancel {
                target: a.send_id.to_string(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.was, CancelledFrom::Closed);
    assert_eq!(r.state, "cancelled");
    shown_done(&ui, &d.send_id).await;
    let (r, _) = h
        .call(
            &home,
            &SendCancel {
                target: d.send_id.to_string(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!((r.was, r.state.as_str()), (CancelledFrom::Closed, "auto"));
    // Another Silicon's send, and a bad ID.
    let other = h.home("si:someone");
    h.register(&other, 7, &ui).await;
    let e = h
        .call(
            &other,
            &SendCancel {
                target: a.send_id.to_string(),
            },
            vec![],
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::SendNotFound);
    assert_eq!(e.exit_code().code(), 4);
    assert!(e.hint().unwrap().starts_with("peek queue"));
    let e = h
        .call(
            &home,
            &SendCancel {
                target: "nope_123".into(),
            },
            vec![],
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::InvalidInput);
    assert_eq!(
        e.message(),
        "`nope_123` is not a send, ask or schedule ID; expected snd_…, ask_… or sch_…"
    );
    assert_eq!(e.details().unwrap()["field"], "id");
    let e = h
        .call(
            &home,
            &SendCancel {
                target: ScheduleId::generate().to_string(),
            },
            vec![],
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::ScheduleNotFound);
    assert!(
        tings(&h).is_empty(),
        "no tings for the Silicon's own actions"
    );
    problems(&h).await;
}

#[tokio::test]
async fn held_sends_cancel_as_held() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:heldcancel", 2, 1000).await;
    ui.request(
        &Presence {
            available: false,
            reason: PresenceReason::Locked,
            paused: false,
        },
        vec![],
    )
    .await
    .unwrap();
    let (a, _) = h.call(&home, &show("held"), vec![]).await.unwrap();
    let (r, _) = h
        .call(
            &home,
            &SendCancel {
                target: a.send_id.to_string(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.was, CancelledFrom::Held);
    assert!(
        ui.try_expect("peek.cancel", Duration::from_millis(200))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn queue_clear_with_and_without_all() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:clear", 3, 1000).await;
    let (a, _) = h.call(&home, &show("A"), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let mut waiter_ask = ask("Waiting?");
    waiter_ask.wait = true;
    let mut cli = DaemonCli::legacy(&h, "0.1.2").await;
    let (b, _) = cli
        .conn()
        .call(
            &waiter_ask,
            Some(&home.auth),
            vec![],
            Duration::from_secs(10),
        )
        .await
        .unwrap();
    let (c, _) = h.call(&home, &show("C"), vec![]).await.unwrap();
    let _ = ui.expect("queue.state").await;
    let _ = ui.expect("queue.state").await;
    let (r, _) = h
        .call(&home, &QueueClear { all: false }, vec![])
        .await
        .unwrap();
    assert_eq!(r.cancelled, vec![b.send_id.clone(), c.send_id.clone()]);
    assert_eq!(r.on_screen.as_ref(), Some(&a.send_id));
    assert!(!r.on_screen_cancelled);
    let badge = ui.expect("queue.state").await;
    assert_eq!(badge.fields["waiting"], 0);
    assert!(
        ui.try_expect("queue.state", Duration::from_millis(200))
            .await
            .is_none(),
        "one queue.state at the end"
    );
    let silicon_peek_client::runtime::daemon::EventWait::Event(ev) =
        cli.conn().next_event(Duration::from_secs(5)).await.unwrap()
    else {
        panic!("the waiter gets its result");
    };
    assert_eq!(ev.fields["state"], "cancelled");
    assert_eq!(close_reason(&h, &c.send_id).as_deref(), Some("cleared"));
    assert_eq!(close_reason(&h, &b.send_id).as_deref(), Some("cancelled"));
    let (r, _) = h
        .call(&home, &QueueClear { all: false }, vec![])
        .await
        .unwrap();
    assert!(r.cancelled.is_empty());
    let (r, _) = h
        .call(&home, &QueueClear { all: true }, vec![])
        .await
        .unwrap();
    assert_eq!(r.cancelled, vec![a.send_id.clone()]);
    assert!(r.on_screen_cancelled);
    let cancel = ui.expect("peek.cancel").await;
    assert_eq!(cancel.fields["send_id"], a.send_id.as_str());
    let (l, _) = h.call(&home, &QueueList {}, vec![]).await.unwrap();
    assert!(l.on_screen.is_none() && l.waiting.is_empty());
    assert!(tings(&h).is_empty());
    problems(&h).await;
}

#[tokio::test]
async fn queue_list_shows_states_summaries_and_ages() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:list", 4, 1000).await;
    let (none, _) = h
        .call(&h.home("si:nobody"), &QueueList {}, vec![])
        .await
        .unwrap();
    assert_eq!(none.slot, None);
    let (a, _) = h
        .call(&home, &ask("Delete   old.zip?   Now"), vec![])
        .await
        .unwrap();
    let _ = ui.expect("peek.show").await;
    let long = "x".repeat(100);
    let (b, _) = h.call(&home, &show(&long), vec![]).await.unwrap();
    let mut e = speak("Deploy done");
    e.expires_in_s = Some(60);
    let (c, _) = h.call(&home, &e, vec![]).await.unwrap();
    let (l, _) = h.call(&home, &QueueList {}, vec![]).await.unwrap();
    assert_eq!(
        l.slot.map(silicon_peek_client::identity::SlotIndex::get),
        Some(4)
    );
    assert_eq!((l.limit, l.scheduled, l.scheduled_limit), (5, 0, 500));
    let on = l.on_screen.unwrap();
    assert_eq!(on.send_id, a.send_id);
    assert_eq!(on.state, QueueItemState::OnScreen);
    assert_eq!(on.kind, "ask");
    assert_eq!(on.summary, "Delete old.zip? Now");
    assert!(on.shown_at.is_some());
    assert_eq!(l.waiting[0].send_id, b.send_id);
    assert_eq!(l.waiting[0].summary.chars().count(), 60);
    assert!(l.waiting[0].summary.ends_with('…'));
    assert_eq!(l.waiting[0].queue_position, 1);
    assert_eq!(l.waiting[1].send_id, c.send_id);
    assert_eq!(l.waiting[1].summary, "Deploy done");
    assert!(l.waiting[1].expires_at.is_some());
    assert_eq!(l.waiting[1].state, QueueItemState::Waiting);
}

// --------------------------------------------------------------- scheduling

#[tokio::test]
async fn a_scheduled_send_fires_when_due_and_reports_shown() {
    let h = Harness::start().await;
    h.accept_deliveries().await;
    let (home, ui) = ready(&h, "si:sched", 3, 1002).await;
    let mut op = show("Stand-up in 5");
    op.due_at = Some(peekd_now(Duration::from_secs(60)));
    op.tz = Some("Asia/Kolkata".into());
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    assert_eq!(r.status, SendStatus::Scheduled);
    let sch = r.schedule_id.clone().unwrap();
    assert_eq!(r.tz.as_deref(), Some("Asia/Kolkata"));
    assert_eq!((r.queue_position, r.waiting), (None, None));
    assert!(
        ui.try_expect("peek.show", Duration::from_millis(200))
            .await
            .is_none()
    );
    let (list, _) = h.call(&home, &ScheduleList {}, vec![]).await.unwrap();
    assert_eq!(list.scheduled.len(), 1);
    assert_eq!(list.scheduled[0].send_id, r.send_id);
    assert_eq!(list.scheduled[0].summary, "Stand-up in 5");
    let (st, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    assert_eq!(st.queue.scheduled, 1);
    h.handle().advance_wall_clock(Duration::from_secs(61));
    let show_ev = ui.expect("peek.show").await;
    assert_eq!(
        show_ev.fields["send_id"],
        r.send_id.as_str(),
        "the pre-assigned ID"
    );
    assert_eq!(show_ev.fields["schedule_id"], sch.as_str());
    let due = wait_ting(&h, "peek.schedule.due", 1).await.remove(0);
    assert_eq!(due["schedule_id"], sch.as_str());
    assert_eq!(due["send_id"], r.send_id.as_str());
    assert_eq!(due["outcome"], "shown");
    assert_eq!(due["waiting_reason"], Value::Null);
    assert_eq!(due["queue_position"], Value::Null);
    let (_, key, _) = tings(&h)
        .into_iter()
        .find(|t| t.0 == "peek.schedule.due")
        .unwrap();
    assert_eq!(key, format!("si:sched/{sch}/due"));
    ui.request(
        &Shown {
            send_id: r.send_id.clone(),
        },
        vec![],
    )
    .await
    .unwrap();
    let shown = wait_ting(&h, "peek.send.shown", 1).await.remove(0);
    assert_eq!(shown["scheduled"], true);
    assert_eq!(shown["schedule_id"], sch.as_str());
    let (list, _) = h.call(&home, &ScheduleList {}, vec![]).await.unwrap();
    assert!(list.scheduled.is_empty());
    let (hist, _) = h
        .call(
            &home,
            &History {
                limit: Some(5),
                before: None,
            },
            vec![],
        )
        .await
        .unwrap();
    let item = &hist.items[0];
    assert_eq!(item.schedule_id.as_ref(), Some(&sch));
    assert!(item.due_at.is_some() && item.shown_at.is_some());
    // Cancelling it now says it already fired.
    let (c, _) = h
        .call(
            &home,
            &ScheduleCancel {
                target: sch.to_string(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(c.state, "fired");
    assert_eq!(c.send_id, r.send_id);
    problems(&h).await;
}

#[tokio::test]
async fn due_sends_queue_behind_others_overflow_and_wait_for_room() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:overflow", 2, 1000).await;
    let (cur, _) = h.call(&home, &show("current"), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    // Due with one current: queued behind others at 1.
    let mut s1 = show("scheduled one");
    s1.due_at = Some(peekd_now(Duration::from_secs(30)));
    let (sch1, _) = h.call(&home, &s1, vec![]).await.unwrap();
    h.handle().advance_wall_clock(Duration::from_secs(31));
    let due = wait_ting(&h, "peek.schedule.due", 1).await.remove(0);
    assert_eq!(due["outcome"], "queued");
    assert_eq!(due["waiting_reason"], "behind_others");
    assert_eq!(due["queue_position"], 1);
    // Fill the waiting list to five, then a due one overflows at 6.
    for t in ["w2", "w3", "w4", "w5"] {
        h.call(&home, &show(t), vec![]).await.unwrap();
    }
    let mut s2 = speak("scheduled two");
    s2.due_at = Some(peekd_now(Duration::from_secs(31 + 30)));
    let (sch2, _) = h.call(&home, &s2, vec![]).await.unwrap();
    h.handle().advance_wall_clock(Duration::from_secs(31));
    let due = wait_ting(&h, "peek.schedule.due", 2).await.remove(1);
    assert_eq!(due["outcome"], "queued");
    assert_eq!(due["waiting_reason"], "queue_full");
    assert_eq!(due["queue_position"], 6);
    let overflow: i64 = query_one(
        &h.db(),
        "SELECT overflow FROM sends WHERE send_id = ?1",
        &[&sch2.send_id.as_str()],
    )
    .unwrap();
    assert_eq!(overflow, 1);
    let (l, _) = h.call(&home, &QueueList {}, vec![]).await.unwrap();
    assert_eq!(l.waiting.len(), 6);
    assert_eq!(l.waiting[5].state, QueueItemState::DueWaiting);
    assert_eq!(l.waiting[5].queue_position, 6);
    assert_eq!(l.waiting[0].send_id, sch1.send_id);
    // A new regular send meanwhile gets queue_full (due_waiting counted).
    let e = h.call(&home, &show("new"), vec![]).await.err().unwrap();
    assert_eq!(*e.code(), ErrorCode::QueueFull);
    assert_eq!(e.details().unwrap()["due_waiting"], 1);
    // A free spot goes to the due send first.
    shown_done(&ui, &cur.send_id).await;
    let _ = ui.expect("peek.show").await;
    let overflow: i64 = query_one(
        &h.db(),
        "SELECT overflow FROM sends WHERE send_id = ?1",
        &[&sch2.send_id.as_str()],
    )
    .unwrap();
    assert_eq!(overflow, 0);
    let (l, _) = h.call(&home, &QueueList {}, vec![]).await.unwrap();
    assert_eq!(l.waiting.len(), 5);
    assert_eq!(l.waiting[4].send_id, sch2.send_id);
    assert_eq!(l.waiting[4].state, QueueItemState::Waiting);
    problems(&h).await;
}

#[tokio::test]
async fn expiry_and_cancel_promote_due_sends_waiting_for_room() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:promote", 3, 1000).await;
    h.call(&home, &show("current"), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let mut first = show("expires while waiting");
    first.expires_in_s = Some(20);
    let (w1, _) = h.call(&home, &first, vec![]).await.unwrap();
    for t in ["w2", "w3", "w4", "w5"] {
        h.call(&home, &show(t), vec![]).await.unwrap();
    }
    // Two due sends overflow at 6 and 7.
    let mut offset = Duration::ZERO;
    let mut due = Vec::new();
    for t in ["due one", "due two"] {
        let mut s = show(t);
        s.due_at = Some(peekd_now(offset + Duration::from_secs(5)));
        due.push(h.call(&home, &s, vec![]).await.unwrap().0);
        offset += Duration::from_secs(6);
        h.handle().advance_wall_clock(Duration::from_secs(6));
    }
    let dues = wait_ting(&h, "peek.schedule.due", 2).await;
    assert_eq!(
        dues.iter()
            .map(|d| d["queue_position"].clone())
            .collect::<Vec<_>>(),
        vec![json!(6), json!(7)]
    );
    eventually(10, "badge +7", || {
        ui.names().iter().filter(|n| *n == "queue.state").count() >= 1
    })
    .await;
    // The waiting send expires: the first due send takes its spot, the badge follows.
    h.handle().advance_wall_clock(Duration::from_secs(20));
    let expired = wait_ting(&h, "peek.send.expired", 1).await.remove(0);
    assert_eq!(expired["send_id"], w1.send_id.as_str());
    eventually(10, "promotion", || {
        query_one::<i64>(
            &h.db(),
            "SELECT overflow FROM sends WHERE send_id = ?1",
            &[&due[0].send_id.as_str()],
        ) == Some(0)
    })
    .await;
    let (l, _) = h.call(&home, &QueueList {}, vec![]).await.unwrap();
    assert_eq!(l.waiting.len(), 6);
    assert_eq!(l.waiting[4].send_id, due[0].send_id);
    assert_eq!(l.waiting[4].state, QueueItemState::Waiting);
    assert_eq!(l.waiting[5].state, QueueItemState::DueWaiting);
    let mut last = None;
    for e in ui.drain() {
        if e.event == "queue.state" {
            last = Some(e.fields["waiting"].clone());
        }
    }
    assert_eq!(last, Some(json!(6)), "the badge dropped from 7 to 6");
    // Cancelling the due-waiting one by its schedule ID reports where it was.
    let (c, _) = h
        .call(
            &home,
            &SendCancel {
                target: due[1].schedule_id.clone().unwrap().to_string(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(c.send_id, due[1].send_id);
    assert_eq!(c.was, CancelledFrom::DueWaiting);
    assert_eq!(c.queue_position, Some(6));
    let badge = ui.expect("queue.state").await;
    assert_eq!(badge.fields["waiting"], 5);
    problems(&h).await;
}

#[tokio::test]
async fn a_due_send_while_away_is_held_and_a_scheduled_replace_replaces() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:dueaway", 5, 1002).await;
    ui.request(
        &Presence {
            available: false,
            reason: PresenceReason::Locked,
            paused: false,
        },
        vec![],
    )
    .await
    .unwrap();
    let mut s = show("while away");
    s.due_at = Some(peekd_now(Duration::from_secs(5)));
    h.call(&home, &s, vec![]).await.unwrap();
    h.handle().advance_wall_clock(Duration::from_secs(6));
    let due = wait_ting(&h, "peek.schedule.due", 1).await.remove(0);
    assert_eq!(due["outcome"], "queued");
    assert_eq!(due["waiting_reason"], "carbon_away");
    assert_eq!(due["queue_position"], 0);
    ui.request(&Presence::default(), vec![]).await.unwrap();
    let current = ui.expect("peek.show").await;
    let mut r = speak("replace at due time");
    r.due_at = Some(peekd_now(Duration::from_secs(6 + 5)));
    r.replace = true;
    let (rs, _) = h.call(&home, &r, vec![]).await.unwrap();
    h.handle().advance_wall_clock(Duration::from_secs(6));
    let due = wait_ting(&h, "peek.schedule.due", 2).await.remove(1);
    assert_eq!(due["outcome"], "replaced");
    assert_eq!(due["replaced_send_id"], current.fields["send_id"]);
    assert_eq!(due["queue_position"], Value::Null);
    let cancel = ui.expect("peek.cancel").await;
    assert_eq!(cancel.fields["reason"], "replaced");
    let next = ui.expect("peek.show").await;
    assert_eq!(next.fields["send_id"], rs.send_id.as_str());
    assert_eq!(next.fields["replaces"], current.fields["send_id"]);
}

#[tokio::test]
async fn a_send_due_after_its_deadline_expires_unseen() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:dueexp", 6, 1000).await;
    let due_at = peekd_now(Duration::from_secs(30));
    let mut s = show("too late");
    s.due_at = Some(due_at);
    s.expires_at = Some(due_at.plus(Duration::from_secs(10)));
    let (a, _) = h.call(&home, &s, vec![]).await.unwrap();
    let mut q = ask("Too late to ask?");
    q.due_at = Some(due_at);
    q.expires_at = Some(due_at.plus(Duration::from_secs(10)));
    let (b, _) = h.call(&home, &q, vec![]).await.unwrap();
    // The Mac "sleeps" past both deadlines.
    h.handle().advance_wall_clock(Duration::from_secs(3600));
    let dues = wait_ting(&h, "peek.schedule.due", 2).await;
    assert!(
        dues.iter()
            .all(|d| d["outcome"] == "expired" && d["waiting_reason"].is_null())
    );
    let se = wait_ting(&h, "peek.send.expired", 1).await.remove(0);
    assert_eq!(se["send_id"], a.send_id.as_str());
    assert_eq!(se["shown"], false);
    assert_eq!(se["scheduled"], true);
    assert_eq!(se["schedule_id"], a.schedule_id.unwrap().as_str());
    let ae = wait_ting(&h, "peek.ask.expired", 1).await.remove(0);
    assert_eq!(ae["ask_id"], b.ask_id.clone().unwrap().as_str());
    assert_eq!(ae["shown"], false);
    let kinds: Vec<String> = tings(&h).into_iter().map(|t| t.0).collect();
    let first_due = kinds.iter().position(|k| k == "peek.schedule.due").unwrap();
    let first_expired = kinds
        .iter()
        .position(|k| k == "peek.send.expired" || k == "peek.ask.expired")
        .unwrap();
    assert!(
        first_due < first_expired,
        "schedule.due comes first: {kinds:?}"
    );
    assert!(
        ui.try_expect("peek.show", Duration::from_millis(300))
            .await
            .is_none()
    );
    let (info, _) = h
        .call(
            &home,
            &AskGet {
                ask_id: b.ask_id.unwrap(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(info.state, AskState::Expired);
    problems(&h).await;
}

#[tokio::test]
async fn overdue_scheduled_sends_fire_in_order_after_a_restart() {
    let mut h = Harness::start().await;
    let (home, ui) = ready(&h, "si:restart", 7, 1000).await;
    ui.close();
    common::eventually(5, "the UI to go", || !h.handle().ui_connected()).await;
    let mut ids = Vec::new();
    for (i, secs) in [(0, 40u64), (1, 20), (2, 30)] {
        let mut s = show(&format!("s{i}"));
        s.due_at = Some(peekd_now(Duration::from_secs(secs)));
        ids.push((secs, h.call(&home, &s, vec![]).await.unwrap().0.send_id));
    }
    // Stopped past every due time: the first pass after start fires them,
    // oldest due first.
    let db = h.db();
    db.execute("UPDATE scheduled SET due_at = due_at - 3600000", [])
        .unwrap();
    h.restart().await;
    let _ = wait_ting(&h, "peek.schedule.due", 3).await;
    let mut order: Vec<(i64, String)> = {
        let mut st = db
            .prepare("SELECT queued_at, send_id FROM sends WHERE schedule_id IS NOT NULL ORDER BY queued_at, send_id")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    order.sort();
    ids.sort_by_key(|(s, _)| *s);
    let fired: Vec<String> = order.into_iter().map(|(_, s)| s).collect();
    let want: Vec<String> = ids.iter().map(|(_, s)| s.to_string()).collect();
    let (l, _) = h.call(&home, &QueueList {}, vec![]).await.unwrap();
    let queue: Vec<String> = l
        .on_screen
        .iter()
        .chain(l.waiting.iter())
        .map(|i| i.send_id.to_string())
        .collect();
    assert_eq!(queue, want, "queue order follows due order");
    assert_eq!(fired.len(), 3);
    problems(&h).await;
}

#[tokio::test]
async fn the_schedule_holds_at_most_500_and_images_survive_pruning() {
    let h = Harness::start().await;
    let (home, _ui) = ready(&h, "si:many", 8, 1000).await;
    let db = h.db();
    let now = Timestamp::now().unix_ms();
    for i in 0..499 {
        db.execute(
            "INSERT INTO scheduled (schedule_id, send_id, context, org_id, actor_id, home_path, api_url, payload, notify, kind, due_at, created_at)
             VALUES (?1, ?2, 'production', 'tos', 'si:many', '/h', 'https://x', x'7b7d', '[]', 'show', ?3, ?4)",
            rusqlite::params![
                ScheduleId::generate().to_string(),
                SendId::generate().to_string(),
                now + 86_400_000 + i,
                now
            ],
        )
        .unwrap();
    }
    let png = b"\x89PNG\r\n\x1a\nscheduled-image".to_vec();
    let mut s = send_op();
    s.show = Some(
        serde_json::from_value(
            json!({"elements":[{"type":"image","path":{"blob":0},"caption":"later"}]}),
        )
        .unwrap(),
    );
    s.due_at = Some(peekd_now(Duration::from_secs(3600)));
    let (r, _) = h.call(&home, &s, vec![png.clone()]).await.unwrap();
    assert_eq!(r.status, SendStatus::Scheduled);
    let e = h.call(&home, &s, vec![png.clone()]).await.err().unwrap();
    assert_eq!(*e.code(), ErrorCode::ScheduleFull);
    assert_eq!(e.exit_code().code(), 4);
    assert_eq!(
        e.details().unwrap(),
        &json!({"scheduled": 500, "limit": 500})
    );
    assert!(
        e.message()
            .starts_with("500 sends are already scheduled for si:many;")
    );
    // The copied image is kept while scheduled, even when old.
    let images = h.cfg.support_dir.join("cache/images");
    let file = std::fs::read_dir(&images)
        .unwrap()
        .flatten()
        .find(|e| std::fs::read(e.path()).unwrap() == png)
        .unwrap()
        .path();
    let old = std::time::SystemTime::now() - Duration::from_hours(30 * 24);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert!(file.exists());
    let (clear, _) = h.call(&home, &ScheduleClear {}, vec![]).await.unwrap();
    assert_eq!(clear.cancelled.len(), 500);
    let (list, _) = h.call(&home, &ScheduleList {}, vec![]).await.unwrap();
    assert!(list.scheduled.is_empty());
    let (clear, _) = h.call(&home, &ScheduleClear {}, vec![]).await.unwrap();
    assert!(clear.cancelled.is_empty());
}

#[tokio::test]
async fn scheduled_images_are_referenced_for_pruning() {
    let h = Harness::start().await;
    let (home, _ui) = ready(&h, "si:prune", 1, 1000).await;
    let png = b"\x89PNG\r\n\x1a\nkeep-me-scheduled".to_vec();
    let mut s = send_op();
    s.show = Some(
        serde_json::from_value(json!({"elements":[{"type":"image","path":{"blob":0}}]})).unwrap(),
    );
    s.due_at = Some(peekd_now(Duration::from_secs(3600)));
    h.call(&home, &s, vec![png.clone()]).await.unwrap();
    let images = h.cfg.support_dir.join("cache/images");
    let file = std::fs::read_dir(&images)
        .unwrap()
        .flatten()
        .find(|e| std::fs::read(e.path()).unwrap() == png)
        .unwrap()
        .path();
    let payloads: Vec<String> = {
        let db = h.db();
        let mut st = db.prepare("SELECT payload FROM scheduled").unwrap();
        st.query_map([], |r| r.get::<_, Vec<u8>>(0))
            .unwrap()
            .map(|b| String::from_utf8_lossy(&b.unwrap()).into_owned())
            .collect()
    };
    let removed = silicon_peek_daemon::bubbles::prune_images(&images, &payloads, Duration::ZERO, 0);
    assert_eq!(removed, 0);
    assert!(file.exists(), "a scheduled send's image is referenced");
}

#[tokio::test]
async fn schedule_cancel_list_order_and_not_found() {
    let h = Harness::start().await;
    let (home, _ui) = ready(&h, "si:schlist", 2, 1000).await;
    let mut late = speak("later");
    late.due_at = Some(peekd_now(Duration::from_secs(7200)));
    let (l, _) = h.call(&home, &late, vec![]).await.unwrap();
    let mut soon = ask("Sooner?");
    soon.due_at = Some(peekd_now(Duration::from_secs(600)));
    soon.expires_at = Some(peekd_now(Duration::from_mins(11)));
    soon.replace = true;
    let (s, _) = h.call(&home, &soon, vec![]).await.unwrap();
    let (list, _) = h.call(&home, &ScheduleList {}, vec![]).await.unwrap();
    assert_eq!(list.limit, 500);
    assert_eq!(
        list.scheduled
            .iter()
            .map(|i| i.send_id.clone())
            .collect::<Vec<_>>(),
        vec![s.send_id.clone(), l.send_id.clone()],
        "soonest first"
    );
    assert!(list.scheduled[0].replace);
    assert_eq!(list.scheduled[0].kind, "ask");
    assert!(list.scheduled[0].ask_id.is_some());
    assert!(list.scheduled[0].expires_at.is_some());
    // By send ID.
    let (c, _) = h
        .call(
            &home,
            &ScheduleCancel {
                target: l.send_id.to_string(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(c.state, "cancelled");
    assert_eq!(c.schedule_id, l.schedule_id.clone().unwrap());
    let e = h
        .call(
            &home,
            &ScheduleCancel {
                target: l.schedule_id.unwrap().to_string(),
            },
            vec![],
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::ScheduleNotFound);
    // `peek cancel` works on scheduled sends too (by schedule ID or ask ID).
    let (c, _) = h
        .call(
            &home,
            &SendCancel {
                target: s.ask_id.clone().unwrap().to_string(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(c.was, CancelledFrom::Scheduled);
    assert_eq!(c.schedule_id, s.schedule_id);
    let e = h
        .call(
            &home,
            &ScheduleCancel {
                target: "ask_x".into(),
            },
            vec![],
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::InvalidInput);
    // Scheduled asks cannot --wait; a scheduled send takes no --expires-in.
    let mut bad = ask("q");
    bad.due_at = Some(peekd_now(Duration::from_secs(60)));
    bad.wait = true;
    let e = h.call(&home, &bad, vec![]).await.err().unwrap();
    assert_eq!(*e.code(), ErrorCode::ConflictingFlags);
    let mut bad = speak("q");
    bad.due_at = Some(peekd_now(Duration::from_secs(60)));
    bad.expires_in_s = Some(60);
    let e = h.call(&home, &bad, vec![]).await.err().unwrap();
    assert_eq!(*e.code(), ErrorCode::ConflictingFlags);
    // A due time in the past (beyond the slack) is refused.
    let mut past = speak("past");
    past.due_at = Some(Timestamp::from_unix_ms(Timestamp::now().unix_ms() - 60_000));
    let e = h.call(&home, &past, vec![]).await.err().unwrap();
    assert_eq!(e.details().unwrap()["field"], "--at");
}

#[tokio::test]
async fn a_wall_clock_jump_fires_due_sends_at_once() {
    let h = Harness::start_with(|c| {
        c.timings.timer_catchup_cap = Duration::from_secs(3600);
    })
    .await;
    let (home, ui) = ready(&h, "si:jump", 3, 1000).await;
    let mut s = show("after sleep");
    s.due_at = Some(peekd_now(Duration::from_mins(30)));
    h.call(&home, &s, vec![]).await.unwrap();
    // The Mac sleeps half an hour; waking moves the wall clock, not tokio's.
    h.handle().advance_wall_clock(Duration::from_secs(1801));
    let ev = tokio::time::timeout(Duration::from_secs(5), ui.expect("peek.show")).await;
    assert!(ev.is_ok(), "fired right after the jump");
}

// ------------------------------------------------------ unregister, logout

#[tokio::test]
async fn unregister_and_logout_cancel_queued_overflow_and_scheduled() {
    use silicon_peek_client::ipc::cli::{Detach, DetachReason};
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:unreg", 4, 1000).await;
    let (a, _) = h.call(&home, &ask("Current?"), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let (b, _) = h.call(&home, &show("waiting"), vec![]).await.unwrap();
    let mut s = speak("scheduled");
    s.due_at = Some(peekd_now(Duration::from_secs(3600)));
    let (c, _) = h.call(&home, &s, vec![]).await.unwrap();
    let (r, _) = h.call(&home, &Unregister {}, vec![]).await.unwrap();
    assert_eq!(
        r.released_slot
            .map(silicon_peek_client::identity::SlotIndex::get),
        Some(4)
    );
    assert_eq!(r.cancelled_asks, vec![a.ask_id.clone().unwrap()]);
    assert_eq!(r.cancelled_sends, vec![b.send_id.clone()]);
    assert_eq!(r.cancelled_scheduled, vec![c.schedule_id.clone().unwrap()]);
    let cancel = ui.expect("peek.cancel").await;
    assert_eq!(cancel.fields["reason"], "unregistered");
    assert!(
        ui.try_expect("peek.show", Duration::from_millis(300))
            .await
            .is_none()
    );
    assert!(tings(&h).is_empty());
    problems(&h).await;
    // Logout (detach) does the same for another Silicon.
    let other = h.home("si:logout");
    h.register(&other, 5, &ui).await;
    let (x, _) = h.call(&other, &show("x"), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let mut s = speak("scheduled");
    s.due_at = Some(peekd_now(Duration::from_secs(3600)));
    h.call(&other, &s, vec![]).await.unwrap();
    h.call(
        &other,
        &Detach {
            reason: DetachReason::Logout,
        },
        vec![],
    )
    .await
    .unwrap();
    assert_eq!(
        close_reason(&h, &x.send_id).as_deref(),
        Some("unregistered")
    );
    let n: i64 = query_one(&h.db(), "SELECT count(*) FROM scheduled", &[]).unwrap();
    assert_eq!(n, 0);
    problems(&h).await;
}

// ------------------------------------------------------------ handshake

#[tokio::test]
async fn hello_announces_every_feature() {
    let h = Harness::start().await;
    let mut c = silicon_peek_client::runtime::daemon::DaemonConnection::connect(
        h.handle().socket_path(),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let hello = c
        .hello(
            &silicon_peek_client::ipc::cli::Hello::cli("macos-aarch64", None),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(
        hello.features,
        features::ALL
            .iter()
            .map(|f| (*f).to_owned())
            .collect::<Vec<_>>()
    );
    assert_eq!(hello.peekd_version, silicon_peek_client::VERSION);
}

// ------------------------------------------------------------ invariants

/// A small deterministic PRNG (xorshift) for the invariant walk.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[tokio::test]
async fn random_operations_keep_the_queue_invariants() {
    let h = Harness::start().await;
    let (home, ui) = ready(&h, "si:chaos", 6, 1002).await;
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut offset = Duration::ZERO;
    for step in 0..120 {
        let op = rng.below(9);
        match op {
            0 | 1 => {
                let mut s = if rng.below(3) == 0 {
                    ask("chaos?")
                } else {
                    show("chaos")
                };
                if rng.below(4) == 0 {
                    s.expires_in_s = Some(10);
                }
                let _ = h.call(&home, &s, vec![]).await;
            }
            2 => {
                let mut s = speak("replace");
                s.replace = true;
                let _ = h.call(&home, &s, vec![]).await;
            }
            3 => {
                let mut s = show("scheduled chaos");
                s.due_at = Some(peekd_now(offset + Duration::from_secs(5)));
                let _ = h.call(&home, &s, vec![]).await;
            }
            4 => {
                offset += Duration::from_secs(11);
                h.handle().advance_wall_clock(Duration::from_secs(11));
                h.handle().timers_now().await;
            }
            5 => {
                let (l, _) = h.call(&home, &QueueList {}, vec![]).await.unwrap();
                if let Some(cur) = l.on_screen {
                    let _ = ui
                        .request(
                            &Shown {
                                send_id: cur.send_id.clone(),
                            },
                            vec![],
                        )
                        .await;
                    if cur.ask_id.is_none() {
                        shown_done(&ui, &cur.send_id).await;
                    } else {
                        let _ = ui
                            .request(
                                &Dismissed {
                                    send_id: cur.send_id,
                                    gesture: Gesture::DownArrow,
                                },
                                vec![],
                            )
                            .await;
                    }
                }
            }
            6 => {
                let (l, _) = h.call(&home, &QueueList {}, vec![]).await.unwrap();
                let all: Vec<_> = l.on_screen.iter().chain(l.waiting.iter()).collect();
                if !all.is_empty() {
                    let pick = all[usize::try_from(rng.below(all.len() as u64)).unwrap()];
                    let _ = h
                        .call(
                            &home,
                            &SendCancel {
                                target: pick.send_id.to_string(),
                            },
                            vec![],
                        )
                        .await;
                }
            }
            7 => {
                let _ = h
                    .call(
                        &home,
                        &QueueClear {
                            all: rng.below(2) == 0,
                        },
                        vec![],
                    )
                    .await;
            }
            _ => {
                let p = if rng.below(2) == 0 {
                    Presence::default()
                } else {
                    Presence {
                        available: rng.below(2) == 0,
                        reason: PresenceReason::Locked,
                        paused: rng.below(2) == 0,
                    }
                };
                let _ = ui.request(&p, vec![]).await;
            }
        }
        let p = h.handle().check_queues().await.unwrap();
        assert!(p.is_empty(), "step {step} (op {op}): {p:?}");
    }
}
