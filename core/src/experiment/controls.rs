//! Control arms for forward test
//!
//! Implements random-token and SOL benchmark control arms to measure
//! edge vs beta performance.

use chrono::{DateTime, Utc};
use rust_decimal::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use rand::prelude::*;

/// Control trade outcome
#[derive(Debug, Clone)]
pub struct ControlTrade {
    /// Control type (random_token or sol_benchmark)
    pub control_type: String,
    /// Token mint address
    pub token_mint: String,
    /// Entry timestamp
    pub entry_time: DateTime<Utc>,
    /// Entry price (USD per token)
    pub entry_price: Decimal,
    /// Exit timestamp (if closed)
    pub exit_time: Option<DateTime<Utc>>,
    /// Exit price (if closed)
    pub exit_price: Option<Decimal>,
    /// Position size in SOL
    pub position_size_sol: Decimal,
    /// Control trade PnL (if closed)
    pub pnl: Option<Decimal>,
}

impl ControlTrade {
    pub fn new(
        control_type: String,
        token_mint: String,
        entry_price: Decimal,
        position_size_sol: Decimal,
    ) -> Self {
        Self {
            control_type,
            token_mint,
            entry_time: Utc::now(),
            entry_price,
            exit_time: None,
            exit_price: None,
            position_size_sol,
            pnl: None,
        }
    }

    /// Close the control trade and calculate PnL.
    ///
    /// Idempotent: closing an already-closed trade returns the recorded PnL
    /// without overwriting the original exit.
    pub fn close(&mut self, exit_price: Decimal) -> Result<Decimal, String> {
        if let Some(pnl) = self.pnl {
            return Ok(pnl);
        }

        if self.entry_price <= Decimal::ZERO {
            return Err(format!(
                "Cannot close control trade with non-positive entry price: {}",
                self.entry_price
            ));
        }

        self.exit_time = Some(Utc::now());
        self.exit_price = Some(exit_price);

        // Calculate PnL: (exit - entry) / entry * position_size
        let pnl = (exit_price - self.entry_price) / self.entry_price * self.position_size_sol;
        self.pnl = Some(pnl);
        Ok(pnl)
    }

    /// Get current PnL if position is still open
    pub fn calculate_unrealized_pnl(&self, current_price: Decimal) -> Decimal {
        if self.entry_price > Decimal::ZERO {
            (current_price - self.entry_price) / self.entry_price * self.position_size_sol
        } else {
            Decimal::ZERO
        }
    }
}

/// Control arms manager
pub struct ControlArms {
    /// Random token control trades
    random_trades: Arc<Mutex<Vec<ControlTrade>>>,
    /// SOL benchmark control trades
    sol_bench_trades: Arc<Mutex<Vec<ControlTrade>>>,
    /// Known liquid tokens for random selection
    liquid_tokens: Vec<String>,
}

impl ControlArms {
    pub fn new(liquid_tokens: Vec<String>) -> Self {
        Self {
            random_trades: Arc::new(Mutex::new(Vec::new())),
            sol_bench_trades: Arc::new(Mutex::new(Vec::new())),
            liquid_tokens,
        }
    }

    /// Fire random token control at matched timestamp
    pub async fn fire_random_token_control(
        &self,
        entry_price: Decimal,
        position_size_sol: Decimal,
    ) -> Result<ControlTrade, String> {
        if self.liquid_tokens.is_empty() {
            return Err("No liquid tokens available for random selection".to_string());
        }

        // Randomly select a liquid token using a real RNG — wall-clock
        // microseconds are predictable and repeat within the same microsecond.
        let idx = rand::rng().random_range(0..self.liquid_tokens.len());
        let selected_token = &self.liquid_tokens[idx];

        let control_trade = ControlTrade::new(
            "random_token".to_string(),
            selected_token.clone(),
            entry_price,
            position_size_sol,
        );

        self.random_trades.lock().await.push(control_trade.clone());

        Ok(control_trade)
    }

    /// Fire SOL benchmark control at matched timestamp
    pub async fn fire_sol_benchmark_control(
        &self,
        entry_price: Decimal,
        position_size_sol: Decimal,
    ) -> ControlTrade {
        let control_trade = ControlTrade::new(
            "sol_benchmark".to_string(),
            crate::constants::mints::SOL.to_string(),
            entry_price,
            position_size_sol,
        );

        self.sol_bench_trades.lock().await.push(control_trade.clone());

        control_trade
    }

