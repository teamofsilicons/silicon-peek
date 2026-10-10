//! `peek ask get|list|cancel` and `peek history`: local state kept by peekd
//! (BLUEPRINT §1.6 CLI ops). These work while Ting delivery is not set up.

use serde_json::Value;
use silicon_peek_client::{
    Error, Result,
    ids::{AskId, SendId},
    ipc::cli::{AskCancel, AskGet, AskList, AskState, History},
    runtime::daemon::REQUEST_TIMEOUT,
    schema::limits,
};

use super::{mac_session, next};
use crate::{
    cli::{AskCommand, AskStateArg, HistoryArgs},
    context::Globals,
    output::{Out, kv},
    service::{self, require_mac},
};

fn state(s: AskStateArg) -> AskState {
    match s {
        AskStateArg::Pending => AskState::Pending,
        AskStateArg::Answered => AskState::Answered,
        AskStateArg::Dismissed => AskState::Dismissed,
        AskStateArg::Expired => AskState::Expired,
        AskStateArg::Cancelled => AskState::Cancelled,
        AskStateArg::Replaced => AskState::Replaced,
    }
}

fn check_limit(limit: Option<u32>) -> Result<()> {
    match limit {
        Some(0) => Err(Error::invalid_input("--limit must be at least 1")),
        Some(n) if n > limits::HISTORY_MAX_LIMIT => Err(Error::invalid_input(format!(
            "--limit {n} exceeds {}",
            limits::HISTORY_MAX_LIMIT
        ))),
        _ => Ok(()),
    }
}

pub async fn run(g: &Globals, out: Out, command: AskCommand) -> Result<()> {
    let name = match &command {
        AskCommand::Get { .. } => "ask get",
        AskCommand::List { .. } => "ask list",
        AskCommand::Cancel { .. } => "ask cancel",
    };
    require_mac(name)?;
    match command {
        AskCommand::Get { ask_id } => {
            let ask_id = AskId::parse(&ask_id)?;
            let (_s, auth) = mac_session(g).await?;
            let mut svc = service::ensure_service().await?;
            let (info, _) = svc
                .call(&AskGet { ask_id }, Some(&auth), Vec::new(), REQUEST_TIMEOUT)
                .await?;
            out.result(&info, kv);
            Ok(())
        }
        AskCommand::List { state: st, limit } => {
            check_limit(limit)?;
            let (_s, auth) = mac_session(g).await?;
            let mut svc = service::ensure_service().await?;
            let (list, _) = svc
                .call(
                    &AskList {
                        state: st.map(state),
                        limit,
                    },
                    Some(&auth),
                    Vec::new(),
                    REQUEST_TIMEOUT,
                )
                .await?;
            out.result(&list, human_list);
            Ok(())
        }
        AskCommand::Cancel { ask_id } => {
            let ask_id = AskId::parse(&ask_id)?;
            let (_s, auth) = mac_session(g).await?;
            let mut svc = service::ensure_service().await?;
            let (r, _) = svc
                .call(
                    &AskCancel { ask_id },
                    Some(&auth),
                    Vec::new(),
                    REQUEST_TIMEOUT,
                )
                .await?;
            out.result(&r, human_cancel);
            Ok(())
        }
    }
}

/// `ask cancel`'s human line: what actually happened, from the returned
/// state (an ask that had already closed was not cancelled).
fn human_cancel(v: &Value) -> String {
    let id = v["ask_id"].as_str().unwrap_or_default();
    match v["state"].as_str().unwrap_or_default() {
        "cancelled" => format!("{id} cancelled; the bubble slides away and no Ting event is sent"),
        "answered" => format!(
            "{id} was already answered; nothing to cancel (the answer was already delivered; see peek ask get {id})"
        ),
        state @ ("dismissed" | "expired") => {
            format!("{id} was already {state}; nothing to cancel")
        }
        "replaced" => format!("{id} was already replaced by a newer send; nothing to cancel"),
        other => format!("{id} is {other}; it was not cancelled"),
    }
}

/// A history row's warning codes, e.g. `  [speech_failed]`.
fn warning_codes(item: &Value) -> String {
    let codes: Vec<&str> = item["warnings"]
        .as_array()
        .map(|w| w.iter().filter_map(|w| w["code"].as_str()).collect())
        .unwrap_or_default();
    if codes.is_empty() {
        String::new()
    } else {
        format!("  [{}]", codes.join(", "))
    }
}

fn human_list(v: &Value) -> String {
    let asks = v["asks"].as_array().cloned().unwrap_or_default();
    if asks.is_empty() {
        return "no asks".to_owned();
    }
    asks.iter()
        .map(|a| {
            let question = a["question"]
                .as_str()
                .map(|q| format!("  \"{q}\""))
                .unwrap_or_default();
            format!(
                "{}  {:<9}{}",
                a["ask_id"].as_str().unwrap_or_default(),
                a["state"].as_str().unwrap_or_default(),
                question
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn history(g: &Globals, out: Out, args: HistoryArgs) -> Result<()> {
    require_mac("history")?;
    let request = History {
        limit: args.limit,
        before: args
            .before
            .map(|b| SendId::parse(&b).map(|s| s.as_str().to_owned()))
            .transpose()?,
    };
    request.validate()?;
    let (_s, auth) = mac_session(g).await?;
    let mut svc = service::ensure_service().await?;
    let (r, _) = svc
        .call(&request, Some(&auth), Vec::new(), REQUEST_TIMEOUT)
        .await?;
    out.result(&r, |v| {
        let items = v["items"].as_array().cloned().unwrap_or_default();
        if items.is_empty() {
            return "no sends yet".to_owned();
        }
        items
            .iter()
            .map(|i| {
                let closed = i["close_reason"]
                    .as_str()
                    .map(|r| format!(" → {r}"))
                    .unwrap_or_default();
                let ask = i["ask_id"]
                    .as_str()
                    .map(|a| format!("  {a} ({})", i["ask_state"].as_str().unwrap_or("?")))
                    .unwrap_or_default();
                format!(
                    "{}  {:<6} {}{closed}{ask}{}",
                    i["send_id"].as_str().unwrap_or_default(),
                    i["kind"].as_str().unwrap_or_default(),
                    i["created_at"].as_str().unwrap_or_default(),
                    warning_codes(i)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    });
    if r.items.len() >= usize::try_from(args.limit.unwrap_or(50)).unwrap_or(50)
        && let Some(last) = r.items.last()
    {
        next(out, &[&format!("peek history --before {}", last.send_id)]);
    }
    Ok(())
}
