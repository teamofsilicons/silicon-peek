//! `peek queue [list|clear]`, `peek cancel` and `peek schedule
//! list|cancel|clear` (0.1.2 contract §5.6): this Silicon's queue on its
//! position and its one-time scheduled sends. All Mac-only, all
//! feature-gated against an older peekd.

use std::fmt::Write as _;

use serde_json::Value;
use silicon_peek_client::{
    Result,
    ipc::cli::{
        QueueClear, QueueList, ScheduleCancel, ScheduleClear, ScheduleList, SendCancel, features,
    },
    runtime::daemon::REQUEST_TIMEOUT,
    timestamp::Timestamp,
};

use super::{mac_session, next, require_features};
use crate::{
    cli::{QueueCommand, ScheduleCommand},
    commands::send::display_zone,
    context::Globals,
    output::Out,
    service::{self, require_mac},
    when::{self, LocalZone},
};

const QUEUE_WHAT: &str = "peek queue / peek cancel";
const SCHEDULE_WHAT: &str = "peek schedule";

async fn connect(
    g: &Globals,
    feature: &'static str,
    what: &'static str,
) -> Result<(service::Service, silicon_peek_client::ipc::AuthBlock)> {
    let (_session, auth) = mac_session(g).await?;
    let svc = service::ensure_service().await?;
    require_features(&svc, &[(feature, what)])?;
    Ok((svc, auth))
}

/// `peek queue` / `peek queue list` / `peek queue clear [--all]`.
pub async fn queue(g: &Globals, out: Out, command: Option<QueueCommand>) -> Result<()> {
    match command {
        None | Some(QueueCommand::List) => {
            require_mac(if command.is_some() {
                "queue list"
            } else {
                "queue"
            })?;
            let (mut svc, auth) = connect(g, features::QUEUE_V2, QUEUE_WHAT).await?;
            let (list, _) = svc
                .call(&QueueList {}, Some(&auth), Vec::new(), REQUEST_TIMEOUT)
                .await?;
            let now = Timestamp::now();
            out.result(&list, |v| human_queue(v, now));
            if !list.waiting.is_empty() {
                next(out, &["peek cancel <SEND_ID>", "peek queue clear"]);
            }
            Ok(())
        }
        Some(QueueCommand::Clear { all }) => {
            require_mac("queue clear")?;
            let (mut svc, auth) = connect(g, features::QUEUE_V2, QUEUE_WHAT).await?;
            let (r, _) = svc
                .call(
                    &QueueClear { all },
                    Some(&auth),
                    Vec::new(),
                    REQUEST_TIMEOUT,
                )
                .await?;
            out.result(&r, human_clear);
            next(out, &["peek queue"]);
            Ok(())
        }
    }
}

/// `peek cancel <SEND_ID>`.
pub async fn cancel(g: &Globals, out: Out, id: &str) -> Result<()> {
    require_mac("cancel")?;
    let (mut svc, auth) = connect(g, features::QUEUE_V2, QUEUE_WHAT).await?;
    let (r, _) = svc
        .call(
            &SendCancel {
                target: id.trim().to_owned(),
            },
            Some(&auth),
            Vec::new(),
            REQUEST_TIMEOUT,
        )
        .await?;
    let zone = display_zone(r.tz.as_deref());
    out.result(&r, |v| human_cancel(v, &zone));
    next(out, &["peek queue"]);
    Ok(())
}

