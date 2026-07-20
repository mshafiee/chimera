//! Tiered profit targets with trailing stops
//!
//! Implements:
//! - Tiered exits (sell 25% at each target)
//! - Trailing stops (after +50%, set trailing stop at -20% from peak)
//! - Time-based exits (auto-exit after 24h if profitable)

use crate::config::ProfitManagementConfig;
use crate::db_abstraction::Database;
use crate::engine::market_regime::MarketRegimeDetector;
use crate::engine::momentum_exit::MomentumExit;
use crate::price_cache::PriceCache;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use serde_json;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

/// Profit target state
pub struct ProfitTargetManager {
    db: Arc<dyn Database>,
    config: Arc<ProfitManagementConfig>,
    price_cache: Arc<PriceCache>,
    /// Active profit targets by trade UUID
    active_targets: Arc<RwLock<std::collections::HashMap<String, ProfitTargetState>>>,
    /// Momentum exit detector (optional)
    momentum_exit: Option<Arc<MomentumExit>>,
    /// Market regime detector (optional)
    market_regime: Option<Arc<MarketRegimeDetector>>,
}

/// Profit target state for a position
#[derive(Debug, Clone)]
struct ProfitTargetState {
    #[allow(dead_code)]
    trade_uuid: String,
    entry_price: Decimal,
    #[allow(dead_code)]
    entry_amount_sol: Decimal,
    current_price: Decimal,
    peak_price: Decimal,
    peak_profit_percent: Decimal,
    targets_hit: Vec<usize>, // Which target indices have been hit (index-based, not value-based)
    trailing_stop_active: bool,
    trailing_stop_price: Decimal,
    entry_time: SystemTime,
    /// Tracks the cumulative fraction of the original position still held after tiered exits.
    /// Starts at 1.0 and is multiplied by (1 - tiered_exit_percent/100) for each tier hit.
    /// Used by the dust check to compute actual remaining position, not entry_amount_sol.
    remaining_fraction: Decimal,
    /// Volatility scale captured at registration time (if data was available).
    /// Used as a fallback when calculate_volatility returns None mid-session.
    initial_vol_scale: Option<Decimal>,
    /// Number of check_targets ticks since position registration.
    /// Used for the cold-start ramp (see VOL_RAMP_TICKS).
    ticks_since_entry: u32,
    /// Last vol_scale value logged — used to suppress log flooding.
    last_logged_vol_scale: Option<Decimal>,
}

/// Profit target action
#[derive(Debug, Clone)]
pub enum ProfitTargetAction {
    /// No action needed
    None,
    /// Sell an absolute SOL amount of the current remaining position.
    /// Using absolute SOL (not a percentage) eliminates ambiguity about whether
    /// the percentage applies to the original or remaining position size.
    ExitAmount(Decimal),
    /// Full exit
    FullExit,
}

/// Number of 5-second ticks over which to ramp the volatility scale from 1.0
/// to the measured value. Prevents a sudden target snap when volatility data
/// first becomes available (~2 min after position open at 5s intervals).
const VOL_RAMP_TICKS: u32 = 60;

