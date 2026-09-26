//! Filing bug reports as GitHub issues (BLUEPRINT §5.2 "Bug reports").

use std::{fmt::Write as _, time::Duration};

use serde::Deserialize;
use serde_json::{Value, json};
use silicon_peek_client::api::{ReportContext, ReportRequest};

use crate::state::AppState;

const TIMEOUT: Duration = Duration::from_secs(10);
const STATUS_MAX_BYTES: usize = 16 * 1024;

/// The issue title: the message's first non-empty line, at most 100
/// characters.
pub(crate) fn title(message: &str) -> String {
    let first = message
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("peek report");
    first.chars().take(100).collect()
}

/// The issue body: the message, the proposed fix, then where it came from.
pub(crate) fn body(report: &ReportRequest) -> String {
    let mut out = report.message.trim().to_owned();
    if let Some(pr) = &report.pr {
        let _ = write!(out, "\n\nProposed fix: {pr}");
    }
    let context = report.context.clone().unwrap_or_default();
    let ReportContext {
        cli_version,
        platform,
        command,
        error_code,
    } = context;
    let _ = write!(
        out,
        "\n\nSubmitted with peek {} on {}",
        cli_version.as_deref().unwrap_or("(unknown version)"),
        platform.as_deref().unwrap_or("(unknown platform)")
    );
    if let Some(command) = command {
        let _ = write!(out, "\nCommand: `peek {command}`");
    }
    if let Some(code) = error_code {
        let _ = write!(out, "\nError code: `{code}`");
    }
    if let Some(status) = &report.status {
        let mut rendered = serde_json::to_string_pretty(status).unwrap_or_default();
        if rendered.len() > STATUS_MAX_BYTES {
            let mut cut = STATUS_MAX_BYTES;
            while !rendered.is_char_boundary(cut) {
                cut -= 1;
            }
            rendered.truncate(cut);
            rendered.push_str("\n… (truncated)");
        }
        let _ = write!(
            out,
            "\n\n<details><summary>peek doctor</summary>\n\n```json\n{rendered}\n```\n</details>"
        );
    }
    out
}

#[derive(Deserialize)]
struct Issue {
    html_url: String,
}

/// `POST {api}/repos/{repo}/issues`; returns the issue URL, or why filing
/// failed (the report then stays `stored`).
pub(crate) async fn file_issue(state: &AppState, report: &ReportRequest) -> Result<String, String> {
    let config = &state.0.config.github;
    let Some(token) = &config.issues_token else {
        return Err("PEEK_GITHUB_ISSUES_TOKEN is not configured".to_owned());
    };
    let response = state
        .0
        .http
        .post(format!("{}/repos/{}/issues", config.api_url, config.repo))
        .timeout(TIMEOUT)
        .bearer_auth(token.expose())
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(&json!({"title": title(&report.message), "body": body(report)}))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "GitHub timed out".to_owned()
            } else {
                "GitHub could not be reached".to_owned()
            }
        })?;
    let status = response.status();
    if !status.is_success() {
        let detail: Value = response.json().await.unwrap_or(Value::Null);
        let message = detail
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("no message");
        return Err(format!("GitHub answered HTTP {status}: {message}"));
    }
    let issue: Issue = response
        .json()
        .await
        .map_err(|_| "GitHub answered without an issue URL".to_owned())?;
    if !issue.html_url.starts_with("https://") {
        return Err("GitHub answered with an unexpected issue URL".to_owned());
    }
    Ok(issue.html_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_and_body_follow_the_blueprint() {
        let report = ReportRequest {
            message: "\n  send hangs when the app is closed\nmore detail".into(),
            pr: Some("https://github.com/teamofsilicons/silicon-peek/pull/7".into()),
            context: Some(ReportContext {
                cli_version: Some("0.1.0".into()),
                platform: Some("macos-aarch64".into()),
                command: Some("send".into()),
                error_code: Some("daemon_unavailable".into()),
            }),
            status: Some(json!({"checks": []})),
        };
        assert_eq!(title(&report.message), "send hangs when the app is closed");
        assert_eq!(title(&"x".repeat(300)).chars().count(), 100);
        let b = body(&report);
        assert!(b.starts_with("send hangs when the app is closed\nmore detail"));
        assert!(
            b.contains("\n\nProposed fix: https://github.com/teamofsilicons/silicon-peek/pull/7")
        );
        assert!(b.contains("Submitted with peek 0.1.0 on macos-aarch64"));
        assert!(b.contains("```json"));
    }
}
