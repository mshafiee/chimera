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

        // Calculate base dynamic stop-loss threshold
        // For consensus signals, use wider stops (lower risk of false signal)
        let mut stop_loss_threshold: f64 = if wqs >= 70.0 {
            // High WQS: wider stop (-20%)
            -20.0
        } else if wqs >= 40.0 {
            // Medium WQS: standard stop (-15%)
            -15.0
        } else {
            // Low WQS: tighter stop (-10%)
            -10.0
        };
        
        // Adaptive stop-loss: adjust based on token volatility (ATR-like calculation)
        // If token is highly volatile, widen stops to avoid getting wicked out
        if let Some(volatility) = self.price_cache.calculate_volatility(token_address) {
            // Volatility is returned as percentage (e.g., 15.0 = 15%)
            // If volatility > 20%, widen stop by 1.5x
            // If volatility > 30%, widen stop by 2x
            // If volatility < 10%, tighten stop by 0.9x (but never below -5%)
            let volatility_multiplier = if volatility > 30.0 {
                2.0
            } else if volatility > 20.0 {
                1.5
            } else if volatility < 10.0 {
                0.9
            } else {
                1.0
            };
            
            stop_loss_threshold *= volatility_multiplier;
            
            // Ensure stop never goes below -5% (too tight) or above -50% (too wide)
            stop_loss_threshold = stop_loss_threshold.max(-50.0).min(-5.0);
            
            tracing::debug!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                volatility_percent = volatility,
                volatility_multiplier = volatility_multiplier,
                adjusted_threshold = stop_loss_threshold,
                "Adaptive stop-loss adjusted based on volatility"
            );
        }
        
        // Widen stop-loss by 5% for consensus signals
        if is_consensus {
            stop_loss_threshold -= 5.0; // Make it wider (e.g., -15% -> -20%)
            tracing::debug!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                original_threshold = stop_loss_threshold + 5.0,
                consensus_threshold = stop_loss_threshold,
                "Consensus signal detected, widening stop-loss by 5%"
            );
        }

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
        // Get daily realized PnL
        let daily_pnl: f64 = match sqlx::query_scalar::<_, f64>(
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

        // Get total exposure from active positions
        // Note: For accurate calculation, you'd need total capital including available balance
        let total_exposure: f64 = match sqlx::query_scalar::<_, f64>(
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

        // Calculate daily loss percentage
        // Only check if we have meaningful exposure (>0.1 SOL)
        if total_exposure > 0.1 {
            let daily_loss_percent = (daily_pnl / total_exposure) * 100.0;
            
            if daily_loss_percent < -5.0 {
                tracing::warn!(
                    daily_loss_percent = daily_loss_percent,
                    daily_pnl = daily_pnl,
                    total_exposure = total_exposure,
                    "Portfolio-level stop triggered: daily loss exceeds 5%"
                );
                return StopLossAction::PauseAll;
            }
        }

        StopLossAction::None
    }
}