/// Compute the volatility scale factor for profit targets.
///
/// Returns a value in `[0, 1]`:
/// - High volatility (>= threshold): returns 1.0 (use full targets).
/// - Low volatility: returns `vol / threshold` (proportionally smaller targets).
/// - No data: returns 1.0 (safe default — full targets for unknown tokens).
///
/// The cold-start ramp smooths the transition from 1.0 to the measured scale
/// over the first `VOL_RAMP_TICKS` ticks after position registration.
fn compute_vol_scale(
    volatility: Option<f64>,
    threshold: Decimal,
    ticks_since_entry: u32,
    initial_vol_scale: Option<Decimal>,
) -> Decimal {
    let raw_scale = match volatility {
        Some(vol) => {
            if threshold.is_zero() {
                return Decimal::ONE;
            }
            let vol_dec = Decimal::from_str(&format!("{:.4}", vol))
                .unwrap_or(Decimal::ZERO);
            // Clamp at zero: a negative volatility reading (shouldn't happen but
            // could from corrupted data) must never produce a negative scale.
            ((vol_dec / threshold).min(Decimal::ONE)).max(Decimal::ZERO)
        }
        None => {
            // No live volatility — use initial estimate if available, else full scale
            match initial_vol_scale {
                Some(init) => init,
                None => return Decimal::ONE,
            }
        }
    };

    // Cold-start ramp: smoothly transition from 1.0 to raw_scale over VOL_RAMP_TICKS.
    // If initial_vol_scale was set (data existed at registration), skip the ramp.
    if initial_vol_scale.is_some() || ticks_since_entry >= VOL_RAMP_TICKS {
        return raw_scale;
    }

    let ramp_progress = Decimal::from(ticks_since_entry) / Decimal::from(VOL_RAMP_TICKS);
    // effective_scale = 1.0 - (1.0 - raw_scale) * ramp_progress
    let effective = Decimal::ONE - (Decimal::ONE - raw_scale) * ramp_progress;
    effective.min(Decimal::ONE)
}

impl ProfitTargetManager {
    pub fn new(
        db: Arc<dyn Database>,
        config: Arc<ProfitManagementConfig>,
        price_cache: Arc<PriceCache>,
    ) -> Self {
        Self {
            db,
            config,
            price_cache,
            active_targets: Arc::new(RwLock::new(std::collections::HashMap::new())),
            momentum_exit: None,
            market_regime: None,
        }
    }

    /// Create with momentum exit detector
    pub fn with_momentum_exit(
        db: Arc<dyn Database>,
        config: Arc<ProfitManagementConfig>,
        price_cache: Arc<PriceCache>,
        momentum_exit: Arc<MomentumExit>,
    ) -> Self {
        Self {
            db,
            config,
            price_cache,
            active_targets: Arc::new(RwLock::new(std::collections::HashMap::new())),
            momentum_exit: Some(momentum_exit),
            market_regime: None,
        }
    }

    /// Create with market regime detector
    pub fn with_market_regime(
        db: Arc<dyn Database>,
        config: Arc<ProfitManagementConfig>,
        price_cache: Arc<PriceCache>,
        market_regime: Arc<MarketRegimeDetector>,
    ) -> Self {
        Self {
            db,
            config,
            price_cache,
            active_targets: Arc::new(RwLock::new(std::collections::HashMap::new())),
            momentum_exit: None,
            market_regime: Some(market_regime),
        }
    }

    /// Create with both momentum exit and market regime
    pub fn with_extras(
        db: Arc<dyn Database>,
        config: Arc<ProfitManagementConfig>,
        price_cache: Arc<PriceCache>,
        momentum_exit: Option<Arc<MomentumExit>>,
        market_regime: Option<Arc<MarketRegimeDetector>>,
    ) -> Self {
        Self {
            db,
            config,
            price_cache,
            active_targets: Arc::new(RwLock::new(std::collections::HashMap::new())),
            momentum_exit,
            market_regime,
        }
    }

