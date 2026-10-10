//! `settings.json`: Peek.app and peekd settings (BLUEPRINT §1.7), mirrored
//! from the UI's `settings.changed`.
//!
//! Peek.app writes the file itself (atomically) and then sends
//! `settings.changed`; peekd applies the change on top of the file as it is
//! on disk now (re-read, never its own stale copy) and keeps every key it
//! does not know, so neither side ever drops the other's writes. Reading is
//! lenient per key: a bad value falls back to its default with a warning.

use std::{collections::BTreeMap, path::PathBuf, sync::RwLock};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use silicon_peek_client::{
    Error, Result,
    runtime::fs::{read_private, write_atomic},
    schema::send::{check_voice, normalize_language},
};

/// The settings file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `normal` or `compact`.
    pub mode: String,
    /// Hotkey modifier, e.g. `ctrl+cmd` (the default), `cmd`, `opt+cmd`.
    pub hotkey_modifier: String,
    /// `main` or `pointer`.
    pub display: String,
    /// `wallpaper` or `screen`.
    pub backdrop: String,
    /// Share usage and diagnostics.
    pub telemetry: bool,
    /// Per-language TTS voice overrides (`{"en":"DtsPFCrhbCbbJkwZsb3d"}`).
    pub voice_defaults: BTreeMap<String, String>,
    /// `auto` or a BCP 47 language for speech-to-text.
    pub stt_language: String,
    /// Keys written by a newer build.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: "normal".to_owned(),
            hotkey_modifier: silicon_peek_client::identity::DEFAULT_HOTKEY_MODIFIER.to_owned(),
            display: "main".to_owned(),
            backdrop: "wallpaper".to_owned(),
            telemetry: true,
            voice_defaults: BTreeMap::new(),
            stt_language: "auto".to_owned(),
            extra: BTreeMap::new(),
        }
    }
}

/// Keys `settings.changed` accepts.
pub const SETTING_KEYS: [&str; 7] = [
    "mode",
    "hotkey_modifier",
    "display",
    "backdrop",
    "telemetry",
    "voice_defaults",
    "stt_language",
];

fn one_of(key: &str, value: &Value, allowed: &[&str]) -> Result<String> {
    match value.as_str() {
        Some(s) if allowed.contains(&s) => Ok(s.to_owned()),
        _ => Err(Error::invalid_input(format!(
            "setting `{key}` must be one of {}, got {value}",
            allowed.join(", ")
        ))),
    }
}

fn boolean(key: &str, value: &Value) -> Result<bool> {
    value.as_bool().ok_or_else(|| {
        Error::invalid_input(format!(
            "setting `{key}` must be true or false, got {value}"
        ))
    })
}

/// Validates a hotkey modifier: `+`-joined, unique parts of cmd, ctrl, opt,
/// shift, including cmd or ctrl.
///
/// # Errors
/// `invalid_input`.
pub fn check_modifier(value: &str) -> Result<()> {
    let parts: Vec<&str> = value.split('+').collect();
    let known = ["cmd", "ctrl", "opt", "shift"];
    let mut seen = Vec::new();
    for p in &parts {
        if !known.contains(p) || seen.contains(p) {
            return Err(Error::invalid_input(format!(
                "hotkey modifier `{value}` is invalid; join distinct parts of cmd, ctrl, opt and shift with `+`"
            )));
        }
        seen.push(*p);
    }
    if !seen.iter().any(|p| *p == "cmd" || *p == "ctrl") {
        return Err(Error::invalid_input(format!(
            "hotkey modifier `{value}` needs cmd or ctrl, or it would steal plain keystrokes"
        )));
    }
    Ok(())
}

fn check_stt_language(value: &str) -> Result<()> {
    if value == "auto" {
        return Ok(());
    }
    let mut parts = value.split('-');
    let primary_ok = parts
        .next()
        .is_some_and(|p| (2..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_lowercase()));
    let rest_ok =
        parts.all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()));
    if primary_ok && rest_ok {
        Ok(())
    } else {
        Err(Error::invalid_input(format!(
            "stt_language `{value}` must be `auto` or a BCP 47 tag such as `en` or `en-US`"
        )))
    }
}

impl Settings {
    /// Reads a settings document leniently: each known key that fails
    /// validation keeps its default (and is reported), unknown keys are kept
    /// verbatim in [`Settings::extra`].
    #[must_use]
    pub fn from_value_lenient(value: &Value) -> (Self, Vec<String>) {
        let mut out = Self::default();
        let mut warnings = Vec::new();
        let Some(object) = value.as_object() else {
            return (out, vec!["settings.json is not a JSON object".to_owned()]);
        };
        for (key, v) in object {
            match key.as_str() {
                k if SETTING_KEYS.contains(&k) => {
                    if v.is_null() {
                        continue;
                    }
                    if let Err(e) = out.apply(k, v) {
                        warnings.push(e.message().to_owned());
                    }
                }
                _ => {
                    out.extra.insert(key.clone(), v.clone());
                }
            }
        }
        (out, warnings)
    }

