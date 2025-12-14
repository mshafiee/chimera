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
use rust_decimal::prelude::*;
use sqlx;

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
    /// * `entry_price` - Entry price (using Decimal for precision)
    /// * `token_address` - Token address
    ///
    /// # Returns
    /// Stop-loss action
    pub async fn check_stop_loss(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        entry_price: Decimal,
        token_address: &str,
    ) -> StopLossAction {
        let current_price = match self.price_cache.get_price_usd(token_address) {
            Some(price) => price,
            None => return StopLossAction::None,
        };

        // Calculate loss percentage using Decimal for precision
        let loss_percent = if !entry_price.is_zero() {
            let diff = entry_price - current_price;
            let ratio = diff / entry_price;
            ratio * Decimal::from(100)
        } else {
            Decimal::ZERO
        };

        // Get wallet WQS for dynamic stop calculation
        let wallet_opt = get_wallet_by_address(&self.db, wallet_address).await;
        let wqs = match wallet_opt {
            Ok(Some(w)) => w.wqs_score.unwrap_or(50.0),
            _ => 50.0,
        };

        // Check if this is a consensus signal (multiple wallets buying same token)
        let is_consensus = {
            // Query signal_aggregation table for recent consensus signals on this token
            let consensus_count: Result<i64, _> = sqlx::query_scalar(
                r#"
                SELECT COUNT(DISTINCT wallet_address)
                FROM signal_aggregation
                WHERE token_address = ?
                  AND direction = 'BUY'
                  AND created_at > datetime('now', '-5 minutes')
                "#
            )
            .bind(token_address)
            .fetch_one(&self.db)
            .await;
            
            match consensus_count {
                Ok(count) => count >= 2, // 2+ wallets = consensus
                Err(_) => false, // On error, assume not consensus
            }
        };

        // Calculate base dynamic stop-loss threshold using Decimal for precision
        // For consensus signals, use wider stops (lower risk of false signal)
        // Use Decimal constants to avoid f64 precision issues
        let mut stop_loss_threshold = if wqs >= 70.0 {
            // High WQS: wider stop (-20%)
            Decimal::from_str("-20.0").unwrap_or(Decimal::ZERO)
        } else if wqs >= 40.0 {
            // Medium WQS: standard stop (-15%)
            Decimal::from_str("-15.0").unwrap_or(Decimal::ZERO)
        } else {
            // Low WQS: tighter stop (-10%)
            Decimal::from_str("-10.0").unwrap_or(Decimal::ZERO)
        };
        
        // Adaptive stop-loss: adjust based on token volatility (ATR-like calculation)
        // If token is highly volatile, widen stops to avoid getting wicked out
        if let Some(volatility) = self.price_cache.calculate_volatility(token_address) {
            // Volatility is returned as percentage (e.g., 15.0 = 15%)
            // If volatility > 20%, widen stop by 1.5x
            // If volatility > 30%, widen stop by 2x
            // If volatility < 10%, tighten stop by 0.9x (but never below -5%)
            // Use Decimal constants to avoid f64 precision issues
            let volatility_multiplier = if volatility > 30.0 {
                Decimal::from_str("2.0").unwrap_or(Decimal::ONE)
            } else if volatility > 20.0 {
                Decimal::from_str("1.5").unwrap_or(Decimal::ONE)
            } else if volatility < 10.0 {
                Decimal::from_str("0.9").unwrap_or(Decimal::ONE)
            } else {
                Decimal::ONE
            };
            
            stop_loss_threshold = stop_loss_threshold * volatility_multiplier;
            
            // Ensure stop never goes below -5% (too tight) or above -50% (too wide)
            let min_threshold = Decimal::from_str("-50.0").unwrap_or(Decimal::ZERO);
            let max_threshold = Decimal::from_str("-5.0").unwrap_or(Decimal::ZERO);
            stop_loss_threshold = stop_loss_threshold.max(min_threshold).min(max_threshold);
            
            tracing::debug!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                volatility_percent = volatility,
                adjusted_threshold = %stop_loss_threshold,
                "Adaptive stop-loss adjusted based on volatility"
            );
        }
        
        // Widen stop-loss by 5% for consensus signals
        if is_consensus {
            let consensus_adjustment = Decimal::from_str("-5.0").unwrap_or(Decimal::ZERO);
            stop_loss_threshold = stop_loss_threshold + consensus_adjustment; // Make it wider (e.g., -15% -> -20%)
            tracing::debug!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                consensus_threshold = %stop_loss_threshold,
                "Consensus signal detected, widening stop-loss by 5%"
            );
        }

        // Check if stop-loss hit (compare using Decimal for precision)
        // loss_percent is negative when losing, stop_loss_threshold is also negative
        // We want to exit when loss_percent <= stop_loss_threshold (more negative)
        if loss_percent <= stop_loss_threshold {
            return StopLossAction::Exit;
        }

        // Check hard stop-loss (config value is already Decimal)
        let hard_stop = self.config.hard_stop_loss;
        if loss_percent <= hard_stop {
            return StopLossAction::Exit;
        }

        StopLossAction::None
    }

    /// Check portfolio-level stop (pause all trading if daily loss >5%)
    pub async fn check_portfolio_stop(&self) -> StopLossAction {
        // Get daily realized PnL (convert from database f64 to Decimal)
        let daily_pnl_f64: f64 = match sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(realized_pnl_sol), 0.0)
            FROM positions
            WHERE DATE(closed_at) = DATE('now')
            "#
        )
        .fetch_one(&self.db)
        .await
        {
            Ok(pnl) => pnl,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to query daily PnL, skipping portfolio stop check");
                return StopLossAction::None;
            }
        };
        let daily_pnl = Decimal::from_f64_retain(daily_pnl_f64).unwrap_or(Decimal::ZERO);

        // Get total exposure from active positions (convert from database f64 to Decimal)
        // Note: For accurate calculation, you'd need total capital including available balance
        let total_exposure_f64: f64 = match sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(entry_amount_sol), 0.0)
            FROM positions
            WHERE state = 'ACTIVE'
            "#
        )
        .fetch_one(&self.db)
        .await
        {
            Ok(exposure) => exposure,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to query total exposure, skipping portfolio stop check");
                return StopLossAction::None;
            }
        };
        let total_exposure = Decimal::from_f64_retain(total_exposure_f64).unwrap_or(Decimal::ZERO);

        // Calculate daily loss percentage using Decimal for precision
        // Only check if we have meaningful exposure (>0.1 SOL)
        let min_exposure = Decimal::from_str("0.1").unwrap_or(Decimal::ZERO);
        if total_exposure > min_exposure {
            let daily_loss_percent = if !total_exposure.is_zero() {
                (daily_pnl / total_exposure) * Decimal::from(100)
            } else {
                Decimal::ZERO
            };
            
            let loss_threshold = Decimal::from_f64_retain(-5.0).unwrap_or(Decimal::ZERO);
            if daily_loss_percent < loss_threshold {
                tracing::warn!(
                    daily_loss_percent = %daily_loss_percent,
                    daily_pnl = %daily_pnl,
                    total_exposure = %total_exposure,
                    "Portfolio-level stop triggered: daily loss exceeds 5%"
                );
                return StopLossAction::PauseAll;
            }
        }

        StopLossAction::None
    }
}
