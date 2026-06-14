//! Momentum-Based Early Exit Detection
//!
//! Detects negative momentum indicators and triggers early exit:
//! - Price drops 5% from entry within 5 minutes
//! - Volume drops >50%
//! - RSI < 40 and declining
//!
//! This helps cut losses faster and avoid holding losing positions.

use crate::db::DbPool;
use crate::engine::volume_cache::VolumeCache;
use crate::price_cache::PriceCache;
use rust_decimal::prelude::*;
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
    #[allow(dead_code)]
    db: DbPool,
    price_cache: Arc<PriceCache>,
    volume_cache: Option<Arc<VolumeCache>>,
}

/// Position entry data for momentum tracking
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct PositionEntry {
    trade_uuid: String,
    token_address: String,
    entry_price: Decimal,
    entry_time: SystemTime,
    entry_amount_sol: Decimal,
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
    pub fn with_volume_cache(
        db: DbPool,
        price_cache: Arc<PriceCache>,
        volume_cache: Arc<VolumeCache>,
    ) -> Self {
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
        entry_price: Decimal,
        entry_time: SystemTime,
    ) -> MomentumExitAction {
        // Get current price
        let current_price = match self.price_cache.get_price_usd(token_address) {
            Some(price) => price,
            None => return MomentumExitAction::None, // No price data, skip check
        };

        // Check 1: Price drops 5% from entry within 5 minutes
        let price_drop_percent = if !entry_price.is_zero() {
            let diff = entry_price - current_price;
            let ratio = diff / entry_price;
            ratio * Decimal::from(100)
        } else {
            Decimal::ZERO
        };
        let elapsed = entry_time.elapsed().unwrap_or_default();
        let elapsed_minutes = elapsed.as_secs() / 60;

        let base_drop_threshold = Decimal::from(5);
        // Widen threshold for high-volatility tokens to avoid shakeout exits.
        // At 30% vol → ~8%, at 50% vol → ~10%, capped at 15%.
        // For positions held >5 min the threshold widens slightly (÷2 of elapsed hours,
        // max +5 pts) so long-held positions aren't exited on normal intraday noise.
        let price_drop_threshold = {
            let vol_bonus = if let Some(vol) = self.price_cache.calculate_volatility(token_address) {
                let vol_dec = Decimal::from_f64_retain(vol).unwrap_or(Decimal::ZERO);
                vol_dec * Decimal::from_str("0.1").unwrap_or(Decimal::ZERO)
            } else {
                Decimal::ZERO
            };
            let age_bonus = if elapsed_minutes > 5 {
                let hours = Decimal::from(elapsed_minutes / 60);
                (hours / Decimal::from(2)).min(Decimal::from(5))
            } else {
                Decimal::ZERO
            };
            (base_drop_threshold + vol_bonus + age_bonus).min(Decimal::from(15))
        };
        if price_drop_percent >= price_drop_threshold {
            let price_drop_f64 = price_drop_percent.to_f64().unwrap_or(0.0);
            tracing::warn!(
                trade_uuid = %trade_uuid,
                price_drop_percent = price_drop_f64,
                elapsed_minutes = elapsed_minutes,
                threshold = ?price_drop_threshold,
                "Negative momentum detected: price drop exceeds threshold"
            );
            return MomentumExitAction::Exit;
        }

        // Check 2: Volume drop (>50% from 24h average)
        if let Some(ref volume_cache) = self.volume_cache {
            if volume_cache.has_volume_drop(token_address, Decimal::from(50)) {
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    "Negative momentum detected: volume dropped >50% from 24h average"
                );
                return MomentumExitAction::Exit;
            }
        }

        // Check 3: RSI declining (RSI < 40 and declining)
        if let Some((current_rsi, previous_rsi)) = self.calculate_rsi(token_address).await {
            if current_rsi < 40.0 && current_rsi < previous_rsi {
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    current_rsi = current_rsi,
                    previous_rsi = previous_rsi,
                    "Negative momentum detected: RSI < 40 and declining"
                );
                return MomentumExitAction::Exit;
            }
        }

        MomentumExitAction::None
    }

    /// Calculate RSI (Relative Strength Index) from price history
    ///
    /// Uses 14-period RSI by default.
    /// Returns Some((current_rsi, previous_rsi)) if sufficient data is available.
    async fn calculate_rsi(&self, token_address: &str) -> Option<(f64, f64)> {
        // Get price history from price cache
        let history = self.price_cache.price_history.read();
        let token_history = history.get(token_address)?;

        if token_history.len() < 16 {
            // Need at least 16 data points for calculating current and previous 14-period RSI
            return None;
        }

        // Get last 16 price changes
        let prices: Vec<f64> = token_history
            .iter()
            .rev()
            .take(16)
            .map(|(_, price)| price.to_f64().unwrap_or(0.0))
            .collect();

        let current_rsi = compute_rsi_from_prices(&prices[0..15])?;
        let previous_rsi = compute_rsi_from_prices(&prices[1..16])?;

        Some((current_rsi, previous_rsi))
    }

    /// Check if position should exit based on momentum
    /// This is a simplified version that only checks price drop
    pub async fn should_exit(
        &self,
        trade_uuid: &str,
        token_address: &str,
        entry_price: Decimal,
        entry_time: SystemTime,
    ) -> bool {
        matches!(
            self.check_momentum(trade_uuid, token_address, entry_price, entry_time)
                .await,
            MomentumExitAction::Exit
        )
    }
}

/// Helper function to calculate RSI from a slice of prices
fn compute_rsi_from_prices(prices: &[f64]) -> Option<f64> {
    if prices.len() < 15 {
        return None;
    }
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
    let avg_gain: f64 = gains.iter().sum::<f64>() / gains.len() as f64;
    let avg_loss: f64 = losses.iter().sum::<f64>() / losses.len() as f64;
    if avg_loss == 0.0 {
        return Some(100.0);
    }
    let rs = avg_gain / avg_loss;
    Some(100.0 - (100.0 / (1.0 + rs)))
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_momentum_exit_price_drop() {
        // This would be tested with actual database and price cache in integration tests
        // Test case: Price drops 6% within 3 minutes -> should exit
    }
}
