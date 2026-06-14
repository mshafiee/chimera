//! Hard and dynamic stop-loss system
//!
//! Implements:
//! - Hard stop-loss at -15% (never let losses run)
//! - Dynamic stops (tighter for low-WQS wallets, wider for high-WQS)
//! - Portfolio-level stop (pause all trading if daily loss >5%)

use crate::config::ProfitManagementConfig;
use crate::db::{get_wallet_by_address, DbPool};
use crate::monitoring::SignalAggregator;
use crate::price_cache::PriceCache;
use rust_decimal::prelude::*;
use sqlx;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Stop-loss manager
pub struct StopLossManager {
    db: DbPool,
    config: Arc<ProfitManagementConfig>,
    price_cache: Arc<PriceCache>,
    /// Optional in-memory consensus cache (avoids per-position DB query every 5 s).
    /// Set via `set_signal_aggregator` after construction.
    signal_aggregator: Arc<RwLock<Option<Arc<SignalAggregator>>>>,
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
            signal_aggregator: Arc::new(RwLock::new(None)),
        }
    }

    /// Wire in the signal aggregator after construction so consensus checks read from
    /// the in-memory cache instead of issuing a DB query on every position tick.
    pub async fn set_signal_aggregator(&self, agg: Arc<SignalAggregator>) {
        *self.signal_aggregator.write().await = Some(agg);
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
        // Negative when price has fallen (e.g. -15.0 for 15% drop), matching negative thresholds.
        // Zero entry_price yields loss_percent=0 — no stop fires, position is held until data is
        // corrected. Forcing an exit on corrupt data risks selling at an unknown price.
        // Zero entry_price means the position was opened with corrupt data.
        // Holding a position with no cost basis is worse than exiting at market — force exit
        // immediately so the capital is recovered rather than locked indefinitely.
        if entry_price.is_zero() {
            tracing::error!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                "CORRUPT_POSITION: entry_price is zero — forcing immediate exit to recover capital"
            );
            return StopLossAction::Exit;
        }

        let loss_percent = {
            let diff = current_price - entry_price;
            let ratio = diff / entry_price;
            ratio * Decimal::from(100)
        };

        // Get wallet WQS for dynamic stop calculation
        let wallet_opt = get_wallet_by_address(&self.db, wallet_address).await;
        let wqs = match wallet_opt {
            Ok(Some(w)) => w.wqs_score.unwrap_or(50.0),
            _ => 50.0,
        };

        // Check if this is a consensus signal — read from SignalAggregator in-memory cache
        // (O(1), no DB query per position per 5-second tick).
        let is_consensus = {
            let agg_guard = self.signal_aggregator.read().await;
            if let Some(ref agg) = *agg_guard {
                agg.is_consensus_token(token_address).await
            } else {
                // Fallback DB query when aggregator not wired (startup window or tests)
                let count: i64 = sqlx::query_scalar(
                    r#"SELECT COUNT(DISTINCT wallet_address)
                       FROM signal_aggregation
                       WHERE token_address = ?
                         AND direction = 'BUY'
                         AND created_at > datetime('now', '-5 minutes')"#,
                )
                .bind(token_address)
                .fetch_one(&self.db)
                .await
                .unwrap_or(0);
                count >= 2
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

            stop_loss_threshold *= volatility_multiplier;

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
            stop_loss_threshold += consensus_adjustment; // Make it wider (e.g., -15% -> -20%)
            tracing::debug!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                consensus_threshold = %stop_loss_threshold,
                "Consensus signal detected, widening stop-loss by 5%"
            );
        }

        // The hard stop is the absolute maximum loss we allow.
        // The adaptive stop may widen, but never beyond the hard stop — cap it.
        // max() on negative numbers gives the TIGHTER (less negative) threshold.
        let hard_stop = self.config.hard_stop_loss;
        let effective_threshold = stop_loss_threshold.max(hard_stop);

        if loss_percent <= effective_threshold {
            return StopLossAction::Exit;
        }

        StopLossAction::None
    }

    /// Check portfolio-level stop (pause all trading if daily loss >5% of total capital).
    ///
    /// `total_capital_sol` must be the configured total trading capital, not active exposure.
    /// Using active exposure as the denominator shrinks as positions close, causing premature
    /// triggers during drawdowns — use the stable config value instead.
    pub async fn check_portfolio_stop(&self, total_capital_sol: Decimal) -> StopLossAction {
        // Use net_pnl_sol from exit trades (after fees/tips/slippage) so the portfolio stop
        // reflects true round-trip cost. Gross realized_pnl_sol on positions understates losses.
        let realized_pnl_f64: f64 = match sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(net_pnl_sol), 0.0)
            FROM trades
            WHERE side = 'SELL'
              AND status = 'CLOSED'
              AND net_pnl_sol IS NOT NULL
              AND updated_at >= datetime('now', '-24 hours')
            "#,
        )
        .fetch_one(&self.db)
        .await
        {
            Ok(pnl) => pnl,
            Err(e) => {
                tracing::error!(error = %e, "Failed to query daily realized PnL — pausing all trading (fail-safe)");
                return StopLossAction::PauseAll;
            }
        };

        // Also include current unrealized losses from open positions so the portfolio stop
        // fires during a flash crash where positions are still open and nothing has closed yet.
        let unrealized_pnl_f64: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(unrealized_pnl_sol), 0.0)
            FROM positions
            WHERE state IN ('ACTIVE', 'EXITING')
              AND unrealized_pnl_sol IS NOT NULL
            "#,
        )
        .fetch_one(&self.db)
        .await
        .unwrap_or(0.0);

        let daily_pnl = Decimal::from_f64_retain(realized_pnl_f64 + unrealized_pnl_f64)
            .unwrap_or(Decimal::ZERO);

        // Only check if total capital is meaningful (>0.1 SOL)
        let min_capital = Decimal::from_str("0.1").unwrap_or(Decimal::ZERO);
        if total_capital_sol > min_capital {
            let daily_loss_percent = (daily_pnl / total_capital_sol) * Decimal::from(100);

            let loss_threshold = Decimal::from_f64_retain(-5.0).unwrap_or(Decimal::ZERO);
            if daily_loss_percent < loss_threshold {
                tracing::warn!(
                    daily_loss_percent = %daily_loss_percent,
                    daily_pnl = %daily_pnl,
                    total_capital_sol = %total_capital_sol,
                    "Portfolio-level stop triggered: 24h loss exceeds 5% of capital"
                );
                return StopLossAction::PauseAll;
            }
        }

        StopLossAction::None
    }
}