    /// Register a new position for profit target tracking.
    /// On restart, restores persisted state (targets_hit, peak, trailing stop) from DB.
    pub async fn register_position(
        &self,
        trade_uuid: &str,
        entry_price: Decimal,
        entry_amount_sol: Decimal,
        token_address: &str,
        entry_time: std::time::SystemTime,
    ) {
        let mut targets = self.active_targets.write().await;

        // Skip if already tracked in-memory (idempotent) — check under write lock to prevent TOCTOU race
        if targets.contains_key(trade_uuid) {
            return;
        }

        let current_price = self
            .price_cache
            .get_price_usd(token_address)
            .unwrap_or(entry_price);

        // Try to restore state from DB (survives restarts)
        let state = match self.db.load_exit_target(trade_uuid).await {
            Ok(Some(data)) => {
                let peak = data.peak_price.max(current_price);
                let peak_pct = data.peak_profit_percent;
                let targets_hit: Vec<usize> =
                    serde_json::from_str(&data.targets_hit).unwrap_or_else(|_| {
                        // Backward compat: old rows stored Decimal values.
                        // Clear and re-evaluate from scratch (safe — may re-trigger
                        // a tier that was already hit, but that's better than panic).
                        tracing::warn!(
                            trade_uuid,
                            raw = %data.targets_hit,
                            "Migrating targets_hit from value-based to index-based (resetting)"
                        );
                        Vec::new()
                    });
                let t_price = data.trailing_stop_price;
                let remaining = data.remaining_fraction;
                tracing::debug!(trade_uuid, %remaining, "Restored profit target state from DB");
                ProfitTargetState {
                    trade_uuid: trade_uuid.to_string(),
                    entry_price,
                    entry_amount_sol,
                    current_price,
                    peak_price: peak,
                    peak_profit_percent: peak_pct,
                    targets_hit,
                    trailing_stop_active: data.trailing_stop_active,
                    trailing_stop_price: t_price,
                    entry_time,
                    remaining_fraction: remaining,
                    initial_vol_scale: {
                        match self.price_cache.calculate_volatility(token_address) {
                            Some(vol) => {
                                let vol_dec = Decimal::from_str(&format!("{:.4}", vol))
                                    .unwrap_or(Decimal::ZERO);
                                if self.config.target_vol_scale_threshold.is_zero() {
                                    None
                                } else {
                                    Some((vol_dec / self.config.target_vol_scale_threshold)
                                        .min(Decimal::ONE))
                                }
                            }
                            None => None,
                        }
                    },
                    ticks_since_entry: 0,
                    last_logged_vol_scale: None,
                }
            }
            _ => {
                // Fresh state — also write to DB so it survives the next restart
                let state = ProfitTargetState {
                    trade_uuid: trade_uuid.to_string(),
                    entry_price,
                    entry_amount_sol,
                    current_price,
                    peak_price: current_price,
                    peak_profit_percent: Decimal::ZERO,
                    targets_hit: Vec::new(),
                    trailing_stop_active: false,
                    trailing_stop_price: Decimal::ZERO,
                    entry_time,
                    remaining_fraction: Decimal::ONE,
                    initial_vol_scale: {
                        // Capture initial volatility estimate if available at registration
                        match self.price_cache.calculate_volatility(token_address) {
                            Some(vol) => {
                                let vol_dec = Decimal::from_str(&format!("{:.4}", vol))
                                    .unwrap_or(Decimal::ZERO);
                                if self.config.target_vol_scale_threshold.is_zero() {
                                    None
                                } else {
                                    let scale = (vol_dec
                                        / self.config.target_vol_scale_threshold)
                                        .min(Decimal::ONE);
                                    Some(scale)
                                }
                            }
                            None => None,
                        }
                    },
                    ticks_since_entry: 0,
                    last_logged_vol_scale: None,
                };
                if let Err(e) = self
                    .db
                    .upsert_exit_target(
                        trade_uuid,
                        entry_price,
                        entry_amount_sol,
                        current_price,
                        rust_decimal::Decimal::ZERO,
                        "[]",
                        false,
                        rust_decimal::Decimal::ZERO,
                        rust_decimal::Decimal::ONE,
                    )
                    .await
                {
                    tracing::warn!(trade_uuid, error = %e, "Failed to persist initial profit target state");
                }
                state
            }
        };

        targets.insert(trade_uuid.to_string(), state);
    }

