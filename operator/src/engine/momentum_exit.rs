//! Momentum-Based Early Exit Detection
//!
//! Detects negative momentum indicators and triggers early exit:
//! - Price drops 5%+ from entry within 8 minutes (8%+ after — widened for
//!   high-volatility tokens and older positions)
//! - Volume drops >65% from 24h average
//! - RSI < 35 and declining
//!
//! # Coverage note
//! There is a known gap between wick_protection_secs (default 10s) and RSI readiness
//! (requires 16 price samples at 30s intervals = ~8 minutes of price data after entry).
//! During seconds 10-480 after entry, neither the wick grace period nor RSI momentum
//! exit is active. The hard stop-loss at -25% (bypassing wick protection) and the
//! primary stop-loss threshold provide coverage throughout this gap. RSI is a
//! secondary, not primary, defense.

use crate::db_abstraction::Database;
use crate::engine::volume_cache::VolumeCache;
use crate::price_cache::PriceCache;
use rust_decimal::prelude::*;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

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
    db: Arc<dyn Database>,
    price_cache: Arc<PriceCache>,
    volume_cache: Option<Arc<VolumeCache>>,
    /// Grace period matching stop_loss.rs wick_protection_secs — price-drop check is suppressed
    /// during this window to avoid exiting on the entry-candle wick.
    wick_protection_secs: u64,
}

impl MomentumExit {
    /// Create a new momentum exit detector
    pub fn new(
        db: Arc<dyn Database>,
        price_cache: Arc<PriceCache>,
        wick_protection_secs: u64,
    ) -> Self {
        Self {
            db,
            price_cache,
            volume_cache: None,
            wick_protection_secs,
        }
    }

