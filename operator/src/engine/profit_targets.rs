//! Tiered profit targets with trailing stops
//!
//! Implements:
//! - Tiered exits (sell 25% at each target)
//! - Trailing stops (after +50%, set trailing stop at -20% from peak)
//! - Time-based exits (auto-exit after 24h if profitable)

use crate::config::ProfitManagementConfig;
use crate::db::{self, DbPool};
use crate::engine::market_regime::MarketRegimeDetector;
use crate::engine::momentum_exit::MomentumExit;
use crate::price_cache::PriceCache;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;
use serde_json;

/// Profit target state
pub struct ProfitTargetManager {
    db: DbPool,
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
    targets_hit: Vec<Decimal>, // Which targets have been hit
    trailing_stop_active: bool,
    trailing_stop_price: Decimal,
    entry_time: SystemTime,
}

/// Profit target action
#[derive(Debug, Clone)]
pub enum ProfitTargetAction {
    /// No action needed
    None,
    /// Exit percentage of position (using Decimal for precision)
    ExitPercent(Decimal),
    /// Full exit
    FullExit,
}

impl ProfitTargetManager {
    pub fn new(
        db: DbPool,
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
        db: DbPool,
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
        db: DbPool,
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
        db: DbPool,
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
        // Skip if already tracked in-memory (idempotent)
        {
            let targets = self.active_targets.read().await;
            if targets.contains_key(trade_uuid) {
                return;
            }
        }

        let current_price = self
            .price_cache
            .get_price_usd(token_address)
            .unwrap_or(entry_price);

        // Try to restore state from DB (survives restarts)
        let state = match db::load_exit_target(&self.db, trade_uuid).await {
            Ok(Some((_, _, db_peak, db_peak_pct, targets_hit_json, trailing_active, trailing_price))) => {
                let peak = Decimal::from_f64_retain(db_peak).unwrap_or(current_price).max(current_price);
                let peak_pct = Decimal::from_f64_retain(db_peak_pct).unwrap_or(Decimal::ZERO);
                let targets_hit: Vec<Decimal> = serde_json::from_str(&targets_hit_json)
                    .unwrap_or_default();
                let t_price = Decimal::from_f64_retain(trailing_price).unwrap_or(Decimal::ZERO);
                tracing::debug!(trade_uuid, "Restored profit target state from DB");
                ProfitTargetState {
                    trade_uuid: trade_uuid.to_string(),
                    entry_price,
                    entry_amount_sol,
                    current_price,
                    peak_price: peak,
                    peak_profit_percent: peak_pct,
                    targets_hit,
                    trailing_stop_active: trailing_active,
                    trailing_stop_price: t_price,
                    // Use the actual trade open time so time-based exits fire correctly
                    // even after a restart.
                    entry_time,
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
                };
                let ep = entry_price.to_f64().unwrap_or(0.0);
                let ea = entry_amount_sol.to_f64().unwrap_or(0.0);
                let pp = current_price.to_f64().unwrap_or(0.0);
                if let Err(e) = db::upsert_exit_target(&self.db, trade_uuid, ep, ea, pp, 0.0, "[]", false, 0.0).await {
                    tracing::warn!(trade_uuid, error = %e, "Failed to persist initial profit target state");
                }
                state
            }
        };

        let mut targets = self.active_targets.write().await;
        targets.insert(trade_uuid.to_string(), state);
    }

