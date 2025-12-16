//! Volume Cache for token trading volume tracking
//!
//! Tracks 24h average volume for tokens to detect volume drops
//! in momentum exit detection.

use chrono::{DateTime, Duration, Utc};
use parking_lot::RwLock;
use rust_decimal::Decimal;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Volume entry
#[derive(Debug, Clone)]
pub struct VolumeEntry {
    /// Volume in USD (using Decimal for precision)
    pub volume_usd: Decimal,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

/// Volume cache for token trading volumes
pub struct VolumeCache {
    /// Volume history by token (token -> VecDeque of (timestamp, volume))
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
        let token_history = history.entry(token_address.to_string()).or_insert_with(VecDeque::new);
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
    /// Returns None if insufficient data
    pub fn get_24h_average_volume(&self, token_address: &str) -> Option<Decimal> {
        let history = self.volume_history.read();
        let token_history = history.get(token_address)?;
        
        if token_history.is_empty() {
            return None;
        }
        
        let total_volume: Decimal = token_history.iter().map(|(_, volume)| *volume).sum();
        let count = Decimal::from(token_history.len());
        
        Some(total_volume / count)
    }

    /// Get current volume (most recent entry)
    pub fn get_current_volume(&self, token_address: &str) -> Option<Decimal> {
        let history = self.volume_history.read();
        let token_history = history.get(token_address)?;
        token_history.back().map(|(_, volume)| *volume)
    }

    /// Check if volume dropped significantly (>50% from 24h average)
    pub fn has_volume_drop(&self, token_address: &str, threshold_percent: Decimal) -> bool {
        if let (Some(current), Some(average)) = (
            self.get_current_volume(token_address),
            self.get_24h_average_volume(token_address),
        ) {
            if average > Decimal::ZERO {
                let drop = average - current;
                let drop_percent = (drop / average) * Decimal::from(100);
                return drop_percent >= threshold_percent;
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






