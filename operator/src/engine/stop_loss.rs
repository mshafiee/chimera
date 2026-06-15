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
use rust_decimal_macros::dec;
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
    /// Cache for daily *realized* PnL only: (timestamp_secs, realized_pnl_sol).
    /// Unrealized PnL is NOT cached — it must be queried fresh every tick so that
    /// a flash crash (positions going deeply negative within seconds) triggers the
    /// portfolio stop without waiting for the cache TTL to expire.
    realized_pnl_cache: RwLock<Option<(i64, Decimal)>>,
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
            realized_pnl_cache: RwLock::new(None),
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
        entry_time: chrono::DateTime<chrono::Utc>,
    ) -> StopLossAction {
        let current_price = match self.price_cache.get_price_usd(token_address) {
            Some(price) => price,
            None => {
                // §1.5 FIX: If this token is actively tracked but hasn't received a
                // price update in >2 minutes, force exit. Without a price feed, all
                // stop-loss protection is silently disabled — the position could lose
                // 100% before the feed recovers. Forcing exit is safer than holding
                // an unmonitored position.
                if self.price_cache.is_price_stale(token_address) {
                    tracing::error!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        "STALE_PRICE: no price update for >2 min on tracked token — forcing exit (risk management blind)"
                    );
                    return StopLossAction::Exit;
                }
                return StopLossAction::None;
            }
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

        // Calculate base dynamic stop-loss threshold using compile-time Decimal constants.
        // High-WQS wallets get wider stops to let proven signals breathe; low-WQS gets tighter.
        let mut stop_loss_threshold = if wqs >= 70.0 {
            dec!(-20) // High WQS: wider stop
        } else if wqs >= 40.0 {
            dec!(-15) // Medium WQS: standard stop
        } else {
            dec!(-10) // Low WQS: tighter stop
        };

        // Adaptive stop-loss: adjust based on token volatility (ATR-like calculation).
        // If token is highly volatile, widen stops to avoid getting wicked out.
        if let Some(volatility) = self.price_cache.calculate_volatility(token_address) {
            // Volatility is returned as percentage (e.g., 15.0 = 15%)
            // If volatility > 20%, widen stop by 1.5x
            // If volatility > 30%, widen stop by 2x
            // If volatility < 10%, tighten stop by 0.9x
            let volatility_multiplier = if volatility > 30.0 {
                dec!(2.0)
            } else if volatility > 20.0 {
                dec!(1.5)
            } else if volatility < 10.0 {
                dec!(0.9)
            } else {
                Decimal::ONE
            };

            stop_loss_threshold *= volatility_multiplier;

            tracing::debug!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                volatility_percent = volatility,
                adjusted_threshold = %stop_loss_threshold,
                "Adaptive stop-loss adjusted based on volatility"
            );
        }

        // Widen stop-loss for consensus signals (applied after volatility).
        // Use a proportional 25% widening instead of a flat -5% so that tight stops
        // receive smaller absolute widening than wide stops — a flat -5% on a -10% base
        // would be a 50% widening, disproportionate relative to a -20% base.
        // A second clamp is applied immediately after so the combined result respects the envelope.
        if is_consensus {
            stop_loss_threshold = stop_loss_threshold * dec!(1.25); // widen by 25% of current threshold
            tracing::debug!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                consensus_threshold = %stop_loss_threshold,
                "Consensus signal detected, widening stop-loss by 5%"
            );
        }

        // Final clamp: never tighter than -5% or wider than -35%.
        // Tightened from -50% to -35% — at 20% portfolio heat cap, a single -50% stop
        // would wipe 10% of total capital. The max_stop_loss_distance config field lets
        // operators override this per-deployment if they deliberately want wider stops.
        // Applied after ALL adjustments (volatility × + consensus ×) so every combination
        // respects the envelope. widest_stop (-35) is numerically smaller; tightest_stop (-5) larger.
        let widest_stop   = dec!(-35).min(self.config.max_stop_loss_distance);
        let tightest_stop = dec!(-5);
        stop_loss_threshold = stop_loss_threshold.max(widest_stop).min(tightest_stop);

        // max_stop_loss_distance is the floor on loss tolerance — the adaptive stop
        // may widen due to volatility/consensus, but never past this value.
        // max() on negative numbers gives the TIGHTER (less negative) threshold.
        let effective_threshold = stop_loss_threshold.max(self.config.max_stop_loss_distance);

        // Warn when max_stop_loss_distance overrides adaptive widening so the operator can
        // see in logs that volatile/consensus tokens are being stopped tighter than intended.
        // To allow adaptive stops to breathe, set max_stop_loss_distance to a larger negative
        // value (e.g. -50) in config.yaml.
        if self.config.max_stop_loss_distance > stop_loss_threshold {
            tracing::warn!(
                trade_uuid = %trade_uuid,
                adaptive_threshold = %stop_loss_threshold,
                max_stop_loss_distance = %self.config.max_stop_loss_distance,
                effective_threshold = %effective_threshold,
                "Adaptive stop-loss widening overridden by max_stop_loss_distance; \
                 set max_stop_loss_distance to a larger negative (e.g. -50) to let adaptive stops breathe"
            );
        }

        if loss_percent <= effective_threshold {
            let elapsed_secs = chrono::Utc::now().signed_duration_since(entry_time).num_seconds();
            if elapsed_secs < self.config.wick_protection_secs as i64 {
                tracing::info!(
                    trade_uuid = %trade_uuid,
                    elapsed_secs,
                    wick_protection_secs = self.config.wick_protection_secs,
                    loss_percent = %loss_percent,
                    "Stop-loss triggered but ignored due to entry grace period (wick protection)"
                );
                return StopLossAction::None;
            }
            return StopLossAction::Exit;
        }

        StopLossAction::None
    }

    /// Check portfolio-level stop (pause all trading if daily loss >5% of total capital).
    ///
    /// Must be called from the position monitoring loop in main.rs on every tick alongside
    /// `check_stop_loss` — it is the only path that returns `StopLossAction::PauseAll`.
    ///
    /// `total_capital_sol` must be the configured total trading capital, not active exposure.
    /// Using active exposure as the denominator shrinks as positions close, causing premature
    /// triggers during drawdowns — use the stable config value instead.
    pub async fn check_portfolio_stop(&self, total_capital_sol: Decimal) -> StopLossAction {
        let now_secs = chrono::Utc::now().timestamp();

        // Realized PnL: cache for 60 s. The trades-table aggregate scan is expensive
        // (full 24 h filter); realized PnL changes only when trades close, so a
        // 60-second cache window is safe. Cache hit avoids the expensive query.
        let cached = {
            let cache = self.realized_pnl_cache.read().await;
            cache.filter(|(ts, _)| now_secs - ts < 60).map(|(_, pnl)| pnl)
        };
        let realized_pnl: Decimal = match cached {
            Some(pnl) => {
                tracing::debug!("Using cached realized PnL");
                pnl
            }
            None => {
                // Use net_pnl_sol (after fees/tips/slippage) for true round-trip cost.
                match sqlx::query_scalar::<_, f64>(
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
                    Ok(pnl) => {
                        let pnl_dec = Decimal::from_f64_retain(pnl).unwrap_or(Decimal::ZERO);
                        *self.realized_pnl_cache.write().await = Some((now_secs, pnl_dec));
                        pnl_dec
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to query daily realized PnL — pausing all trading (fail-safe)");
                        return StopLossAction::PauseAll;
                    }
                }
            }
        };

        // Unrealized PnL: always queried fresh — NOT cached.
        // This is a lightweight SUM over a small set of active positions, and it must
        // be current so that a flash crash (positions going deeply negative within
        // seconds) triggers the portfolio stop without waiting for a cache TTL.
        let unrealized_pnl: Decimal = match sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(unrealized_pnl_sol), 0.0)
            FROM positions
            WHERE state IN ('ACTIVE', 'EXITING')
              AND unrealized_pnl_sol IS NOT NULL
            "#,
        )
        .fetch_one(&self.db)
        .await
        {
            Ok(pnl) => Decimal::from_f64_retain(pnl).unwrap_or(Decimal::ZERO),
            Err(e) => {
                tracing::error!(error = %e, "Failed to query unrealized PnL — pausing all trading (fail-safe)");
                return StopLossAction::PauseAll;
            }
        };

        let daily_pnl = realized_pnl + unrealized_pnl;
        self.evaluate_portfolio_stop(daily_pnl, total_capital_sol)
    }

    fn evaluate_portfolio_stop(&self, daily_pnl: Decimal, total_capital_sol: Decimal) -> StopLossAction {
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