    /// Close random token control
    pub async fn close_random_control(&self, token_mint: &str, exit_price: Decimal) -> Result<Decimal, String> {
        let mut trades = self.random_trades.lock().await;
        // Close the MOST RECENT open trade for this token — with multiple open
        // trades on the same mint, closing the first would associate the exit
        // price/PnL with the wrong entry.
        if let Some(trade) = trades.iter_mut().rev().find(|t| t.token_mint == token_mint && t.exit_time.is_none()) {
            trade.close(exit_price)
        } else {
            Err(format!("No open random control trade found for {}", token_mint))
        }
    }

    /// Close SOL benchmark control
    pub async fn close_sol_benchmark(&self, exit_price: Decimal) -> Result<Decimal, String> {
        let mut trades = self.sol_bench_trades.lock().await;
        // Close the most recent open SOL benchmark trade.
        if let Some(trade) = trades.iter_mut().rev().find(|t| t.exit_time.is_none()) {
            trade.close(exit_price)
        } else {
            Err("No open SOL benchmark trade found".to_string())
        }
    }

    /// Get all random control trades
    pub async fn get_random_controls(&self) -> Vec<ControlTrade> {
        self.random_trades.lock().await.clone()
    }

    /// Get all SOL benchmark trades
    pub async fn get_sol_benchmarks(&self) -> Vec<ControlTrade> {
        self.sol_bench_trades.lock().await.clone()
    }

    /// Calculate aggregate statistics for a control type
    ///
    /// Returns `Err` for an unknown control type and `Ok(None)` when there are
    /// no closed trades — an empty record must not be reported as a 0% win rate.
    pub async fn get_control_stats(&self, control_type: &str) -> Result<Option<ControlStats>, String> {
        let trades = match control_type {
            "random_token" => self.get_random_controls().await,
            "sol_benchmark" => self.get_sol_benchmarks().await,
            _ => return Err(format!("Unknown control type: {}", control_type)),
        };

        let closed_trades: Vec<_> = trades.iter().filter(|t| t.exit_time.is_some()).collect();

        if closed_trades.is_empty() {
            return Ok(None);
        }

        let total_pnl: Decimal = closed_trades.iter()
            .filter_map(|t| t.pnl)
            .sum();

        let win_count = closed_trades.iter().filter(|t| {
            t.pnl.is_some_and(|p| p > Decimal::ZERO)
        }).count();

        let avg_pnl = total_pnl / Decimal::from(closed_trades.len() as u64);
        let win_rate = (win_count as f64) / (closed_trades.len() as f64);

        Ok(Some(ControlStats {
            total_trades: closed_trades.len(),
            total_pnl,
            avg_pnl,
            win_rate,
            win_count,
            loss_count: closed_trades.len() - win_count,
        }))
    }
}

/// Control arm statistics
#[derive(Debug, Clone, Default)]
pub struct ControlStats {
    /// Total number of closed trades
    pub total_trades: usize,
    /// Total PnL across all trades
    pub total_pnl: Decimal,
    /// Average PnL per trade
    pub avg_pnl: Decimal,
    /// Win rate (0.0 - 1.0)
    pub win_rate: f64,
    /// Number of winning trades
    pub win_count: usize,
    /// Number of losing trades
    pub loss_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_control_trade_creation() {
        let trade = ControlTrade::new(
            "random_token".to_string(),
            "test_mint".to_string(),
            Decimal::from_str("1.0").unwrap(),
            Decimal::from_str("0.02").unwrap(),
        );

        assert_eq!(trade.control_type, "random_token");
        assert_eq!(trade.token_mint, "test_mint");
        assert!(trade.exit_time.is_none());
    }

    #[tokio::test]
    async fn test_control_trade_close() {
        let mut trade = ControlTrade::new(
            "random_token".to_string(),
            "test_mint".to_string(),
            Decimal::from_str("1.0").unwrap(),
            Decimal::from_str("0.02").unwrap(),
        );

        let pnl = trade.close(Decimal::from_str("1.10").unwrap()).unwrap();

        assert_eq!(pnl, Decimal::from_str("0.002").unwrap()); // 10% gain on 0.02 SOL
        assert!(trade.exit_time.is_some());
        assert_eq!(trade.exit_price, Some(Decimal::from_str("1.10").unwrap()));

        // Closing again is idempotent and preserves the original exit
        let pnl2 = trade.close(Decimal::from_str("0.50").unwrap()).unwrap();
        assert_eq!(pnl, pnl2);
        assert_eq!(trade.exit_price, Some(Decimal::from_str("1.10").unwrap()));
    }

    #[tokio::test]
    async fn test_control_trade_close_rejects_bad_entry() {
        let mut trade = ControlTrade::new(
            "random_token".to_string(),
            "test_mint".to_string(),
            Decimal::ZERO,
            Decimal::from_str("0.02").unwrap(),
        );

        assert!(trade.close(Decimal::from_str("1.10").unwrap()).is_err());
    }

