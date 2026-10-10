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