    /// Check profit targets and return action if needed.
    ///
    /// `strategy` is the position's strategy string ("SHIELD" or "SPEAR") — used to
    /// differentiate time-exit thresholds and trailing-stop distance.
    pub async fn check_targets(&self, trade_uuid: &str, token_address: &str, strategy: &str) -> ProfitTargetAction {
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
        let is_new_peak = current_price > state.peak_price;
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
        // Use a distinct name so we don't shadow the `guard` write-lock binding above.
        let profit_level_targets: Vec<Decimal> = self.config.targets.iter().map(|t| *t * multiplier).collect();

        // Track whether state changed so we can persist once at the end
        let mut state_changed = is_new_peak;

        // Check tiered profit targets.
        // If price jumps multiple tiers in a single tick (e.g. +20% → +110%), accumulate
        // all newly-hit targets and compound the exit fraction so the caller sells the
        // correct proportion in one transaction rather than deferring tiers to later ticks
        // where the price may already be reversing.
        let mut tiered_action: Option<ProfitTargetAction> = None;
        {
            let exit_pct = self.config.tiered_exit_percent / Decimal::from(100);
            let mut compound_remaining = Decimal::ONE;
            let mut any_hit = false;
            for target in &profit_level_targets {
                if profit_percent >= *target && !state.targets_hit.contains(target) {
                    state.targets_hit.push(*target);
                    state_changed = true;
                    any_hit = true;
                    compound_remaining *= Decimal::ONE - exit_pct;
                }
            }
            if any_hit {
                let total_exit_pct = (Decimal::ONE - compound_remaining) * Decimal::from(100);
                tiered_action = Some(ProfitTargetAction::ExitPercent(total_exit_pct));
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
        let base_trailing_distance = if strategy == "SPEAR" {
            self.config.trailing_stop_distance * dec!(1.5)
        } else {
            self.config.trailing_stop_distance
        };
        let trailing_distance = if let Some(vol) = self.price_cache.calculate_volatility(token_address) {
            let vol_mult = if vol > 50.0 {
                dec!(1.5)
            } else if vol > 30.0 {
                dec!(1.25)
            } else {
                Decimal::ONE
            };
            (base_trailing_distance * vol_mult).min(Decimal::from(40))
        } else {
            base_trailing_distance
        };
        if profit_percent >= self.config.trailing_stop_activation && !state.trailing_stop_active {
            state.trailing_stop_active = true;
            let trailing_distance_ratio = trailing_distance / Decimal::from(100);
            state.trailing_stop_price = state.peak_price * (Decimal::ONE - trailing_distance_ratio);
            state_changed = true;
        }

        // Check if trailing stop hit
        let trailing_hit = state.trailing_stop_active && state.current_price <= state.trailing_stop_price;

        // Ratchet trailing stop price on new high — use peak_price, not current_price,
        // so a stale price_cache read can't set the stop tighter than the actual peak.
        if state.trailing_stop_active && is_new_peak {
            let trailing_distance_ratio = trailing_distance / Decimal::from(100);
            state.trailing_stop_price =
                state.peak_price * (Decimal::ONE - trailing_distance_ratio);
            state_changed = true;
        }

        // Snapshot state for DB persistence (before releasing the lock)
        let (db_ep, db_ea, db_pp, db_ppp, db_th_json, db_tsa, db_tsp) = if state_changed {
            let ep = state.entry_price.to_f64().unwrap_or(0.0);
            let ea = state.entry_amount_sol.to_f64().unwrap_or(0.0);
            let pp = state.peak_price.to_f64().unwrap_or(0.0);
            let ppp = state.peak_profit_percent.to_f64().unwrap_or(0.0);
            let th: Vec<f64> = state.targets_hit.iter().filter_map(|d| d.to_f64()).collect();
            let th_json = serde_json::to_string(&th).unwrap_or_else(|_| "[]".to_string());
            let tsa = state.trailing_stop_active;
            let tsp = state.trailing_stop_price.to_f64().unwrap_or(0.0);
            (ep, ea, pp, ppp, th_json, tsa, tsp)
        } else {
            (0.0, 0.0, 0.0, 0.0, String::new(), false, 0.0)
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
                elapsed_hours >= if is_spear { 12 } else { self.config.time_exit_hours }
            } else if profit_percent > Decimal::ZERO {
                // Low-profit: Shield 16h, Spear 8h — free capital before it goes flat
                elapsed_hours >= if is_spear { 8 } else { 16 }
            } else {
                // Losing: tighten exits — Spear 2h, Shield 4h.
                // Solana memecoins rarely recover from sustained underwater positions;
                // holding losers for 4-8h burns capital that could compound elsewhere.
                elapsed_hours >= if is_spear { 2 } else { 4 }
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
        if state_changed {
            let trade_uuid_owned = trade_uuid.to_string();
            if let Err(e) = db::upsert_exit_target(
                &self.db, &trade_uuid_owned, db_ep, db_ea, db_pp, db_ppp, &db_th_json, db_tsa, db_tsp,
            ).await {
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

    /// Sweep HWM entries for positions that closed outside the FullExit path.
    /// Called periodically from the position monitoring loop (~every 5 minutes).
    pub async fn sweep_hwm_stale_entries(&self) -> usize {
        match &self.momentum_exit {
            Some(m) => m.sweep_stale_entries().await,
            None => 0,
        }
    }

    /// Remove position from tracking and delete persisted state
    pub async fn remove_position(&self, trade_uuid: &str) {
        let mut targets = self.active_targets.write().await;
        targets.remove(trade_uuid);
        drop(targets);
        if let Some(ref momentum) = self.momentum_exit {
            momentum.remove_position(trade_uuid);
        }
        if let Err(e) = db::delete_exit_target(&self.db, trade_uuid).await {
            tracing::warn!(trade_uuid, error = %e, "Failed to delete exit target state from DB");
        }
    }
}