    /// Check profit targets and return action if needed.
    ///
    /// `strategy` is the position's strategy string ("SHIELD" or "SPEAR") — used to
    /// differentiate time-exit thresholds and trailing-stop distance.
    pub async fn check_targets(
        &self,
        trade_uuid: &str,
        token_address: &str,
        strategy: &str,
    ) -> ProfitTargetAction {
        let current_price = match self.price_cache.get_price_usd(token_address) {
            Some(price) => price,
            None => return ProfitTargetAction::None,
        };

        let mut guard = self.active_targets.write().await;
        let state = match guard.get_mut(trade_uuid) {
            Some(s) => s,
            None => return ProfitTargetAction::None,
        };

        // Update current price and peak
        state.current_price = current_price;
        let is_new_peak = current_price >= state.peak_price;
        if is_new_peak {
            state.peak_price = current_price;
        }

        // Calculate current profit using Decimal for precision
        let profit_percent = if !state.entry_price.is_zero() {
            let diff = state.current_price - state.entry_price;
            let ratio = diff / state.entry_price;
            ratio * Decimal::from(100)
        } else {
            Decimal::ZERO
        };

        state.peak_profit_percent = profit_percent.max(state.peak_profit_percent);

        // Get profit targets scaled by market regime multiplier
        let multiplier = if let Some(ref regime_detector) = self.market_regime {
            regime_detector.get_regime_multiplier(token_address)
        } else {
            Decimal::ONE
        };

        // Increment tick counter for cold-start ramp
        state.ticks_since_entry = state.ticks_since_entry.saturating_add(1);

        // Compute volatility scale factor
        let vol = self.price_cache.calculate_volatility(token_address);
        let vol_scale = compute_vol_scale(
            vol,
            self.config.target_vol_scale_threshold,
            state.ticks_since_entry,
            state.initial_vol_scale,
        );

        // Log vol_scale at DEBUG, but only when it changes by >10% (Issue 4: log flooding)
        let should_log = match state.last_logged_vol_scale {
            None => true,
            Some(last) => {
                let change = ((vol_scale - last).abs() / last.max(dec!(0.001))) * dec!(100);
                change > dec!(10)
            }
        };
        if should_log {
            state.last_logged_vol_scale = Some(vol_scale);
            tracing::debug!(
                trade_uuid,
                token = %token_address,
                vol_scale = %vol_scale,
                volatility = ?vol,
                "Volatility scale computed for profit targets"
            );
        }

        // Scale profit targets: apply regime multiplier × vol_scale, floor at min_target_pct
        let min_target = self.config.min_target_pct;
        let profit_level_targets: Vec<Decimal> = self
            .config
            .targets
            .iter()
            .map(|t| {
                let scaled = *t * multiplier * vol_scale;
                scaled.max(min_target)
            })
            .collect();

        // Track whether state changed so we can persist once at the end
        let mut state_changed = is_new_peak;

        // Check tiered profit targets.
        // If price jumps multiple tiers in a single tick (e.g. +20% → +110%), accumulate
        // all newly-hit targets and compound the exit fraction so the caller sells the
        // correct proportion in one transaction rather than deferring tiers to later ticks
        // where the price may already be reversing.
        let mut tiered_action: Option<ProfitTargetAction> = None;
        {
            let mut new_targets_hit = 0;

            for (i, target) in profit_level_targets.iter().enumerate() {
                if profit_percent >= *target && !state.targets_hit.contains(&i) {
                    state.targets_hit.push(i);
                    state_changed = true;
                    new_targets_hit += 1;
                }
            }

            if new_targets_hit > 0 {
                // Calculate compounding exit percentage of the CURRENT position.
                // Selling f% of the remaining balance k times leaves (1 - f)^k of the balance.
                // The fraction of the current balance to sell in this tick is 1 - (1 - f)^k.
                let exit_fraction_remaining = self.config.tiered_exit_percent / Decimal::from(100);
                let retain_fraction = Decimal::ONE - exit_fraction_remaining;

                let mut current_retain = Decimal::ONE;
                for _ in 0..new_targets_hit {
                    current_retain *= retain_fraction;
                }

                let exit_fraction_current = Decimal::ONE - current_retain;

                // Compute remaining BEFORE updating state.remaining_fraction so
                // we get the pre-sell actual_remaining, not the post-sell value.
                let pre_sell_remaining = state.entry_amount_sol * state.remaining_fraction;

                // Update remaining_fraction: track how much of the original position is held
                state.remaining_fraction *= current_retain;

                // Dust check: if the remaining position after the tiered exit would be
                // smaller than min_size_sol, perform a full exit instead of leaving an
                // economically unviable dust position that costs more in gas to close
                // than it is worth.
                let remaining_after_exit = pre_sell_remaining * current_retain;
                if remaining_after_exit > Decimal::ZERO
                    && remaining_after_exit < self.config.min_size_sol
                {
                    tracing::info!(
                        trade_uuid,
                        remaining_after_exit = %remaining_after_exit,
                        min_size_sol = %self.config.min_size_sol,
                        "Tiered exit would leave dust position — performing full exit instead"
                    );
                    tiered_action = Some(ProfitTargetAction::FullExit);
                } else {
                    // Emit an absolute SOL amount rather than a percentage of "the position."
                    // This eliminates the oversell bug where the executor might apply
                    // the percentage against the original entry_amount instead of the
                    // current remaining balance after prior tiered exits.
                    let sell_amount = exit_fraction_current * pre_sell_remaining;
                    tiered_action = Some(ProfitTargetAction::ExitAmount(sell_amount));
                }
            }
        }

        // Check trailing stop: activate at configured threshold regardless of regime.
        // Profit targets scale with regime to let winners run, but the trailing stop must
        // protect capital at the same rate — deferring it in bull markets (where positions
        // are larger) would maximise unprotected exposure at exactly the wrong time.
        //
        // Spear positions use a 50% wider trailing distance to avoid being shaken out by
        // the higher volatility of their more aggressive token selection.
        // Additionally, the trailing distance widens for highly volatile tokens (mirrors
        // stop_loss.rs adaptive logic) so microcaps aren't stopped out by normal intraday
        // retracements. Cap at 40% so the stop remains actionable.
        // Scale trailing stop activation by vol_scale (Issue 3 fix)
        let scaled_activation = (self.config.trailing_stop_activation * vol_scale).max(min_target);

        let base_trailing_distance = if strategy == "SPEAR" {
            self.config.trailing_stop_distance * dec!(1.5)
        } else {
            self.config.trailing_stop_distance
        };
        // Scale trailing distance by vol_scale so low-vol tokens get tighter stops
        let scaled_base_distance = base_trailing_distance * vol_scale;
        let trailing_distance =
            if let Some(vol) = self.price_cache.calculate_volatility(token_address) {
                let vol_mult = if vol > 50.0 {
                    dec!(1.5)
                } else if vol > 30.0 {
                    dec!(1.25)
                } else {
                    Decimal::ONE
                };
                (scaled_base_distance * vol_mult).min(Decimal::from(40))
            } else {
                scaled_base_distance
            };
        if profit_percent >= scaled_activation && !state.trailing_stop_active {
            state.trailing_stop_active = true;
            let trailing_distance_ratio = trailing_distance / Decimal::from(100);
            let raw_stop = state.peak_price * (Decimal::ONE - trailing_distance_ratio);
            // Floor: once trailing stop activates, never let it sit below a small profit lock
            let floor_price = state.entry_price * (Decimal::ONE + min_target / Decimal::from(100));
            state.trailing_stop_price = raw_stop.max(floor_price);
            state_changed = true;
        }

        // Check if trailing stop hit
        let trailing_hit =
            state.trailing_stop_active && state.current_price <= state.trailing_stop_price;

        // Ratchet trailing stop price on new high — use peak_price, not current_price,
        // so a stale price_cache read can't set the stop tighter than the actual peak.
        // Ensure the stop price is monotonically increasing (never drops due to a sudden
        // spike in volatility widening the trailing distance).
        if state.trailing_stop_active && is_new_peak {
            let trailing_distance_ratio = trailing_distance / Decimal::from(100);
            let new_trailing_stop_price =
                state.peak_price * (Decimal::ONE - trailing_distance_ratio);
            // Floor clamp: trailing stop never drops below entry + min_target_pct profit
            let floor_price = state.entry_price * (Decimal::ONE + min_target / Decimal::from(100));
            let clamped = new_trailing_stop_price.max(floor_price);
            if clamped > state.trailing_stop_price {
                state.trailing_stop_price = clamped;
                state_changed = true;
            }
        }

        // Snapshot state for DB persistence (before releasing the lock)
        let db_snapshot = if state_changed {
            let th: Vec<usize> = state.targets_hit.clone();
            let th_json = serde_json::to_string(&th).unwrap_or_else(|_| "[]".to_string());
            Some((
                state.entry_price,
                state.entry_amount_sol,
                state.peak_price,
                state.peak_profit_percent,
                th_json,
                state.trailing_stop_active,
                state.trailing_stop_price,
                state.remaining_fraction,
            ))
        } else {
            None
        };

        // Determine final action before releasing the lock.
        // Spear positions use shorter hold times (higher volatility, faster decay).
        // Shield positions hold longer to allow conservative signals to play out.
        let is_spear = strategy == "SPEAR";
        let time_exit = if let Ok(elapsed) = state.entry_time.elapsed() {
            let elapsed_hours = elapsed.as_secs() / 3600;
            if profit_percent > dec!(25) {
                // High-profit: Shield holds to 48h, Spear exits sooner at 24h
                elapsed_hours >= if is_spear { 24 } else { 48 }
            } else if profit_percent > dec!(10) {
                // Medium-profit: Shield 24h, Spear 12h
                elapsed_hours
                    >= if is_spear {
                        12
                    } else {
                        self.config.time_exit_hours
                    }
            } else if profit_percent > Decimal::ZERO {
                // Low-profit: use losing_time_exit_hours for the strategy so operators
                // can control all near-breakeven/losing exit timing through one config knob
                // per strategy, rather than discovering that time_exit_hours doesn't apply.
                elapsed_hours
                    >= if is_spear {
                        self.config.losing_time_exit_hours_spear
                    } else {
                        self.config.losing_time_exit_hours_shield
                    }
            } else {
                // Losing: use configured time-exit hours for the strategy.
                // losing_time_exit_threshold_percent (default -3%) determines whether the loss
                // is "significant" — but both significant and minor losses now use the
                // configured losing_time_exit_hours_* values instead of hardcoded fallbacks.
                let exit_limit_hours = if is_spear {
                    self.config.losing_time_exit_hours_spear
                } else {
                    self.config.losing_time_exit_hours_shield
                };
                elapsed_hours >= exit_limit_hours
            }
        } else {
            false
        };

        let entry_price_snap = state.entry_price;
        let entry_time_snap = state.entry_time;

        // Release the write lock before async DB calls. `guard` is the actual RwLock
        // WriteGuard from line 213; `profit_level_targets` is a plain Vec and needs no drop.
        drop(guard);

        // Persist state changes to DB (outside the lock)
        if let Some((ep, ea, pp, ppp, th_json, tsa, tsp, rf)) = db_snapshot {
            let trade_uuid_owned = trade_uuid.to_string();
            if let Err(e) = self
                .db
                .upsert_exit_target(&trade_uuid_owned, ep, ea, pp, ppp, &th_json, tsa, tsp, rf)
                .await
            {
                tracing::warn!(trade_uuid, error = %e, "Failed to persist profit target state");
            }
        }

        // Momentum exit takes priority: a crash should override a partial tiered exit
        if let Some(ref momentum) = self.momentum_exit {
            if momentum
                .should_exit(trade_uuid, token_address, entry_price_snap, entry_time_snap)
                .await
            {
                tracing::info!(
                    trade_uuid = %trade_uuid,
                    "Momentum exit triggered: negative momentum detected"
                );
                return ProfitTargetAction::FullExit;
            }
        }

        // Tiered exit (only if no momentum crash)
        if let Some(action) = tiered_action {
            return action;
        }

        if trailing_hit || time_exit {
            return ProfitTargetAction::FullExit;
        }

        ProfitTargetAction::None
    }

