//! Reading inputs: secrets from files or stdin, JSON arguments (`JSON`,
//! `@FILE`, `-`), image and drawing bytes. Paths are resolved against the
//! current directory (BLUEPRINT §7.4). peek never prompts: a `-` input whose
//! stdin is a terminal is refused instead of silently waiting for typing.

use std::{
    io::{IsTerminal as _, Read as _},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use silicon_peek_client::{
    Error, ErrorCode, Result, Secret, json,
    schema::{check_drawing_bytes, check_image_bytes, limits},
};

/// Secrets are one short line; refuse anything larger than this.
const SECRET_MAX_BYTES: u64 = 64 * 1024;
/// `--show` / `--ask` / `config set` JSON is small; refuse anything larger.
const JSON_MAX_BYTES: u64 = 1024 * 1024;

/// Resolves `path` against the current directory.
pub fn resolve(path: &Path) -> Result<PathBuf> {
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let cwd = std::env::current_dir().map_err(|e| {
            Error::invalid_input(format!(
                "the current directory is unusable ({e}), so `{}` cannot be resolved",
                path.display()
            ))
            .with_hint("cd into an existing directory, or pass an absolute path")
        })?;
        cwd.join(path)
    };
    // `components()` drops every `.` (`/a/./b` → `/a/b`); `..` stays, since
    // only the filesystem knows where it leads through a symlink.
    Ok(full.components().collect())
}

fn stdin_bytes(flag: &str, limit: u64) -> Result<Vec<u8>> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(Error::invalid_input(format!(
            "{flag} - reads stdin, but stdin is a terminal and peek never prompts"
        ))
        .with_hint(format!(
            "pipe the value in, for example: printf %s \"$VALUE\" | peek … {flag} -"
        )));
    }
    let mut buf = Vec::new();
    stdin
        .lock()
        .take(limit + 1)
        .read_to_end(&mut buf)
        .map_err(|e| Error::invalid_input(format!("reading {flag} from stdin failed: {e}")))?;
    if buf.len() as u64 > limit {
        return Err(Error::invalid_input(format!(
            "{flag} on stdin is larger than {limit} bytes"
        )));
    }
    Ok(buf)
}

fn file_bytes(flag: &str, path: &Path, limit: u64) -> Result<Vec<u8>> {
    let full = resolve(path)?;
    let file = std::fs::File::open(&full).map_err(|e| {
        Error::invalid_input(format!("{flag}: cannot read {}: {e}", full.display()))
            .with_hint("check the path; relative paths are resolved against the current directory")
    })?;
    let mut buf = Vec::new();
    file.take(limit + 1).read_to_end(&mut buf).map_err(|e| {
        Error::invalid_input(format!("{flag}: reading {} failed: {e}", full.display()))
    })?;
    if buf.len() as u64 > limit {
        return Err(Error::invalid_input(format!(
            "{flag}: {} is larger than {limit} bytes",
            full.display()
        )));
    }
    Ok(buf)
}

/// Reads one secret line from `source` (a path, or `-` for stdin). The
/// trailing newline and surrounding whitespace are removed.
pub fn read_secret(flag: &str, source: &str) -> Result<Secret> {
    let bytes = if source == "-" {
        stdin_bytes(flag, SECRET_MAX_BYTES)?
    } else {
        file_bytes(flag, Path::new(source), SECRET_MAX_BYTES)?
    };
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::invalid_input(format!("{flag}: the value is not UTF-8 text")))?;
    let line = text.lines().next().unwrap_or_default().trim();
    if line.is_empty() {
        return Err(Error::invalid_input(format!(
            "{flag}: the first line is empty; expected the value on one line"
        )));
    }
    Ok(Secret::new(line))
}

