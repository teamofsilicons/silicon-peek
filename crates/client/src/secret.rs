//! Secret material with explicit exposure and redacted formatting.

use std::fmt;

use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};

/// A credential (token, SLT, app secret, home token, JWT or API key).
///
/// `Debug` and `Display` never render the value. Serialization does expose it,
/// because secrets must reach `session.json`, request bodies and IPC frames;
/// never serialize a type holding a `Secret` into logs, telemetry or reports.
#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct Secret(SecretString);

impl Secret {
    /// Wraps a value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(SecretString::from(value.into()))
    }

    /// Returns the secret value. Call sites are the audit trail.
    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }

    /// Whether the value is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.expose().is_empty()
    }

    /// Constant-time equality, for comparing presented credentials.
    #[must_use]
    pub fn ct_eq(&self, other: &str) -> bool {
        constant_time_eq(self.expose().as_bytes(), other.as_bytes())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Serialize for Secret {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.expose())
    }
}

impl PartialEq for Secret {
    fn eq(&self, other: &Self) -> bool {
        self.ct_eq(other.expose())
    }
}

impl Eq for Secret {}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// Compares two byte strings without an early exit on the first difference.
///
/// The length is not hidden; every credential this crate compares has a fixed
/// length, so that leaks nothing.
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    // A volatile-free barrier: `black_box` keeps the optimizer from turning the
    // fold into an early-exit comparison.
    std::hint::black_box(diff) == 0
}
