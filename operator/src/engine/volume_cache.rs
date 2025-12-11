//! Volume Cache for token trading volume tracking
//!
//! Tracks 24h average volume for tokens to detect volume drops
//! in momentum exit detection.

use chrono::{DateTime, Duration, Utc};
use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Volume entry
#[derive(Debug, Clone)]
pub struct VolumeEntry {
    /// Volume in USD
    pub volume_usd: f64,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

/// Volume cache for token trading volumes
pub struct VolumeCache {
    /// Volume history by token (token -> VecDeque of (timestamp, volume))
    volume_history: Arc<RwLock<HashMap<String, VecDeque<(DateTime<Utc>, f64)>>>>,
}

impl VolumeCache {
    /// Create a new volume cache
    pub fn new() -> Self {
        Self {
            volume_history: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record volume for a token
    pub fn record_volume(&self, token_address: &str, volume_usd: f64) {
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
    pub fn get_24h_average_volume(&self, token_address: &str) -> Option<f64> {
        let history = self.volume_history.read();
        let token_history = history.get(token_address)?;
        
        if token_history.is_empty() {
            return None;
        }
        
        let total_volume: f64 = token_history.iter().map(|(_, volume)| *volume).sum();
        let count = token_history.len() as f64;
        
        Some(total_volume / count)
    }

    /// Get current volume (most recent entry)
    pub fn get_current_volume(&self, token_address: &str) -> Option<f64> {
        let history = self.volume_history.read();
        let token_history = history.get(token_address)?;
        token_history.back().map(|(_, volume)| *volume)
    }

    /// Check if volume dropped significantly (>50% from 24h average)
    pub fn has_volume_drop(&self, token_address: &str, threshold_percent: f64) -> bool {
        if let (Some(current), Some(average)) = (
            self.get_current_volume(token_address),
            self.get_24h_average_volume(token_address),
        ) {
            if average > 0.0 {
                let drop_percent = ((average - current) / average) * 100.0;
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


