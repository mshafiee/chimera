//! Kelly Criterion Position Sizing
//!
//! Implements Kelly Criterion for optimal position sizing using the standard
//! edge/odds form: k = (p*b - q) / b  where b = avg_win / avg_loss.
//!
//! Hard-caps full_kelly at 0.5 (50%) to prevent ruin-level allocations even
//! for exceptionally high-edge wallets. Uses conservative fraction (default 25%)
//! of full Kelly for actual sizing.

use crate::db_abstraction::Database;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::sync::Arc;

/// Kelly position sizer
pub struct KellySizer {
    db: Arc<dyn Database>,
    /// Conservative multiplier (use 25% of full Kelly, using Decimal for precision)
    conservative_multiplier: Decimal,
}

/// Kelly sizing result
#[derive(Debug, Clone)]
pub struct KellyResult {
    /// Full Kelly percentage (capped at 0.5 / 50%, using Decimal for precision)
    pub full_kelly: Decimal,
    /// Conservative Kelly percentage (25% of full, max 1.0, using Decimal for precision)
    pub conservative_kelly: Decimal,
    /// Recommended position size as percentage of capital (using Decimal for precision)
    pub recommended_size_percent: Decimal,
    /// Win rate (0.0-1.0, using Decimal for precision)
    pub win_rate: Decimal,
    /// Empirical loss rate (loss_count / total valid trades, 0.0-1.0).
    /// Break-even trades are counted in the total but in neither win nor loss,
    /// so `loss_rate` is NOT `1 - win_rate`.
    pub loss_rate: Decimal,
    /// Average win amount (using Decimal for precision)
    pub avg_win: Decimal,
    /// Average loss amount (using Decimal for precision)
    pub avg_loss: Decimal,
    /// Number of closed trades used to compute this result
    pub trade_count: usize,
    /// Velocity multiplier based on trade frequency
    pub velocity_multiplier: Decimal,
}

/// Growth-optimal Kelly fraction for per-trade return percentages.
///
/// `f* = (p·avg_win − q·avg_loss) / (avg_win·avg_loss)`.
///
/// This is the classic edge/odds form `(p·b − q)/b` only when a losing trade
/// costs the whole stake (avg_loss = 1). With partial losses the growth-optimal
/// allocation is a factor `1/avg_loss` larger. Result is unbounded; callers
/// clamp to their risk envelope.
fn compute_full_kelly(p: Decimal, q: Decimal, avg_win: Decimal, avg_loss: Decimal) -> Decimal {
    if avg_win.is_zero() || avg_loss.is_zero() {
        return Decimal::ZERO;
    }
    (p * avg_win - q * avg_loss) / (avg_win * avg_loss)
}

impl KellySizer {
    /// Create a new Kelly sizer
    pub fn new(db: Arc<dyn Database>) -> Self {
        Self {
            db,
            conservative_multiplier: dec!(0.25), // Use 25% of full Kelly
        }
    }

    /// Create with custom conservative multiplier
    pub fn with_conservative_multiplier(db: Arc<dyn Database>, multiplier: f64) -> Self {
        let mult = Decimal::from_f64_retain(multiplier.clamp(0.0, 1.0)).unwrap_or(Decimal::ZERO);
        Self {
            db,
            conservative_multiplier: mult,
        }
    }

    /// Calculate Kelly Criterion for a wallet
    ///
    /// # Arguments
    /// * `wallet_address` - Wallet address to calculate Kelly for
    /// * `lookback_days` - Number of days to look back for historical trades
    ///
    /// # Returns
    /// KellyResult with sizing recommendations
    pub async fn calculate_kelly(
        &self,
        wallet_address: &str,
        strategy: chimera_core::models::Strategy,
        lookback_days: i64,
    ) -> Result<KellyResult, String> {
        // Get historical trades for this wallet
        let from_date = chrono::Utc::now() - chrono::Duration::days(lookback_days);
        let from_date_str = from_date.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let strategy_str = strategy.to_string();

        let trades = self
            .db
            .get_trades_filtered(
                Some(&from_date_str),
                None,
                Some("CLOSED"),
                Some(&strategy_str),
                Some(wallet_address),
                i64::MAX,
                0,
            )
            .await
            .map_err(|e| format!("Failed to query trades: {}", e))?;

        if trades.is_empty() {
            return Err("No historical trades found for Kelly calculation".to_string());
        }

        // Calculate win rate and average win/loss
        let mut wins = Vec::new();
        let mut losses = Vec::new();
        let mut valid_trades_count = 0;

        for trade in &trades {
            // A3: exclude rows quarantined from the pre-fix accounting model —
            // their PnL is not decision-grade evidence.
            if !trade.pnl_data_valid {
                continue;
            }
            if let Some(pnl) = trade.net_pnl_sol {
                let entry_size = trade.amount_sol;
                if !entry_size.is_zero() {
                    let pnl_pct = pnl / entry_size;
                    if pnl > Decimal::ZERO {
                        // Cap individual wins at 300% (3.0) to prevent outliers from skewing avg_win
                        let capped_pnl_pct = pnl_pct.min(Decimal::from(3));
                        wins.push(capped_pnl_pct);
                        valid_trades_count += 1;
                    } else if pnl < Decimal::ZERO {
                        losses.push(pnl_pct.abs()); // Store as positive for calculation
                        valid_trades_count += 1;
                    } else {
                        // Break-even trade: include in valid_trades_count so wallets with
                        // many break-even positions (e.g. grid/market-making strategies)
                        // can still reach the minimum threshold for Kelly sizing.
                        valid_trades_count += 1;
                    }
                }
            }
        }

        let total_trades = Decimal::from(valid_trades_count);
        let win_count = Decimal::from(wins.len());
        let loss_count = Decimal::from(losses.len());

        if total_trades.is_zero() {
            return Err("No valid trades for Kelly calculation".to_string());
        }

        if valid_trades_count < 15 {
            return Err(format!(
                "Insufficient trade history for reliable Kelly calculation ({valid_trades_count} trades, need ≥15)"
            ));
        }

        let win_rate = win_count / total_trades;
        let loss_rate = loss_count / total_trades;

        let avg_win = if wins.is_empty() {
            Decimal::ZERO
        } else {
            let sum: Decimal = wins.iter().sum();
            sum / Decimal::from(wins.len())
        };

        let avg_loss = if losses.is_empty() {
            // No loss history yet (all trades were wins): use a conservative 15% assumed
            // loss per trade to prevent ruin-level Kelly allocations on wallets with
            // pure win streaks. Without this, avg_loss=0 collapses the formula to
            // full_kelly = win_rate (e.g. 90% for a 90% win-rate wallet — catastrophic).
            // This matches the Shield stop-loss depth and is revised downward as actual
            // loss data accumulates.
            dec!(0.15)
        } else {
            let sum: Decimal = losses.iter().sum();
            // Enforce a 1% floor: extremely tight stop-losses produce avg_loss → 0,
            // causing Kelly → win_rate and ignoring actual downside risk.
            (sum / Decimal::from(losses.len())).max(dec!(0.01))
        };

        // Calculate Kelly Criterion for fractional per-trade returns:
        //   f* = (p * avg_win - q * avg_loss) / (avg_win * avg_loss)
        // avg_win/avg_loss are per-trade return fractions (pnl / amount_sol),
        // NOT full-stake odds, so the classic (p*b - q)/b form (which assumes a
        // loss costs the whole stake) would under-allocate by ~1/avg_loss.
        // Hard-cap full_kelly at 0.5 (50%): even wallets with extreme edges must
        // never risk more than half the bankroll on a single trade. Copy-trading
        // edge estimates are inherently unreliable — full Kelly near 100% invites ruin.
        let full_kelly = compute_full_kelly(win_rate, loss_rate, avg_win, avg_loss)
            .max(Decimal::ZERO)
            .min(dec!(0.5));

        // Trade velocity confidence: a wallet with the same win rate is statistically
        // more reliable when it generates more trades per day because each outcome is
        // an independent sample that tightens the confidence interval on the true win rate.
        // Scale the conservative Kelly fraction — never push past full Kelly.
        //   < 0.5 trades/day  → 0.80× (sparse history, widen caution margin)
        //   0.5–1 trades/day  → 1.00× (neutral)
        //   1–2  trades/day   → 1.15× (good statistical depth)
        //   ≥ 2  trades/day   → 1.25× (high frequency, tighter confidence interval)
        // The span is computed over the same trade set as valid_trades_count
        // (pnl-valid rows only), taking the min/max over the returned rows so
        // an unsorted backend cannot produce a negative/meaningless span.
        let parse_time = |s: &str| -> Option<chrono::DateTime<chrono::Utc>> {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|d| d.with_timezone(&chrono::Utc))
                .ok()
                .or_else(|| {
                    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()?;
                    Some(chrono::DateTime::from_naive_utc_and_offset(
                        naive,
                        chrono::Utc,
                    ))
                })
        };
        let mut newest_time: Option<chrono::DateTime<chrono::Utc>> = None;
        let mut oldest_time: Option<chrono::DateTime<chrono::Utc>> = None;
        for trade in &trades {
            if !trade.pnl_data_valid {
                continue;
            }
            if let Some(t) = parse_time(&trade.created_at) {
                newest_time = Some(newest_time.map_or(t, |n| n.max(t)));
                oldest_time = Some(oldest_time.map_or(t, |o| o.min(t)));
            }
        }
        let true_timespan_days = if let (Some(newest_time), Some(oldest_time)) =
            (newest_time, oldest_time)
        {
            let span = (newest_time - oldest_time).num_seconds() as f64 / 86400.0;
            span.min(lookback_days as f64).max(1.0)
        } else {
            lookback_days as f64
        };

