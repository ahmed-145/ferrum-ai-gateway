use dashmap::DashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    windows: DashMap<String, Mutex<Vec<Instant>>>,
    limit: u32,
    window_duration: Duration,
}

impl RateLimiter {
    pub fn new(limit: u32, window_secs: u64) -> Self {
        Self {
            windows: DashMap::new(),
            limit,
            window_duration: Duration::from_secs(window_secs),
        }
    }

    /// Checks if the request is allowed under the rate limit for the given key.
    /// Returns true if allowed, false if rate limited.
    pub fn check_limit(&self, key: &str) -> bool {
        let now = Instant::now();
        let cutoff = now.checked_sub(self.window_duration).unwrap_or(now);

        let entry = self.windows.entry(key.to_string()).or_insert_with(|| Mutex::new(Vec::new()));
        let mut timestamps = entry.value().lock().unwrap();

        // Retain only timestamps within the active sliding window
        timestamps.retain(|&ts| ts > cutoff);

        if timestamps.len() < self.limit as usize {
            timestamps.push(now);
            true
        } else {
            false
        }
    }

    /// Periodically cleans up empty key entries from the map to prevent memory growth.
    pub fn cleanup(&self) {
        let now = Instant::now();
        let cutoff = now.checked_sub(self.window_duration).unwrap_or(now);

        // Retain keys that have at least one active timestamp
        self.windows.retain(|_, mutex_ts| {
            if let Ok(timestamps) = mutex_ts.get_mut() {
                timestamps.retain(|&ts| ts > cutoff);
                !timestamps.is_empty()
            } else {
                true // Keep if poisoned lock to avoid losing it
            }
        });
    }
}