    /// Create with volume cache
    pub fn with_volume_cache(
        db: Arc<dyn Database>,
        price_cache: Arc<PriceCache>,
        volume_cache: Arc<VolumeCache>,
        wick_protection_secs: u64,
    ) -> Self {
        Self {
            db,
            price_cache,
            volume_cache: Some(volume_cache),
            wick_protection_secs,
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
            Some(price) => {
                // Check staleness even when price is cached — aligns with stop_loss.rs
                // staleness guard. Both modules must agree on escalation.
                if self.price_cache.is_price_stale(token_address) {
                    tracing::error!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        "STALE_PRICE: cached price is stale (>90s old) — momentum exit forcing exit"
                    );
                    return MomentumExitAction::Exit;
                }
                price
            }
            None => {
                // §1.5 FIX: If this token is actively tracked but hasn't received a
                // price update in >90s, force exit. Aligns with stop_loss.rs
                // staleness guard — both modules must agree on escalation.
                if self.price_cache.is_price_stale(token_address) {
                    tracing::error!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        "STALE_PRICE: no price update for >90s on tracked token — momentum exit forcing exit"
                    );
                    return MomentumExitAction::Exit;
                }
                tracing::debug!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    "Momentum exit: no price data — skipping check"
                );
                return MomentumExitAction::None; // No price data, skip check
            }
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

        // Guard: corrupt price data — skip the momentum check rather than
        // converting a zero/negative cache value into a 100%+ drop that forces
        // an exit (the opposite of the intent behind the entry_price guard).
        if current_price <= Decimal::ZERO {
            tracing::warn!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                current_price = ?current_price,
                "CORRUPT_PRICE: current_price is zero/negative — skipping momentum check"
            );
            return MomentumExitAction::None;
        }

        // Check 1: Price drops 5% from entry within 8 minutes (base threshold)
        let price_drop_percent = if !entry_price.is_zero() {
            let diff = entry_price - current_price;
            let ratio = diff / entry_price;
            ratio * Decimal::from(100)
        } else {
            Decimal::ZERO
        };
        // Clock-skew guard: `entry_time.elapsed()` errors when the entry
        // timestamp is in the future. A zero elapsed time would silently
        // suppress the price-drop/volume/RSI checks until the clock catches
        // up — surface it instead so momentum protection cannot be silently
        // disabled.
        let elapsed = match entry_time.elapsed() {
            Ok(d) => d,
            Err(e) => {
                tracing::error!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    error = ?e,
                    "CLOCK_SKEW: entry_time is in the future — momentum checks suppressed"
                );
                Duration::ZERO
            }
        };
        let elapsed_minutes = elapsed.as_secs() / 60;

        // Respect the same wick-protection grace period as stop_loss.rs.
        // A sharp single-candle wick immediately after entry should not trigger a momentum
        // exit when stop_loss.rs would ignore it. Volume and RSI checks are likewise
        // gated behind wick protection — during the first wick_protection_secs after entry,
        // they are unreliable indicators of structural breakdown.
        let in_wick_window = elapsed.as_secs() < self.wick_protection_secs;

        if !in_wick_window {
            // RSI requires 16 samples at 30-second intervals (~8 min). Before RSI is
            // available, use a tighter base so new positions get equivalent protection.
            // Once RSI is active (≥8 min), widen to 8% to avoid false exits on normal
            // Solana intraday noise (30%+ daily vol).
            let base_drop_threshold = if elapsed_minutes < 8 {
                Decimal::from(5)
            } else {
                Decimal::from(8)
            };
            // Widen threshold for high-volatility tokens to avoid shakeout exits.
            // At 30% vol → 8+6=14%, at 50% vol → 8+10=18%, capped at 20%.
            // For positions held >5 min the threshold widens slightly (÷2 of elapsed hours,
            // max +5 pts) so long-held positions aren't exited on normal intraday noise.
            let (vol_bonus, age_bonus) = {
                let vol_bonus =
                    if let Some(vol) = self.price_cache.calculate_volatility(token_address) {
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
                (vol_bonus, age_bonus)
            };
            let price_drop_threshold =
                (base_drop_threshold + vol_bonus + age_bonus).min(Decimal::from(20));
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
            tracing::debug!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                price_drop_percent = ?price_drop_percent,
                price_drop_threshold = ?price_drop_threshold,
                base_drop_threshold = ?base_drop_threshold,
                vol_bonus = ?vol_bonus,
                age_bonus = ?age_bonus,
                elapsed_minutes = elapsed_minutes,
                "Momentum price-drop check passed — holding"
            );
        } else {
            tracing::debug!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                elapsed_secs = elapsed.as_secs(),
                wick_protection_secs = self.wick_protection_secs,
                price_drop_percent = ?price_drop_percent,
                "Momentum price-drop check suppressed: within wick-protection window"
            );
        }

        // Check 3: Volume drop (>65% from 24h average).
        // Gated to positions ≥5 minutes old: volume naturally dips 40–60% outside US trading
        // hours, and a freshly-opened position should not be immediately dumped on a pre-existing
        // low-volume condition that entry logic already accepted.
        // Also gated behind wick protection: during the first wick_protection_secs after entry,
        // volume and RSI are unreliable indicators of structural breakdown.
        let volume_check_ready = elapsed.as_secs() >= 300 && !in_wick_window;
        if volume_check_ready {
            if let Some(ref volume_cache) = self.volume_cache {
                let volume_drop_threshold = Decimal::from(65);
                let volume_drop_percent = match (
                    volume_cache.get_24h_average_volume(token_address),
                    volume_cache.get_current_volume(token_address),
                ) {
                    (Some(avg), Some(cur)) if avg > Decimal::ZERO => {
                        Some(((avg - cur) / avg) * Decimal::from(100))
                    }
                    _ => None,
                };
                if volume_cache.has_volume_drop(token_address, volume_drop_threshold) {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        volume_drop_percent = ?volume_drop_percent,
                        volume_drop_threshold = %volume_drop_threshold,
                        "Negative momentum detected: volume dropped >65% from 24h average"
                    );
                    return MomentumExitAction::Exit;
                }
                tracing::debug!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    volume_drop_percent = ?volume_drop_percent,
                    volume_drop_threshold = %volume_drop_threshold,
                    "Momentum volume-drop check passed — holding"
                );
            }
        }

        // Check 4: RSI declining (RSI < 35 and declining).
        // 40 triggered on normal pullbacks; 35 indicates genuine momentum breakdown.
        // Also gated behind wick protection: RSI < 35 within the first wick_protection_secs
        // after entry may reflect normal post-entry price action, not genuine breakdown.
        if !in_wick_window {
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
                tracing::debug!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    current_rsi = current_rsi,
                    previous_rsi = previous_rsi,
                    rsi_threshold = 35.0_f64,
                    "Momentum RSI check passed — holding"
                );
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
        let history = self.price_cache.price_history_read();
        let token_history = history.get(token_address)?;

        // Sample up to 30 price points at 30-second intervals (~15 min total window)
        // to allow the RSI EMA (Wilder's smoothing) to warm up properly.
        // Use 30-second intervals to match the price cache update frequency (~5 sec)
        // and avoid consecutive samples using the same price data point, which produces
        // an artificially smooth RSI that under-reacts to actual price movements.
        const RSI_SAMPLE_INTERVAL_SECS: i64 = 30;
        let mut prices = Vec::new();
        let mut last_sampled_time: Option<chrono::DateTime<chrono::Utc>> = None;

        let mut sorted_history: Vec<_> = token_history.iter().collect();
        sorted_history.sort_by_key(|(t, _)| *t);

        // Iterate newest-first (rev) so each new sample is at least RSI_SAMPLE_INTERVAL_SECS
        // before the PREVIOUSLY sampled point. The resulting `prices` vec is newest-first:
        //   prices[0] = most recent, prices[len-1] = oldest.
        // compute_rsi_from_prices() expects this order and reverses internally to produce
        // chronological change deltas. Both directions are intentional and must stay in sync.
        for (time, price) in sorted_history.iter().rev() {
            if let Some(last_time) = last_sampled_time {
                if last_time.signed_duration_since(*time).num_seconds() >= RSI_SAMPLE_INTERVAL_SECS
                {
                    let price_f64 = price.to_f64().unwrap_or(0.0);
                    // If the Decimal price is non-zero but f64 is zero, precision was
                    // lost — RSI computed from garbage data is worse than no RSI at all.
                    if !price.is_zero() && price_f64 == 0.0 {
                        tracing::debug!(
                            token_address = token_address,
                            "Skipping RSI: price too small for f64 precision"
                        );
                        return None;
                    }
                    prices.push(price_f64);
                    last_sampled_time = Some(*time);
                }
            } else {
                let price_f64 = price.to_f64().unwrap_or(0.0);
                if !price.is_zero() && price_f64 == 0.0 {
                    tracing::debug!(
                        token_address = token_address,
                        "Skipping RSI: price too small for f64 precision"
                    );
                    return None;
                }
                prices.push(price_f64);
                last_sampled_time = Some(*time);
            }

            if prices.len() >= 30 {
                break;
            }
        }

        if prices.len() < 16 {
            // A 14-period Wilder RSI needs 15 changes (=16 prices) to produce
            // both a previous and current RSI value; at 30s sampling this is
            // ~8 minutes. The tighter 5% price-drop threshold applies until
            // then (see check_momentum).
            return None;
        }

        // Compute current and previous RSI in a single pass to ensure Wilder's smoothing continuity
        compute_rsi_from_prices(&prices)
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
        match self
            .check_momentum(trade_uuid, token_address, entry_price, entry_time)
            .await
        {
            MomentumExitAction::Exit => {
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    "Momentum exit check: exit signal confirmed"
                );
                true
            }
            MomentumExitAction::None => {
                tracing::debug!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    "Momentum exit check passed — holding"
                );
                false
            }
        }
    }
}

