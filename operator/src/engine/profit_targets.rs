//! Tiered profit targets with trailing stops
//!
//! Implements:
//! - Tiered exits (sell 25% at each target)
//! - Trailing stops (after +50%, set trailing stop at -20% from peak)
//! - Time-based exits (auto-exit after 24h if profitable)

use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;
use crate::config::ProfitManagementConfig;
use crate::db::DbPool;
use crate::price_cache::PriceCache;

/// Profit target state
pub struct ProfitTargetManager {
    db: DbPool,
    config: Arc<ProfitManagementConfig>,
    price_cache: Arc<PriceCache>,
    /// Active profit targets by trade UUID
    active_targets: Arc<RwLock<std::collections::HashMap<String, ProfitTargetState>>>,
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

        // Calculate current profit
        let profit_percent = ((current_price - state.entry_price) / state.entry_price) * 100.0;
        state.peak_profit_percent = profit_percent.max(state.peak_profit_percent);

        // Check tiered profit targets
        for target in &self.config.targets {
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

        // Check time-based exit (after time_exit_hours if profitable)
        if let Ok(elapsed) = state.entry_time.elapsed() {
            if elapsed.as_secs() >= self.config.time_exit_hours * 3600 && profit_percent > 0.0 {
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