/// `peek schedule list|cancel|clear`.
pub async fn schedule(g: &Globals, out: Out, command: ScheduleCommand) -> Result<()> {
    require_mac(match command {
        ScheduleCommand::List => "schedule list",
        ScheduleCommand::Cancel { .. } => "schedule cancel",
        ScheduleCommand::Clear => "schedule clear",
    })?;
    let (mut svc, auth) = connect(g, features::SCHEDULE, SCHEDULE_WHAT).await?;
    match command {
        ScheduleCommand::List => {
            let (list, _) = svc
                .call(&ScheduleList {}, Some(&auth), Vec::new(), REQUEST_TIMEOUT)
                .await?;
            let local = when::mac_local_zone();
            let now = Timestamp::now();
            out.result(&list, |v| human_schedule(v, &local, now));
            if !list.scheduled.is_empty() {
                next(out, &["peek schedule cancel <ID>", "peek schedule clear"]);
            }
        }
        ScheduleCommand::Cancel { id } => {
            let (r, _) = svc
                .call(
                    &ScheduleCancel {
                        target: id.trim().to_owned(),
                    },
                    Some(&auth),
                    Vec::new(),
                    REQUEST_TIMEOUT,
                )
                .await?;
            let zone = display_zone(r.tz.as_deref());
            out.result(&r, |v| human_schedule_cancel(v, &zone));
            if r.state == "fired" {
                next(out, &[&format!("peek cancel {}", r.send_id)]);
            } else {
                next(out, &["peek schedule list"]);
            }
        }
        ScheduleCommand::Clear => {
            let (r, _) = svc
                .call(&ScheduleClear {}, Some(&auth), Vec::new(), REQUEST_TIMEOUT)
                .await?;
            out.result(&r, human_schedule_clear);
        }
    }
    Ok(())
}