        let trades_per_day = if true_timespan_days > 0.0 {
            valid_trades_count as f64 / true_timespan_days
        } else {
            0.0
        };
        let velocity_multiplier = if trades_per_day >= 2.0 {
            dec!(1.25)
        } else if trades_per_day >= 1.0 {
            dec!(1.15)
        } else if trades_per_day >= 0.5 {
            Decimal::ONE
        } else {
            dec!(0.8)
        };

        // Apply velocity multiplier to full Kelly first, then apply conservative multiplier.
        // The velocity boost CAN exceed full Kelly — this is intentional, so that high-velocity
        // regimes amplify sizing. The downstream .min(full_kelly) on conservative_kelly
        // prevents the final recommendation from exceeding full Kelly.
        let velocity_boosted_kelly = full_kelly * velocity_multiplier;
        let conservative_kelly = (velocity_boosted_kelly * self.conservative_multiplier)
            .min(full_kelly)
            .min(Decimal::ONE); // Clamp to 100% of capital for the recommendation

        Ok(KellyResult {
            full_kelly,
            conservative_kelly,
            // recommended_size_percent is simply conservative_kelly * 100 —
            // avg_loss is embedded in the Kelly formula's denominator
            // (avg_win*avg_loss), so no further division is needed here.
            recommended_size_percent: conservative_kelly * Decimal::from(100),
            win_rate,
            loss_rate,
            avg_win,
            avg_loss,
            trade_count: valid_trades_count,
            velocity_multiplier,
        })
    }

    /// Calculate position size in SOL based on Kelly
    ///
    /// # Arguments
    /// * `wallet_address` - Wallet address
    /// * `total_capital_sol` - Total capital available (using Decimal for precision)
    /// * `lookback_days` - Number of days to look back
    ///
    /// # Returns
    /// Recommended position size in SOL (using Decimal for precision)
    pub async fn calculate_position_size(
        &self,
        wallet_address: &str,
        strategy: chimera_core::models::Strategy,
        total_capital_sol: Decimal,
        lookback_days: i64,
    ) -> Result<Decimal, String> {
        let kelly = self
            .calculate_kelly(wallet_address, strategy, lookback_days)
            .await?;
        // Do NOT divide by avg_loss here. conservative_kelly already incorporates
        // avg_loss through the Kelly formula denominator (avg_win*avg_loss).
        // Dividing again by avg_loss double-penalises the position size.
        let size_sol = total_capital_sol * kelly.conservative_kelly;
        Ok(size_sol)
    }
}

impl KellyResult {
    /// Calculate expected return percentage from Kelly metrics
    ///
    /// Formula: (win_rate * avg_win_pct) - (loss_rate * avg_loss_pct)
    ///
    /// This represents the expected profit/loss percentage per trade based on
    /// historical performance. For example, a return of 0.05 means 5% expected
    /// profit per trade on average.
    ///
    /// # Returns
    /// Expected return as a decimal (e.g., 0.05 = 5%)
    pub fn expected_return_pct(&self) -> Decimal {
        // Use the empirical loss rate: break-even trades must not be
        // reclassified as full average losses (`1 - win_rate` would do exactly
        // that whenever win_rate + loss_rate < 1).
        let expected_win = self.win_rate * self.avg_win;
        let expected_loss = self.loss_rate * self.avg_loss;
        expected_win - expected_loss
    }

    /// Calculate expected profit in SOL for a given position size
    ///
    /// Formula: position_size_sol * expected_return_pct
    ///
    /// This gives the actual expected profit in SOL for a specific position size,
    /// which should be compared against transaction costs (tip, fees, slippage)
    /// to determine if a trade is mathematically profitable.
    ///
    /// # Arguments
    /// * `position_size_sol` - Position size in SOL
    ///
    /// # Returns
    /// Expected profit in SOL
    pub fn expected_profit_sol(&self, position_size_sol: Decimal) -> Decimal {
        position_size_sol * self.expected_return_pct()
    }
}

#[cfg(test)]
pub(crate) mod tests {

    use super::*;
    use crate::db_abstraction::*;
    use async_trait::async_trait;
    use chimera_core::error::{AppError, AppResult};
    use parking_lot::RwLock;
    use std::collections::HashMap;

    // =====================================================================
    // Shared MockDatabase for infra unit tests.
    //
    // Configurable responses for every Database method exercised by infra
    // unit tests; all other trait methods panic via `unimplemented!()`.
    // Test-only (cfg(test)), never compiled into production builds.
    // =====================================================================