    /// The settings document: known keys plus every preserved extra key.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let object = match serde_json::to_value(self) {
            Ok(Value::Object(o)) => o,
            _ => serde_json::Map::new(),
        };
        Value::Object(object)
    }

    /// Applies one `settings.changed`.
    ///
    /// # Errors
    /// `invalid_input` naming the key and the allowed values; nothing changes.
    pub fn apply(&mut self, key: &str, value: &Value) -> Result<()> {
        match key {
            "mode" => self.mode = one_of(key, value, &["normal", "compact"])?,
            "display" => self.display = one_of(key, value, &["main", "pointer"])?,
            "backdrop" => self.backdrop = one_of(key, value, &["wallpaper", "screen"])?,
            "telemetry" => self.telemetry = boolean(key, value)?,
            "hotkey_modifier" => {
                let s = value.as_str().ok_or_else(|| {
                    Error::invalid_input(
                        "setting `hotkey_modifier` must be a string such as \"cmd\"",
                    )
                })?;
                check_modifier(s)?;
                s.clone_into(&mut self.hotkey_modifier);
            }
            "stt_language" => {
                let s = value.as_str().ok_or_else(|| {
                    Error::invalid_input("setting `stt_language` must be a string")
                })?;
                check_stt_language(s)?;
                s.clone_into(&mut self.stt_language);
            }
            "voice_defaults" => {
                let obj = value.as_object().ok_or_else(|| {
                    Error::invalid_input(
                        r#"setting `voice_defaults` must be an object like {"en":"DtsPFCrhbCbbJkwZsb3d"}"#,
                    )
                })?;
                let mut next = BTreeMap::new();
                for (lang, voice) in obj {
                    let voice = voice.as_str().ok_or_else(|| {
                        Error::invalid_input(format!(
                            "voice_defaults.{lang} must be a voice name string"
                        ))
                    })?;
                    let language = normalize_language(lang)?;
                    check_voice(voice)?;
                    next.insert(language, voice.to_owned());
                }
                self.voice_defaults = next;
            }
            other => {
                return Err(Error::invalid_input(format!(
                    "`{other}` is not a Peek setting; valid keys: {}",
                    SETTING_KEYS.join(", ")
                ))
                .with_details(json!({"valid_keys": SETTING_KEYS})));
            }
        }
        Ok(())
    }
}

/// The settings file with an in-memory copy.
#[derive(Debug)]
pub struct SettingsStore {
    path: PathBuf,
    current: RwLock<Settings>,
}

impl SettingsStore {
    /// Loads `path` (defaults when absent or unreadable; bad values are
    /// logged and fall back to their defaults).
    #[must_use]
    pub fn load(path: PathBuf) -> Self {
        let current = Self::read(&path);
        Self {
            path,
            current: RwLock::new(current),
        }
    }

    fn read(path: &std::path::Path) -> Settings {
        match read_private(path) {
            Ok(Some(bytes)) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(v) => {
                    let (settings, warnings) = Settings::from_value_lenient(&v);
                    for w in warnings {
                        tracing::warn!(path = %path.display(), problem = %w, "settings.json: a value was ignored");
                    }
                    settings
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "settings.json is invalid; using defaults");
                    Settings::default()
                }
            },
            Ok(None) => Settings::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e.message(), "settings.json is unreadable; using defaults");
                Settings::default()
            }
        }
    }

    /// A copy of the current settings.
    #[must_use]
    pub fn get(&self) -> Settings {
        self.current
            .read()
            .map_or_else(|p| p.into_inner().clone(), |g| g.clone())
    }

    /// Applies one change on top of the file as it is on disk now (Peek.app
    /// may have written other keys since peekd last looked) and persists it
    /// atomically, keeping every key peekd does not know.
    ///
    /// # Errors
    /// `invalid_input` for a bad value; I/O failures.
    pub fn apply(&self, key: &str, value: &Value) -> Result<Settings> {
        let mut next = if self.path.exists() {
            Self::read(&self.path)
        } else {
            self.get()
        };
        next.apply(key, value)?;
        let mut bytes = serde_json::to_vec_pretty(&next.to_value())
            .map_err(|e| Error::internal(format!("serializing settings failed: {e}")))?;
        bytes.push(b'\n');
        let dir = self
            .path
            .parent()
            .ok_or_else(|| Error::internal("settings.json has no parent directory"))?;
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| Error::internal("settings.json has no file name"))?;
        write_atomic(dir, &name, &bytes)?;
        match self.current.write() {
            Ok(mut g) => *g = next.clone(),
            Err(p) => *p.into_inner() = next.clone(),
        }
        Ok(next)
    }
}
