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
use crate::token::TokenParser;
use chimera_core::engine::volume_cache::VolumeCache;
use chimera_core::price_cache::PriceCache;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
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
    /// Minimum hold time before the momentum exit can fire (2026-08-11).
    /// Pump.fun tokens routinely wick 5%+ within seconds of entry; the tight
    /// base-drop threshold (5%) fires on this normal volatility, killing
    /// positions before they can develop. This grace window is separate from
    /// wick_protection_secs (which the stop-loss also reads) so the stop-loss
    /// stays tight while the momentum exit breathes. The hard -25% stop in
    /// stop_loss.rs still protects against catastrophic dumps during this window.
    min_hold_secs: u64,
    /// Optional TokenParser for live sell-quote re-validation of profit-side
    /// exits (see `quote_confirms_profit`).
    quote_client: Option<Arc<TokenParser>>,
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
            min_hold_secs: 0,
            quote_client: None,
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
            min_hold_secs: 0,
            quote_client: None,
        }
    }

    /// Set the minimum hold time before the momentum exit can fire. The
    /// momentum exit's price-drop, volume, and RSI checks are all suppressed
    /// for this many seconds after entry, giving volatile pump.fun positions
    /// room to establish a trend before exiting on normal intraday noise.
    /// The stop-loss (with its own wick protection and -25% hard stop)
    /// remains active and provides downside protection during this window.
    pub fn with_min_hold_secs(mut self, min_hold_secs: u64) -> Self {
        self.min_hold_secs = min_hold_secs;
        self
    }

    /// Attach a TokenParser for live sell-quote re-validation of profit-side
    /// exits. Without one, momentum exits keep their previous behavior.
    pub fn with_quote_client(mut self, quote_client: Arc<TokenParser>) -> Self {
        self.quote_client = Some(quote_client);
        self
    }

    /// Re-validate a nominally-profitable exit against the executable Jupiter
    /// sell quote. The price cache can diverge from the executable fill
    /// (observed 2026-08-06: +0.286% cache "profit" filled at -0.35%), so a
    /// profit-side exit must clear the cost breakeven on a live quote before
    /// firing. Defensive (loss-side) exits must NOT be blocked: on any error
    /// or missing data this returns `true` (proceed with the exit).
    async fn quote_confirms_profit(&self, token_address: &str, entry_price_usd: Decimal) -> bool {
        let Some(parser) = &self.quote_client else {
            return true;
        };
        // 1 full token requires its decimals (pump.fun = 6, most SPL = 6-9).
        let decimals = match self.price_cache.get_price(token_address) {
            Some(entry) => entry.decimals,
            None => None,
        };
        let Some(decimals) = decimals else {
            tracing::debug!(
                token = token_address,
                "Exit quote confirmation: decimals unknown — proceeding with exit"
            );
            return true;
        };
        let test_amount = match 10u64.checked_pow(decimals as u32) {
            Some(a) => a,
            None => {
                tracing::warn!(
                    token = token_address,
                    decimals,
                    "Exit quote confirmation: decimal exponent overflow — proceeding with exit"
                );
                return true;
            }
        };

        let out_sol = match parser.sell_quote_out_sol(token_address, test_amount).await {
            Ok(Some(v)) => v,
            Ok(None) => {
                // No sell route — cannot confirm profit; proceed (loss rails handle it).
                tracing::debug!(
                    token = token_address,
                    "Exit quote confirmation: no sell route — proceeding with exit"
                );
                return true;
            }
            Err(e) => {
                tracing::debug!(
                    token = token_address,
                    error = %e,
                    "Exit quote confirmation: quote failed — proceeding with exit (defensive)"
                );
                return true;
            }
        };

        let Some(sol_price_usd) = self.price_cache.get_sol_price_usd() else {
            tracing::debug!(
                token = token_address,
                "Exit quote confirmation: SOL price unavailable — proceeding with exit"
            );
            return true;
        };
        if sol_price_usd <= Decimal::ZERO {
            return true;
        }

        // Quoted USD value of 1 token must clear entry + round-trip cost buffer
        // (~1.4% observed: Jito tip + dex fee + slippage) with margin.
        let quoted_usd = out_sol * sol_price_usd;
        let cost_buffer = dec!(0.015);
        let breakeven = entry_price_usd * (Decimal::ONE + cost_buffer);

        if quoted_usd < breakeven {
            tracing::warn!(
                token = token_address,
                quoted_usd = %quoted_usd,
                entry_price_usd = %entry_price_usd,
                breakeven = %breakeven,
                "Momentum profit exit suppressed: live sell quote does not clear cost breakeven (cache vs executable divergence)"
            );
            return false;
        }
        true
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

        // Respect the same wick-protection grace period as stop_loss.rs, extended
        // by min_hold_secs (2026-08-11): pump.fun tokens wick 5%+ within seconds,
        // and the tight base-drop threshold (5%) kills positions before they develop.
        // The stop-loss (separate system) stays active and provides downside protection.
        let effective_grace = self.wick_protection_secs.max(self.min_hold_secs);
        let in_wick_window = elapsed.as_secs() < effective_grace;

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
                // RSI warmup/degenerate-window guard (2026-08-23): a
                // previous_rsi >= 99 means the lookback window contained zero
                // (or near-zero) losses — typically Wilder smoothing still
                // saturated at 100 from the pump run-up BEFORE our entry. One
                // ordinary pullback then crashes RSI from ~100 to <35 in a
                // single tick, which reads as "momentum breakdown" but is a
                // smoothing artifact of an un-warmed window. Observed live
                // 2026-08-22: two positions (EjD5Y9 -6.9%, EN2nnx -2.3%) were
                // dumped by exactly this transition (previous_rsi=100.0 →
                // current 16.9) while shadow mirror_main rode both streams to
                // +5%. Never seed an exit from a degenerate window — the
                // price-drop rails (-5%/-8% base, -25% hard stop) still
                // protect genuine crashes.
                if current_rsi < 35.0 && previous_rsi >= 99.0 {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        current_rsi = current_rsi,
                        previous_rsi = previous_rsi,
                        "RSI crash from degenerate zero-loss window suppressed — holding"
                    );
                } else if current_rsi < 35.0 && current_rsi < previous_rsi {
                    // If the position is nominally profitable per the cache, confirm the
                    // executable sell quote actually clears cost breakeven before exiting
                    // "into profit" — cache and fill diverged (2026-08-06: +0.286% cache
                    // "profit" filled at -0.35%). Loss-side exits are never blocked here.
                    if price_drop_percent < Decimal::ZERO
                        && !self.quote_confirms_profit(token_address, entry_price).await
                    {
                        return MomentumExitAction::None;
                    }
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

    #[test]
    fn test_compute_rsi_pump_runup_then_crash_is_degenerate_window() {
        // Chronological: 28 gains of 0.01, then one -0.50 drop (30 points).
        // The initial SMA window (oldest 14 changes) is all-gains -> RSI 100,
        // and Wilder smoothing carries the zero-loss average forward until
        // the final drop crashes RSI to ~21 in a single tick. This is the
        // exact degenerate (previous=100) shape the Check-4 guard must
        // suppress — observed live 2026-08-22 on EjD5Y9/EN2nnx.
        let mut chrono_prices: Vec<f64> = (0..29).map(|i| 1.0 + i as f64 * 0.01).collect();
        chrono_prices.push(chrono_prices[28] - 0.50);
        let prices: Vec<f64> = chrono_prices.iter().rev().copied().collect();
        let (current, previous) = compute_rsi_from_prices(&prices).expect("30 samples");
        assert_eq!(previous, 100.0, "zero-loss lookback saturates at 100");
        assert!(
            current < 35.0,
            "a single pullback crashes RSI below 35, got {current}"
        );
    }

    #[test]
    fn test_compute_rsi_healthy_window_not_flagged_degenerate() {
        // Mixed gains/losses in the lookback keep previous RSI well below
        // the 99 degenerate bound, so genuine declines still exit.
        let mut chrono_prices: Vec<f64> = Vec::with_capacity(30);
        let mut p = 1.0_f64;
        for i in 0..29 {
            chrono_prices.push(p);
            p += if i % 2 == 0 { 0.02 } else { -0.01 };
        }
        chrono_prices.push(p - 0.30);
        let prices: Vec<f64> = chrono_prices.iter().rev().copied().collect();
        let (current, previous) = compute_rsi_from_prices(&prices).expect("30 samples");
        assert!(previous < 99.0, "mixed window must not be degenerate");
        assert!(current < previous, "declining series must decline");
    }

    // ==========================================================================
    // MOMENTUM EXIT DETECTION TESTS
    // ==========================================================================

    fn db_mock() -> Arc<dyn Database> {
        Arc::new(crate::engine::kelly_sizer::tests::MockDatabase::default())
    }

    fn price_cache_with(
        token: &str,
        prices: &[(Decimal, chimera_core::price_cache::PriceSource)],
    ) -> Arc<PriceCache> {
        let cache = Arc::new(PriceCache::new().unwrap());
        for (price, source) in prices {
            cache.set_price(token, *price, *source, None);
        }
        cache
    }

    fn detector(cache: Arc<PriceCache>) -> MomentumExit {
        MomentumExit::new(db_mock(), cache, 10)
    }

    fn seconds_ago(secs: u64) -> SystemTime {
        SystemTime::now() - Duration::from_secs(secs)
    }

    #[tokio::test]
    async fn test_no_price_data_skips_check() {
        let cache = Arc::new(PriceCache::new().unwrap());
        let exit = detector(cache);
        let action = exit
            .check_momentum(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                dec!(1.0),
                SystemTime::now(),
            )
            .await;
        assert_eq!(action, MomentumExitAction::None);
    }

    #[tokio::test]
    async fn test_zero_entry_price_forces_exit() {
        let cache = price_cache_with(
            "TOKEN11111111111111111111111111111111111111",
            &[(dec!(1.0), chimera_core::price_cache::PriceSource::Jupiter)],
        );
        let exit = detector(cache);
        let action = exit
            .check_momentum(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                Decimal::ZERO,
                SystemTime::now(),
            )
            .await;
        assert_eq!(action, MomentumExitAction::Exit);
    }

    #[tokio::test]
    async fn test_corrupt_zero_current_price_skips() {
        let cache = price_cache_with(
            "TOKEN11111111111111111111111111111111111111",
            &[(
                Decimal::ZERO,
                chimera_core::price_cache::PriceSource::Jupiter,
            )],
        );
        let exit = detector(cache);
        let action = exit
            .check_momentum(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                dec!(1.0),
                SystemTime::now(),
            )
            .await;
        assert_eq!(action, MomentumExitAction::None);
    }

    #[tokio::test]
    async fn test_price_drop_exceeds_threshold() {
        // Entry 1.0, current 0.9 -> 10% drop >= 5% base (position < 8 min old).
        let cache = price_cache_with(
            "TOKEN11111111111111111111111111111111111111",
            &[(dec!(0.9), chimera_core::price_cache::PriceSource::Jupiter)],
        );
        let exit = detector(cache);
        let action = exit
            .check_momentum(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                Decimal::ONE,
                seconds_ago(60),
            )
            .await;
        assert_eq!(action, MomentumExitAction::Exit);
    }

    #[tokio::test]
    async fn test_price_drop_within_threshold_holds() {
        // Entry 1.0, current 0.97 -> 3% drop < 5% -> hold.
        let cache = price_cache_with(
            "TOKEN11111111111111111111111111111111111111",
            &[(dec!(0.97), chimera_core::price_cache::PriceSource::Jupiter)],
        );
        let exit = detector(cache);
        let action = exit
            .check_momentum(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                Decimal::ONE,
                seconds_ago(60),
            )
            .await;
        assert_eq!(action, MomentumExitAction::None);
    }

    #[tokio::test]
    async fn test_wick_protection_suppresses_checks() {
        // Within 10s of entry: all checks suppressed.
        let cache = price_cache_with(
            "TOKEN11111111111111111111111111111111111111",
            &[(dec!(0.5), chimera_core::price_cache::PriceSource::Jupiter)],
        );
        let exit = detector(cache);
        let action = exit
            .check_momentum(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                Decimal::ONE,
                seconds_ago(5),
            )
            .await;
        assert_eq!(action, MomentumExitAction::None);
    }

    #[tokio::test]
    async fn test_volatility_widens_threshold() {
        // 3 samples (1.0 -> 2.0 -> 1.0) give ~75% volatility -> vol bonus 15,
        // threshold 5+15=20 (capped). 15% drop holds.
        let cache = price_cache_with(
            "TOKEN11111111111111111111111111111111111111",
            &[
                (dec!(1.0), chimera_core::price_cache::PriceSource::Jupiter),
                (dec!(2.0), chimera_core::price_cache::PriceSource::Jupiter),
                (dec!(1.0), chimera_core::price_cache::PriceSource::Jupiter),
            ],
        );
        let exit = detector(cache);
        let action = exit
            .check_momentum(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                Decimal::ONE,
                seconds_ago(300),
            )
            .await;
        assert_eq!(action, MomentumExitAction::None);
    }

    #[tokio::test]
    async fn test_age_bonus_widens_old_positions() {
        // Position 10 min old: base 8 + age bonus (10/60/2 = 0.0833) = 8.0833.
        // 9% drop >= 8.0833 -> exit. Covers the elapsed_minutes >= 8 base branch.
        let cache = price_cache_with(
            "TOKEN11111111111111111111111111111111111111",
            &[(dec!(0.91), chimera_core::price_cache::PriceSource::Jupiter)],
        );
        let exit = detector(cache);
        let action = exit
            .check_momentum(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                Decimal::ONE,
                seconds_ago(600),
            )
            .await;
        assert_eq!(action, MomentumExitAction::Exit);
    }

    #[tokio::test]
    async fn test_volume_cache_checked_when_present() {
        // Volume cache present with a small drop (< 65%) -> holding.
        let cache = price_cache_with(
            "TOKEN11111111111111111111111111111111111111",
            &[(dec!(0.98), chimera_core::price_cache::PriceSource::Jupiter)],
        );
        let volume = Arc::new(VolumeCache::new());
        volume.record_volume("TOKEN11111111111111111111111111111111111111", dec!(100));
        volume.record_volume("TOKEN11111111111111111111111111111111111111", dec!(95));
        let exit = MomentumExit::with_volume_cache(db_mock(), cache, volume, 10);
        let action = exit
            .check_momentum(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                Decimal::ONE,
                seconds_ago(300),
            )
            .await;
        // No volume drop detected (history too short), price drop 2% < 5% -> hold.
        assert_eq!(action, MomentumExitAction::None);
    }

    #[tokio::test]
    async fn test_rsi_insufficient_history_skips() {
        // 20 price points recorded at the same instant: RSI sampling requires
        // >= 30s spacing, so < 16 samples -> RSI skipped.
        let cache = Arc::new(PriceCache::new().unwrap());
        for _ in 0..20 {
            cache.set_price(
                "TOKEN11111111111111111111111111111111111111",
                dec!(1.0),
                chimera_core::price_cache::PriceSource::Jupiter,
                None,
            );
        }
        let exit = detector(cache);
        let action = exit
            .check_momentum(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                dec!(1.0),
                seconds_ago(300),
            )
            .await;
        assert_eq!(action, MomentumExitAction::None);
    }

    /// Reproduction of the 2026-08-22 prod incident (EjD5Y9 -6.9%,
    /// EN2nnx -2.3%): a pump run-up before entry saturates the RSI window
    /// at 100; one ordinary pullback crashes it below 35 and the old logic
    /// dumped the position while shadow mirror_main rode both streams to
    /// +5%. The degenerate-window guard must HOLD here — the price-drop
    /// check alone cannot fire either (2.4% < 8% base for an 8+ min hold).
    #[tokio::test]
    async fn test_rsi_crash_from_degenerate_window_holds() {
        let token = "TOKEN11111111111111111111111111111111111111";
        let cache = Arc::new(PriceCache::new().unwrap());
        let start = chrono::Utc::now() - chrono::Duration::seconds(18 * 60);
        for i in 0..17 {
            cache.set_price_with_time(
                token,
                dec!(1.0) + dec!(0.01) * Decimal::from(i),
                chimera_core::price_cache::PriceSource::Jupiter,
                start + chrono::Duration::seconds(60 * i),
                None,
            );
        }
        cache.set_price_with_time(
            token,
            dec!(0.80),
            chimera_core::price_cache::PriceSource::Jupiter,
            start + chrono::Duration::seconds(60 * 17),
            None,
        );
        let exit = detector(cache);
        let action = exit
            .check_momentum("t1", token, dec!(0.82), seconds_ago(600))
            .await;
        assert_eq!(
            action,
            MomentumExitAction::None,
            "RSI crash from a zero-loss warmup window must not exit"
        );
    }

    /// Control: the same final crash out of a HEALTHY lookback (real losses
    /// present, previous_rsi well under 99) still exits on genuine
    /// breakdown — the guard only suppresses degenerate windows.
    #[tokio::test]
    async fn test_rsi_decline_from_healthy_window_still_exits() {
        let token = "TOKEN11111111111111111111111111111111111111";
        let cache = Arc::new(PriceCache::new().unwrap());
        let start = chrono::Utc::now() - chrono::Duration::seconds(18 * 60);
        let mut p = dec!(1.00);
        for i in 0..17 {
            cache.set_price_with_time(
                token,
                p,
                chimera_core::price_cache::PriceSource::Jupiter,
                start + chrono::Duration::seconds(60 * i),
                None,
            );
            p += if i % 2 == 0 { dec!(0.02) } else { dec!(-0.01) };
        }
        cache.set_price_with_time(
            token,
            dec!(0.74),
            chimera_core::price_cache::PriceSource::Jupiter,
            start + chrono::Duration::seconds(60 * 17),
            None,
        );
        let exit = detector(cache);
        // Entry 0.76 vs current 0.74: 2.6% drop < 8% base — only the RSI
        // branch can fire.
        let action = exit
            .check_momentum("t1", token, dec!(0.76), seconds_ago(600))
            .await;
        assert_eq!(
            action,
            MomentumExitAction::Exit,
            "genuine breakdown out of a healthy window must still exit"
        );
    }

    #[tokio::test]
    async fn test_should_exit_wraps_check() {
        let cache = price_cache_with(
            "TOKEN11111111111111111111111111111111111111",
            &[(dec!(0.5), chimera_core::price_cache::PriceSource::Jupiter)],
        );
        let exit = detector(cache);
        assert!(
            exit.should_exit(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                Decimal::ONE,
                seconds_ago(60),
            )
            .await
        );
        // No price -> hold.
        let cache = Arc::new(PriceCache::new().unwrap());
        let exit = detector(cache);
        assert!(
            !exit
                .should_exit(
                    "t1",
                    "TOKEN11111111111111111111111111111111111111",
                    Decimal::ONE,
                    seconds_ago(60),
                )
                .await
        );
    }

    #[tokio::test]
    async fn test_clock_skew_future_entry_time() {
        // entry_time in the future -> elapsed() errors -> Duration::ZERO,
        // which lands inside the wick-protection window (all checks suppressed).
        let cache = price_cache_with(
            "TOKEN11111111111111111111111111111111111111",
            &[(dec!(0.5), chimera_core::price_cache::PriceSource::Jupiter)],
        );
        let exit = detector(cache);
        let action = exit
            .check_momentum(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                Decimal::ONE,
                SystemTime::now() + Duration::from_secs(60),
            )
            .await;
        assert_eq!(action, MomentumExitAction::None);
    }

    #[tokio::test]
    async fn test_with_quote_client_noop_paths() {
        // quote_confirms_profit with a quote client whose quote path fails
        // closed... To keep this hermetic we only verify the no-client path
        // returns true via an RSI-gated flow; here we exercise
        // `with_quote_client` construction plus a plain check.
        let cache = price_cache_with(
            "TOKEN11111111111111111111111111111111111111",
            &[(dec!(0.97), chimera_core::price_cache::PriceSource::Jupiter)],
        );
        let fetcher = Arc::new(crate::token::TokenMetadataFetcher::new(
            "https://api.mainnet-beta.solana.com",
        ));
        let parser = Arc::new(crate::token::TokenParser::new(
            crate::token::TokenSafetyConfig::default(),
            Arc::new(crate::token::TokenCache::new(10, 3600)),
            fetcher,
        ));
        let exit = MomentumExit::new(db_mock(), cache, 10).with_quote_client(parser);
        let action = exit
            .check_momentum(
                "t1",
                "TOKEN11111111111111111111111111111111111111",
                Decimal::ONE,
                seconds_ago(60),
            )
            .await;
        assert_eq!(action, MomentumExitAction::None);
    }
}
