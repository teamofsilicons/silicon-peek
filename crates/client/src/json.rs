//! Strict JSON: duplicate object keys are rejected at every depth.
//!
//! `serde_json` silently keeps the last of two duplicate keys, which lets two
//! parsers disagree about the same bytes. Every JSON input peek accepts from a
//! person, a Silicon or another process (CLI arguments, `config set`, IPC
//! frames, store files) goes through [`parse_value`] first (the
//! `ting_client::strict_json` rule).

use std::fmt;

use serde::{
    Deserialize,
    de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};

use crate::error::{Error, ErrorCode, Result};

struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON with unique object keys")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::from(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::from(v)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Strict, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom("non-finite number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::String(v.to_owned())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::String(v)))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut out = Vec::with_capacity(a.size_hint().unwrap_or(0).min(1024));
                while let Some(Strict(v)) = a.next_element()? {
                    out.push(v);
                }
                Ok(Strict(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut out = Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if out.contains_key(&k) {
                        return Err(de::Error::custom(format_args!(
                            "duplicate object key `{k}`"
                        )));
                    }
                    let Strict(v) = a.next_value()?;
                    out.insert(k, v);
                }
                Ok(Strict(Value::Object(out)))
            }
        }
        d.deserialize_any(V)
    }
}

/// Parses one JSON value, rejecting duplicate keys, invalid UTF-8 and
/// trailing content.
///
/// # Errors
/// `invalid_json`, with the parser's line and column.
pub fn parse_value(bytes: &[u8]) -> Result<Value> {
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let Strict(v) = Strict::deserialize(&mut d).map_err(|e| {
        Error::new(ErrorCode::InvalidJson, format!("invalid JSON: {e}"))
            .with_hint("pass exactly one JSON value, with unique object keys")
    })?;
    d.end().map_err(|e| {
        Error::new(
            ErrorCode::InvalidJson,
            format!("invalid JSON: trailing content after the value ({e})"),
        )
        .with_hint("pass exactly one JSON value")
    })?;
    Ok(v)
}

/// Parses one JSON object strictly.
///
/// # Errors
/// `invalid_json` for malformed input; `invalid_input` when the value is not
/// an object. `what` names the input in the message (for example `--show`).
pub fn parse_object(bytes: &[u8], what: &str) -> Result<Map<String, Value>> {
    match parse_value(bytes) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(other) => Err(Error::invalid_input(format!(
            "{what} must be a JSON object, got {}",
            kind(&other)
        ))),
        Err(e) => Err(
            Error::new(ErrorCode::InvalidJson, format!("{what}: {}", e.message()))
                .with_hint("pass exactly one JSON object, with unique keys"),
        ),
    }
}

/// Strictly parses bytes, then deserializes them into `T` (whose own serde
/// attributes decide whether unknown fields are refused).
///
/// # Errors
/// `invalid_json` for malformed input; `invalid_input` when the value does not
/// match `T`, with serde's explanation.
pub fn from_slice<T: DeserializeOwned>(bytes: &[u8], what: &str) -> Result<T> {
    let value = parse_value(bytes).map_err(|e| {
        Error::new(ErrorCode::InvalidJson, format!("{what}: {}", e.message()))
            .with_hint("pass exactly one JSON value, with unique object keys")
    })?;
    from_value(value, what)
}

/// Deserializes an already strictly-parsed value into `T`.
///
/// # Errors
/// `invalid_input` with serde's explanation of the mismatch.
pub fn from_value<T: DeserializeOwned>(value: Value, what: &str) -> Result<T> {
    serde_json::from_value(value)
        .map_err(|e| Error::invalid_input(format!("{what} is not valid: {e}")))
}

/// A short name for a JSON value's type, for error messages.
#[must_use]
pub fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_keys_are_rejected_at_any_depth() {
        assert!(parse_value(br#"{"a":1,"a":2}"#).is_err());
        assert!(parse_value(br#"{"a":{"b":1,"b":2}}"#).is_err());
        assert!(parse_value(br#"[{"x":1},{"y":[{"z":1,"z":1}]}]"#).is_err());
        assert!(parse_value(br#"{"a":1,"b":{"a":2}}"#).is_ok());
    }

    #[test]
    fn trailing_content_and_bad_utf8_are_rejected() {
        assert!(parse_value(br#"{"a":1} {"b":2}"#).is_err());
        assert!(parse_value(b"\"\xff\"").is_err());
        assert!(parse_value(b"  {\"a\":1}\n").is_ok());
    }

    #[test]
    fn object_parsing_names_the_input() {
        let e = parse_object(b"[1]", "--show").err();
        assert!(e.is_some_and(|e| e.message().contains("--show must be a JSON object")));
        let e = parse_object(br#"{"a":1,"a":1}"#, "--ask").err();
        assert!(e.is_some_and(
            |e| *e.code() == ErrorCode::InvalidJson && e.message().starts_with("--ask")
        ));
    }

    #[test]
    fn numbers_keep_integer_form() -> Result<()> {
        let v = parse_value(br#"{"i":42,"n":-3,"f":1.5}"#)?;
        assert_eq!(v["i"].as_u64(), Some(42));
        assert_eq!(v["n"].as_i64(), Some(-3));
        assert_eq!(v["f"].as_f64(), Some(1.5));
        Ok(())
    }
}
