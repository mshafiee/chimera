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
