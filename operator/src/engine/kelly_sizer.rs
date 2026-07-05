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
    /// Average win amount (using Decimal for precision)
    pub avg_win: Decimal,
    /// Average loss amount (using Decimal for precision)
    pub avg_loss: Decimal,
    /// Number of closed trades used to compute this result
    pub trade_count: usize,
    /// Velocity multiplier based on trade frequency
    pub velocity_multiplier: Decimal,
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
        strategy: crate::models::Strategy,
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

        // Calculate Kelly Criterion using the standard edge/odds form:
        //   k = (p * b - q) / b   where b = avg_win / avg_loss (win/loss ratio)
        // This is mathematically equivalent to the fractional-return form but
        // makes the odds-ratio (b) and edge explicit for auditability.
        // Hard-cap full_kelly at 0.5 (50%): even wallets with extreme edges must
        // never risk more than half the bankroll on a single trade. Copy-trading
        // edge estimates are inherently unreliable — full Kelly near 100% invites ruin.
        let full_kelly = if !avg_win.is_zero() && !avg_loss.is_zero() {
            let b = avg_win / avg_loss;
            let p = win_rate;
            let q = loss_rate;
            let k = ((p * b) - q) / b;
            k.max(Decimal::ZERO).min(dec!(0.5))
        } else {
            Decimal::ZERO
        };

        // Trade velocity confidence: a wallet with the same win rate is statistically
        // more reliable when it generates more trades per day because each outcome is
        // an independent sample that tightens the confidence interval on the true win rate.
        // Scale the conservative Kelly fraction — never push past full Kelly.
        //   < 0.5 trades/day  → 0.80× (sparse history, widen caution margin)
        //   0.5–1 trades/day  → 1.00× (neutral)
        //   1–2  trades/day   → 1.15× (good statistical depth)
        //   ≥ 2  trades/day   → 1.25× (high frequency, tighter confidence interval)
        let true_timespan_days = if let (Some(newest), Some(oldest)) =
            (trades.first(), trades.last())
        {
            let parse_time = |s: &str| -> Option<chrono::DateTime<chrono::Utc>> {
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .ok()
                    .or_else(|| {
                        let naive =
                            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()?;
                        Some(chrono::DateTime::from_naive_utc_and_offset(
                            naive,
                            chrono::Utc,
                        ))
                    })
            };
            if let (Some(newest_time), Some(oldest_time)) = (
                parse_time(&newest.created_at),
                parse_time(&oldest.created_at),
            ) {
                let span = (newest_time - oldest_time).num_seconds() as f64 / 86400.0;
                span.min(lookback_days as f64).max(1.0)
            } else {
                lookback_days as f64
            }
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
            // [T-H1] recommended_size_percent is simply conservative_kelly * 100 — do not
            // divide by avg_loss here; avg_loss is already embedded in the Kelly formula.
            recommended_size_percent: conservative_kelly * Decimal::from(100),
            win_rate,
            avg_win,
            avg_loss,
            trade_count: trades.len(),
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
        strategy: crate::models::Strategy,
        total_capital_sol: Decimal,
        lookback_days: i64,
    ) -> Result<Decimal, String> {
        let kelly = self
            .calculate_kelly(wallet_address, strategy, lookback_days)
            .await?;
        // [T-H1] Do NOT divide by avg_loss here. conservative_kelly already incorporates
        // avg_loss through the Kelly formula: (win_rate*avg_win - loss_rate*avg_loss)/avg_win.
        // Dividing again by avg_loss double-penalises the position size, making it far too small.
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
        let win_rate = self.win_rate;
        let loss_rate = Decimal::ONE - win_rate;
        let expected_win = win_rate * self.avg_win;
        let expected_loss = loss_rate * self.avg_loss;
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
mod tests {

    #[test]
    fn test_kelly_calculation() {
        // Example: 60% win rate, avg win = 10% (0.1), avg loss = 5% (0.05)
        // b = avg_win / avg_loss = 0.1 / 0.05 = 2.0
        // kelly = (p*b - q) / b = (0.6*2.0 - 0.4) / 2.0 = (1.2 - 0.4) / 2.0 = 0.4
        // Hard-capped at 0.5 → 0.4 passes through
        // Conservative (25%) = 0.10 = 10% of capital

        // This would be tested with actual database in integration tests
    }
}
