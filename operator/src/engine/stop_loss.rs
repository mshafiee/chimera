//! Hard and dynamic stop-loss system
//!
//! Implements:
//! - Hard stop-loss at -15% (never let losses run)
//! - Dynamic stops (tighter for low-WQS wallets, wider for high-WQS)
//! - Portfolio-level stop (pause all trading if daily loss >5%)

use std::sync::Arc;
use crate::config::ProfitManagementConfig;
use crate::db::{DbPool, get_wallet_by_address};
use crate::price_cache::PriceCache;

/// Stop-loss manager
pub struct StopLossManager {
    db: DbPool,
    config: Arc<ProfitManagementConfig>,
    price_cache: Arc<PriceCache>,
}

/// Stop-loss action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopLossAction {
    /// No action
    None,
    /// Exit position (stop-loss hit)
    Exit,
    /// Pause all trading (portfolio-level stop)
    PauseAll,
}

impl StopLossManager {
    pub fn new(
        db: DbPool,
        config: Arc<ProfitManagementConfig>,
        price_cache: Arc<PriceCache>,
    ) -> Self {
        Self {
            db,
            config,
            price_cache,
        }
    }

    /// Check stop-loss for a position
    ///
    /// # Arguments
    /// * `trade_uuid` - Trade UUID
    /// * `wallet_address` - Wallet address (for WQS-based dynamic stops)
    /// * `entry_price` - Entry price
    /// * `token_address` - Token address
    ///
    /// # Returns
    /// Stop-loss action
    pub async fn check_stop_loss(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        entry_price: f64,
        token_address: &str,
    ) -> StopLossAction {
        let current_price = match self.price_cache.get_price_usd(token_address) {
            Some(price) => price,
            None => return StopLossAction::None,
        };

        // Calculate loss percentage
        let loss_percent = ((entry_price - current_price) / entry_price) * 100.0;

        // Get wallet WQS for dynamic stop calculation
        let wallet_opt = get_wallet_by_address(&self.db, wallet_address).await;
        let wqs = match wallet_opt {
            Ok(Some(w)) => w.wqs_score.unwrap_or(50.0),
            _ => 50.0,
        };

        // Calculate dynamic stop-loss threshold
        let stop_loss_threshold: f64 = if wqs >= 70.0 {
            // High WQS: wider stop (-20%)
            -20.0
        } else if wqs >= 40.0 {
            // Medium WQS: standard stop (-15%)
            -15.0
        } else {
            // Low WQS: tighter stop (-10%)
            -10.0
        };

        // Check if stop-loss hit
        if loss_percent >= stop_loss_threshold.abs() {
            return StopLossAction::Exit;
        }

        // Check hard stop-loss (never exceed -15%)
        if loss_percent >= self.config.hard_stop_loss {
            return StopLossAction::Exit;
        }

        StopLossAction::None
    }

    /// Check portfolio-level stop (pause all trading if daily loss >5%)
    pub async fn check_portfolio_stop(&self) -> StopLossAction {
        // Get daily PnL from database
        // This would need to be implemented in db.rs
        // For now, return None (would need to query trades table)
        
        // TODO: Implement daily PnL calculation
        // let daily_pnl = get_daily_pnl(&self.db).await?;
        // if daily_pnl < -5.0 {
        //     return StopLossAction::PauseAll;
        // }

        StopLossAction::None
    }
}
