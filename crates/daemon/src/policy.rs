// SPDX-License-Identifier: MIT OR Apache-2.0
//! What to keep and how often to poll, from `[sync]` in `config.toml`.

use std::sync::Arc;
use std::time::Duration;

use familiar_config::model::{OlderBodies, Sync as SyncConfig};
use familiar_config::validate::{parse_period, parse_size};

/// Seconds in a day.
pub const DAY: i64 = 86_400;

/// The current time as Unix seconds. Tests replace it to pin the window.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The system clock.
pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    })
}

/// The sync engine's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPolicy {
    /// Full bodies are kept for messages newer than this many days.
    pub window_days: u32,
    /// Headers are kept for this many days; `None` keeps all history.
    pub headers_days: Option<u32>,
    /// Whether bodies outside the window may be fetched on demand.
    pub older_bodies: OlderBodies,
    /// The most stored bytes the on-demand body cache may hold.
    pub cache_limit: u64,
    /// How often the Inbox is polled.
    pub inbox_interval: Duration,
    /// How often every other folder is polled.
    pub folder_interval: Duration,
    /// How often the folder tree is polled.
    pub tree_interval: Duration,
    /// How often bodies are aged out of the window and the cache trimmed.
    pub ageing_interval: Duration,
    /// Bodies downloaded per step of the body backfill.
    pub body_batch: usize,
    /// First wait after a failed round; doubled per failure in a row.
    pub backoff_min: Duration,
    /// Longest wait after failures.
    pub backoff_max: Duration,
}

impl Default for SyncPolicy {
    fn default() -> Self {
        Self {
            window_days: 365,
            headers_days: None,
            older_bodies: OlderBodies::OnDemand,
            cache_limit: 2 << 30,
            inbox_interval: Duration::from_secs(30),
            folder_interval: Duration::from_secs(300),
            tree_interval: Duration::from_secs(300),
            ageing_interval: Duration::from_secs(3600),
            body_batch: 64,
            backoff_min: Duration::from_secs(30),
            backoff_max: Duration::from_secs(600),
        }
    }
}

impl SyncPolicy {
    /// The policy for a `[sync]` table (already validated by `fam config validate`).
    pub fn from_config(s: &SyncConfig) -> Result<Self, String> {
        let window_days =
            parse_period(&s.window).ok_or_else(|| format!("bad sync.window {:?}", s.window))?;
        let headers_days = match s.headers.as_str() {
            "all" => None,
            "window" => Some(window_days),
            h => Some(parse_period(h).ok_or_else(|| format!("bad sync.headers {h:?}"))?),
        };
        let cache_limit = parse_size(&s.cache_limit)
            .ok_or_else(|| format!("bad sync.cache_limit {:?}", s.cache_limit))?;
        Ok(Self {
            window_days,
            headers_days: headers_days.map(|h| h.max(window_days)),
            older_bodies: s.older_bodies,
            cache_limit,
            ..Self::default()
        })
    }

    /// The oldest `received_at` inside the body window at `now`.
    pub fn window_start(&self, now: i64) -> i64 {
        now - i64::from(self.window_days) * DAY
    }

    /// The oldest `received_at` whose header is kept, if headers are limited.
    pub fn headers_start(&self, now: i64) -> Option<i64> {
        self.headers_days.map(|d| now - i64::from(d) * DAY)
    }

    /// The wait after `failures` failed rounds in a row.
    pub fn backoff(&self, failures: u32) -> Duration {
        let shift = failures.saturating_sub(1).min(16);
        self.backoff_min
            .saturating_mul(1 << shift)
            .min(self.backoff_max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config() {
        let p = SyncPolicy::from_config(&SyncConfig::default()).unwrap();
        assert_eq!(p.window_days, 365);
        assert_eq!(p.headers_days, None);
        assert_eq!(p.cache_limit, 2 * 1024 * 1024 * 1024);
        assert_eq!(p.inbox_interval, Duration::from_secs(30));
        assert_eq!(p.folder_interval, Duration::from_secs(300));
        let s = SyncConfig {
            window: "90d".into(),
            headers: "window".into(),
            cache_limit: "500MB".into(),
            ..SyncConfig::default()
        };
        let p = SyncPolicy::from_config(&s).unwrap();
        assert_eq!((p.window_days, p.headers_days), (90, Some(90)));
        assert_eq!(p.window_start(100 * DAY), 10 * DAY);
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let p = SyncPolicy::default();
        assert_eq!(p.backoff(1), Duration::from_secs(30));
        assert_eq!(p.backoff(2), Duration::from_secs(60));
        assert_eq!(p.backoff(5), Duration::from_secs(480));
        assert_eq!(p.backoff(6), Duration::from_secs(600));
        assert_eq!(p.backoff(60), Duration::from_secs(600));
    }
}