    /// Sweep stale entries for positions that closed outside the FullExit path.
    /// Called periodically from the position monitoring loop (~every 5 minutes).
    pub async fn sweep_hwm_stale_entries(&self) -> usize {
        let active = match self.db.get_active_positions().await {
            Ok(positions) => positions
                .into_iter()
                .map(|p| p.trade_uuid)
                .collect::<Vec<_>>(),
            Err(e) => {
                tracing::warn!(error = %e, "HWM sweep: DB query failed, skipping");
                return 0;
            }
        };
        let active_set: std::collections::HashSet<String> = active.into_iter().collect();
        let mut map = self.active_targets.write().await;
        let before = map.len();
        map.retain(|uuid, _| active_set.contains(uuid));
        before - map.len()
    }

    /// Remove position from tracking and delete persisted state
    pub async fn remove_position(&self, trade_uuid: &str) {
        let mut targets = self.active_targets.write().await;
        targets.remove(trade_uuid);
        drop(targets);
        if let Err(e) = self.db.delete_exit_target(trade_uuid).await {
            tracing::warn!(trade_uuid, error = %e, "Failed to delete exit target state from DB");
        }
    }
}

#[cfg(test)]
mod vol_scale_tests {
    use super::*;

    #[test]
    fn test_high_volatility_full_scale() {
        // Vol=60%, threshold=30% → scale=1.0 (capped)
        let scale = compute_vol_scale(Some(60.0), dec!(30.0), 100, None);
        assert_eq!(scale, Decimal::ONE);
    }

