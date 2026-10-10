//! In-memory fixed-window rate limits (one process, one instance).

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// A limit of `limit` units per `window`, per key.
pub(crate) struct RateLimiter {
    limit: u32,
    window: Duration,
    buckets: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    /// A limiter allowing `limit` units per `window` for each key.
    pub(crate) fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Takes `units` for `key`; on refusal returns how long until the window
    /// resets (whole seconds, at least 1).
    pub(crate) fn take(&self, key: &str, units: u32) -> Result<(), u64> {
        let now = Instant::now();
        let Ok(mut buckets) = self.buckets.lock() else {
            // A poisoned limiter must not take the service down; fail open.
            return Ok(());
        };
        if buckets.len() > 10_000 {
            let window = self.window;
            buckets.retain(|_, (start, _)| now.duration_since(*start) < window);
        }
        let entry = buckets.entry(key.to_owned()).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        if entry.1.saturating_add(units) > self.limit {
            let reset = self.window.saturating_sub(now.duration_since(entry.0));
            return Err(reset.as_secs().max(1));
        }
        entry.1 += units;
        Ok(())
    }

    /// The configured limit.
    pub(crate) fn limit(&self) -> u32 {
        self.limit
    }
}