/// Helper function to calculate RSI from a slice of prices
fn compute_rsi_from_prices(prices: &[f64]) -> Option<(f64, f64)> {
    if prices.len() < 16 {
        return None;
    }

    // prices are newest at index 0, oldest at index len-1
    // We need to calculate changes going FORWARD in time (oldest to newest)
    let mut changes = Vec::with_capacity(prices.len() - 1);
    for i in (1..prices.len()).rev() {
        let change = prices[i - 1] - prices[i];
        changes.push(change);
    }

    // Calculate initial SMA using the first 14 periods (the oldest 14 changes)
    let mut avg_gain = 0.0;
    let mut avg_loss = 0.0;
    for change in &changes[0..14] {
        if *change > 0.0 {
            avg_gain += change;
        } else {
            avg_loss += change.abs();
        }
    }
    avg_gain /= 14.0;
    avg_loss /= 14.0;

    let calc_rsi = |gain: f64, loss: f64| -> f64 {
        if loss == 0.0 {
            return 100.0;
        }
        let rs = gain / loss;
        100.0 - (100.0 / (1.0 + rs))
    };

    let mut previous_rsi = calc_rsi(avg_gain, avg_loss);
    let mut current_rsi = previous_rsi;

    // Apply Wilder's Smoothing for the remaining periods
    for change in &changes[14..] {
        previous_rsi = current_rsi;

        let mut gain = 0.0;
        let mut loss = 0.0;
        if *change > 0.0 {
            gain = *change;
        } else {
            loss = change.abs();
        }
        avg_gain = (avg_gain * 13.0 + gain) / 14.0;
        avg_loss = (avg_loss * 13.0 + loss) / 14.0;

        current_rsi = calc_rsi(avg_gain, avg_loss);
    }

    Some((current_rsi, previous_rsi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_rsi_monotonic_rise_is_100() {
        // Newest-first prices that rise from 1.0 (oldest) to 1.29 (newest):
        // every change is a gain → RSI = 100.
        let prices: Vec<f64> = (0..30).rev().map(|i| 1.0 + i as f64 * 0.01).collect();
        let (current, previous) = compute_rsi_from_prices(&prices).expect("rsi from 30 samples");
        assert_eq!(current, 100.0);
        assert_eq!(previous, 100.0);
    }

    #[test]
    fn test_compute_rsi_monotonic_fall_is_0() {
        // Newest-first prices that fall from 1.29 (oldest) to 1.0 (newest):
        // every change is a loss → RSI = 0.
        let prices: Vec<f64> = (0..30).map(|i| 1.0 + i as f64 * 0.01).collect();
        let (current, previous) = compute_rsi_from_prices(&prices).expect("rsi from 30 samples");
        assert_eq!(current, 0.0);
        assert_eq!(previous, 0.0);
    }

    #[test]
    fn test_compute_rsi_requires_16_samples() {
        assert!(compute_rsi_from_prices(&[1.0; 15]).is_none());
        assert!(compute_rsi_from_prices(&[1.0; 16]).is_some());
    }
}
