//! `peek register side|drawing` and `peek unregister` (BLUEPRINT §1.9.1,
//! §1.9.2, §7.4). Inputs are validated and the drawing is read (relative to
//! the current directory) before peekd is contacted.

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::{Value, json};
use silicon_peek_client::{
    Error, Result,
    config::Config,
    identity::SlotIndex,
    ipc::{
        AuthBlock,
        cli::{RegisterDrawing, RegisterSide, StatusOp, Unregister, Warning},
    },
    runtime::daemon::{LONG_REQUEST_TIMEOUT, REQUEST_TIMEOUT},
};

use super::{mac_session, next};
use crate::{
    cli::RegisterCommand,
    context::Globals,
    input,
    output::{Out, kv},
    service::{self, require_mac},
};

fn side_name(index: u64) -> String {
    SlotIndex::new(index)
        .ok()
        .and_then(|i| serde_json::to_value(i.side()).ok())
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

pub async fn run(g: &Globals, out: Out, command: RegisterCommand) -> Result<()> {
    match command {
        RegisterCommand::Side { index } => {
            let index = match index {
                Some(index) => index,
                None => u64::from(crate::context::store()?.read_config()?.position.ok_or_else(|| {
                    Error::invalid_input("no default position configured")
                        .with_details(json!({"missing_argument": "1-8"}))
                        .with_hint("pass a position: peek register side 3; or set one: peek config set '{\"position\":3}'")
                })?.get()),
            };
            side(g, out, index).await
        }
        RegisterCommand::Drawing {
            file,
            check,
            preview,
            dump_frame,
        } => {
            let file = match file {
                Some(file) => file,
                None => PathBuf::from(crate::context::store()?.read_config()?.drawing.ok_or_else(|| {
                    Error::invalid_input("no default drawing configured")
                        .with_details(json!({"missing_argument": "FILE.js"}))
                        .with_hint("pass a file: peek register drawing ./logo.js; or set one: peek config set '{\"drawing\":\"./logo.js\"}'")
                })?),
            };
            drawing(g, out, file, check, preview, dump_frame).await
        }
    }
}

/// Apply defaults once, preserving any explicit registration.
pub async fn apply_defaults(
    svc: &mut service::Service,
    auth: &AuthBlock,
    config: &Config,
) -> Result<Vec<Warning>> {
    if config.position.is_none() && config.drawing.is_none() {
        return Ok(Vec::new());
    }
    let (status, _) = svc
        .call(&StatusOp {}, Some(auth), Vec::new(), REQUEST_TIMEOUT)
        .await?;
    // ponytail: setup assumes registration is not changed concurrently;
    // add conditional daemon registration if concurrent setup needs support.
    let drawing = if status.drawing.is_none() {
        config
            .drawing
            .as_deref()
            .map(std::path::Path::new)
            .map(input::read_drawing)
            .transpose()?
    } else {
        None
    };
    let mut warnings = status.warnings;
    if status.slot.is_none()
        && let Some(index) = config.position
    {
        let (result, _) = svc
            .call(
                &RegisterSide { index },
                Some(auth),
                Vec::new(),
                REQUEST_TIMEOUT,
            )
            .await?;
        warnings.extend(result.warnings);
    }
    if let Some((filename, bytes)) = drawing {
        let (result, _) = svc
            .call(
                &RegisterDrawing {
                    filename,
                    check_only: false,
                    preview: false,
                    dump_frame: None,
                },
                Some(auth),
                vec![bytes],
                LONG_REQUEST_TIMEOUT,
            )
            .await?;
        warnings.extend(result.warnings);
    }
    Ok(warnings)
}

async fn side(g: &Globals, out: Out, index: u64) -> Result<()> {
    require_mac("register side")?;
    let index = SlotIndex::new(index)?;
    let (_session, auth) = mac_session(g).await?;
    let mut svc = service::ensure_service().await?;
    let (result, _) = svc
        .call(
            &RegisterSide { index },
            Some(&auth),
            Vec::new(),
            REQUEST_TIMEOUT,
        )
        .await?;
    out.warnings(&result.warnings);
    let mut value = serde_json::to_value(&result).unwrap_or(Value::Null);
    if value.get("hotkey").is_none_or(Value::is_null) {
        value["hotkey"] = json!(result.slot.index.default_hotkey());
    }
    if value.get("warnings").is_none() {
        value["warnings"] = json!([]);
    }
    out.value(&value, |v| {
        let i = v["slot"]["index"].as_u64().unwrap_or_default();
        let mut s = format!(
            "position {i} ({}) is yours; the Carbon reaches it with {}",
            v["slot"]["side"].as_str().unwrap_or_default(),
            v["hotkey"].as_str().unwrap_or_default()
        );
        if let Some(from) = v["moved_from"].as_u64() {
            let _ = write!(
                s,
                "\nmoved from position {from} ({}); the drawing keeps running",
                side_name(from)
            );
        }
        s
    });
    next(
        out,
        &[
            "peek register drawing ./logo.js",
            "peek send --speak \"Hello\"",
        ],
    );
    Ok(())
}

fn kib(bytes: u64) -> String {
    // One decimal, as in visual.md A9 ("3.1 KB").
    let tenths = (bytes * 10 + 512) / 1024;
    format!("{}.{} KB", tenths / 10, tenths % 10)
}

fn human_drawing(v: &Value, silicon: &str) -> String {
    let stats = &v["stats"];
    let frames = stats["frames"].as_u64().unwrap_or_default();
    let ms = |k: &str| {
        stats[k]
            .as_f64()
            .map_or_else(|| "?".to_owned(), |x| format!("{x:.2}ms"))
    };
    let mut out = vec![
        format!(
            "✓ loaded ({})",
            kib(v["bytes"].as_u64().unwrap_or_default())
        ),
        format!(
            "✓ {frames}/{frames} frames ok   p50 {}  p95 {}  max {}",
            ms("p50_ms"),
            ms("p95_ms"),
            ms("max_ms")
        ),
        format!("✓ ops/frame  max {}", stats["ops_max"]),
    ];
    let rebuilds = stats["glass_rebuilds"].as_u64().unwrap_or_default();
    // Excessive (⚠, never ✓) when the glass is rebuilt in more than 10% of
    // the frames (a single rebuild never is), or the validator flagged the
    // glass. No frames, no glass line.
    let glass_flagged = v["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|w| w["code"].as_str().is_some_and(|c| c.starts_with("glass")));
    if frames > 0 {
        let excessive = (rebuilds > 1 && rebuilds.saturating_mul(10) > frames) || glass_flagged;
        out.push(format!(
            "{} glass rebuilt {rebuilds} time{} in {frames} frames",
            if excessive { "⚠" } else { "✓" },
            if rebuilds == 1 { "" } else { "s" }
        ));
    }
    for w in v["warnings"].as_array().into_iter().flatten() {
        out.push(format!("⚠ {}", w["message"].as_str().unwrap_or_default()));
    }
    for l in v["logs"].as_array().into_iter().flatten() {
        let line = l.as_str().map_or_else(|| l.to_string(), str::to_owned);
        out.push(format!("  log: {line}"));
    }
    if v["active"] == true {
        match v["slot"].as_u64() {
            Some(slot) => out.push(format!(
                "drawing active for silicon \"{silicon}\" at slot {slot} ({})",
                side_name(slot)
            )),
            None => out.push(format!(
                "drawing active for silicon \"{silicon}\" (no position yet: peek register side <1-8>)"
            )),
        }
    } else {
        out.push("drawing validated (--check); the active drawing is unchanged".to_owned());
    }
    if let Some(p) = v["preview"].as_str() {
        out.push(format!("preview written to {p}"));
    }
    if let Some(d) = v.get("dump") {
        out.push(serde_json::to_string_pretty(d).unwrap_or_default());
    }
    out.join("\n")
}

async fn drawing(
    g: &Globals,
    out: Out,
    file: PathBuf,
    check: bool,
    preview: Option<PathBuf>,
    dump_frame: Option<u32>,
) -> Result<()> {
    require_mac("register drawing")?;
    let (filename, bytes) = input::read_drawing(&file)?;
    let preview_path = preview.as_deref().map(input::resolve).transpose()?;
    if let Some(p) = &preview_path {
        let parent = p.parent().filter(|d| !d.as_os_str().is_empty());
        if parent.is_some_and(|d| !d.is_dir()) {
            return Err(Error::invalid_input(format!(
                "--preview {}: the directory {} does not exist",
                p.display(),
                parent.map(|d| d.display().to_string()).unwrap_or_default()
            )));
        }
    }
    let (session, auth) = mac_session(g).await?;
    let silicon = session
        .store
        .read_session()
        .ok()
        .and_then(|f| {
            f.slot(&session.slot_key)
                .map(|s| s.actor_id().handle().to_owned())
        })
        .unwrap_or_default();
    let mut svc = service::ensure_service().await?;
    let op = RegisterDrawing {
        filename,
        check_only: check,
        preview: preview_path.is_some(),
        dump_frame,
    };
    let (result, blobs) = svc
        .call(&op, Some(&auth), vec![bytes], LONG_REQUEST_TIMEOUT)
        .await?;
    let mut value = serde_json::to_value(&result).unwrap_or(Value::Null);
    if let Some(p) = &preview_path {
        match blobs.first() {
            Some(png) => {
                write_preview(p, png)?;
                value["preview"] = json!(p.display().to_string());
            }
            None => out.hint(format!(
                "peekd returned no preview image, so {} was not written",
                p.display()
            )),
        }
    }
    if out.json {
        out.value(&value, kv);
    } else {
        // The A9 block carries the warnings; nothing extra on stderr.
        out.value(&value, |v| human_drawing(v, &silicon));
    }
    if result.active {
        next(out, &["peek send --speak \"Hello\"", "peek status"]);
    }
    Ok(())
}

fn write_preview(path: &std::path::Path, png: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("png.{}.tmp", std::process::id()));
    std::fs::write(&tmp, png)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            Error::invalid_input(format!(
                "writing the preview to {} failed: {e}",
                path.display()
            ))
        })
}