fn ts(v: &Value) -> Option<Timestamp> {
    v.as_str().and_then(|s| Timestamp::parse(s).ok())
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The `peek queue` view (contract §5.6).
pub(crate) fn human_queue(v: &Value, now: Timestamp) -> String {
    let Some(slot) = v["slot"].as_u64() else {
        return "no position registered (peek register side <1-8>)".to_owned();
    };
    let scheduled = v["scheduled"].as_u64().unwrap_or_default();
    let waiting = v["waiting"].as_array().cloned().unwrap_or_default();
    let on_screen = v.get("on_screen").filter(|o| !o.is_null());
    let Some(on_screen) = on_screen else {
        let mut line = format!("position {slot}: nothing on screen or waiting");
        if scheduled > 0 {
            let _ = write!(line, " · {scheduled} scheduled (peek schedule list)");
        }
        return line;
    };
    let held = match v["held"].as_str() {
        Some("carbon_away") => " · held: the Carbon's screen is locked or asleep",
        Some("paused") => " · held: Peek is paused",
        Some("app_not_running") => " · held: Peek.app is not running",
        _ => "",
    };
    let mut lines = vec![format!(
        "position {slot} · 1 on screen · {} waiting (limit {}) · {scheduled} scheduled{held}",
        waiting.len(),
        v["limit"].as_u64().unwrap_or(5)
    )];
    for item in std::iter::once(on_screen).chain(waiting.iter()) {
        let label = match item["state"].as_str().unwrap_or_default() {
            "on_screen" => "on screen".to_owned(),
            "held" => "held".to_owned(),
            "due_waiting" => "due".to_owned(),
            _ => item["queue_position"].to_string(),
        };
        let age = when::age(item["age_ms"].as_u64().unwrap_or_default());
        let summary = format!("\"{}\"", item["summary"].as_str().unwrap_or_default());
        let expires = ts(&item["expires_at"])
            .map(|e| format!("expires {}", when::render_relative(e, now)))
            .unwrap_or_default();
        let row = format!(
            "  {label:<9}  {}  {:<10}{age:>5}  {summary:<20}  {expires}",
            item["send_id"].as_str().unwrap_or_default(),
            item["kind"].as_str().unwrap_or_default(),
        );
        lines.push(row.trim_end().to_owned());
    }
    lines.join("\n")
}

/// The `peek queue clear` line.
pub(crate) fn human_clear(v: &Value) -> String {
    let n = v["cancelled"].as_array().map_or(0, Vec::len);
    let on_screen = v["on_screen"].as_str();
    let gone = v["on_screen_cancelled"] == true;
    let waiting = if gone { n.saturating_sub(1) } else { n };
    match (gone, on_screen) {
        (true, Some(id)) if waiting == 0 => {
            format!("cleared the one on screen ({id}); nothing was waiting")
        }
        (true, Some(id)) => format!(
            "cleared {} and the one on screen ({id})",
            plural(waiting, "waiting send", "waiting sends")
        ),
        _ if waiting == 0 => "nothing was waiting".to_owned(),
        (false, Some(id)) => format!(
            "cleared {}; the one on screen ({id}) stays",
            plural(waiting, "waiting send", "waiting sends")
        ),
        _ => format!(
            "cleared {}",
            plural(waiting, "waiting send", "waiting sends")
        ),
    }
}

/// The `peek cancel` line, by where the send was.
pub(crate) fn human_cancel(v: &Value, zone: &LocalZone) -> String {
    let id = v["send_id"].as_str().unwrap_or_default();
    match v["was"].as_str().unwrap_or_default() {
        "on_screen" | "held" => {
            format!("{id} cancelled; the bubble slides away and no Ting event is sent")
        }
        "waiting" | "due_waiting" => format!(
            "{id} cancelled; it was #{} in line and will not be shown",
            v["queue_position"]
        ),
        "scheduled" => match ts(&v["due_at"]) {
            Some(due) => format!(
                "{id} cancelled; it was scheduled for {} and will not be sent",
                when::render_local(due, &zone.zone, zone.name.as_deref())
            ),
            None => format!("{id} cancelled; it was scheduled and will not be sent"),
        },
        _ => format!(
            "{id} had already closed ({}); nothing to cancel",
            v["state"].as_str().unwrap_or_default()
        ),
    }
}

/// The `peek schedule list` view.
pub(crate) fn human_schedule(v: &Value, local: &LocalZone, now: Timestamp) -> String {
    let items = v["scheduled"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        return "nothing scheduled".to_owned();
    }
    let position = v["slot"]
        .as_u64()
        .map(|s| format!(" for position {s}"))
        .unwrap_or_default();
    let mut lines = vec![format!(
        "{} scheduled{position} (limit {})",
        items.len(),
        v["limit"].as_u64().unwrap_or(500)
    )];
    for item in &items {
        let zone = item["tz"]
            .as_str()
            .and_then(|t| when::parse_tz(t).ok())
            .map_or_else(|| local.zone.clone(), |t| t.zone);
        let due = ts(&item["due_at"]).unwrap_or(now);
        let when_col = format!(
            "{} ({})",
            when::render_short(due, &zone),
            when::render_relative(due, now)
        );
        let mut row = format!(
            "  {}  {}  {:<6}{when_col:<32}  \"{}\"",
            item["schedule_id"].as_str().unwrap_or_default(),
            item["send_id"].as_str().unwrap_or_default(),
            item["kind"].as_str().unwrap_or_default(),
            item["summary"].as_str().unwrap_or_default()
        );
        let mut extras = Vec::new();
        if let Some(exp) = ts(&item["expires_at"]) {
            extras.push(format!("expires {}", when::render_near(exp, due, &zone)));
        }
        if item["replace"] == true {
            extras.push("replace".to_owned());
        }
        if !extras.is_empty() {
            row.push_str("  ");
            row.push_str(&extras.join(" · "));
        }
        lines.push(row);
    }
    lines.join("\n")
}

/// The `peek schedule cancel` line.
pub(crate) fn human_schedule_cancel(v: &Value, zone: &LocalZone) -> String {
    let sch = v["schedule_id"].as_str().unwrap_or_default();
    let snd = v["send_id"].as_str().unwrap_or_default();
    if v["state"] == "fired" {
        return format!("{sch} already fired as {snd}; withdraw it with peek cancel {snd}");
    }
    match ts(&v["due_at"]) {
        Some(due) => format!(
            "{sch} ({snd}) cancelled; it was due {} and will not be sent",
            when::render_local(due, &zone.zone, zone.name.as_deref())
        ),
        None => format!("{sch} ({snd}) cancelled; it will not be sent"),
    }
}

/// The `peek schedule clear` line.
pub(crate) fn human_schedule_clear(v: &Value) -> String {
    match v["cancelled"].as_array().map_or(0, Vec::len) {
        0 => "nothing was scheduled".to_owned(),
        n => format!(
            "cancelled {}",
            plural(n, "scheduled send", "scheduled sends")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kolkata() -> LocalZone {
        LocalZone {
            zone: jiff::tz::db()
                .get("Asia/Kolkata")
                .unwrap_or(jiff::tz::TimeZone::UTC),
            name: Some("Asia/Kolkata".to_owned()),
            fallback_utc: false,
        }
    }

    fn now() -> Timestamp {
        Timestamp::parse("2026-09-27T06:12:00Z").unwrap_or(Timestamp::from_unix(0))
    }

    const A: &str = "snd_0192a000000070008000000000000001";
    const B: &str = "snd_0192a000000070008000000000000002";
    const C: &str = "snd_0192a000000070008000000000000003";

    fn item(
        id: &str,
        kind: &str,
        state: &str,
        pos: u32,
        age_ms: u64,
        summary: &str,
        expires: Option<&str>,
    ) -> Value {
        json!({"send_id": id, "ask_id": null, "kind": kind, "summary": summary, "state": state,
            "queue_position": pos, "created_at": "2026-09-27T06:11:48Z", "queued_at": "2026-09-27T06:11:48Z",
            "age_ms": age_ms, "expires_at": expires, "shown_at": null, "schedule_id": null, "due_at": null})
    }

    #[test]
    fn queue_view_matches_the_contract() {
        let v = json!({"slot": 3, "limit": 5, "held": null, "scheduled": 4, "scheduled_limit": 500,
            "on_screen": item(A, "ask", "on_screen", 0, 12_000, "Delete old.zip?", Some("2026-09-27T06:21:48Z")),
            "waiting": [item(B, "show", "waiting", 1, 8_000, "Build finished", None),
                        item(C, "speak", "waiting", 2, 3_000, "Deploy done", Some("2026-09-27T06:12:50Z"))]});
        assert_eq!(
            human_queue(&v, now()),
            format!(
                "position 3 · 1 on screen · 2 waiting (limit 5) · 4 scheduled\n  \
                 on screen  {A}  ask         12s  \"Delete old.zip?\"     expires in 9m 48s\n  \
                 1          {B}  show         8s  \"Build finished\"\n  \
                 2          {C}  speak        3s  \"Deploy done\"         expires in 50s"
            )
        );
        let mut held = v.clone();
        held["held"] = json!("paused");
        held["on_screen"]["state"] = json!("held");
        held["waiting"][1]["state"] = json!("due_waiting");
        let text = human_queue(&held, now());
        assert!(text.starts_with("position 3 · 1 on screen · 2 waiting (limit 5) · 4 scheduled · held: Peek is paused\n  held       "), "{text}");
        assert!(text.contains(&format!("\n  due        {C}")), "{text}");
        for (reason, line) in [
            (
                "carbon_away",
                " · held: the Carbon's screen is locked or asleep",
            ),
            ("app_not_running", " · held: Peek.app is not running"),
        ] {
            let mut h = v.clone();
            h["held"] = json!(reason);
            assert!(
                human_queue(&h, now())
                    .lines()
                    .next()
                    .is_some_and(|l| l.ends_with(line))
            );
        }
        let empty = json!({"slot": 3, "limit": 5, "held": null, "scheduled": 4, "scheduled_limit": 500,
            "on_screen": null, "waiting": []});
        assert_eq!(
            human_queue(&empty, now()),
            "position 3: nothing on screen or waiting · 4 scheduled (peek schedule list)"
        );
        let mut none = empty.clone();
        none["scheduled"] = json!(0);
        assert_eq!(
            human_queue(&none, now()),
            "position 3: nothing on screen or waiting"
        );
        let unplaced = json!({"slot": null, "limit": 5, "held": null, "scheduled": 0, "scheduled_limit": 500,
            "on_screen": null, "waiting": []});
        assert_eq!(
            human_queue(&unplaced, now()),
            "no position registered (peek register side <1-8>)"
        );
    }

    #[test]
    fn clear_lines() {
        let r = |cancelled: &[&str], on: Option<&str>, gone: bool| {
            human_clear(
                &json!({"cancelled": cancelled, "on_screen": on, "on_screen_cancelled": gone}),
            )
        };
        assert_eq!(
            r(&[B, C], Some(A), false),
            format!("cleared 2 waiting sends; the one on screen ({A}) stays")
        );
        assert_eq!(
            r(&[A, B, C], Some(A), true),
            format!("cleared 2 waiting sends and the one on screen ({A})")
        );
        assert_eq!(
            r(&[B], Some(A), false),
            format!("cleared 1 waiting send; the one on screen ({A}) stays")
        );
        assert_eq!(
            r(&[A], Some(A), true),
            format!("cleared the one on screen ({A}); nothing was waiting")
        );
        assert_eq!(r(&[], None, false), "nothing was waiting");
        assert_eq!(r(&[], None, true), "nothing was waiting");
        assert_eq!(r(&[], Some(A), false), "nothing was waiting");
        assert_eq!(r(&[B, C], None, false), "cleared 2 waiting sends");
    }

    #[test]
    fn cancel_lines() {
        let z = kolkata();
        let c = |was: &str, extra: Value| {
            let mut v = json!({"send_id": A, "ask_id": null, "schedule_id": null, "was": was,
                "queue_position": null, "state": "cancelled", "due_at": null, "tz": null});
            if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
                o.extend(e.clone());
            }
            human_cancel(&v, &z)
        };
        assert_eq!(
            c("on_screen", json!({})),
            format!("{A} cancelled; the bubble slides away and no Ting event is sent")
        );
        assert_eq!(
            c("held", json!({})),
            format!("{A} cancelled; the bubble slides away and no Ting event is sent")
        );
        assert_eq!(
            c("waiting", json!({"queue_position": 2})),
            format!("{A} cancelled; it was #2 in line and will not be shown")
        );
        assert_eq!(
            c("due_waiting", json!({"queue_position": 6})),
            format!("{A} cancelled; it was #6 in line and will not be shown")
        );
        assert_eq!(
            c("scheduled", json!({"due_at": "2026-09-27T12:30:00Z"})),
            format!(
                "{A} cancelled; it was scheduled for 2026-09-27 18:00 IST (Asia/Kolkata) and will not be sent"
            )
        );
        assert_eq!(
            c("closed", json!({"state": "speech_done"})),
            format!("{A} had already closed (speech_done); nothing to cancel")
        );
    }

    #[test]
    fn schedule_lines() {
        let z = kolkata();
        let s1 = "sch_0192a000000070008000000000000001";
        let s2 = "sch_0192a000000070008000000000000002";
        let v = json!({"limit": 500, "slot": 3, "scheduled": [
            {"schedule_id": s1, "send_id": A, "ask_id": null, "kind": "ask", "summary": "Stand-up in 5?",
             "due_at": "2026-09-27T12:30:00Z", "tz": "Asia/Kolkata", "expires_at": "2026-09-27T13:00:00Z",
             "replace": true, "created_at": "2026-09-27T06:00:00Z"},
            {"schedule_id": s2, "send_id": B, "ask_id": null, "kind": "show", "summary": "Morning",
             "due_at": "2026-09-27T23:12:00Z", "tz": null, "expires_at": null,
             "replace": false, "created_at": "2026-09-27T06:00:00Z"}]});
        assert_eq!(
            human_schedule(&v, &z, now()),
            format!(
                "2 scheduled for position 3 (limit 500)\n  \
                 {s1}  {A}  ask   2026-09-27 18:00 IST (in 6h 18m)  \"Stand-up in 5?\"  expires 18:30 · replace\n  \
                 {s2}  {B}  show  2026-09-28 04:42 IST (in 17h)     \"Morning\""
            )
        );
        assert_eq!(
            human_schedule(&json!({"limit": 500, "scheduled": []}), &z, now()),
            "nothing scheduled"
        );
        assert_eq!(
            human_schedule_cancel(
                &json!({"schedule_id": s1, "send_id": A, "state": "cancelled",
                "due_at": "2026-09-27T12:30:00Z", "tz": "Asia/Kolkata"}),
                &z
            ),
            format!(
                "{s1} ({A}) cancelled; it was due 2026-09-27 18:00 IST (Asia/Kolkata) and will not be sent"
            )
        );
        assert_eq!(
            human_schedule_cancel(
                &json!({"schedule_id": s1, "send_id": A, "state": "fired"}),
                &z
            ),
            format!("{s1} already fired as {A}; withdraw it with peek cancel {A}")
        );
        assert_eq!(
            human_schedule_clear(&json!({"cancelled": [s1, s2, s1]})),
            "cancelled 3 scheduled sends"
        );
        assert_eq!(
            human_schedule_clear(&json!({"cancelled": [s1]})),
            "cancelled 1 scheduled send"
        );
        assert_eq!(
            human_schedule_clear(&json!({"cancelled": []})),
            "nothing was scheduled"
        );
    }
}
