//! Kelly Criterion Position Sizing
//!
//! Implements Kelly Criterion for optimal position sizing:
//! kelly = (win_rate * avg_win - loss_rate * avg_loss) / avg_win
//!
//! Uses conservative Kelly (25% of full Kelly) to reduce risk.

use crate::db::{self, DbPool};
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;

/// Kelly position sizer
pub struct KellySizer {
    db: DbPool,
    /// Conservative multiplier (use 25% of full Kelly, using Decimal for precision)
    conservative_multiplier: Decimal,
}

/// Kelly sizing result
#[derive(Debug, Clone)]
pub struct KellyResult {
    /// Full Kelly percentage (can exceed 1.0 for highly profitable strategies, using Decimal for precision)
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
    pub fn new(db: DbPool) -> Self {
        Self {
            db,
            conservative_multiplier: dec!(0.25), // Use 25% of full Kelly
        }
    }

    /// Create with custom conservative multiplier
    pub fn with_conservative_multiplier(db: DbPool, multiplier: f64) -> Self {
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

        let trades = db::get_trades(
            &self.db,
            Some(&from_date_str),
            None,
            Some("CLOSED"), // Only closed trades
            Some(&strategy_str),
            Some(wallet_address),
            None,
            None,
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

        // Calculate Kelly Criterion using Decimal for precision
        // The Kelly formula for position size (when returns are fractional, not 100% loss) is:
        // kelly = (win_rate * avg_win - loss_rate * avg_loss) / avg_win
         let full_kelly = if !avg_win.is_zero() {
             let numerator = (win_rate * avg_win) - (loss_rate * avg_loss);
             let denominator = avg_win;
             (numerator / denominator).max(Decimal::ZERO).min(Decimal::ONE)
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
        let true_timespan_days = if let (Some(newest), Some(oldest)) = (trades.first(), trades.last()) {
            let parse_time = |s: &str| -> Option<chrono::DateTime<chrono::Utc>> {
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .ok()
                    .or_else(|| {
                        let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()?;
                        Some(chrono::DateTime::from_naive_utc_and_offset(naive, chrono::Utc))
                    })
            };
            if let (Some(newest_time), Some(oldest_time)) = (parse_time(&newest.created_at), parse_time(&oldest.created_at)) {
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

        // Apply conservative multiplier scaled by velocity confidence.
        // Hard-cap at full_kelly: the velocity bonus increases the effective Kelly
        // fraction but can never push past the mathematically optimal bound.
        let conservative_kelly = (full_kelly * self.conservative_multiplier * velocity_multiplier)
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
        let kelly = self.calculate_kelly(wallet_address, strategy, lookback_days).await?;
        // [T-H1] Do NOT divide by avg_loss here. conservative_kelly already incorporates
        // avg_loss through the Kelly formula: (win_rate*avg_win - loss_rate*avg_loss)/avg_win.
        // Dividing again by avg_loss double-penalises the position size, making it far too small.
        let size_sol = total_capital_sol * kelly.conservative_kelly;
        Ok(size_sol)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_kelly_calculation() {
        // Example: 60% win rate, avg win = 10% (0.1), avg loss = 5% (0.05)
        // kelly = (0.6 * 0.1 - 0.4 * 0.05) / 0.1
        // kelly = (0.06 - 0.02) / 0.1 = 0.4
        // Conservative (25%) = 0.10 = 10% of capital

        // This would be tested with actual database in integration tests
    }
}
