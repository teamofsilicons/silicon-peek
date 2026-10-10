//! RFC 3339 UTC timestamps (`2026-09-26T10:00:07Z`) and unix-time helpers.

use std::{
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::error::{Error, Result};

/// Current unix time in whole seconds.
#[must_use]
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// A UTC instant with millisecond precision, serialized as RFC 3339 with `Z`.
///
/// Whole seconds render without a fraction (`…T10:00:07Z`); otherwise the
/// milliseconds are kept (`…T10:00:07.25Z`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Timestamp {
    unix_ms: i64,
}

impl Timestamp {
    /// Now.
    #[must_use]
    pub fn now() -> Self {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        Self { unix_ms: ms }
    }

    /// From unix seconds.
    #[must_use]
    pub const fn from_unix(seconds: i64) -> Self {
        Self {
            unix_ms: seconds.saturating_mul(1000),
        }
    }

    /// From unix milliseconds.
    #[must_use]
    pub const fn from_unix_ms(ms: i64) -> Self {
        Self { unix_ms: ms }
    }

    /// Unix seconds (floored).
    #[must_use]
    pub const fn unix(self) -> i64 {
        self.unix_ms.div_euclid(1000)
    }

    /// Unix milliseconds.
    #[must_use]
    pub const fn unix_ms(self) -> i64 {
        self.unix_ms
    }

    /// This instant plus a duration (saturating).
    #[must_use]
    pub fn plus(self, d: Duration) -> Self {
        let ms = i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
        Self {
            unix_ms: self.unix_ms.saturating_add(ms),
        }
    }

    /// Parses RFC 3339. The offset must be `Z` (UTC).
    ///
    /// # Errors
    /// `invalid_input` for anything that is not RFC 3339 UTC.
    pub fn parse(s: &str) -> Result<Self> {
        if !s.ends_with('Z') {
            return Err(Error::invalid_input(format!(
                "timestamp `{s}` must be RFC 3339 UTC ending in `Z`"
            )));
        }
        let dt = OffsetDateTime::parse(s, &Rfc3339)
            .map_err(|e| Error::invalid_input(format!("timestamp `{s}` is not RFC 3339: {e}")))?;
        let ms = dt.unix_timestamp_nanos().div_euclid(1_000_000);
        let unix_ms = i64::try_from(ms)
            .map_err(|_| Error::invalid_input(format!("timestamp `{s}` is out of range")))?;
        Ok(Self { unix_ms })
    }

    /// The RFC 3339 rendering in UTC with fixed millisecond precision
    /// (`2026-09-26T11:07:18.730Z`), so timestamps compare correctly as
    /// strings and parse with fixed-width readers.
    #[must_use]
    pub fn to_rfc3339(self) -> String {
        let nanos = i128::from(self.unix_ms) * 1_000_000;
        match OffsetDateTime::from_unix_timestamp_nanos(nanos) {
            Ok(dt) if (0..=9999).contains(&dt.year()) => format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
                dt.year(),
                u8::from(dt.month()),
                dt.day(),
                dt.hour(),
                dt.minute(),
                dt.second(),
                dt.millisecond()
            ),
            // Outside RFC 3339's years: clamp to the epoch rather than
            // panic; no peek timestamp is ever that far off.
            _ => "1970-01-01T00:00:00.000Z".to_owned(),
        }
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_rfc3339())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(|e| serde::de::Error::custom(e.message().to_owned()))
    }
}
