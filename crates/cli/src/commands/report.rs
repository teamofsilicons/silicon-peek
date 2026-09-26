//! `peek report "<message>" [--pr URL] [--via backend|gh] [--attach-status]`
//! (BLUEPRINT §7.6, D20). The backend stores the report and files a GitHub
//! issue; `--via gh` files it with the local GitHub CLI. Refused under
//! `--test`. Only the text, the PR, the version and platform (and, with
//! `--attach-status`, the non-secret doctor output) are sent.

use std::fmt::Write as _;
use std::{process::Stdio, time::Duration};

use serde_json::{Value, json};
use silicon_peek_client::{
    Error, ErrorCode, REPOSITORY_URL, Result, VERSION,
    api::{ReportContext, ReportRequest},
    ids::IdempotencyKey,
    platform,
};
use tokio::io::AsyncWriteExt as _;

use super::{doctor, fresh};
use crate::{
    cli::{ReportArgs, ReportVia},
    context::Globals,
    output::Out,
};

const REPO: &str = "teamofsilicons/silicon-peek";

fn title(message: &str) -> String {
    let first = message
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("peek bug report");
    first.trim().chars().take(100).collect()
}

fn body(message: &str, pr: Option<&str>, status: Option<&Value>) -> String {
    let mut b = message.trim().to_owned();
    if let Some(pr) = pr {
        let _ = write!(b, "\n\nProposed fix: {pr}");
    }
    if let Some(s) = status {
        let _ = write!(
            b,
            "\n\n<details><summary>peek doctor</summary>\n\n```json\n{}\n```\n</details>",
            serde_json::to_string_pretty(s).unwrap_or_default()
        );
    }
    let _ = write!(b, "\n\nSubmitted with peek {VERSION} on {}", platform());
    b
}

pub async fn run(g: &Globals, out: Out, args: ReportArgs) -> Result<()> {
    if g.is_testing()? && !args.dry_run {
        return Err(Error::new(
            ErrorCode::ConflictingFlags,
            "peek report is refused in a testing environment: reports go to the production issue tracker",
        )
        .with_hint("run it without --test / --app-secret-file / SILICON_PEEK_TEST"));
    }
    if args.message.trim().is_empty() {
        return Err(Error::invalid_input(
            "the report message is empty; describe what you ran, what happened and what you expected",
        ));
    }
    if let Some(pr) = &args.pr
        && !ReportRequest::pr_is_valid(pr)
    {
        return Err(
            Error::invalid_input(format!("--pr `{pr}` is not a pull request of {REPO}"))
                .with_hint(format!("pass https://github.com/{REPO}/pull/<number>")),
        );
    }
    let status = if args.attach_status {
        Some(doctor::checks_value(g).await)
    } else {
        None
    };
    if args.dry_run {
        let value = dry_run(g, &args, status).await?;
        out.value(&value, |v| {
            format!(
                "dry run: nothing was sent. peek report without --dry-run would file:\n{}",
                serde_json::to_string_pretty(&v["would_send"]).unwrap_or_default()
            )
        });
        return Ok(());
    }
    let value = match args.via {
        ReportVia::Backend => backend(g, &args, status).await?,
        ReportVia::Gh => gh(&args, status.as_ref()).await?,
    };
    out.value(&value, |v| {
        let mut s = format!(
            "report {} ({})",
            v["id"].as_str().unwrap_or("submitted"),
            v["status"].as_str().unwrap_or_default()
        );
        if let Some(url) = v["issue_url"].as_str() {
            let _ = write!(s, ": {url}");
        }
        s
    });
    if args.pr.is_none() {
        out.hint(format!(
            "You can also submit a fix at {REPOSITORY_URL} and pass --pr"
        ));
    }
    Ok(())
}

fn backend_request(args: &ReportArgs, status: Option<Value>) -> ReportRequest {
    ReportRequest {
        message: args.message.trim().to_owned(),
        pr: args.pr.clone(),
        context: Some(ReportContext {
            cli_version: Some(VERSION.to_owned()),
            platform: Some(platform()),
            command: None,
            error_code: None,
        }),
        status,
    }
}