    #[derive(Default)]
    pub(crate) struct MockDatabase {
        pub trades_filtered: RwLock<Vec<TradeDetail>>,
        pub fail_trades_filtered: RwLock<bool>,
        pub active_positions: RwLock<Vec<Position>>,
        pub fail_active_positions: RwLock<bool>,
        pub trades_by_status: RwLock<HashMap<String, Vec<Trade>>>,
        pub wallets_by_status: RwLock<HashMap<String, Vec<Wallet>>>,
        pub recent_jito_tips: RwLock<Vec<Decimal>>,
        pub jito_tip_count: RwLock<u32>,
        pub inserted_tips: RwLock<Vec<(Decimal, Option<String>, Option<String>, bool)>>,
        pub total_realized_pnl: RwLock<Decimal>,
        pub wallets: RwLock<HashMap<String, Wallet>>,
        pub wallet_monitoring: RwLock<HashMap<String, WalletMonitoring>>,
        pub wallet_details: RwLock<Vec<WalletDetail>>,
        pub all_wallet_monitoring: RwLock<Vec<WalletMonitoring>>,
        pub active_wallets_with_webhook_ids: RwLock<Vec<(String, String)>>,
        pub wallets_needing_registration: RwLock<Vec<String>>,
        pub cleared_webhook_ids: RwLock<Vec<String>>,
        pub status_updates: RwLock<Vec<(String, String)>>,
        pub demotion_counts: RwLock<HashMap<String, i32>>,
        pub incremented_demotions: RwLock<Vec<String>>,
        pub config_updates: RwLock<Vec<(String, String, String)>>,
        pub inserted_trades: RwLock<Vec<String>>,
        pub inserted_positions: RwLock<Vec<String>>,
        pub updated_trade_statuses: RwLock<Vec<String>>,
        pub updated_positions: RwLock<Vec<String>>,
        pub snapshots: RwLock<
            Vec<(
                Decimal,
                Decimal,
                Decimal,
                Decimal,
                i32,
                Option<Decimal>,
                Option<String>,
            )>,
        >,
        pub deleted_snapshot_days: RwLock<Vec<i32>>,
        pub wallet_pnl_stats: RwLock<HashMap<String, Option<(i64, Decimal, Decimal)>>>,
    }

    #[allow(clippy::unimplemented)]
    #[async_trait]
    impl Database for MockDatabase {
        async fn close(&self) -> AppResult<()> {
            unimplemented!()
        }

        async fn run_migrations(&self) -> AppResult<()> {
            unimplemented!()
        }

        async fn startup_integrity_check(&self) -> AppResult<()> {
            unimplemented!()
        }

        async fn recover_executing_trades(&self) -> AppResult<u32> {
            unimplemented!()
        }

        async fn trade_uuid_exists(&self, _trade_uuid: &str) -> AppResult<bool> {
            unimplemented!()
        }

        async fn insert_trade(&self, trade: &InsertTrade) -> AppResult<i64> {
            self.inserted_trades.write().push(trade.trade_uuid.clone());
            Ok(1)
        }

        async fn update_trade_status(&self, update: &UpdateTradeStatus) -> AppResult<()> {
            self.updated_trade_statuses
                .write()
                .push(update.trade_uuid.clone());
            Ok(())
        }

        async fn get_trade_by_uuid(&self, _trade_uuid: &str) -> AppResult<Option<Trade>> {
            unimplemented!()
        }

        async fn get_queued_trades(&self, _limit: i32) -> AppResult<Vec<Trade>> {
            unimplemented!()
        }

        async fn get_trades_by_status(&self, status: &str, _limit: i32) -> AppResult<Vec<Trade>> {
            Ok(self
                .trades_by_status
                .read()
                .get(status)
                .cloned()
                .unwrap_or_default())
        }

        async fn insert_position(&self, position: &InsertPosition) -> AppResult<i64> {
            self.inserted_positions
                .write()
                .push(position.trade_uuid.clone());
            Ok(1)
        }

        async fn update_position(&self, update: &UpdatePosition) -> AppResult<()> {
            self.updated_positions
                .write()
                .push(update.trade_uuid.clone());
            Ok(())
        }

        async fn get_active_positions(&self) -> AppResult<Vec<Position>> {
            if *self.fail_active_positions.read() {
                return Err(AppError::Internal("mock active positions failure".into()));
            }
            Ok(self.active_positions.read().clone())
        }

        async fn get_position_by_trade_uuid(&self, _trade_uuid: &str) -> AppResult<Option<Position>> {
            unimplemented!()
        }

        async fn get_active_position_by_wallet_token(
            &self,
            _wallet_address: &str,
            _token_address: &str,
        ) -> AppResult<Option<Position>> {
            unimplemented!()
        }

        async fn get_unresolved_trade_by_wallet_token(
            &self,
            _wallet_address: &str,
            _token_address: &str,
        ) -> AppResult<Option<String>> {
            unimplemented!()
        }