    #[test]
    fn test_moderate_volatility_partial_scale() {
        // Vol=15%, threshold=30% → scale=0.5
        let scale = compute_vol_scale(Some(15.0), dec!(30.0), 100, None);
        assert_eq!(scale, dec!(0.5));
    }

    #[test]
    fn test_low_volatility_small_scale() {
        // Vol=5%, threshold=30% → scale=0.1667
        let scale = compute_vol_scale(Some(5.0), dec!(30.0), 100, None);
        // 5/30 = 0.16666... — check it's between 0.16 and 0.17
        assert!(scale > dec!(0.16) && scale < dec!(0.17));
    }

    #[test]
    fn test_no_volatility_uses_full_scale() {
        let scale = compute_vol_scale(None, dec!(30.0), 100, None);
        assert_eq!(scale, Decimal::ONE);
    }

    #[test]
    fn test_no_volatility_but_has_initial_estimate() {
        // No live vol data, but initial estimate was vol=10% → scale=10/30=0.333
        let scale = compute_vol_scale(None, dec!(30.0), 100, Some(dec!(0.3333)));
        assert!(scale > dec!(0.30) && scale < dec!(0.40));
    }

    #[test]
    fn test_cold_start_ramp_smooths_transition() {
        // Vol=5% (scale would be 0.167), but only 3 ticks elapsed (ramp 60 ticks)
        // ramp_progress = 3/60 = 0.05
        // effective_scale = 1.0 - (1.0 - 0.167) * 0.05 = 1.0 - 0.0417 = 0.958
        let scale = compute_vol_scale(Some(5.0), dec!(30.0), 3, None);
        assert!(scale > dec!(0.95) && scale < dec!(0.97));
    }