pub async fn unregister(g: &Globals, out: Out) -> Result<()> {
    require_mac("unregister")?;
    let (_session, auth) = mac_session(g).await?;
    let mut svc = service::ensure_service().await?;
    let (result, _) = svc
        .call(&Unregister {}, Some(&auth), Vec::new(), REQUEST_TIMEOUT)
        .await?;
    out.result(&result, human_unregister);
    next(out, &["peek register side <1-8>"]);
    Ok(())
}

/// The `peek unregister` line (contract §5.6).
fn human_unregister(v: &Value) -> String {
    let released = v["released_slot"].as_u64().map_or_else(
        || "held no position".to_owned(),
        |s| format!("released position {s} ({})", side_name(s)),
    );
    let count = |k: &str| v[k].as_array().map_or(0, Vec::len);
    format!(
        "{released}; drawing deleted; {} pending ask(s), {} queued send(s) and {} scheduled send(s) cancelled",
        count("cancelled_asks"),
        count("cancelled_sends"),
        count("cancelled_scheduled")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unregister_line_counts_everything_cancelled() {
        let v = json!({"released_slot": 3, "cancelled_asks": ["ask_1"], "cancelled_sends": ["snd_1", "snd_2"],
            "cancelled_scheduled": ["sch_1", "sch_2", "sch_3"]});
        assert_eq!(
            human_unregister(&v),
            "released position 3 (right); drawing deleted; 1 pending ask(s), 2 queued send(s) and 3 scheduled send(s) cancelled"
        );
        // An older peekd leaves the new lists out.
        let old = json!({"released_slot": null, "cancelled_asks": []});
        assert_eq!(
            human_unregister(&old),
            "held no position; drawing deleted; 0 pending ask(s), 0 queued send(s) and 0 scheduled send(s) cancelled"
        );
    }

    fn a9(rebuilds: u64, frames: u64, warnings: &Value) -> String {
        let v = json!({"sha256":"x","bytes":3174,"stats":{"frames":frames,"p50_ms":0.31,"p95_ms":0.58,"max_ms":0.92,"ops_max":212,"glass_rebuilds":rebuilds},
            "warnings":warnings,"logs":[],"active":true,"slot":5,"server_sync":"pending"});
        human_drawing(&v, "dj")
    }

    #[test]
    fn excessive_glass_rebuilds_are_a_warning_never_a_check() {
        let glass = |text: &str| {
            text.lines()
                .find(|l| l.contains("glass rebuilt"))
                .map(str::to_owned)
                .unwrap_or_default()
        };
        assert_eq!(
            glass(&a9(1, 90, &json!([]))),
            "✓ glass rebuilt 1 time in 90 frames"
        );
        assert_eq!(
            glass(&a9(9, 90, &json!([]))),
            "✓ glass rebuilt 9 times in 90 frames"
        );
        assert_eq!(
            glass(&a9(10, 90, &json!([]))),
            "⚠ glass rebuilt 10 times in 90 frames"
        );
        assert_eq!(
            glass(&a9(0, 90, &json!([]))),
            "✓ glass rebuilt 0 times in 90 frames"
        );
        assert_eq!(
            glass(&a9(1, 5, &json!([]))),
            "✓ glass rebuilt 1 time in 5 frames",
            "one rebuild is fine"
        );
        let flagged =
            json!([{"code": "glass_outline_churn", "message": "the glass outline changes often"}]);
        assert_eq!(
            glass(&a9(1, 90, &flagged)),
            "⚠ glass rebuilt 1 time in 90 frames"
        );
        assert!(
            !a9(0, 0, &json!([])).contains("glass rebuilt"),
            "no frames, no glass line"
        );
    }

    #[test]
    fn a9_success_block() {
        let v = json!({"sha256":"x","bytes":3174,"stats":{"frames":90,"p50_ms":0.31,"p95_ms":0.58,"max_ms":0.92,"ops_max":212,"glass_rebuilds":1},
            "warnings":[],"logs":[],"active":true,"slot":5,"server_sync":"pending"});
        let text = human_drawing(&v, "dj");
        assert_eq!(
            text,
            "✓ loaded (3.1 KB)\n✓ 90/90 frames ok   p50 0.31ms  p95 0.58ms  max 0.92ms\n✓ ops/frame  max 212\n✓ glass rebuilt 1 time in 90 frames\ndrawing active for silicon \"dj\" at slot 5 (bottom)"
        );
    }

    #[test]
    fn sides_are_kebab_case() {
        assert_eq!(side_name(2), "top-right");
        assert_eq!(side_name(8), "top-left");
        assert_eq!(side_name(9), "");
    }
}
