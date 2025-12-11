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
    /// SOL mint address
    sol_mint: String,
}

impl MarketRegimeDetector {
    /// Create a new market regime detector
    pub fn new(price_cache: Arc<PriceCache>) -> Self {
        Self {
            price_cache,
            price_history: Arc::new(parking_lot::RwLock::new(VecDeque::new())),
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
    use super::*;

    #[test]
    fn test_regime_detection() {
        // This would be tested with actual price history in integration tests
    }
}


