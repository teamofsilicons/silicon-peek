//! A finite JSON number that keeps integers integral on the wire.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Largest integer an `f64` represents exactly (2^53).
const MAX_SAFE: f64 = 9_007_199_254_740_992.0;

/// A finite number. Integral values serialize as JSON integers (`42`, not
/// `42.0`), so slider answers and ask limits read naturally in Ting payloads.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Default)]
pub struct Num(f64);

impl Num {
    /// Wraps a finite value; `None` for NaN or infinity.
    #[must_use]
    pub fn new(v: f64) -> Option<Self> {
        v.is_finite().then_some(Self(v))
    }

    /// The value.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }

    /// Whether the value is an exactly representable integer.
    #[must_use]
    pub fn is_integral(self) -> bool {
        self.0.fract() == 0.0 && self.0.abs() <= MAX_SAFE
    }
}

impl From<i32> for Num {
    fn from(v: i32) -> Self {
        Self(f64::from(v))
    }
}

impl fmt::Display for Num {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // f64's Display already renders 42.0 as `42`.
        fmt::Display::fmt(&self.0, f)
    }
}

impl Serialize for Num {
    #[allow(clippy::cast_possible_truncation)] // guarded by is_integral (|v| ≤ 2^53)
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.is_integral() {
            s.serialize_i64(self.0 as i64)
        } else {
            s.serialize_f64(self.0)
        }
    }
}

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = f64::deserialize(d)?;
        Self::new(v).ok_or_else(|| serde::de::Error::custom("number must be finite"))
    }
}
