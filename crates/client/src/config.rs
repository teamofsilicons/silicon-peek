//! `$SILICON_HOME/.peek/config.json` and the strict merge behind
//! `peek config set '<json-object>'` (BLUEPRINT §7.3).
//!
//! ```json
//! {"schema":1,"telemetry":true,"voice":null,"language":null,"notify":[],
//!  "api_url":null,"delivery_max_age_hours":168}
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    error::{Error, ErrorCode, Result},
    identity::ApiUrl,
    json::{kind, parse_object},
    schema::send::{Notify, check_voice, normalize_language},
};

/// The current config schema.
pub const CONFIG_SCHEMA: u32 = 1;

/// Default `delivery_max_age_hours` (7 days).
pub const DEFAULT_DELIVERY_MAX_AGE_HOURS: u32 = 168;

/// Every key `config set` accepts, in documentation order.
pub const CONFIG_KEYS: [&str; 6] = [
    "telemetry",
    "voice",
    "language",
    "notify",
    "api_url",
    "delivery_max_age_hours",
];

/// This home's configuration. Missing keys take their defaults, so a file
/// written by an older peek still loads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Always 1 for this build.
    pub schema: u32,
    /// This home's telemetry (default on).
    pub telemetry: bool,
    /// Default TTS voice; `null` picks the per-language default.
    pub voice: Option<String>,
    /// TTS language when detection is ambiguous; `null` detects.
    pub language: Option<String>,
    /// Default `--notify`.
    pub notify: Vec<Notify>,
    /// Same as `--api`; `null` uses the production origin.
    pub api_url: Option<ApiUrl>,
    /// Outbox expiry for this Silicon's tings, 1–168 hours.
    pub delivery_max_age_hours: u32,
    /// Keys written by a newer peek, preserved on rewrite.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: CONFIG_SCHEMA,
            telemetry: true,
            voice: None,
            language: None,
            notify: Vec::new(),
            api_url: None,
            delivery_max_age_hours: DEFAULT_DELIVERY_MAX_AGE_HOURS,
            extra: BTreeMap::new(),
        }
    }
}

fn unknown_key(k: &str) -> Error {
    Error::new(
        ErrorCode::UnknownConfigKey,
        format!("`{k}` is not a peek config key"),
    )
    .with_hint(format!("valid keys: {}", CONFIG_KEYS.join(", ")))
    .with_details(json!({"key": k, "valid_keys": CONFIG_KEYS}))
}

impl Config {
    /// Parses `config set` input (a JSON object string) strictly.
    ///
    /// # Errors
    /// `invalid_json` for malformed JSON or duplicate keys, `invalid_input`
    /// for a non-object, `unknown_config_key` with `details.valid_keys`.
    pub fn parse_patch(input: &str) -> Result<Map<String, Value>> {
        let patch = parse_object(input.as_bytes(), "config").map_err(|e| {
            let hint = format!(
                r#"pass one JSON object, for example peek config set '{{"telemetry":false}}'; valid keys: {}"#,
                CONFIG_KEYS.join(", ")
            );
            let details = json!({"valid_keys": CONFIG_KEYS});
            Error::new(e.code().clone(), e.message().to_owned())
                .with_hint(hint)
                .with_details(details)
        })?;
        for k in patch.keys() {
            if !CONFIG_KEYS.contains(&k.as_str()) {
                return Err(unknown_key(k));
            }
        }
        Ok(patch)
    }

    /// Validates every value of a patch and applies it; `null` resets a key
    /// to its default. The config is unchanged when any value is invalid.
    ///
    /// # Errors
    /// `unknown_config_key` or `invalid_input` naming the key.
    pub fn merge(&mut self, patch: &Map<String, Value>) -> Result<()> {
        let mut next = self.clone();
        for (k, v) in patch {
            next.set(k, v)?;
        }
        *self = next;
        Ok(())
    }

    /// Sets one key (`null` resets it).
    ///
    /// # Errors
    /// `unknown_config_key` or `invalid_input`.
    pub fn set(&mut self, key: &str, value: &Value) -> Result<()> {
        self.set_value(key, value).map_err(|e| {
            e.with_input_field(
                key,
                "see `peek config --help` for every key and its accepted values",
            )
        })
    }