        async fn close_position(
            &self,
            _trade_uuid: &str,
            _exit_price: Decimal,
            _exit_tx_signature: &str,
            _realized_pnl_sol: Decimal,
            _realized_pnl_usd: Decimal,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn force_close_orphan_position(&self, _trade_uuid: &str, _reason: &str) -> AppResult<()> {
            unimplemented!()
        }

        async fn get_wallet(&self, address: &str) -> AppResult<Option<Wallet>> {
            Ok(self.wallets.read().get(address).cloned())
        }

        async fn get_active_wallets(&self) -> AppResult<Vec<Wallet>> {
            Ok(self.wallets_by_status.read().get("ACTIVE").cloned().unwrap_or_default())
        }

        async fn update_wallet_status(&self, address: &str, status: &str) -> AppResult<()> {
            self.status_updates
                .write()
                .push((address.to_string(), status.to_string()));
            Ok(())
        }

        async fn merge_roster(&self, _roster_db_path: &str) -> AppResult<u32> {
            unimplemented!()
        }

        async fn get_wallets_by_status(&self, status: &str) -> AppResult<Vec<Wallet>> {
            Ok(self
                .wallets_by_status
                .read()
                .get(status)
                .cloned()
                .unwrap_or_default())
        }

        async fn get_wallets_by_conviction_tier(
            &self,
            _tier: chimera_core::config::ConvictionTier,
        ) -> AppResult<Vec<Wallet>> {
            unimplemented!()
        }

        async fn get_wallets_with_wqs(
            &self,
            _status: Option<&str>,
            _min_wqs: Option<i32>,
            _max_wqs: Option<i32>,
        ) -> AppResult<Vec<Wallet>> {
            unimplemented!()
        }

        async fn get_promotion_candidates(
            &self,
            _min_wqs: f64,
            _max_age_days: i64,
            _limit: i64,
        ) -> AppResult<Vec<Wallet>> {
            unimplemented!()
        }

        async fn demote_dormant_active_wallets(&self, _max_age_days: i64) -> AppResult<u64> {
            unimplemented!()
        }

        async fn get_circuit_breaker_state(&self) -> AppResult<CircuitBreakerState> {
            unimplemented!()
        }

        async fn update_circuit_breaker_state(
            &self,
            _state: &str,
            _tripped_at: Option<chrono::DateTime<chrono::Utc>>,
            _trip_reason: Option<&str>,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn get_kill_switch_state(&self) -> AppResult<KillSwitchState> {
            unimplemented!()
        }

        async fn set_kill_switch_state(&self, _state: &str, _reason: Option<&str>) -> AppResult<()> {
            unimplemented!()
        }

        async fn insert_dlq(
            &self,
            _trade_uuid: Option<&str>,
            _payload: &str,
            _reason: &str,
            _error_details: Option<&str>,
            _source_ip: Option<&str>,
        ) -> AppResult<i64> {
            unimplemented!()
        }

        async fn get_admin_wallet_role(&self, _wallet_address: &str) -> AppResult<Option<String>> {
            unimplemented!()
        }

        async fn get_trade_statistics(&self) -> AppResult<TradeStatistics> {
            unimplemented!()
        }

        async fn get_recent_trades(&self, _limit: i64, _offset: i64) -> AppResult<Vec<Trade>> {
            unimplemented!()
        }

        async fn get_wallet_performance(
            &self,
            _wallet_address: &str,
        ) -> AppResult<Option<WalletPerformance>> {
            unimplemented!()
        }

        async fn get_pool_stats(&self) -> AppResult<PoolStats> {
            unimplemented!()
        }

        async fn insert_jito_tip(
            &self,
            tip_amount_sol: &Decimal,
            bundle_signature: Option<&str>,
            strategy: Option<&str>,
            success: bool,
        ) -> AppResult<i64> {
            self.inserted_tips.write().push((
                *tip_amount_sol,
                bundle_signature.map(|s| s.to_string()),
                strategy.map(|s| s.to_string()),
                success,
            ));
            // Mirror a real backend: inserted tips are visible to subsequent reads.
            self.recent_jito_tips.write().push(*tip_amount_sol);
            Ok(1)
        }

        async fn get_recent_jito_tips(&self, limit: i32) -> AppResult<Vec<Decimal>> {
            Ok(self
                .recent_jito_tips
                .read()
                .iter()
                .rev()
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn get_jito_tip_count(&self) -> AppResult<u32> {
            Ok(*self.jito_tip_count.read())
        }

        async fn prune_old_jito_tips(&self) -> AppResult<u64> {
            unimplemented!()
        }

        async fn get_pnl_window(
            &self,
            _from_hours: &str,
            _to_hours: Option<&str>,
        ) -> AppResult<Decimal> {
            unimplemented!()
        }

        async fn get_pnl_24h(&self) -> AppResult<Decimal> {
            unimplemented!()
        }

        async fn get_pnl_7d(&self) -> AppResult<Decimal> {
            unimplemented!()
        }

        async fn get_pnl_30d(&self) -> AppResult<Decimal> {
            unimplemented!()
        }

        async fn get_total_realized_pnl(&self) -> AppResult<Decimal> {
            Ok(*self.total_realized_pnl.read())
        }

        async fn record_portfolio_snapshot(
            &self,
            nav_sol: Decimal,
            capital_sol: Decimal,
            realized_pnl_sol: Decimal,
            unrealized_pnl_sol: Decimal,
            open_positions: i32,
            sol_price_usd: Option<Decimal>,
            trade_mode: Option<String>,
        ) -> AppResult<()> {
            self.snapshots.write().push((
                nav_sol,
                capital_sol,
                realized_pnl_sol,
                unrealized_pnl_sol,
                open_positions,
                sol_price_usd,
                trade_mode,
            ));
            Ok(())
        }

        async fn get_portfolio_nav_history(
            &self,
            _days: u32,
        ) -> AppResult<Vec<crate::db_abstraction::types::PortfolioSnapshot>> {
            unimplemented!()
        }

        async fn delete_portfolio_snapshots_before(&self, days: i32) -> AppResult<u64> {
            self.deleted_snapshot_days.write().push(days);
            Ok(0)
        }

        async fn get_capital_deployed_30d(&self) -> AppResult<Decimal> {
            unimplemented!()
        }

        async fn cancel_stale_trades(&self, _max_age_minutes: i32) -> AppResult<u64> {
            unimplemented!()
        }

        async fn get_strategy_performance(
            &self,
            _strategy: &str,
            _days: i32,
        ) -> AppResult<(f64, Decimal, u32)> {
            unimplemented!()
        }

        async fn get_consecutive_losses(&self) -> AppResult<u32> {
            unimplemented!()
        }

        async fn get_consecutive_losses_since(
            &self,
            _since: Option<chrono::DateTime<chrono::Utc>>,
        ) -> AppResult<u32> {
            unimplemented!()
        }

        async fn get_max_drawdown_percent(
            &self,
            _total_capital_sol: Decimal,
        ) -> AppResult<(Decimal, Decimal)> {
            unimplemented!()
        }

        async fn activate_trade_and_open_position(
            &self,
            _trade_uuid: &str,
            _wallet_address: &str,
            _token_address: &str,
            _token_symbol: Option<&str>,
            _strategy: &str,
            _amount_sol: Decimal,
            _entry_price: Decimal,
            _tx_signature: &str,
            _max_heat_sol: Option<Decimal>,
            _entry_sol_price_usd: Option<Decimal>,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn atomic_portfolio_heat_check_and_open_position(
            &self,
            _trade_uuid: &str,
            _wallet_address: &str,
            _token_address: &str,
            _token_symbol: Option<&str>,
            _strategy: &str,
            _amount_sol: Decimal,
            _entry_price: Decimal,
            _tx_signature: &str,
            _max_heat_sol: Option<Decimal>,
            _entry_sol_price_usd: Option<Decimal>,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn close_position_full(
            &self,
            _trade_uuid: &str,
            _wallet_address: &str,
            _token_address: &str,
            _exit_price: Decimal,
            _signature: &str,
            _sol_price_usd: Option<Decimal>,
            _exit_fraction: Decimal,
            _confirmed: bool,
        ) -> AppResult<bool> {
            unimplemented!()
        }

        async fn update_position_token_amount(&self, _trade_uuid: &str, _token_amount: u64) -> AppResult<()> {
            unimplemented!()
        }

        async fn revert_position_exit(&self, _position_trade_uuid: &str) -> AppResult<()> {
            unimplemented!()
        }

        async fn get_stuck_positions(&self, _stuck_seconds: i64) -> AppResult<Vec<PositionRecord>> {
            unimplemented!()
        }

        async fn update_position_state(&self, _trade_uuid: &str, _new_state: &str) -> AppResult<()> {
            unimplemented!()
        }

        async fn update_position_unrealized_pnl(
            &self,
            _trade_uuid: &str,
            _current_price: Decimal,
            _pnl_sol: Decimal,
            _pnl_pct: Decimal,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn get_active_positions_with_entry(&self) -> AppResult<Vec<ActivePositionEntry>> {
            unimplemented!()
        }

        async fn get_active_position_tokens(&self) -> AppResult<Vec<ActivePositionSummary>> {
            unimplemented!()
        }

        async fn get_position_peak_price(&self, _trade_uuid: &str) -> AppResult<Option<String>> {
            unimplemented!()
        }

        async fn upsert_wallet(
            &self,
            address: &str,
            wqs_score: Option<Decimal>,
            _roi_7d: Option<Decimal>,
            _roi_30d: Option<Decimal>,
            _trade_count_30d: Option<i32>,
            win_rate: Option<Decimal>,
            _max_drawdown_30d: Option<Decimal>,
            _avg_trade_size_sol: Option<Decimal>,
            _notes: Option<&str>,
        ) -> AppResult<bool> {
            let mut wallets = self.wallets.write();
            if let Some(w) = wallets.get_mut(address) {
                w.wqs_score = wqs_score;
                w.win_rate = win_rate;
                Ok(true)
            } else {
                Ok(false)
            }
        }

        async fn update_wallet_status_ext(
            &self,
            address: &str,
            status: &str,
            _ttl_hours: Option<i32>,
            _reason: Option<&str>,
        ) -> AppResult<bool> {
            let mut wallets = self.wallets.write();
            if let Some(w) = wallets.get_mut(address) {
                w.status = status.to_string();
                Ok(true)
            } else {
                Ok(false)
            }
        }

        async fn get_expired_ttl_wallets(&self) -> AppResult<Vec<String>> {
            unimplemented!()
        }

        async fn demote_wallet(&self, _address: &str, _reason: &str) -> AppResult<()> {
            unimplemented!()
        }

        async fn has_recent_token_loss(
            &self,
            _token_address: &str,
            _within_minutes: i64,
        ) -> AppResult<bool> {
            unimplemented!()
        }

        async fn get_wallet_monitoring(
            &self,
            wallet_address: &str,
        ) -> AppResult<Option<WalletMonitoring>> {
            Ok(self.wallet_monitoring.read().get(wallet_address).cloned())
        }

        async fn find_webhook_with_capacity(&self, _max_wallets: i64) -> AppResult<Option<String>> {
            unimplemented!()
        }

        async fn clear_webhook_id_for_webhook(&self, _webhook_id: &str) -> AppResult<()> {
            unimplemented!()
        }

        async fn upsert_wallet_monitoring(
            &self,
            _wallet_address: &str,
            _helius_webhook_id: Option<&str>,
            _monitoring_enabled: bool,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn update_wallet_monitoring_signature(
            &self,
            _wallet_address: &str,
            _signature: &str,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn get_wallets_needing_webhook_registration(&self) -> AppResult<Vec<String>> {
            Ok(self.wallets_needing_registration.read().clone())
        }

        async fn get_active_wallets_with_webhook_ids(&self) -> AppResult<Vec<(String, String)>> {
            Ok(self.active_wallets_with_webhook_ids.read().clone())
        }

        async fn clear_webhook_id(&self, wallet_address: &str) -> AppResult<()> {
            self.cleared_webhook_ids
                .write()
                .push(wallet_address.to_string());
            Ok(())
        }

        async fn get_stale_webhook_wallets(&self, _threshold_days: i32) -> AppResult<Vec<String>> {
            unimplemented!()
        }

        async fn get_all_wallet_monitoring(&self) -> AppResult<Vec<WalletMonitoring>> {
            Ok(self.all_wallet_monitoring.read().clone())
        }

        async fn update_webhook_health_status(
            &self,
            _wallet_address: &str,
            _health_status: &str,
            _webhook_id: Option<&str>,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn update_webhook_status(
            &self,
            wallet_address: &str,
            webhook_status: &str,
        ) -> AppResult<()> {
            self.status_updates
                .write()
                .push((wallet_address.to_string(), webhook_status.to_string()));
            Ok(())
        }

        async fn update_last_speculative_signal(
            &self,
            _wallet_address: &str,
            _timestamp: chrono::DateTime<chrono::Utc>,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn get_inactivity_demotion_count(&self, wallet_address: &str) -> AppResult<i32> {
            Ok(self
                .demotion_counts
                .read()
                .get(wallet_address)
                .copied()
                .unwrap_or(0))
        }

        async fn increment_inactivity_demotion_count(&self, wallet_address: &str) -> AppResult<()> {
            self.incremented_demotions
                .write()
                .push(wallet_address.to_string());
            Ok(())
        }

        async fn reset_inactivity_demotion_count(&self, _wallet_address: &str) -> AppResult<()> {
            unimplemented!()
        }

        async fn log_webhook_lifecycle_event(
            &self,
            _wallet_address: &str,
            _action: &str,
            _status: &str,
            _webhook_id: Option<&str>,
            _details: Option<&str>,
            _error_message: Option<&str>,
            _duration_ms: Option<i32>,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn increment_webhook_registration_attempts(
            &self,
            _wallet_address: &str,
            _error: Option<&str>,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn get_webhook_configuration(&self, _key: &str) -> AppResult<Option<String>> {
            unimplemented!()
        }

        async fn update_webhook_configuration(
            &self,
            key: &str,
            value: &str,
            updated_by: &str,
        ) -> AppResult<()> {
            self.config_updates.write().push((
                key.to_string(),
                value.to_string(),
                updated_by.to_string(),
            ));
            Ok(())
        }

        async fn get_orphaned_webhooks(&self, _helius_webhook_ids: &[String]) -> AppResult<Vec<String>> {
            unimplemented!()
        }

        async fn upsert_exit_target(
            &self,
            _trade_uuid: &str,
            _entry_price: Decimal,
            _entry_amount_sol: Decimal,
            _peak_price: Decimal,
            _peak_profit_percent: Decimal,
            _targets_hit_json: &str,
            _trailing_stop_active: bool,
            _trailing_stop_price: Decimal,
            _remaining_fraction: Decimal,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn load_exit_target(&self, _trade_uuid: &str) -> AppResult<Option<ExitTargetData>> {
            unimplemented!()
        }

        async fn delete_exit_target(&self, _trade_uuid: &str) -> AppResult<()> {
            unimplemented!()
        }

        async fn insert_reconciliation_log(
            &self,
            _trade_uuid: &str,
            _expected_state: &str,
            _actual_on_chain: Option<&str>,
            _discrepancy: &str,
            _on_chain_tx_signature: Option<&str>,
            _notes: Option<&str>,
        ) -> AppResult<i64> {
            unimplemented!()
        }

        async fn get_reconciliation_status(
            &self,
            _discrepancies_limit: i32,
        ) -> AppResult<ReconciliationStatus> {
            unimplemented!()
        }

        async fn get_reconciliation_history(&self, _limit: i32) -> AppResult<Vec<ReconciliationRun>> {
            unimplemented!()
        }

        async fn count_reconciliation_runs(&self) -> AppResult<i64> {
            unimplemented!()
        }

        async fn get_reconciliation_stats(&self, _time_range: &str) -> AppResult<ReconciliationStats> {
            unimplemented!()
        }

        async fn resolve_discrepancy(&self, _id: i64, _resolved_by: &str, _resolution: &str) -> AppResult<()> {
            unimplemented!()
        }

        async fn get_trades_filtered(
            &self,
            _from_date: Option<&str>,
            _to_date: Option<&str>,
            _status_filter: Option<&str>,
            _strategy_filter: Option<&str>,
            _wallet_address_filter: Option<&str>,
            _limit: i64,
            _offset: i64,
        ) -> AppResult<Vec<TradeDetail>> {
            if *self.fail_trades_filtered.read() {
                return Err(AppError::Internal("mock trades failure".into()));
            }
            Ok(self.trades_filtered.read().clone())
        }

        async fn count_trades_filtered(
            &self,
            _from_date: Option<&str>,
            _to_date: Option<&str>,
            _status_filter: Option<&str>,
            _strategy_filter: Option<&str>,
            _wallet_address_filter: Option<&str>,
        ) -> AppResult<i64> {
            unimplemented!()
        }

        async fn update_trade_costs(
            &self,
            _trade_uuid: &str,
            _jito_tip_sol: Decimal,
            _dex_fee_sol: Decimal,
            _slippage_cost_sol: Decimal,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn mark_trade_dead_letter(&self, _trade_uuid: &str, _payload: &str, _error: &str) -> AppResult<()> {
            unimplemented!()
        }

        async fn log_config_change(
            &self,
            _key: &str,
            _old_value: Option<&str>,
            _new_value: &str,
            _changed_by: &str,
            _reason: Option<&str>,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn get_dead_letter_entries(&self, _limit: i32, _offset: i32) -> AppResult<Vec<DeadLetterItem>> {
            unimplemented!()
        }

        async fn get_dead_letter_entry(&self, _trade_uuid: &str) -> AppResult<Option<DeadLetterItem>> {
            unimplemented!()
        }

        async fn count_dead_letter_entries(&self) -> AppResult<i64> {
            unimplemented!()
        }

        async fn get_retryable_dlq_items(&self, _limit: i64) -> AppResult<Vec<RetryableDlqItem>> {
            unimplemented!()
        }

        async fn update_dlq_item(
            &self,
            _trade_uuid: &str,
            _retry_count: i64,
            _can_retry: bool,
            _mark_processed: bool,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn update_dlq_items_batch(&self, _items: Vec<UpdateDlqItemParams>) -> AppResult<usize> {
            unimplemented!()
        }

        async fn get_config_audit_entries(&self, _limit: i32, _offset: i32) -> AppResult<Vec<ConfigAuditItem>> {
            unimplemented!()
        }

        async fn get_config_audit_entries_by_key_prefix(
            &self,
            _prefix: &str,
            _limit: i32,
        ) -> AppResult<Vec<ConfigAuditItem>> {
            unimplemented!()
        }

        async fn count_config_audit_entries(&self) -> AppResult<i64> {
            unimplemented!()
        }

        async fn get_webhook_audit_log(
            &self,
            _wallet_address: Option<&str>,
            _action: Option<&str>,
            _status: Option<&str>,
            _limit: Option<i64>,
        ) -> AppResult<Vec<WebhookAuditLog>> {
            unimplemented!()
        }

        async fn count_trades_by_status(&self, _status: &str) -> AppResult<i64> {
            unimplemented!()
        }

        async fn get_closed_trade_count_for_wallet(&self, _wallet_address: &str) -> AppResult<i64> {
            unimplemented!()
        }

        async fn get_wallet_copy_stats(
            &self,
            _wallet_address: &str,
        ) -> AppResult<(i64, Decimal)> {
            unimplemented!()
        }

        async fn get_token_mirror_avg_pnl(
            &self,
            _token_address: &str,
            _window_hours: i32,
            _min_samples: i32,
        ) -> AppResult<Option<Decimal>> {
            unimplemented!()
        }

        async fn get_wallet_pnl_statistics(
            &self,
            wallet_address: &str,
            _window_days: i32,
        ) -> AppResult<Option<(i64, Decimal, Decimal)>> {
            Ok(self
                .wallet_pnl_stats
                .read()
                .get(wallet_address)
                .cloned()
                .unwrap_or(None))
        }

        async fn get_wallet_copy_performance(
            &self,
            _wallet_address: &str,
        ) -> AppResult<Option<WalletCopyPerformance>> {
            unimplemented!()
        }

        async fn get_trade_latency_stats(&self, _hours: i32) -> AppResult<TradeLatencyStats> {
            unimplemented!()
        }

        async fn get_trade_latency_histogram(
            &self,
            _hours: i32,
            _bucket_bounds: &[f64],
        ) -> AppResult<Vec<LatencyBucket>> {
            unimplemented!()
        }

        async fn get_positions(&self, _state_filter: Option<&str>) -> AppResult<Vec<PositionDetail>> {
            unimplemented!()
        }

        async fn get_whale_buy_prices(
            &self,
            _wallet_address: &str,
            _token_address: &str,
            _window_hours: i64,
        ) -> AppResult<Vec<Decimal>> {
            unimplemented!()
        }

        async fn has_recent_net_loss(
            &self,
            _token_address: &str,
            _window_hours: i64,
            _loss_threshold_pct: Decimal,
        ) -> AppResult<bool> {
            unimplemented!()
        }

        async fn get_wallets(&self, _status_filter: Option<&str>) -> AppResult<Vec<WalletDetail>> {
            Ok(self.wallet_details.read().clone())
        }

        fn pool(&self) -> DbPool {
            unimplemented!()
        }

        async fn insert_trade_and_create_position(
            &self,
            _trade: &InsertTrade,
            _position: &InsertPosition,
        ) -> AppResult<i64> {
            unimplemented!()
        }

        async fn update_trade_status_and_position(
            &self,
            _trade_uuid: &str,
            _trade_status: &str,
            _position_state: Option<&str>,
        ) -> AppResult<()> {
            unimplemented!()
        }

        async fn get_evaluation_data(
            &self,
        ) -> AppResult<(Decimal, Decimal, Decimal, Decimal)> {
            unimplemented!()
        }
    }

    // =====================================================================
    // Kelly sizer tests
    // =====================================================================

    fn trade_detail(
        uuid: &str,
        pnl: Decimal,
        amount: Decimal,
        created_at: &str,
        valid: bool,
    ) -> TradeDetail {
        TradeDetail {
            id: 0,
            trade_uuid: uuid.to_string(),
            wallet_address: "wallet".to_string(),
            token_address: "token".to_string(),
            token_symbol: None,
            strategy: "SHIELD".to_string(),
            side: "BUY".to_string(),
            amount_sol: amount,
            price_at_signal: None,
            tx_signature: None,
            status: "CLOSED".to_string(),
            retry_count: 0,
            error_message: None,
            pnl_sol: None,
            pnl_usd: None,
            jito_tip_sol: None,
            dex_fee_sol: None,
            slippage_cost_sol: None,
            total_cost_sol: None,
            net_pnl_sol: Some(pnl),
            pnl_data_valid: valid,
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
        }
    }

    fn kelly_db(trades: Vec<TradeDetail>) -> Arc<dyn Database> {
        let mock = MockDatabase {
            trades_filtered: RwLock::new(trades),
            ..Default::default()
        };
        Arc::new(mock)
    }

    #[test]
    fn test_kelly_formula_partial_losses() {
        // p = 0.6, avg_win = 0.1 (10%), avg_loss = 0.05 (5%).
        // Growth-optimal allocation: f* = (0.6*0.1 - 0.4*0.05)/(0.1*0.05) = 8.0.
        // The classic (p*b - q)/b with b = 2 would give 0.4 — a 20x under-allocation.
        let k = compute_full_kelly(dec!(0.6), dec!(0.4), dec!(0.1), dec!(0.05));
        assert_eq!(k, dec!(8.0));
    }

    #[test]
    fn test_kelly_formula_full_stake_losses_reduces_to_classic() {
        // With avg_loss = 1.0 (a loss costs the whole stake), f* reduces to the
        // classic (p*b - q)/b form.
        let p = dec!(0.6);
        let q = dec!(0.4);
        let avg_win = dec!(2.0);
        let avg_loss = Decimal::ONE;
        let k = compute_full_kelly(p, q, avg_win, avg_loss);
        let classic = ((p * (avg_win / avg_loss)) - q) / (avg_win / avg_loss);
        assert_eq!(k, classic);
        assert_eq!(k, dec!(0.4));
    }

    #[test]
    fn test_kelly_formula_zero_edge() {
        // p*avg_win == q*avg_loss → zero edge → f* = 0 (no allocation).
        let k = compute_full_kelly(dec!(0.5), dec!(0.5), dec!(0.1), dec!(0.1));
        assert_eq!(k, Decimal::ZERO);
        // Non-positive edge → f* <= 0 (rejected by callers).
        let k = compute_full_kelly(dec!(0.4), dec!(0.6), dec!(0.1), dec!(0.1));
        assert!(k < Decimal::ZERO);
    }

    #[test]
    fn test_kelly_calculation() {
        // Example: 60% win rate, avg win = 10% (0.1), avg loss = 5% (0.05)
        // f* = (0.6*0.1 - 0.4*0.05)/(0.1*0.05) = 8.0 → hard-capped at 0.5.
        // Conservative (25%) = 0.125 = 12.5% of capital.
        let k = compute_full_kelly(dec!(0.6), dec!(0.4), dec!(0.1), dec!(0.05));
        let full_capped = k.max(Decimal::ZERO).min(dec!(0.5));
        assert_eq!(full_capped, dec!(0.5));
        let conservative = (full_capped * dec!(0.25)).min(full_capped).min(Decimal::ONE);
        assert_eq!(conservative, dec!(0.125));
    }

    #[test]
    fn test_expected_return_uses_empirical_loss_rate() {
        // 6 wins, 3 losses, 1 break-even out of 10 valid trades.
        // win_rate = 0.6, loss_rate = 0.3 (NOT 0.4 — the break-even is not a loss).
        let result = KellyResult {
            full_kelly: dec!(0.5),
            conservative_kelly: dec!(0.125),
            recommended_size_percent: dec!(12.5),
            win_rate: dec!(0.6),
            loss_rate: dec!(0.3),
            avg_win: dec!(0.1),
            avg_loss: dec!(0.05),
            trade_count: 10,
            velocity_multiplier: Decimal::ONE,
        };
        let expected = dec!(0.6) * dec!(0.1) - dec!(0.3) * dec!(0.05);
        assert_eq!(result.expected_return_pct(), expected);
        assert_eq!(result.expected_return_pct(), dec!(0.045));
        assert_eq!(
            result.expected_profit_sol(dec!(1.0)),
            dec!(0.045)
        );
    }

    // ==========================================================================
    // DATABASE-BACKED KELLY CALCULATION TESTS
    // ==========================================================================

    fn rfc3339(days_ago: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::days(days_ago))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    }

    /// 16 trades: 10 wins @ +10%, 5 losses @ -5%, 1 break-even. Times span
    /// exactly 2 days so velocity >= 2 trades/day (multiplier 1.25).
    fn standard_trades() -> Vec<TradeDetail> {
        let mut trades = Vec::new();
        for i in 0..10 {
            trades.push(trade_detail(
                &format!("win-{i}"),
                dec!(0.1),
                Decimal::ONE,
                &rfc3339(i),
                true,
            ));
        }
        for i in 0..5 {
            trades.push(trade_detail(
                &format!("loss-{i}"),
                dec!(-0.05),
                Decimal::ONE,
                &rfc3339(1 + i),
                true,
            ));
        }
        trades.push(trade_detail(
            "breakeven",
            Decimal::ZERO,
            Decimal::ONE,
            &rfc3339(1),
            true,
        ));
        // Oldest trade exactly 2 days before the newest.
        trades[0].created_at = (chrono::Utc::now() - chrono::Duration::days(2))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        trades
    }

    #[tokio::test]
    async fn test_kelly_full_calculation() {
        let db = kelly_db(standard_trades());
        let sizer = KellySizer::new(db);

        let result = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap();

        // win_rate = 10/16, loss_rate = 5/16
        assert_eq!(result.win_rate, dec!(10) / dec!(16));
        assert_eq!(result.loss_rate, dec!(5) / dec!(16));
        // avg_win = 0.1, avg_loss = 0.05
        assert_eq!(result.avg_win, dec!(0.1));
        assert_eq!(result.avg_loss, dec!(0.05));
        // f* = (0.625*0.1 - 0.3125*0.05)/(0.1*0.05) = 9.375 -> capped 0.5
        assert_eq!(result.full_kelly, dec!(0.5));
        // Velocity: 16 trades / 2 days = 8/day -> 1.25x
        assert_eq!(result.velocity_multiplier, dec!(1.25));
        // conservative = min(0.5*1.25*0.25, 0.5, 1.0) = 0.15625
        assert_eq!(result.conservative_kelly, dec!(0.15625));
        assert_eq!(result.recommended_size_percent, dec!(15.625));
        assert_eq!(result.trade_count, 16);
    }

    #[tokio::test]
    async fn test_kelly_no_loss_history_uses_fallback() {
        let trades: Vec<TradeDetail> = (0..16)
            .map(|i| {
                trade_detail(
                    &format!("w{i}"),
                    dec!(0.1),
                    Decimal::ONE,
                    &rfc3339(i % 2),
                    true,
                )
            })
            .collect();
        let db = kelly_db(trades);
        let sizer = KellySizer::new(db);

        let result = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap();

        assert_eq!(result.win_rate, Decimal::ONE);
        assert_eq!(result.loss_rate, Decimal::ZERO);
        // No loss history -> conservative 15% assumed loss
        assert_eq!(result.avg_loss, dec!(0.15));
        assert_eq!(result.avg_win, dec!(0.1));
        // (1.0*0.1 - 0)/(0.1*0.15) = 6.67 -> capped at 0.5
        assert_eq!(result.full_kelly, dec!(0.5));
    }

    #[tokio::test]
    async fn test_kelly_loss_floor_applied() {
        // Losses of 0.5% (below the 1% floor) must be floored to 1%.
        let mut trades: Vec<TradeDetail> = (0..10)
            .map(|i| {
                trade_detail(
                    &format!("w{i}"),
                    dec!(0.1),
                    Decimal::ONE,
                    &rfc3339(i),
                    true,
                )
            })
            .collect();
        for i in 0..6 {
            trades.push(trade_detail(
                &format!("l{i}"),
                dec!(-0.005),
                Decimal::ONE,
                &rfc3339(1),
                true,
            ));
        }
        let db = kelly_db(trades);
        let sizer = KellySizer::new(db);

        let result = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap();

        assert_eq!(result.avg_loss, dec!(0.01), "1% floor must apply");
    }

    #[tokio::test]
    async fn test_kelly_win_capped_at_300_percent() {
        let mut trades = Vec::new();
        // One outlier win of +500% must be capped at 300%.
        trades.push(trade_detail(
            "outlier",
            dec!(5.0),
            Decimal::ONE,
            &rfc3339(0),
            true,
        ));
        for i in 0..9 {
            trades.push(trade_detail(
                &format!("w{i}"),
                dec!(0.1),
                Decimal::ONE,
                &rfc3339(1),
                true,
            ));
        }
        for i in 0..6 {
            trades.push(trade_detail(
                &format!("l{i}"),
                dec!(-0.05),
                Decimal::ONE,
                &rfc3339(1),
                true,
            ));
        }
        let db = kelly_db(trades);
        let sizer = KellySizer::new(db);

        let result = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap();

        assert_eq!(result.avg_win, dec!(0.39)); // (3.0 + 9*0.1) / 10
    }

    #[tokio::test]
    async fn test_kelly_break_even_only_trades() {
        // All 16 trades break even: win_rate = 0, avg_win = 0 -> full_kelly 0.
        let trades: Vec<TradeDetail> = (0..16)
            .map(|i| {
                trade_detail(
                    &format!("be{i}"),
                    Decimal::ZERO,
                    Decimal::ONE,
                    &rfc3339(i % 2),
                    true,
                )
            })
            .collect();
        let db = kelly_db(trades);
        let sizer = KellySizer::new(db);

        let result = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap();

        assert_eq!(result.full_kelly, Decimal::ZERO);
        assert_eq!(result.win_rate, Decimal::ZERO);
        assert_eq!(result.trade_count, 16);
    }

    #[tokio::test]
    async fn test_kelly_velocity_brackets() {
        // < 0.5 trades/day -> 0.8x multiplier: 16 trades over 40 days.
        let mut trades = standard_trades();
        trades[0].created_at = (chrono::Utc::now() - chrono::Duration::days(40))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        let sizer = KellySizer::new(kelly_db(trades));
        let result = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 90)
            .await
            .unwrap();
        assert_eq!(result.velocity_multiplier, dec!(0.8));

        // 0.5-1 trades/day -> 1.0x: 16 trades over 24 days (lookback 30 clamps span).
        let mut trades = standard_trades();
        trades[0].created_at = (chrono::Utc::now() - chrono::Duration::days(24))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        let sizer = KellySizer::new(kelly_db(trades));
        let result = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap();
        assert_eq!(result.velocity_multiplier, Decimal::ONE);

        // 1-2 trades/day -> 1.15x: 16 trades over 10 days.
        let mut trades = standard_trades();
        trades[0].created_at = (chrono::Utc::now() - chrono::Duration::days(10))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        let sizer = KellySizer::new(kelly_db(trades));
        let result = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap();
        assert_eq!(result.velocity_multiplier, dec!(1.15));
    }

    #[tokio::test]
    async fn test_kelly_unparseable_timestamps_fall_back_to_lookback() {
        let mut trades = standard_trades();
        for t in &mut trades {
            t.created_at = "not-a-timestamp".to_string();
        }
        let sizer = KellySizer::new(kelly_db(trades));
        let result = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap();
        // 16 / 30 days = 0.53/day -> neutral 1.0 multiplier.
        assert_eq!(result.velocity_multiplier, Decimal::ONE);
    }

    #[tokio::test]
    async fn test_kelly_alternate_timestamp_format() {
        // "%Y-%m-%d %H:%M:%S" (space-separated) must parse via the fallback.
        let mut trades = standard_trades();
        for t in &mut trades {
            t.created_at = (chrono::Utc::now() - chrono::Duration::days(1))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
        }
        let sizer = KellySizer::new(kelly_db(trades));
        let result = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap();
        assert_eq!(result.trade_count, 16);
    }

    #[tokio::test]
    async fn test_kelly_empty_trades_error() {
        let sizer = KellySizer::new(kelly_db(Vec::new()));
        let err = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap_err();
        assert!(err.contains("No historical trades"));
    }

    #[tokio::test]
    async fn test_kelly_all_invalid_pnl_rows_error() {
        let trades: Vec<TradeDetail> = (0..16)
            .map(|i| trade_detail(&format!("bad{i}"), dec!(0.1), Decimal::ONE, &rfc3339(i), false))
            .collect();
        let sizer = KellySizer::new(kelly_db(trades));
        let err = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap_err();
        assert!(err.contains("No valid trades"));
    }

    #[tokio::test]
    async fn test_kelly_insufficient_trades_error() {
        let trades: Vec<TradeDetail> = (0..10)
            .map(|i| trade_detail(&format!("t{i}"), dec!(0.1), Decimal::ONE, &rfc3339(i), true))
            .collect();
        let sizer = KellySizer::new(kelly_db(trades));
        let err = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap_err();
        assert!(err.contains("Insufficient trade history"));
    }

    #[tokio::test]
    async fn test_kelly_db_error_propagates() {
        let mock = MockDatabase {
            fail_trades_filtered: RwLock::new(true),
            ..Default::default()
        };
        let sizer = KellySizer::new(Arc::new(mock));
        let err = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap_err();
        assert!(err.contains("Failed to query trades"));
    }

    #[tokio::test]
    async fn test_calculate_position_size() {
        let sizer = KellySizer::new(kelly_db(standard_trades()));
        let size = sizer
            .calculate_position_size(
                "wallet",
                chimera_core::models::Strategy::Shield,
                dec!(100),
                30,
            )
            .await
            .unwrap();
        // 100 SOL * conservative_kelly (0.15625)
        assert_eq!(size, dec!(15.625));
    }

    #[tokio::test]
    async fn test_with_conservative_multiplier_clamp() {
        // 1.5 clamps to 1.0 -> conservative == full_kelly cap interaction.
        let sizer = KellySizer::with_conservative_multiplier(kelly_db(standard_trades()), 1.5);
        let result = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap();
        assert_eq!(result.conservative_kelly, dec!(0.5)); // full kelly cap

        // NaN -> from_f64_retain fails -> ZERO multiplier.
        let sizer = KellySizer::with_conservative_multiplier(kelly_db(standard_trades()), f64::NAN);
        let result = sizer
            .calculate_kelly("wallet", chimera_core::models::Strategy::Shield, 30)
            .await
            .unwrap();
        assert_eq!(result.conservative_kelly, Decimal::ZERO);
    }

    #[test]
    fn test_compute_full_kelly_zero_avg_loss() {
        assert_eq!(compute_full_kelly(dec!(0.5), dec!(0.5), dec!(0.1), Decimal::ZERO), Decimal::ZERO);
    }
}
