//! Kelly Criterion Position Sizing
//!
//! Implements Kelly Criterion for optimal position sizing:
//! kelly = (win_rate * avg_win - loss_rate * avg_loss) / avg_win
//!
//! Uses conservative Kelly (25% of full Kelly) to reduce risk.

use crate::db::{self, DbPool};
use rust_decimal::prelude::*;

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
}

impl KellySizer {
    /// Create a new Kelly sizer
    pub fn new(db: DbPool) -> Self {
        Self {
            db,
            conservative_multiplier: Decimal::from_f64_retain(0.25).unwrap_or(Decimal::from_f64_retain(0.25).unwrap_or(Decimal::ZERO)), // Use 25% of full Kelly
        }
    }

    /// Create with custom conservative multiplier
    pub fn with_conservative_multiplier(db: DbPool, multiplier: f64) -> Self {
        let mult = Decimal::from_f64_retain(multiplier.max(0.0).min(1.0)).unwrap_or(Decimal::ZERO);
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
        lookback_days: i64,
    ) -> Result<KellyResult, String> {
        // Get historical trades for this wallet
        let from_date = chrono::Utc::now() - chrono::Duration::days(lookback_days);
        let from_date_str = from_date.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let trades = db::get_trades(
            &self.db,
            Some(&from_date_str),
            None,
            Some("CLOSED"), // Only closed trades
            None,
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

        for trade in &trades {
            if let Some(pnl) = trade.net_pnl_sol {
                if pnl > Decimal::ZERO {
                    wins.push(pnl);
                } else if pnl < Decimal::ZERO {
                    losses.push(pnl.abs()); // Store as positive for calculation
                }
            }
        }

        let total_trades = Decimal::from(trades.len());
        let win_count = Decimal::from(wins.len());
        let loss_count = Decimal::from(losses.len());

        if total_trades.is_zero() {
            return Err("No valid trades for Kelly calculation".to_string());
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
            sum / Decimal::from(losses.len())
        };

        // Calculate Kelly Criterion using Decimal for precision
        // kelly = (win_rate * avg_win - loss_rate * avg_loss) / avg_win
        let full_kelly = if !avg_win.is_zero() {
            let numerator = (win_rate * avg_win) - (loss_rate * avg_loss);
            (numerator / avg_win).max(Decimal::ZERO).min(Decimal::ONE) // Clamp to 0-1
        } else {
            Decimal::ZERO
        };

        // Apply conservative multiplier (using Decimal for precision)
        let conservative_kelly = full_kelly * self.conservative_multiplier;

        Ok(KellyResult {
            full_kelly,
            conservative_kelly,
            recommended_size_percent: conservative_kelly * Decimal::from(100),
            win_rate,
            avg_win,
            avg_loss,
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
        total_capital_sol: Decimal,
        lookback_days: i64,
    ) -> Result<Decimal, String> {
        let kelly = self.calculate_kelly(wallet_address, lookback_days).await?;
        let size_sol = total_capital_sol * kelly.conservative_kelly;
        Ok(size_sol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kelly_calculation() {
        // Example: 60% win rate, avg win = 0.1 SOL, avg loss = 0.05 SOL
        // kelly = (0.6 * 0.1 - 0.4 * 0.05) / 0.1
        // kelly = (0.06 - 0.02) / 0.1 = 0.4
        // Conservative (25%) = 0.1 = 10% of capital

        // This would be tested with actual database in integration tests
    }
}




