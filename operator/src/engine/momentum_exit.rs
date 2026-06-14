//! Momentum-Based Early Exit Detection
//!
//! Detects negative momentum indicators and triggers early exit:
//! - Price drops 8%+ from entry within 5 minutes (base; widens for high-volatility tokens and older positions)
//! - Volume drops >65% from 24h average
//! - RSI < 35 and declining

use crate::db::DbPool;
use crate::engine::volume_cache::VolumeCache;
use crate::price_cache::PriceCache;
use parking_lot::RwLock;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::collections::HashMap;
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
    /// Grace period matching stop_loss.rs wick_protection_secs — price-drop check is suppressed
    /// during this window to avoid exiting on the entry-candle wick.
    wick_protection_secs: u64,
    /// High-water mark per trade UUID — tracks peak observed price for trailing-stop logic.
    high_water_marks: Arc<RwLock<HashMap<String, Decimal>>>,
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
    pub fn new(db: DbPool, price_cache: Arc<PriceCache>, wick_protection_secs: u64) -> Self {
        Self {
            db,
            price_cache,
            volume_cache: None,
            wick_protection_secs,
            high_water_marks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create with volume cache
    pub fn with_volume_cache(
        db: DbPool,
        price_cache: Arc<PriceCache>,
        volume_cache: Arc<VolumeCache>,
        wick_protection_secs: u64,
    ) -> Self {
        Self {
            db,
            price_cache,
            volume_cache: Some(volume_cache),
            wick_protection_secs,
            high_water_marks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Remove HWM state when a position is closed to prevent unbounded map growth.
    pub fn remove_position(&self, trade_uuid: &str) {
        self.high_water_marks.write().remove(trade_uuid);
    }

    /// Sweep stale HWM entries for positions that closed via paths other than
    /// `ProfitTargetManager::remove_position` (stop-loss, engine close, recovery).
    /// Returns the number of entries removed.
    pub async fn sweep_stale_entries(&self) -> usize {
        let active = match crate::db::get_active_trade_uuids(&self.db).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "HWM sweep: DB query failed, skipping");
                return 0;
            }
        };
        let active_set: std::collections::HashSet<String> = active.into_iter().collect();
        let mut map = self.high_water_marks.write();
        let before = map.len();
        map.retain(|uuid, _| active_set.contains(uuid));
        before - map.len()
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

        // Update high-water mark. The lock is held only for the map mutation; drop it
        // before the checks below to keep the hot path as short as possible.
        let hwm_snap = {
            let mut hwm_map = self.high_water_marks.write();
            let hwm = hwm_map.entry(trade_uuid.to_string()).or_insert(entry_price);
            if current_price > *hwm {
                *hwm = current_price;
            }
            *hwm
        };

        // Guard: corrupt position data — align with stop_loss.rs behavior
        if entry_price.is_zero() {
            tracing::error!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                "CORRUPT_POSITION: entry_price is zero in momentum_exit — forcing exit to recover capital"
            );
            return MomentumExitAction::Exit;
        }

        // Check 1: Price drops 8% from entry within 5 minutes (base threshold)
        let price_drop_percent = if !entry_price.is_zero() {
            let diff = entry_price - current_price;
            let ratio = diff / entry_price;
            ratio * Decimal::from(100)
        } else {
            Decimal::ZERO
        };
        let elapsed = entry_time.elapsed().unwrap_or_default();
        let elapsed_minutes = elapsed.as_secs() / 60;

        // Respect the same wick-protection grace period as stop_loss.rs.
        // A sharp single-candle wick immediately after entry should not trigger a momentum
        // exit when stop_loss.rs would ignore it. Volume and RSI checks are ungated because
        // they reflect genuine structural breakdown rather than a transient wick.
        let in_wick_window = elapsed.as_secs() < self.wick_protection_secs;

        if !in_wick_window {
            // RSI requires 16 samples at 20-second intervals (~5 min). Before RSI is
            // available, use a tighter base so new positions get equivalent protection.
            // Once RSI is active (≥6 min), widen to 8% to avoid false exits on normal
            // Solana intraday noise (30%+ daily vol).
            let base_drop_threshold = if elapsed_minutes < 6 {
                Decimal::from(5)
            } else {
                Decimal::from(8)
            };
            // Widen threshold for high-volatility tokens to avoid shakeout exits.
            // At 30% vol → 8+6=14%, at 50% vol → 8+10=18%, capped at 20%.
            // For positions held >5 min the threshold widens slightly (÷2 of elapsed hours,
            // max +5 pts) so long-held positions aren't exited on normal intraday noise.
            let price_drop_threshold = {
                let vol_bonus = if let Some(vol) = self.price_cache.calculate_volatility(token_address) {
                    let vol_dec = Decimal::from_f64_retain(vol).unwrap_or(Decimal::ZERO);
                    vol_dec * Decimal::from_str("0.2").unwrap_or(Decimal::ZERO)
                } else {
                    Decimal::ZERO
                };
                let age_bonus = if elapsed_minutes > 5 {
                    // Use f64 division to avoid the integer-division cliff where positions
                    // 5–59 minutes old get zero bonus but 60 minutes jumps to 0.5%.
                    let hours = Decimal::from_f64_retain(elapsed_minutes as f64 / 60.0)
                        .unwrap_or(Decimal::ZERO);
                    (hours / Decimal::from(2)).min(Decimal::from(5))
                } else {
                    Decimal::ZERO
                };
                (base_drop_threshold + vol_bonus + age_bonus).min(Decimal::from(20))
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
        } else {
            tracing::debug!(
                trade_uuid = %trade_uuid,
                elapsed_secs = elapsed.as_secs(),
                wick_protection_secs = self.wick_protection_secs,
                price_drop_percent = ?price_drop_percent,
                "Momentum price-drop check suppressed: within wick-protection window"
            );
        }

        // Check 2: Trailing stop from HWM.
        // Only activates once the position is ≥50% in profit (HWM ≥ 1.5× entry) so normal
        // early-stage volatility doesn't trigger it. Once active, exits if price falls 25%
        // from the peak — protecting unrealized gains that the entry-price check cannot.
        if !entry_price.is_zero() {
            let hwm_gain_pct = (hwm_snap - entry_price) / entry_price * dec!(100);
            if hwm_gain_pct >= dec!(50) {
                let drop_from_hwm = (hwm_snap - current_price) / hwm_snap * dec!(100);
                if drop_from_hwm >= dec!(25) {
                    let drop_f64 = drop_from_hwm.to_f64().unwrap_or(0.0);
                    let hwm_f64 = hwm_snap.to_f64().unwrap_or(0.0);
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        drop_from_hwm_pct = drop_f64,
                        high_water_mark = hwm_f64,
                        "Trailing stop hit: dropped from HWM"
                    );
                    return MomentumExitAction::Exit;
                }
            }
        }

        // Check 3: Volume drop (>65% from 24h average).
        // Gated to positions ≥5 minutes old: volume naturally dips 40–60% outside US trading
        // hours, and a freshly-opened position should not be immediately dumped on a pre-existing
        // low-volume condition that entry logic already accepted.
        let volume_check_ready = elapsed.as_secs() >= 300;
        if volume_check_ready {
            if let Some(ref volume_cache) = self.volume_cache {
                if volume_cache.has_volume_drop(token_address, Decimal::from(65)) {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        "Negative momentum detected: volume dropped >65% from 24h average"
                    );
                    return MomentumExitAction::Exit;
                }
            }
        }

        // Check 4: RSI declining (RSI < 35 and declining).
        // 40 triggered on normal pullbacks; 35 indicates genuine momentum breakdown.
        if let Some((current_rsi, previous_rsi)) = self.calculate_rsi(token_address).await {
            if current_rsi < 35.0 && current_rsi < previous_rsi {
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    current_rsi = current_rsi,
                    previous_rsi = previous_rsi,
                    "Negative momentum detected: RSI < 35 and declining"
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

        // Sample 16 price points at 20-second intervals (~5 min total window).
        // Solana memecoins can rug or peak within minutes; the previous 60-second
        // interval (15-min window) was too slow to catch liquidity events in time.
        const RSI_SAMPLE_INTERVAL_SECS: i64 = 20;
        let mut prices = Vec::new();
        let mut last_sampled_time: Option<chrono::DateTime<chrono::Utc>> = None;

        for (time, price) in token_history.iter().rev() {
            if let Some(last_time) = last_sampled_time {
                if last_time.signed_duration_since(*time).num_seconds() >= RSI_SAMPLE_INTERVAL_SECS {
                    prices.push(price.to_f64().unwrap_or(0.0));
                    last_sampled_time = Some(*time);
                }
            } else {
                // Always sample the most recent price
                prices.push(price.to_f64().unwrap_or(0.0));
                last_sampled_time = Some(*time);
            }

            if prices.len() >= 16 {
                break;
            }
        }

        if prices.len() < 16 {
            // Need at least 16 data points spanning ~5 minutes for current and previous 14-period RSI
            return None;
        }

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