    #[test]
    fn test_ramp_completes_after_60_ticks() {
        // After 60 ticks, ramp is fully applied — scale = raw 5/30 = 0.167
        let scale = compute_vol_scale(Some(5.0), dec!(30.0), 60, None);
        assert!(scale > dec!(0.16) && scale < dec!(0.17));
    }

    #[test]
    fn test_zero_volatility_full_scale() {
        // Vol=0% is degenerate but shouldn't crash — scale=0, but callers clamp to min_target_pct
        let scale = compute_vol_scale(Some(0.0), dec!(30.0), 100, None);
        assert_eq!(scale, Decimal::ZERO);
    }

    #[test]
    fn test_index_based_tracking_no_double_sell() {
        // Simulate: target[0] scaled to 4.2% on tick 1, then 3.8% on tick 2.
        // With value-based tracking, tick 2 would re-trigger tier 0.
        // With index-based tracking, tier 0 stays hit regardless of value drift.
        let mut targets_hit: Vec<usize> = vec![];

        // Tick 1: profit=4.2%, scaled_targets=[4.2, 8.3, 16.7, 33.3]
        // Tier 0 (4.2) is hit
        for (i, target) in [dec!(4.2), dec!(8.3), dec!(16.7), dec!(33.3)]
            .iter()
            .enumerate()
        {
            if dec!(4.2) >= *target && !targets_hit.contains(&i) {
                targets_hit.push(i);
            }
        }
        assert_eq!(targets_hit, vec![0]);

        // Tick 2: profit=3.9%, scaled_targets=[3.8, 7.5, 15.0, 30.0] (volatility dropped)
        // Value 3.8 is NOT in old targets_hit (which had 4.2), so value-based would
        // re-trigger. Index-based: tier 0 already hit → skip.
        for (i, target) in [dec!(3.8), dec!(7.5), dec!(15.0), dec!(30.0)]
            .iter()
            .enumerate()
        {
            if dec!(3.9) >= *target && !targets_hit.contains(&i) {
                targets_hit.push(i);
            }
        }
        // Still only tier 0 — no double sell
        assert_eq!(targets_hit, vec![0]);
    }

