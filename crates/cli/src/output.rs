//! Output rules (BLUEPRINT §7.5).
//!
//! - `--json`: exactly one compact JSON value on stdout on success; on failure
//!   stdout stays empty and one `{"error":{…}}` object goes to stderr.
//! - Human mode: readable text on stdout; hints, warnings and `Next:` lines on
//!   stderr (suppressed by `--quiet`); errors as `error:` / `hint:` / `help:` /
//!   `request:` lines.

use std::fmt::Write as _;
use std::io::Write as _;

use serde::Serialize;
use serde_json::Value;
use silicon_peek_client::{Error, ErrorCode, ipc::cli::Warning};

/// How this invocation prints.
#[derive(Clone, Copy, Debug)]
pub struct Out {
    /// `--json`.
    pub json: bool,
    /// `--quiet`.
    pub quiet: bool,
}

impl Out {
    /// Prints a result: compact JSON with `--json`, else `human(value)`.
    pub fn value(self, value: &Value, human: impl FnOnce(&Value) -> String) {
        let text = if self.json {
            value.to_string()
        } else {
            human(value)
        };
        let mut stdout = std::io::stdout().lock();
        // A closed stdout (EPIPE) is the reader's choice; never panic on it.
        let _ = writeln!(stdout, "{text}");
        let _ = stdout.flush();
    }

    /// Serializes `result` and prints it.
    pub fn result<T: Serialize>(self, result: &T, human: impl FnOnce(&Value) -> String) {
        let value = serde_json::to_value(result).unwrap_or(Value::Null);
        self.value(&value, human);
    }

    /// A hint or `Next:` line on stderr, in human mode only.
    pub fn hint(self, line: impl AsRef<str>) {
        if !self.json && !self.quiet {
            eprintln!("{}", line.as_ref());
        }
    }

    /// Warnings from peekd, on stderr in human mode (they are part of the
    /// JSON result otherwise).
    pub fn warnings(self, warnings: &[Warning]) {
        if self.json {
            return;
        }
        for w in warnings {
            eprintln!("warning: {} ({})", w.message, w.code);
            if w.code == "drawing_fallback_active"
                && let Some(stack) = w
                    .details
                    .as_ref()
                    .and_then(|d| d.get("stack"))
                    .and_then(Value::as_str)
            {
                for line in stack.lines() {
                    eprintln!("    {line}");
                }
            }
        }
    }

    /// Prints a failure.
    pub fn error(self, error: &Error, path: &[String]) {
        if self.json {
            eprintln!("{}", error.envelope());
            return;
        }
        if *error.code() == ErrorCode::DrawingInvalid
            && let Some(block) = drawing_failure(error)
        {
            eprintln!("{block}");
        }
        eprintln!("error: {}", error.message());
        if let Some(h) = error.hint() {
            eprintln!("hint: {h}");
        }
        if !path.is_empty() && !path[0].starts_with("__") {
            eprintln!("help: peek {} --help", path.join(" "));
        }
        if let Some(r) = error.request_id() {
            eprintln!("request: {r}");
        }
    }
}

/// The visual.md A9 failure block for a `drawing_invalid` error.
fn drawing_failure(error: &Error) -> Option<String> {
    let details = error.details()?;
    let e = details.get("error").unwrap_or(details);
    let message = e.get("message").and_then(Value::as_str)?;
    let mut out = match e.get("frame").and_then(Value::as_u64) {
        // The engine may already say "frame 60 threw: …" or "frame 4 was
        // interrupted": one prefix, never two.
        Some(frame) if message.starts_with(&format!("frame {frame} ")) => format!("✗ {message}"),
        Some(frame) => format!("✗ frame {frame} threw: {message}"),
        None => format!("✗ {message}"),
    };
    if let Some(stack) = e.get("stack").and_then(Value::as_str) {
        // peek's own prelude frames say nothing about the author's script.
        for line in stack.lines().filter(|l| !l.contains("peek-prelude.js")) {
            out.push_str("\n    ");
            out.push_str(line);
        }
    }
    if let (Some(frame), Some(summary)) = (
        e.get("frame").and_then(Value::as_u64),
        e.get("input_summary").and_then(Value::as_str),
    ) {
        let _ = write!(out, "\n  input at frame {frame}: {summary}");
    }
    let check_only = details.get("check_only") == Some(&Value::Bool(true));
    let previous = details.get("previous_active") == Some(&Value::Bool(true));
    out.push_str(match (check_only, previous) {
        (true, _) => "\ndrawing NOT valid (--check: nothing was registered)",
        (false, true) => "\ndrawing NOT registered (previous drawing still active)",
        (false, false) => "\ndrawing NOT registered",
    });
    Some(out)
}

/// Renders a JSON value as indented `key: value` lines for human mode.
pub fn kv(value: &Value) -> String {
    let mut out = String::new();
    render(value, 0, &mut out);
    out.trim_end().to_owned()
}

fn scalar(v: &Value) -> String {
    match v {
        Value::Null => "-".to_owned(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn render(value: &Value, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                match v {
                    Value::Object(m) if !m.is_empty() => {
                        let _ = writeln!(out, "{pad}{k}:");
                        render(v, indent + 1, out);
                    }
                    Value::Array(a) if a.iter().any(Value::is_object) => {
                        let _ = writeln!(out, "{pad}{k}:");
                        render(v, indent + 1, out);
                    }
                    Value::Array(a) => {
                        let items: Vec<String> = a.iter().map(scalar).collect();
                        let joined = if items.is_empty() {
                            "-".to_owned()
                        } else {
                            items.join(", ")
                        };
                        let _ = writeln!(out, "{pad}{k}: {joined}");
                    }
                    Value::Object(_) => {
                        let _ = writeln!(out, "{pad}{k}: -");
                    }
                    other => {
                        let _ = writeln!(out, "{pad}{k}: {}", scalar(other));
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                if item.is_object() {
                    let _ = writeln!(out, "{pad}-");
                    render(item, indent + 1, out);
                } else {
                    let _ = writeln!(out, "{pad}- {}", scalar(item));
                }
            }
        }
        other => {
            let _ = writeln!(out, "{pad}{}", scalar(other));
        }
    }
}
