//! Market Regime Detection
//!
//! Detects market regime (bull/bear/sideways) from SOL price trends
//! and adjusts profit targets accordingly.

use crate::price_cache::PriceCache;
use std::sync::Arc;
use std::collections::VecDeque;

/// Market regime type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketRegime {
    /// Bull market (upward trend)
    Bull,
    /// Bear market (downward trend)
    Bear,
    /// Sideways market (no clear trend)
    Sideways,
}

impl std::fmt::Display for MarketRegime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MarketRegime::Bull => write!(f, "BULL"),
            MarketRegime::Bear => write!(f, "BEAR"),
            MarketRegime::Sideways => write!(f, "SIDEWAYS"),
        }
    }
}

/// Market regime detector
pub struct MarketRegimeDetector {
    price_cache: Arc<PriceCache>,
    /// Price history for trend analysis (last 24 hours)
    price_history: Arc<parking_lot::RwLock<VecDeque<(chrono::DateTime<chrono::Utc>, f64)>>>,
    /// Volume history for trend analysis (weekly snapshots of total Solana DEX volume)
    volume_history: Arc<parking_lot::RwLock<VecDeque<(chrono::DateTime<chrono::Utc>, f64)>>>,
    /// SOL mint address
    sol_mint: String,
}

impl MarketRegimeDetector {
    /// Create a new market regime detector
    pub fn new(price_cache: Arc<PriceCache>) -> Self {
        Self {
            price_cache,
            price_history: Arc::new(parking_lot::RwLock::new(VecDeque::new())),
            volume_history: Arc::new(parking_lot::RwLock::new(VecDeque::new())),
            sol_mint: "So11111111111111111111111111111111111111112".to_string(),
        }
    }

    /// Update price history (called periodically)
    pub async fn update_price_history(&self) {
        if let Some(price_entry) = self.price_cache.get_price(&self.sol_mint) {
            let mut history = self.price_history.write();
            let now = chrono::Utc::now();
            
            // Add current price
            history.push_back((now, price_entry.price_usd));
            
            // Keep only last 24 hours (assuming updates every hour = 24 entries)
            let cutoff = now - chrono::Duration::hours(24);
            while let Some(front) = history.front() {
                if front.0 < cutoff {
                    history.pop_front();
                } else {
                    break;
                }
            }
        }
    }

    /// Detect current market regime
    ///
    /// # Returns
    /// MarketRegime based on price trend
    pub fn detect_regime(&self) -> MarketRegime {
        let history = self.price_history.read();
        
        if history.len() < 3 {
            // Not enough data, default to sideways
            return MarketRegime::Sideways;
        }

        // Calculate price change over last 24 hours
        let prices: Vec<f64> = history.iter().map(|(_, price)| *price).collect();
        let first_price = prices.first().unwrap_or(&0.0);
        let last_price = prices.last().unwrap_or(&0.0);
        
        if *first_price == 0.0 || *last_price == 0.0 {
            return MarketRegime::Sideways;
        }

        let price_change_percent = ((last_price - first_price) / first_price) * 100.0;

        // Classify regime based on price change
        if price_change_percent > 5.0 {
            MarketRegime::Bull
        } else if price_change_percent < -5.0 {
            MarketRegime::Bear
        } else {
            MarketRegime::Sideways
        }
    }

    /// Update volume history (called periodically, e.g., daily)
    /// 
    /// # Arguments
    /// * `total_dex_volume_usd` - Total Solana DEX volume in USD for the period
    pub fn update_volume_history(&self, total_dex_volume_usd: f64) {
        let mut history = self.volume_history.write();
        let now = chrono::Utc::now();
        
        // Add current volume snapshot
        history.push_back((now, total_dex_volume_usd));
        
        // Keep only last 2 weeks (14 entries if called daily)
        let cutoff = now - chrono::Duration::days(14);
        while let Some(front) = history.front() {
            if front.0 < cutoff {
                history.pop_front();
            } else {
                break;
            }
        }
    }

    /// Check volume trend (week-over-week)
    ///
    /// # Returns
    /// Volume trend multiplier:
    /// - > 1.0 if volume is increasing (bullish)
    /// - < 1.0 if volume is decreasing (bearish, reduce position sizes)
    /// - 1.0 if no clear trend or insufficient data
    pub fn get_volume_trend_multiplier(&self) -> f64 {
        let history = self.volume_history.read();
        
        if history.len() < 7 {
            // Need at least 7 days of data (1 week)
            return 1.0;
        }

        // Get volumes from last week and previous week
        let now = chrono::Utc::now();
        let one_week_ago = now - chrono::Duration::days(7);
        
        let mut last_week_volume = 0.0;
        let mut last_week_count = 0;
        let mut previous_week_volume = 0.0;
        let mut previous_week_count = 0;

        for (timestamp, volume) in history.iter() {
            if *timestamp >= one_week_ago {
                last_week_volume += volume;
                last_week_count += 1;
            } else {
                previous_week_volume += volume;
                previous_week_count += 1;
            }
        }

        if last_week_count == 0 || previous_week_count == 0 {
            return 1.0;
        }

        let last_week_avg = last_week_volume / last_week_count as f64;
        let previous_week_avg = previous_week_volume / previous_week_count as f64;

        if previous_week_avg == 0.0 {
            return 1.0;
        }

        // Calculate week-over-week change
        let volume_change_ratio = last_week_avg / previous_week_avg;

        // Return multiplier:
        // - If volume drops >20%, reduce position sizes by 30%
        // - If volume drops 10-20%, reduce by 15%
        // - If volume increases >20%, increase by 10% (but cap at 1.2x)
        // - Otherwise, neutral (1.0)
        if volume_change_ratio < 0.8 {
            // Volume dropped >20%
            0.7
        } else if volume_change_ratio < 0.9 {
            // Volume dropped 10-20%
            0.85
        } else if volume_change_ratio > 1.2 {
            // Volume increased >20%
            1.1
        } else {
            // Neutral
            1.0
        }
    }

    /// Get position sizing multiplier based on market regime and volume trend
    ///
    /// # Returns
    /// Multiplier to apply to base position size (0.0 - 2.0)
    pub fn get_position_sizing_multiplier(&self) -> f64 {
        let volume_multiplier = self.get_volume_trend_multiplier();
        
        // In low volume regimes, reduce position sizes globally
        // This prevents getting stuck in illiquid positions
        volume_multiplier.max(0.5).min(2.0) // Clamp between 0.5x and 2.0x
    }

    /// Get profit targets for current regime
    ///
    /// # Returns
    /// Vector of profit target percentages
    pub fn get_profit_targets(&self) -> Vec<f64> {
        match self.detect_regime() {
            MarketRegime::Bull => vec![50.0, 100.0, 200.0, 500.0],  // Higher targets in bull
            MarketRegime::Bear => vec![15.0, 30.0, 50.0, 100.0],   // Lower targets in bear
            MarketRegime::Sideways => vec![10.0, 20.0, 30.0],      // Quick scalps in sideways
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_regime_detection() {
        // This would be tested with actual price history in integration tests
    }
}




