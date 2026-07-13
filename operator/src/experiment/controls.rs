//! Control arms for forward test
//!
//! Implements random-token and SOL benchmark control arms to measure
//! edge vs beta performance.

use chrono::{DateTime, Utc};
use rust_decimal::prelude::*;
use std::collections::HashMap;
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

    /// Close the control trade and calculate PnL
    pub fn close(&mut self, exit_price: Decimal) -> Decimal {
        self.exit_time = Some(Utc::now());
        self.exit_price = Some(exit_price);

        // Calculate PnL: (exit - entry) / entry * position_size
        if self.entry_price > Decimal::ZERO {
            self.pnl = Some(
                (exit_price - self.entry_price) / self.entry_price * self.position_size_sol
            );
            self.pnl.unwrap()
        } else {
            Decimal::ZERO
        }
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

        // Randomly select a liquid token
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as usize;
        let idx = timestamp % self.liquid_tokens.len();
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
        if let Some(trade) = trades.iter_mut().find(|t| t.token_mint == token_mint && t.exit_time.is_none()) {
            Ok(trade.close(exit_price))
        } else {
            Err(format!("No open random control trade found for {}", token_mint))
        }
    }

    /// Close SOL benchmark control
    pub async fn close_sol_benchmark(&self, exit_price: Decimal) -> Result<Decimal, String> {
        let mut trades = self.sol_bench_trades.lock().await;
        if let Some(trade) = trades.iter_mut().find(|t| t.exit_time.is_none()) {
            Ok(trade.close(exit_price))
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
    pub async fn get_control_stats(&self, control_type: &str) -> ControlStats {
        let trades = match control_type {
            "random_token" => self.get_random_controls().await,
            "sol_benchmark" => self.get_sol_benchmarks().await,
            _ => return ControlStats::default(),
        };

        let closed_trades: Vec<_> = trades.iter().filter(|t| t.exit_time.is_some()).collect();

        if closed_trades.is_empty() {
            return ControlStats::default();
        }

        let total_pnl: Decimal = closed_trades.iter()
            .filter_map(|t| t.pnl)
            .sum();

        let win_count = closed_trades.iter().filter(|t| {
            t.pnl.map_or(false, |p| p > Decimal::ZERO)
        }).count();

        let avg_pnl = total_pnl / Decimal::from(closed_trades.len() as u64);
        let win_rate = (win_count as f64) / (closed_trades.len() as f64);

        ControlStats {
            total_trades: closed_trades.len(),
            total_pnl,
            avg_pnl,
            win_rate,
            win_count,
            loss_count: closed_trades.len() - win_count,
        }
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

        let pnl = trade.close(Decimal::from_str("1.10").unwrap());

        assert_eq!(pnl, Decimal::from_str("0.002").unwrap()); // 10% gain on 0.02 SOL
        assert!(trade.exit_time.is_some());
        assert_eq!(trade.exit_price, Some(Decimal::from_str("1.10").unwrap()));
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

        let stats = arms.get_control_stats("random_token").await;

        assert_eq!(stats.total_trades, 3);
        assert_eq!(stats.win_count, 2);
        assert_eq!(stats.loss_count, 1);
        assert!((stats.win_rate - 0.666).abs() < 0.01); // ~66.7% win rate
    }
}