    #[test]
    fn test_effective_targets_high_vol_unchanged() {
        // Vol=60%, threshold=30% → scale=1.0
        // base [25,50,100,200] × 1.0 = [25,50,100,200]
        let scale = compute_vol_scale(Some(60.0), dec!(30.0), 100, None);
        assert_eq!(scale, Decimal::ONE);
        let min_target = dec!(5.0);
        let effective: Vec<Decimal> = [dec!(25), dec!(50), dec!(100), dec!(200)]
            .into_iter()
            .map(|t| (t * scale).max(min_target))
            .collect();
        assert_eq!(effective, vec![dec!(25), dec!(50), dec!(100), dec!(200)]);
    }

    #[test]
    fn test_effective_targets_low_vol_floored() {
        // Vol=3%, threshold=30% → scale=0.1
        // base [25,50,100,200] × 0.1 = [2.5,5,10,20] → floored to [5,5,10,20]
        let scale = compute_vol_scale(Some(3.0), dec!(30.0), 100, None);
        let min_target = dec!(5.0);
        let effective: Vec<Decimal> = [dec!(25), dec!(50), dec!(100), dec!(200)]
            .into_iter()
            .map(|t| (t * scale).max(min_target))
            .collect();
        assert_eq!(effective[0], min_target); // floored
        assert_eq!(effective[1], min_target); // floored
        assert_eq!(effective[2], dec!(10));
        assert_eq!(effective[3], dec!(20));
    }

    #[test]
    fn test_trailing_activation_scales_and_floors() {
        // Vol=5%, threshold=30% → scale=0.167
        // activation 30% × 0.167 = 5.0 → floored at min_target 5.0
        let scale = compute_vol_scale(Some(5.0), dec!(30.0), 100, None);
        let activation = (dec!(30) * scale).max(dec!(5.0));
        assert!(activation >= dec!(5.0)); // never below min_target
    }

    #[test]
    fn test_cold_start_ramp_gradual() {
        // At tick 0: ramp=0 → effective_scale=1.0 (full targets, safe)
        let scale_t0 = compute_vol_scale(Some(5.0), dec!(30.0), 0, None);
        assert!(scale_t0 > dec!(0.99));

        // At tick 30 (halfway): ramp=0.5 → effective_scale ≈ 0.583
        let scale_t30 = compute_vol_scale(Some(5.0), dec!(30.0), 30, None);
        assert!(scale_t30 > dec!(0.55) && scale_t30 < dec!(0.62));

        // At tick 60 (done): effective_scale ≈ 0.167
        let scale_t60 = compute_vol_scale(Some(5.0), dec!(30.0), 60, None);
        assert!(scale_t60 > dec!(0.16) && scale_t60 < dec!(0.17));
    }

    #[test]
    fn test_initial_estimate_skips_ramp() {
        // If initial_vol_scale is set, no ramp — immediate scale
        let scale = compute_vol_scale(None, dec!(30.0), 5, Some(dec!(0.333)));
        assert_eq!(scale, dec!(0.333));
    }
}