/// `--dry-run`: what would be filed, and where; nothing is sent.
async fn dry_run(g: &Globals, args: &ReportArgs, status: Option<Value>) -> Result<Value> {
    Ok(match args.via {
        ReportVia::Backend => {
            // The same backend a real report would use, without creating a
            // store for it.
            let api = match crate::context::existing_store().ok().flatten() {
                Some(store) => match g.session(store, false).await {
                    Ok(session) => session.client.api_url().clone(),
                    Err(_) => g
                        .explicit_api()?
                        .unwrap_or_else(silicon_peek_client::identity::ApiUrl::production),
                },
                None => g
                    .explicit_api()?
                    .unwrap_or_else(silicon_peek_client::identity::ApiUrl::production),
            };
            json!({
                "dry_run": true,
                "via": "backend",
                "would_send": {
                    "method": "POST",
                    "url": format!("{}{}", api.as_str().trim_end_matches('/'), silicon_peek_client::api::routes::REPORTS),
                    "body": backend_request(args, status),
                },
            })
        }
        ReportVia::Gh => json!({
            "dry_run": true,
            "via": "gh",
            "would_send": {
                "command": format!("gh issue create --repo {REPO} --title <title> --body-file -"),
                "title": title(&args.message),
                "body": body(&args.message, args.pr.as_deref(), status.as_ref()),
            },
        }),
    })
}

async fn backend(g: &Globals, args: &ReportArgs, status: Option<Value>) -> Result<Value> {
    let key = g
        .idempotency_key()?
        .unwrap_or_else(IdempotencyKey::generate);
    let request = backend_request(args, status);
    let session = g.session(crate::context::store()?, false).await?;
    // Attribute the report to the session when there is a usable one; a
    // report must still go through without login.
    let client = match fresh(&session).await {
        Ok(slot) => match g.session_org(&slot) {
            Ok(org) => session.client.with_session(slot.access_token, org),
            Err(_) => session.client.clone(),
        },
        Err(_) => session.client.clone(),
    };
    let response = client.report(&request, &key).await?;
    Ok(
        json!({"submitted": true, "id": response.id, "status": response.status,
              "issue_url": response.issue_url, "via": "backend"}),
    )
}

async fn gh(args: &ReportArgs, status: Option<&Value>) -> Result<Value> {
    let text = body(&args.message, args.pr.as_deref(), status);
    let mut child = tokio::process::Command::new("gh")
        .args(["issue", "create", "--repo", REPO, "--title"])
        .arg(title(&args.message))
        .args(["--body-file", "-"])
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            Error::new(
                ErrorCode::NotFound,
                format!("the GitHub CLI `gh` could not be started: {e}"),
            )
            .with_hint("install gh and run `gh auth login`, or drop --via gh to file through the peek backend")
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await.map_err(|e| {
            Error::new(
                ErrorCode::Other("gh_failed".to_owned()),
                format!("writing the report body to gh failed: {e}"),
            )
        })?;
    }
    let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .map_err(|_| {
            Error::new(
                ErrorCode::Other("gh_failed".to_owned()),
                "gh issue create did not finish within 60 s",
            )
            .with_retryable(true)
        })?
        .map_err(|e| {
            Error::new(
                ErrorCode::Other("gh_failed".to_owned()),
                format!("waiting for gh failed: {e}"),
            )
        })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(Error::new(
            ErrorCode::Other("gh_failed".to_owned()),
            format!(
                "gh issue create failed ({}): {}",
                output.status,
                err.trim().lines().last().unwrap_or("no output")
            ),
        )
        .with_hint("check `gh auth status`, or drop --via gh to file through the peek backend")
        .with_retryable(true));
    }
    let url = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("https://"))
        .map(str::to_owned);
    Ok(json!({"submitted": true, "id": null, "status": "filed", "issue_url": url, "via": "gh"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_bodies() {
        assert_eq!(title("\n  first line\nsecond"), "first line");
        assert_eq!(title(&"x".repeat(150)).chars().count(), 100);
        let b = body(
            "msg",
            Some("https://github.com/teamofsilicons/silicon-peek/pull/1"),
            None,
        );
        assert!(b.starts_with(
            "msg\n\nProposed fix: https://github.com/teamofsilicons/silicon-peek/pull/1"
        ));
        assert!(b.ends_with(&format!("Submitted with peek {VERSION} on {}", platform())));
    }
}