    #[tokio::test]
    async fn test_control_arms() {
        let arms = ControlArms::new(vec![
            "So11111111111111111111111111111111111111112".to_string(), // SOL
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(), // USDC
        ]);

        // Fire random control
        let random_trade = arms.fire_random_token_control(
            Decimal::from_str("1.0").unwrap(),
            Decimal::from_str("0.02").unwrap(),
        ).await.unwrap();

        assert_eq!(random_trade.control_type, "random_token");

        // Fire SOL benchmark
        let sol_trade = arms.fire_sol_benchmark_control(
            Decimal::from_str("150.0").unwrap(),
            Decimal::from_str("0.02").unwrap(),
        ).await;

        assert_eq!(sol_trade.control_type, "sol_benchmark");
        assert_eq!(sol_trade.token_mint, crate::constants::mints::SOL.to_string());
    }

    #[tokio::test]
    async fn test_control_stats() {
        let arms = ControlArms::new(vec!["So11111111111111111111111111111111111111112".to_string()]);

        // Fire and close 3 trades (2 wins, 1 loss)
        for i in 0..3 {
            let trade = arms.fire_random_token_control(
                Decimal::from_str("1.0").unwrap(),
                Decimal::from_str("0.02").unwrap(),
            ).await.unwrap();

            let exit_price = if i < 2 {
                Decimal::from_str("1.10").unwrap() // Win
            } else {
                Decimal::from_str("0.95").unwrap() // Loss
            };

            arms.close_random_control(&trade.token_mint, exit_price).await.unwrap();
        }

        let stats = arms.get_control_stats("random_token").await.unwrap().unwrap();

        assert_eq!(stats.total_trades, 3);
        assert_eq!(stats.win_count, 2);
        assert_eq!(stats.loss_count, 1);
        assert!((stats.win_rate - 0.666).abs() < 0.01); // ~66.7% win rate

        // No closed trades → Ok(None), not a fabricated 0% record
        let empty_stats = arms.get_control_stats("sol_benchmark").await.unwrap();
        assert!(empty_stats.is_none());

        // Unknown control type → Err
        assert!(arms.get_control_stats("bogus").await.is_err());
    }

    #[test]
    fn test_calculate_unrealized_pnl_branches() {
        let trade = ControlTrade::new(
            "random_token".to_string(),
            "mint".to_string(),
            Decimal::from_str("2.0").unwrap(),
            Decimal::from_str("0.1").unwrap(),
        );
        // entry > 0 → (current - entry) / entry * size.
        let pnl = trade.calculate_unrealized_pnl(Decimal::from_str("2.5").unwrap());
        assert_eq!(pnl, Decimal::from_str("0.025").unwrap());

        // Zero entry → ZERO (no panic).
        let zero = ControlTrade::new(
            "random_token".to_string(),
            "mint".to_string(),
            Decimal::ZERO,
            Decimal::from_str("0.1").unwrap(),
        );
        assert_eq!(zero.calculate_unrealized_pnl(Decimal::from_str("1.0").unwrap()), Decimal::ZERO);
    }

    #[tokio::test]
    async fn test_fire_random_control_with_no_tokens_errors() {
        let arms = ControlArms::new(vec![]);
        let err = arms
            .fire_random_token_control(Decimal::from_str("1.0").unwrap(), Decimal::from_str("0.02").unwrap())
            .await
            .unwrap_err();
        assert!(err.contains("No liquid tokens available"), "{err}");
    }

    #[tokio::test]
    async fn test_close_random_control_no_open_trade_errors() {
        let arms = ControlArms::new(vec!["mint".to_string()]);
        let err = arms
            .close_random_control("mint", Decimal::from_str("1.0").unwrap())
            .await
            .unwrap_err();
        assert!(err.contains("No open random control trade"), "{err}");
    }

    #[tokio::test]
    async fn test_close_sol_benchmark_paths() {
        let arms = ControlArms::new(vec!["mint".to_string()]);
        // No open trade → Err.
        let err = arms.close_sol_benchmark(Decimal::from_str("1.0").unwrap()).await.unwrap_err();
        assert!(err.contains("No open SOL benchmark trade"), "{err}");

        // Fire a benchmark then close → Ok.
        arms.fire_sol_benchmark_control(Decimal::from_str("1.0").unwrap(), Decimal::from_str("0.1").unwrap()).await;
        let pnl = arms.close_sol_benchmark(Decimal::from_str("1.1").unwrap()).await.unwrap();
        assert!(pnl > Decimal::ZERO);
        assert_eq!(arms.get_sol_benchmarks().await.len(), 1);
    }
}