    fn set_value(&mut self, key: &str, value: &Value) -> Result<()> {
        let bad = |expected: &str| {
            Error::invalid_input(format!(
                "config `{key}` must be {expected}, got {}",
                kind(value)
            ))
        };
        let d = Self::default();
        match key {
            "telemetry" => {
                self.telemetry = match value {
                    Value::Null => d.telemetry,
                    Value::Bool(b) => *b,
                    _ => return Err(bad("true or false")),
                };
            }
            "voice" => {
                self.voice = match value {
                    Value::Null => None,
                    Value::String(s) => {
                        check_voice(s)?;
                        Some(s.clone())
                    }
                    _ => return Err(bad("an Aura-2 voice name or null")),
                };
            }
            "language" => {
                self.language = match value {
                    Value::Null => None,
                    Value::String(s) => Some(normalize_language(s)?),
                    _ => return Err(bad("a BCP 47 primary subtag (e.g. \"en\") or null")),
                };
            }
            "notify" => {
                self.notify = match value {
                    Value::Null => Vec::new(),
                    Value::Array(items) => {
                        let mut out = Vec::with_capacity(items.len());
                        for item in items {
                            let s = item.as_str().ok_or_else(|| {
                                bad(r#"an array of "speech_finished", "show_dismissed" and/or "shown""#)
                            })?;
                            out.push(Notify::parse(s)?);
                        }
                        out.sort_unstable();
                        out.dedup();
                        out
                    }
                    _ => {
                        return Err(bad(
                            r#"an array of "speech_finished", "show_dismissed" and/or "shown""#,
                        ));
                    }
                };
            }
            "api_url" => {
                self.api_url = match value {
                    Value::Null => None,
                    Value::String(s) => Some(ApiUrl::parse(s)?),
                    _ => return Err(bad("an https origin or null")),
                };
            }
            "delivery_max_age_hours" => {
                self.delivery_max_age_hours = match value {
                    Value::Null => d.delivery_max_age_hours,
                    Value::Number(n) => n
                        .as_u64()
                        .and_then(|h| u32::try_from(h).ok())
                        .filter(|h| (1..=168).contains(h))
                        .ok_or_else(|| {
                            Error::invalid_input(format!(
                                "config `delivery_max_age_hours` is {n}; it must be an integer from 1 to 168 (Ting keys live 14 days)"
                            ))
                        })?,
                    _ => return Err(bad("an integer from 1 to 168")),
                };
            }
            other => return Err(unknown_key(other)),
        }
        Ok(())
    }

    /// Resets one key to its default.
    ///
    /// # Errors
    /// `unknown_config_key`.
    pub fn unset(&mut self, key: &str) -> Result<()> {
        self.set(key, &Value::Null)
    }

    /// One key's current value.
    ///
    /// # Errors
    /// `unknown_config_key`.
    pub fn get(&self, key: &str) -> Result<Value> {
        let v = self.to_public_value();
        if CONFIG_KEYS.contains(&key) {
            Ok(v.get(key).cloned().unwrap_or(Value::Null))
        } else {
            Err(unknown_key(key))
        }
    }

    /// The printable config (every key, no unknown extras). It holds no
    /// secrets.
    #[must_use]
    pub fn to_public_value(&self) -> Value {
        json!({
            "schema": self.schema,
            "telemetry": self.telemetry,
            "voice": self.voice,
            "language": self.language,
            "notify": self.notify,
            "api_url": self.api_url,
            "delivery_max_age_hours": self.delivery_max_age_hours,
        })
    }

    /// The subset peekd mirrors (`config.sync`).
    #[must_use]
    pub fn sync_payload(&self) -> crate::ipc::cli::ConfigSyncConfig {
        crate::ipc::cli::ConfigSyncConfig {
            voice: self.voice.clone(),
            language: self.language.clone(),
            notify: self.notify.clone(),
            telemetry: self.telemetry,
            env_opt_out: false,
        }
    }

    /// Checks a config read from disk (a hand-edited file may be invalid).
    ///
    /// # Errors
    /// `invalid_input` naming the bad key.
    pub fn validate(&self) -> Result<()> {
        let mut probe = Self::default();
        let v = self.to_public_value();
        for k in CONFIG_KEYS {
            probe.set(k, &v[k])?;
        }
        Ok(())
    }

    /// Loads a stored config object leniently: every valid key is applied,
    /// unknown keys are preserved in `extra`, and invalid keys are returned
    /// (left at their defaults) so `config set` can repair them.
    #[must_use]
    pub fn from_stored(stored: &Map<String, Value>) -> (Self, Vec<String>) {
        let mut c = Self::default();
        let mut invalid = Vec::new();
        for (k, v) in stored {
            if k == "schema" {
                continue;
            }
            if CONFIG_KEYS.contains(&k.as_str()) {
                if c.set(k, v).is_err() {
                    invalid.push(k.clone());
                }
            } else {
                c.extra.insert(k.clone(), v.clone());
            }
        }
        (c, invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_the_blueprint() {
        assert_eq!(
            Config::default().to_public_value(),
            json!({"schema":1,"telemetry":true,"voice":null,"language":null,"notify":[],"api_url":null,"delivery_max_age_hours":168})
        );
    }

    #[test]
    fn strict_parsing() {
        let code = |s: &str| Config::parse_patch(s).err().map(|e| e.code().clone());
        assert_eq!(code("[]"), Some(ErrorCode::InvalidInput));
        assert_eq!(code("nope"), Some(ErrorCode::InvalidJson));
        assert_eq!(
            code(r#"{"telemetry":true,"telemetry":false}"#),
            Some(ErrorCode::InvalidJson)
        );
        let e = Config::parse_patch(r#"{"telemetry":false,"colour":"red"}"#).err();
        assert!(e.is_some_and(|e| {
            *e.code() == ErrorCode::UnknownConfigKey
                && e.exit_code().code() == 2
                && e.details()
                    .is_some_and(|d| d["valid_keys"].as_array().is_some_and(|a| a.len() == 6))
        }));
        assert!(Config::parse_patch("{}").is_ok());
    }

    #[test]
    fn merge_validates_every_key_and_null_resets() -> Result<()> {
        let mut c = Config::default();
        let patch = Config::parse_patch(
            r#"{"telemetry":false,"voice":"aura-2-thalia-en","language":"EN","notify":["show_dismissed","speech_finished"],"api_url":"http://127.0.0.1:9/","delivery_max_age_hours":24}"#,
        )?;
        c.merge(&patch)?;
        assert!(!c.telemetry);
        assert_eq!(c.language.as_deref(), Some("en"));
        assert_eq!(
            c.notify,
            vec![Notify::SpeechFinished, Notify::ShowDismissed]
        );
        assert_eq!(
            c.api_url.as_ref().map(ApiUrl::as_str),
            Some("http://127.0.0.1:9")
        );
        assert_eq!(c.delivery_max_age_hours, 24);
        c.merge(&Config::parse_patch(
            r#"{"telemetry":null,"voice":null,"notify":null,"delivery_max_age_hours":null}"#,
        )?)?;
        assert!(c.telemetry);
        assert!(c.voice.is_none());
        assert!(c.notify.is_empty());
        assert_eq!(c.delivery_max_age_hours, 168);
        Ok(())
    }

    #[test]
    fn invalid_values_leave_the_config_untouched() -> Result<()> {
        let mut c = Config::default();
        for bad in [
            r#"{"telemetry":"no"}"#,
            r#"{"voice":"thalia"}"#,
            r#"{"language":"english"}"#,
            r#"{"notify":["everything"]}"#,
            r#"{"notify":"speech_finished"}"#,
            r#"{"api_url":"http://example.com"}"#,
            r#"{"delivery_max_age_hours":0}"#,
            r#"{"delivery_max_age_hours":169}"#,
            r#"{"delivery_max_age_hours":1.5}"#,
            r#"{"telemetry":false,"voice":"bad"}"#,
        ] {
            assert!(c.merge(&Config::parse_patch(bad)?).is_err(), "{bad}");
            assert_eq!(c, Config::default(), "{bad} must not partially apply");
        }
        c.merge(&Config::parse_patch(r#"{"delivery_max_age_hours":1}"#)?)?;
        c.merge(&Config::parse_patch(r#"{"delivery_max_age_hours":168}"#)?)?;
        Ok(())
    }

    #[test]
    fn stored_configs_load_leniently_for_repair() {
        let stored = json!({"schema":1,"telemetry":"maybe","voice":"aura-2-thalia-en","future":1});
        let (c, invalid) = Config::from_stored(stored.as_object().unwrap_or(&Map::new()));
        assert_eq!(invalid, vec!["telemetry".to_owned()]);
        assert!(c.telemetry, "an invalid key falls back to its default");
        assert_eq!(c.voice.as_deref(), Some("aura-2-thalia-en"));
        assert_eq!(c.extra.get("future"), Some(&json!(1)));
    }

    #[test]
    fn get_unset_and_unknown_extras() -> Result<()> {
        let mut c: Config = serde_json::from_value(json!({"schema":1,"telemetry":true,"voice":null,"language":null,"notify":[],"api_url":null,"delivery_max_age_hours":168,"future_key":7}))
            .map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(c.extra.get("future_key"), Some(&json!(7)));
        assert_eq!(c.get("telemetry")?, json!(true));
        assert!(c.get("future_key").is_err());
        c.set("telemetry", &json!(false))?;
        c.unset("telemetry")?;
        assert!(c.telemetry);
        let round = serde_json::to_value(&c).map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(round["future_key"], 7);
        c.validate()?;
        Ok(())
    }
}
