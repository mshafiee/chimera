//! Tiered profit targets with trailing stops
//!
//! Implements:
//! - Tiered exits (sell 25% at each target)
//! - Trailing stops (after +50%, set trailing stop at -20% from peak)
//! - Time-based exits (auto-exit after 24h if profitable)

use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;
use rust_decimal::prelude::*;
use crate::config::ProfitManagementConfig;
use crate::db::DbPool;
use crate::price_cache::PriceCache;
use crate::engine::momentum_exit::MomentumExit;
use crate::engine::market_regime::MarketRegimeDetector;

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
    trade_uuid: String,
    entry_price: f64,
    entry_amount_sol: f64,
    current_price: f64,
    peak_price: f64,
    peak_profit_percent: f64,
    targets_hit: Vec<f64>, // Which targets have been hit
    trailing_stop_active: bool,
    trailing_stop_price: f64,
    entry_time: SystemTime,
}

/// Profit target action
#[derive(Debug, Clone)]
pub enum ProfitTargetAction {
    /// No action needed
    None,
    /// Exit percentage of position
    ExitPercent(f64),
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

    /// Register a new position for profit target tracking
    pub async fn register_position(
        &self,
        trade_uuid: &str,
        entry_price: f64,
        entry_amount_sol: f64,
        token_address: &str,
    ) {
        let current_price = self.price_cache
            .get_price_usd(token_address)
            .unwrap_or(entry_price);

        let state = ProfitTargetState {
            trade_uuid: trade_uuid.to_string(),
            entry_price,
            entry_amount_sol,
            current_price,
            peak_price: current_price,
            peak_profit_percent: 0.0,
            targets_hit: Vec::new(),
            trailing_stop_active: false,
            trailing_stop_price: 0.0,
            entry_time: SystemTime::now(),
        };

        let mut targets = self.active_targets.write().await;
        targets.insert(trade_uuid.to_string(), state);
    }

    /// Check profit targets and return action if needed
    pub async fn check_targets(&self, trade_uuid: &str, token_address: &str) -> ProfitTargetAction {
        let current_price = match self.price_cache.get_price_usd(token_address) {
            Some(price) => price,
            None => return ProfitTargetAction::None,
        };

        let mut targets = self.active_targets.write().await;
        let state = match targets.get_mut(trade_uuid) {
            Some(s) => s,
            None => return ProfitTargetAction::None,
        };

        // Update current price and peak
        state.current_price = current_price;
        if current_price > state.peak_price {
            state.peak_price = current_price;
        }

        // Calculate current profit using Decimal for precision
        // Convert f64 to Decimal safely
        let current_price_dec = Decimal::from_f64_retain(current_price).unwrap_or(Decimal::ZERO);
        let entry_price_dec = Decimal::from_f64_retain(state.entry_price).unwrap_or(Decimal::ZERO);
        
        let profit_percent = if !entry_price_dec.is_zero() {
            let diff = current_price_dec - entry_price_dec;
            let ratio = diff / entry_price_dec;
            (ratio * Decimal::from(100)).to_f64().unwrap_or(0.0)
        } else {
            0.0
        };

        state.peak_profit_percent = profit_percent.max(state.peak_profit_percent);

        // Get profit targets (dynamic based on market regime if available)
        let targets = if let Some(ref regime_detector) = self.market_regime {
            regime_detector.get_profit_targets()
        } else {
            self.config.targets.clone()
        };

        // Check tiered profit targets
        for target in &targets {
            if profit_percent >= *target && !state.targets_hit.contains(target) {
                state.targets_hit.push(*target);
                return ProfitTargetAction::ExitPercent(self.config.tiered_exit_percent);
            }
        }

        // Check trailing stop (activate after trailing_stop_activation %)
        if profit_percent >= self.config.trailing_stop_activation && !state.trailing_stop_active {
            state.trailing_stop_active = true;
            state.trailing_stop_price = state.peak_price * (1.0 - self.config.trailing_stop_distance / 100.0);
        }

        // Check if trailing stop hit
        if state.trailing_stop_active && current_price <= state.trailing_stop_price {
            return ProfitTargetAction::FullExit;
        }

        // Update trailing stop price if price increases
        if state.trailing_stop_active && current_price > state.peak_price {
            state.trailing_stop_price = current_price * (1.0 - self.config.trailing_stop_distance / 100.0);
        }

        // Refined time-based exit logic
        if let Ok(elapsed) = state.entry_time.elapsed() {
            let elapsed_hours = elapsed.as_secs() / 3600;
            
            // If profitable >10%: Extend to 48h
            if profit_percent > 10.0 {
                if elapsed_hours >= 48 {
                    return ProfitTargetAction::FullExit;
                }
            }
            // If profitable <5%: Exit after 12h (lock in small profits)
            else if profit_percent > 0.0 && profit_percent < 5.0 {
                if elapsed_hours >= 12 {
                    return ProfitTargetAction::FullExit;
                }
            }
            // If at loss: Exit after 6h (cut losses faster)
            else if profit_percent < 0.0 {
                if elapsed_hours >= 6 {
                    return ProfitTargetAction::FullExit;
                }
            }
            // Default: Original time_exit_hours for moderate profits (5-10%)
            else if elapsed_hours >= self.config.time_exit_hours as u64 && profit_percent > 0.0 {
                return ProfitTargetAction::FullExit;
            }
        }

        // Check momentum exit (early exit on negative momentum)
        if let Some(ref momentum) = self.momentum_exit {
            if momentum
                .should_exit(trade_uuid, token_address, state.entry_price, state.entry_time)
                .await
            {
                tracing::info!(
                    trade_uuid = %trade_uuid,
                    "Momentum exit triggered: negative momentum detected"
                );
                return ProfitTargetAction::FullExit;
            }
        }

        ProfitTargetAction::None
    }

    /// Remove position from tracking
    pub async fn remove_position(&self, trade_uuid: &str) {
        let mut targets = self.active_targets.write().await;
        targets.remove(trade_uuid);
    }
}
