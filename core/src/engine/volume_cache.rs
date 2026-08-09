//! Volume Cache for token trading volume tracking
//!
//! Tracks 24h average volume for tokens to detect volume drops
//! in momentum exit detection.

use chrono::{DateTime, Duration, Utc};
use parking_lot::RwLock;
use rust_decimal::Decimal;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Volume cache for token trading volumes
pub struct VolumeCache {
    /// Volume history by token (token -> VecDeque of (timestamp, volume))
    #[allow(clippy::type_complexity)]
    volume_history: Arc<RwLock<HashMap<String, VecDeque<(DateTime<Utc>, Decimal)>>>>,
}

impl VolumeCache {
    /// Create a new volume cache
    pub fn new() -> Self {
        Self {
            volume_history: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record volume for a token
    pub fn record_volume(&self, token_address: &str, volume_usd: Decimal) {
        let now = Utc::now();
        let mut history = self.volume_history.write();
        // Evict idle tokens (no updates for 24h) so a long-running operator
        // processing many ephemeral tokens cannot grow memory without bound.
        history.retain(|_, h| {
            h.back().is_none_or(|(t, _)| now.signed_duration_since(*t) <= Duration::hours(24))
        });
        let token_history = history.entry(token_address.to_string()).or_default();
        token_history.push_back((now, volume_usd));

        // Keep only last 24 hours
        let cutoff = now - Duration::hours(24);
        while let Some(front) = token_history.front() {
            if front.0 < cutoff {
                token_history.pop_front();
            } else {
                break;
            }
        }
    }

    /// Record volume with a custom timestamp (test only).
    pub fn record_volume_with_time(
        &self,
        token_address: &str,
        volume_usd: Decimal,
        now: DateTime<Utc>,
    ) {
        let mut history = self.volume_history.write();
        history.retain(|_, h| {
            h.back().is_none_or(|(t, _)| now.signed_duration_since(*t) <= Duration::hours(24))
        });
        let token_history = history.entry(token_address.to_string()).or_default();
        token_history.push_back((now, volume_usd));

        let cutoff = now - Duration::hours(24);
        while let Some(front) = token_history.front() {
            if front.0 < cutoff {
                token_history.pop_front();
            } else {
                break;
            }
        }
    }

    /// Replace a token's full volume history (test only).
    pub fn set_volume_history(&self, token_address: &str, samples: Vec<(DateTime<Utc>, Decimal)>) {
        self.volume_history
            .write()
            .insert(token_address.to_string(), samples.into_iter().collect());
    }

    /// Snapshot a token's volume history (test only).
    pub fn volume_history_snapshot(
        &self,
        token_address: &str,
    ) -> Vec<(DateTime<Utc>, Decimal)> {
        self.volume_history
            .read()
            .get(token_address)
            .map(|h| h.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Get 24h average volume for a token
    ///
    /// Returns None if insufficient data or the newest sample is stale
    /// (>10 minutes — same staleness guard as `has_volume_drop`, so callers
    /// never act on volume signals from indexer lag/downtime).
    pub fn get_24h_average_volume(&self, token_address: &str) -> Option<Decimal> {
        let history = self.volume_history.read();
        let token_history = history.get(token_address)?;

        if let Some((newest_time, _)) = token_history.back() {
            if Utc::now().signed_duration_since(*newest_time).num_minutes() > 10 {
                return None;
            }
        }

        if token_history.is_empty() {
            return None;
        }

        let total_volume: Decimal = token_history.iter().map(|(_, volume)| *volume).sum();
        let count = Decimal::from(token_history.len());

        Some(total_volume / count)
    }

    /// Get current volume (most recent entry)
    ///
    /// Returns None when the newest sample is stale (>10 minutes).
    pub fn get_current_volume(&self, token_address: &str) -> Option<Decimal> {
        let history = self.volume_history.read();
        let token_history = history.get(token_address)?;
        let (newest_time, volume) = token_history.back()?;
        if Utc::now().signed_duration_since(*newest_time).num_minutes() > 10 {
            return None;
        }
        Some(*volume)
    }

    /// Check if volume dropped significantly compared to a time-matched baseline.
    ///
    /// Compares the 60-minute recent average to the prior 23-hour baseline
    /// (excluding the most recent hour). This accounts for normal diurnal volume
    /// patterns (lower during Asian/EU overlap vs US hours) and avoids false exits
    /// from a single quiet data point.
    ///
    /// Falls back to single-point vs full 24h average when insufficient data exists
    /// (< 3 recent samples or < 12 baseline samples).
    pub fn has_volume_drop(&self, token_address: &str, threshold_percent: Decimal) -> bool {
        let history = self.volume_history.read();
        let token_history = match history.get(token_address) {
            Some(h) if !h.is_empty() => h,
            _ => return false,
        };

        let now = Utc::now();

        // Fail safe: if the most-recent data point is > 10 minutes old the cache is
        // stale (indexer lag or downtime). Do not exit on stale signals.
        if let Some((newest_time, _)) = token_history.back() {
            if now.signed_duration_since(*newest_time).num_minutes() > 10 {
                return false;
            }
        }

        let recent_cutoff = now - Duration::minutes(60);
        let baseline_cutoff = now - Duration::hours(24);

        let recent_samples: Vec<Decimal> = token_history
            .iter()
            .filter(|(t, _)| *t >= recent_cutoff)
            .map(|(_, v)| *v)
            .collect();

        let baseline_samples: Vec<Decimal> = token_history
            .iter()
            .filter(|(t, _)| *t >= baseline_cutoff && *t < recent_cutoff)
            .map(|(_, v)| *v)
            .collect();

        // Windowed comparison: recent 60 min vs prior 23 h baseline
        if recent_samples.len() >= 3 && baseline_samples.len() >= 12 {
            let recent_avg = recent_samples.iter().copied().sum::<Decimal>()
                / Decimal::from(recent_samples.len());
            let baseline_avg = baseline_samples.iter().copied().sum::<Decimal>()
                / Decimal::from(baseline_samples.len());
            if baseline_avg > Decimal::ZERO {
                let drop_pct = (baseline_avg - recent_avg) / baseline_avg * Decimal::from(100);
                return drop_pct >= threshold_percent;
            }
        }

        // Fallback: single most-recent point vs the remaining history.
        // The current sample is EXCLUDED from the baseline so a single low
        // `current` cannot drag the average down and mask a genuine drop.
        // Require at least 30 minutes of recorded history — 2 data points
        // spanning 5 minutes produce a meaningless "24h average" and can
        // trigger false exits.
        if let Some((oldest_time, _)) = token_history.front() {
            if now.signed_duration_since(*oldest_time).num_minutes() < 30 {
                return false;
            }
        }
        if let Some(current) = token_history.back().map(|(_, v)| *v) {
            let n = token_history.len().saturating_sub(1);
            if n > 0 {
                let total: Decimal = token_history.iter().take(n).map(|(_, v)| *v).sum();
                let avg = total / Decimal::from(n);
                if avg > Decimal::ZERO {
                    let drop_pct = (avg - current) / avg * Decimal::from(100);
                    return drop_pct >= threshold_percent;
                }
            }
        }

        false
    }
}

impl Default for VolumeCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn test_record_and_avg_volume() {
        let cache = VolumeCache::new();
        cache.record_volume("token1", dec!(10));
        cache.record_volume("token1", dec!(20));
        cache.record_volume("token1", dec!(30));
        assert_eq!(cache.get_24h_average_volume("token1"), Some(dec!(20)));
        assert_eq!(cache.get_current_volume("token1"), Some(dec!(30)));
    }

    #[test]
    fn test_missing_token_returns_none() {
        let cache = VolumeCache::new();
        assert_eq!(cache.get_24h_average_volume("missing"), None);
        assert_eq!(cache.get_current_volume("missing"), None);
        assert!(!cache.has_volume_drop("missing", dec!(50)));
    }

    #[test]
    fn test_empty_history_returns_none() {
        let cache = VolumeCache::new();
        cache.set_volume_history("token1", Vec::new());
        assert_eq!(cache.get_24h_average_volume("token1"), None);
    }

    #[test]
    fn test_default_impl() {
        let cache: VolumeCache = Default::default();
        cache.record_volume("token1", dec!(5));
        assert_eq!(cache.get_current_volume("token1"), Some(dec!(5)));
    }

    #[test]
    fn test_stale_volume_returns_none() {
        let cache = VolumeCache::new();
        let t = now() - Duration::minutes(11);
        cache.set_volume_history("token1", vec![(t, dec!(100))]);
        assert_eq!(cache.get_24h_average_volume("token1"), None);
        assert_eq!(cache.get_current_volume("token1"), None);
        assert!(!cache.has_volume_drop("token1", dec!(50)));
    }

    #[test]
    fn test_history_trims_older_than_24h() {
        let cache = VolumeCache::new();
        let t = now();
        cache.set_volume_history(
            "token1",
            vec![
                (t - Duration::hours(25), dec!(1)),
                (t - Duration::hours(23), dec!(2)),
                (t - Duration::hours(1), dec!(3)),
            ],
        );
        // 24h window: the -25h sample must be trimmed on the next record
        cache.record_volume_with_time("token1", dec!(4), t);
        let history = cache.volume_history_snapshot("token1");
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].0, t - Duration::hours(23));
        assert_eq!(history[0].1, dec!(2));
    }

    #[test]
    fn test_idle_token_evicted_on_record() {
        let cache = VolumeCache::new();
        let t = now();
        cache.record_volume_with_time("idle", dec!(1), t - Duration::hours(25));
        // Recording a different token evicts entries idle for 24h+
        cache.record_volume("active", dec!(2));
        assert_eq!(cache.get_24h_average_volume("idle"), None);
        assert_eq!(cache.get_current_volume("active"), Some(dec!(2)));
    }

    #[test]
    fn test_has_volume_drop_windowed_path() {
        let cache = VolumeCache::new();
        let t = now();
        let mut samples = Vec::new();
        for i in 1..=12 {
            samples.push((t - Duration::minutes(23 * 60 - i * 60), dec!(100)));
        }
        samples.push((t - Duration::minutes(50), dec!(10)));
        samples.push((t - Duration::minutes(30), dec!(10)));
        samples.push((t - Duration::minutes(5), dec!(10)));
        cache.set_volume_history("token1", samples);

        assert!(cache.has_volume_drop("token1", dec!(50)));
        // Below threshold: no drop reported
        assert!(!cache.has_volume_drop("token1", dec!(95)));
    }

    #[test]
    fn test_has_volume_drop_zero_baseline_skips_windowed() {
        let cache = VolumeCache::new();
        let t = now();
        let mut samples = Vec::new();
        for i in 1..=12 {
            samples.push((t - Duration::minutes(23 * 60 - i * 60), dec!(0)));
        }
        samples.push((t - Duration::minutes(50), dec!(0)));
        samples.push((t - Duration::minutes(30), dec!(0)));
        samples.push((t - Duration::minutes(5), dec!(0)));
        cache.set_volume_history("token1", samples);
        // All-zero data: no drop reported, both branches for zero averages hit
        assert!(!cache.has_volume_drop("token1", dec!(10)));
    }

    #[test]
    fn test_has_volume_drop_fallback_path() {
        let cache = VolumeCache::new();
        let t = now();
        let mut samples = Vec::new();
        // 11 baseline samples (below the 12 required for windowed comparison)
        for i in 1..=11 {
            samples.push((t - Duration::minutes(23 * 60 - i * 60), dec!(100)));
        }
        samples.push((t - Duration::minutes(30), dec!(10)));
        samples.push((t - Duration::minutes(5), dec!(10)));
        cache.set_volume_history("token1", samples);

        // Fallback: current (10) vs average of the rest (~92) = ~89% drop
        assert!(cache.has_volume_drop("token1", dec!(50)));
    }

    #[test]
    fn test_has_volume_drop_fallback_insufficient_history() {
        let cache = VolumeCache::new();
        let t = now();
        // Oldest sample under 30 minutes: bail out
        cache.set_volume_history(
            "token1",
            vec![(t - Duration::minutes(10), dec!(10)), (t - Duration::minutes(5), dec!(10))],
        );
        assert!(!cache.has_volume_drop("token1", dec!(50)));

        // Single sample: n == 0, no baseline to compare
        cache.set_volume_history("token1", vec![(t - Duration::minutes(1), dec!(10))]);
        assert!(!cache.has_volume_drop("token1", dec!(50)));
    }

    #[test]
    fn test_has_volume_drop_all_zero_fallback() {
        let cache = VolumeCache::new();
        let t = now();
        let samples = vec![
            (t - Duration::minutes(120), dec!(0)),
            (t - Duration::minutes(60), dec!(0)),
            (t - Duration::minutes(1), dec!(0)),
        ];
        cache.set_volume_history("token1", samples);
        assert!(!cache.has_volume_drop("token1", dec!(10)));
    }
}
