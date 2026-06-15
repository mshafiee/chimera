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
    /// Full Kelly percentage (0.0-1.0, using Decimal for precision)
    pub full_kelly: Decimal,
    /// Conservative Kelly percentage (25% of full, using Decimal for precision)
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

        if valid_trades_count < 20 {
            return Err(format!(
                "Insufficient trade history for reliable Kelly calculation ({valid_trades_count} trades, need ≥20)"
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
            Decimal::ZERO
        } else {
            let sum: Decimal = losses.iter().sum();
            // Enforce a 1% floor: extremely tight stop-losses produce avg_loss → 0,
            // causing Kelly → win_rate and ignoring actual downside risk.
            (sum / Decimal::from(losses.len())).max(dec!(0.01))
        };

        // Calculate Kelly Criterion using Decimal for precision
        // kelly = (win_rate * avg_win - loss_rate * avg_loss) / avg_win
        let full_kelly = if !avg_win.is_zero() {
            let numerator = (win_rate * avg_win) - (loss_rate * avg_loss);
            (numerator / avg_win).max(Decimal::ZERO).min(Decimal::ONE) // Clamp to 0-1
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
        let trades_per_day = if lookback_days > 0 {
            valid_trades_count as f64 / lookback_days as f64
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
            .min(full_kelly);

        Ok(KellyResult {
            full_kelly,
            conservative_kelly,
            recommended_size_percent: if avg_loss > Decimal::ZERO {
                (conservative_kelly / avg_loss) * Decimal::from(100)
            } else {
                conservative_kelly * Decimal::from(100)
            },
            win_rate,
            avg_win,
            avg_loss,
            trade_count: trades.len(),
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
        let mut size_sol = total_capital_sol * kelly.conservative_kelly;
        if kelly.avg_loss > Decimal::ZERO {
            size_sol /= kelly.avg_loss;
        }
        Ok(size_sol)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_kelly_calculation() {
        // Example: 60% win rate, avg win = 0.1 SOL, avg loss = 0.05 SOL
        // kelly = (0.6 * 0.1 - 0.4 * 0.05) / 0.1
        // kelly = (0.06 - 0.02) / 0.1 = 0.4
        // Conservative (25%) = 0.1 = 10% of capital

        // This would be tested with actual database in integration tests
    }
}
