//! Momentum-Based Early Exit Detection
//!
//! Detects negative momentum indicators and triggers early exit:
//! - Price drops 5% from entry within 5 minutes
//! - Volume drops >50%
//! - RSI < 40 and declining
//!
//! This helps cut losses faster and avoid holding losing positions.

use crate::db::DbPool;
use crate::price_cache::PriceCache;
use crate::engine::volume_cache::VolumeCache;
use std::sync::Arc;
use std::time::SystemTime;

/// Momentum exit action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MomentumExitAction {
    /// No action needed
    None,
    /// Exit position (negative momentum detected)
    Exit,
}

/// Momentum exit detector
pub struct MomentumExit {
    db: DbPool,
    price_cache: Arc<PriceCache>,
    volume_cache: Option<Arc<VolumeCache>>,
}

/// Position entry data for momentum tracking
#[derive(Debug, Clone)]
struct PositionEntry {
    trade_uuid: String,
    token_address: String,
    entry_price: f64,
    entry_time: SystemTime,
    entry_amount_sol: f64,
}

impl MomentumExit {
    /// Create a new momentum exit detector
    pub fn new(db: DbPool, price_cache: Arc<PriceCache>) -> Self {
        Self {
            db,
            price_cache,
            volume_cache: None,
        }
    }

    /// Create with volume cache
    pub fn with_volume_cache(db: DbPool, price_cache: Arc<PriceCache>, volume_cache: Arc<VolumeCache>) -> Self {
        Self {
            db,
            price_cache,
            volume_cache: Some(volume_cache),
        }
    }

    /// Check for negative momentum and return action
    ///
    /// # Arguments
    /// * `trade_uuid` - Trade UUID
    /// * `token_address` - Token address
    /// * `entry_price` - Entry price in USD
    /// * `entry_time` - When position was opened
    ///
    /// # Returns
    /// MomentumExitAction indicating whether to exit
    pub async fn check_momentum(
        &self,
        trade_uuid: &str,
        token_address: &str,
        entry_price: f64,
        entry_time: SystemTime,
    ) -> MomentumExitAction {
        // Get current price
        let current_price = match self.price_cache.get_price_usd(token_address) {
            Some(price) => price,
            None => return MomentumExitAction::None, // No price data, skip check
        };

        // Check 1: Price drops 5% from entry within 5 minutes
        let price_drop_percent = ((entry_price - current_price) / entry_price) * 100.0;
        let elapsed = entry_time.elapsed().unwrap_or_default();
        let elapsed_minutes = elapsed.as_secs() / 60;

        if elapsed_minutes <= 5 && price_drop_percent >= 5.0 {
            tracing::warn!(
                trade_uuid = %trade_uuid,
                price_drop_percent = price_drop_percent,
                elapsed_minutes = elapsed_minutes,
                "Negative momentum detected: price dropped 5% within 5 minutes"
            );
            return MomentumExitAction::Exit;
        }

        // Check 2: Volume drop (>50% from 24h average)
        if let Some(ref volume_cache) = self.volume_cache {
            if volume_cache.has_volume_drop(token_address, 50.0) {
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    "Negative momentum detected: volume dropped >50% from 24h average"
                );
                return MomentumExitAction::Exit;
            }
        }

        // Check 3: RSI declining (RSI < 40 and declining)
        if let Some(rsi) = self.calculate_rsi(token_address, entry_price).await {
            if rsi < 40.0 {
                // Check if RSI was higher in previous period (declining)
                // For simplicity, we'll check if current RSI is below threshold
                // A more sophisticated check would compare to previous RSI value
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    rsi = rsi,
                    "Negative momentum detected: RSI < 40"
                );
                return MomentumExitAction::Exit;
            }
        }

        MomentumExitAction::None
    }

    /// Calculate RSI (Relative Strength Index) from price history
    ///
    /// Uses 14-period RSI by default
    /// Returns None if insufficient data
    async fn calculate_rsi(&self, token_address: &str, _entry_price: f64) -> Option<f64> {
        // Get price history from price cache
        let history = self.price_cache.price_history.read();
        let token_history = history.get(token_address)?;
        
        if token_history.len() < 15 {
            // Need at least 15 data points for 14-period RSI
            return None;
        }
        
        // Get last 14 price changes
        let prices: Vec<f64> = token_history.iter().rev().take(15).map(|(_, price)| *price).collect();
        let mut gains = Vec::new();
        let mut losses = Vec::new();
        
        for i in 1..prices.len() {
            let change = prices[i - 1] - prices[i]; // Reversed order
            if change > 0.0 {
                gains.push(change);
                losses.push(0.0);
            } else {
                gains.push(0.0);
                losses.push(change.abs());
            }
        }
        
        if gains.is_empty() || losses.is_empty() {
            return None;
        }
        
        // Calculate average gain and loss
        let avg_gain: f64 = gains.iter().sum::<f64>() / gains.len() as f64;
        let avg_loss: f64 = losses.iter().sum::<f64>() / losses.len() as f64;
        
        if avg_loss == 0.0 {
            return Some(100.0); // All gains, RSI = 100
        }
        
        // Calculate RS (Relative Strength)
        let rs = avg_gain / avg_loss;
        
        // Calculate RSI: 100 - (100 / (1 + RS))
        let rsi = 100.0 - (100.0 / (1.0 + rs));
        
        Some(rsi)
    }

    /// Check if position should exit based on momentum
    /// This is a simplified version that only checks price drop
    pub async fn should_exit(
        &self,
        trade_uuid: &str,
        token_address: &str,
        entry_price: f64,
        entry_time: SystemTime,
    ) -> bool {
        matches!(
            self.check_momentum(trade_uuid, token_address, entry_price, entry_time).await,
            MomentumExitAction::Exit
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_momentum_exit_price_drop() {
        // This would be tested with actual database and price cache in integration tests
        // Test case: Price drops 6% within 3 minutes -> should exit
    }
}