/// Parses a `JSON`, `@FILE` or `-` argument strictly (duplicate keys refused).
pub fn read_json(flag: &str, raw: &str) -> Result<Value> {
    let bytes = if raw == "-" {
        stdin_bytes(flag, JSON_MAX_BYTES)?
    } else if let Some(path) = raw.strip_prefix('@') {
        if path.is_empty() {
            return Err(Error::invalid_input(format!(
                "{flag} @ needs a file name, for example {flag} @question.json"
            )));
        }
        file_bytes(flag, Path::new(path), JSON_MAX_BYTES)?
    } else {
        raw.as_bytes().to_vec()
    };
    json::parse_value(&bytes).map_err(|e| {
        Error::new(
            ErrorCode::InvalidJson,
            format!("{flag} is not valid JSON: {}", e.message().trim_start_matches("invalid JSON: ")),
        )
        .with_hint(
            "pass one JSON object with unique keys, as a literal, @FILE or - (stdin); see peek send --help",
        )
    })
}

/// Reads an image referenced by `--show`/`--ask` and checks its size and format.
pub fn read_image(path: &str) -> Result<Vec<u8>> {
    let full = resolve(Path::new(path))?;
    let unreadable = |why: String| {
        Error::new(
            ErrorCode::ImageUnreadable,
            format!("image `{path}` ({}) cannot be read: {why}", full.display()),
        )
        .with_hint("image paths are resolved against the current directory; check the path and permissions")
        .with_details(json!({"path": path, "resolved": full.display().to_string()}))
    };
    let meta = std::fs::metadata(&full).map_err(|e| unreadable(e.to_string()))?;
    if !meta.is_file() {
        return Err(unreadable("it is not a regular file".to_owned()));
    }
    let limit = limits::IMAGE_MAX_BYTES as u64;
    if meta.len() > limit {
        return Err(Error::new(
            ErrorCode::ImageTooLarge,
            format!(
                "image `{path}` is {} bytes; the limit is {limit} bytes (10 MiB)",
                meta.len()
            ),
        )
        .with_hint("downscale it; Peek shows images at most 512 px")
        .with_details(json!({"path": path, "limit": limit, "actual": meta.len()})));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    std::fs::File::open(&full)
        .and_then(|f| f.take(limit + 1).read_to_end(&mut bytes))
        .map_err(|e| unreadable(e.to_string()))?;
    check_image_bytes(path, &bytes)?;
    Ok(bytes)
}

/// Reads a drawing file and checks it (≤ 256 KiB, UTF-8, not empty).
pub fn read_drawing(path: &Path) -> Result<(String, Vec<u8>)> {
    let full = resolve(path)?;
    let shown = path.display().to_string();
    let meta = std::fs::metadata(&full).map_err(|e| {
        Error::invalid_input(format!(
            "drawing `{shown}` ({}) cannot be read: {e}",
            full.display()
        ))
        .with_hint("the path is resolved against the current directory")
    })?;
    if !meta.is_file() {
        return Err(Error::invalid_input(format!(
            "drawing `{shown}` ({}) is not a regular file",
            full.display()
        )));
    }
    let limit = limits::DRAWING_MAX_BYTES as u64;
    if meta.len() > limit {
        return Err(Error::new(
            ErrorCode::DrawingTooLarge,
            format!(
                "drawing `{shown}` is {} bytes; the limit is {limit} bytes (256 KiB)",
                meta.len()
            ),
        )
        .with_hint("minify the script or move large data out of it")
        .with_details(json!({"limit": limit, "actual": meta.len()})));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&full)
        .and_then(|f| f.take(limit + 1).read_to_end(&mut bytes))
        .map_err(|e| Error::invalid_input(format!("reading drawing `{shown}` failed: {e}")))?;
    // At most limit + 1 bytes were read, so a file that grew meanwhile is still
    // refused as drawing_too_large without buffering all of it.
    check_drawing_bytes(&shown, &bytes)?;
    let name = full
        .file_name()
        .map_or_else(|| shown.clone(), |n| n.to_string_lossy().into_owned());
    Ok((name, bytes))
}
