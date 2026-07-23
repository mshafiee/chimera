//! Experiment ledger for recording forward test data
//!
//! Records all paper trades, tracer executions, control arms, and
//! execution gaps in the operator database for verdict evaluation.

use chrono::{DateTime, Utc};
use rust_decimal::prelude::*;
use serde::{Deserialize, Serialize};

/// Single trade record in the experiment ledger
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentTrade {
    /// Trade UUID
    pub trade_uuid: String,
    /// Wallet address that triggered the signal
    pub wallet: String,
    /// Token mint address
    pub token: String,
    /// Signal side (BUY/SELL)
    pub signal_side: String,
    /// Paper fill price (per token)
    pub paper_fill_price: Option<Decimal>,
    /// Real fill price from tracer (per token)
    pub real_fill_price: Option<Decimal>,
    /// Paper PnL (USD)
    pub paper_pnl: Option<Decimal>,
    /// Real PnL from tracer (USD)
    pub real_pnl: Option<Decimal>,
    /// Entry latency in milliseconds
    pub entry_latency_ms: Option<u64>,
    /// Jito tip paid (SOL)
    pub jito_tip_sol: Option<Decimal>,
    /// DEX fee paid (SOL)
    pub dex_fee_sol: Option<Decimal>,
    /// Execution gap percentage (real - paper) / paper
    pub execution_gap: Option<Decimal>,
    /// Control arm random token PnL
    pub control_random_pnl: Option<Decimal>,
    /// SOL benchmark PnL
    pub sol_bench_pnl: Option<Decimal>,
    /// Is this a tracer trade?
    pub is_tracer: bool,
    /// Toxic flow flag
    pub toxic_flag: bool,
    /// Entry timestamp
    pub entry_time: DateTime<Utc>,
    /// Exit timestamp (if closed)
    pub exit_time: Option<DateTime<Utc>>,
    /// Strategy type (Shield/Spear)
    pub strategy: String,
}

impl ExperimentTrade {
    pub fn new(
        trade_uuid: String,
        wallet: String,
        token: String,
        signal_side: String,
        strategy: String,
    ) -> Self {
        Self {
            trade_uuid,
            wallet,
            token,
            signal_side,
            paper_fill_price: None,
            real_fill_price: None,
            paper_pnl: None,
            real_pnl: None,
            entry_latency_ms: None,
            jito_tip_sol: None,
            dex_fee_sol: None,
            execution_gap: None,
            control_random_pnl: None,
            sol_bench_pnl: None,
            is_tracer: false,
            toxic_flag: false,
            entry_time: Utc::now(),
            exit_time: None,
            strategy,
        }
    }

    /// Update with paper execution result
    pub fn update_paper_result(
        &mut self,
        fill_price: Decimal,
        latency_ms: u64,
    ) {
        self.paper_fill_price = Some(fill_price);
        self.entry_latency_ms = Some(latency_ms);
    }

    /// Update with tracer execution result
    pub fn update_tracer_result(
        &mut self,
        real_fill_price: Decimal,
        execution_gap: Decimal,
        jito_tip: Decimal,
        dex_fee: Decimal,
    ) {
        self.real_fill_price = Some(real_fill_price);
        self.execution_gap = Some(execution_gap);
        self.jito_tip_sol = Some(jito_tip);
        self.dex_fee_sol = Some(dex_fee);
        self.is_tracer = true;
    }

    /// Close the trade and calculate PnL
    pub fn close_trade(&mut self, exit_price: Decimal) -> Result<Decimal, String> {
        if self.paper_fill_price.is_none() {
            return Err("Cannot close trade without paper fill price".to_string());
        }

        self.exit_time = Some(Utc::now());

        // Calculate paper PnL
        if let Some(paper_entry) = self.paper_fill_price {
            if paper_entry > Decimal::ZERO {
                self.paper_pnl = Some((exit_price - paper_entry) / paper_entry * Decimal::from(100));
            }
        }

        // Calculate real PnL if tracer executed
        if let Some(real_entry) = self.real_fill_price {
            if real_entry > Decimal::ZERO {
                self.real_pnl = Some((exit_price - real_entry) / real_entry * Decimal::from(100));
            }
        }

        self.paper_pnl.ok_or("Failed to calculate paper PnL".to_string())
    }
}

/// Experiment ledger manager
pub struct ExperimentLedger {
    trades: Vec<ExperimentTrade>,
}

impl Default for ExperimentLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl ExperimentLedger {
    pub fn new() -> Self {
        Self {
            trades: Vec::new(),
        }
    }

    /// Record a new trade
    pub fn record_trade(&mut self, trade: ExperimentTrade) {
        self.trades.push(trade);
    }

    /// Update existing trade
    pub fn update_trade<F>(&mut self, trade_uuid: &str, update_fn: F) -> Result<(), String>
    where
        F: FnOnce(&mut ExperimentTrade),
    {
        if let Some(trade) = self.trades.iter_mut().find(|t| t.trade_uuid == trade_uuid) {
            update_fn(trade);
            Ok(())
        } else {
            Err(format!("Trade {} not found in ledger", trade_uuid))
        }
    }

    /// Get trade by UUID
    pub fn get_trade(&self, trade_uuid: &str) -> Option<&ExperimentTrade> {
        self.trades.iter().find(|t| t.trade_uuid == trade_uuid)
    }

    /// Get all trades
    pub fn get_all_trades(&self) -> Vec<ExperimentTrade> {
        self.trades.clone()
    }

    /// Get only tracer trades
    pub fn get_tracer_trades(&self) -> Vec<ExperimentTrade> {
        self.trades.iter()
            .filter(|t| t.is_tracer)
            .cloned()
            .collect()
    }

    /// Get only paper trades
    pub fn get_paper_trades(&self) -> Vec<ExperimentTrade> {
        self.trades.iter()
            .filter(|t| !t.is_tracer)
            .cloned()
            .collect()
    }

    /// Get closed trades
    pub fn get_closed_trades(&self) -> Vec<ExperimentTrade> {
        self.trades.iter()
            .filter(|t| t.exit_time.is_some())
            .cloned()
            .collect()
    }

    /// Calculate aggregate statistics
    pub fn calculate_statistics(&self) -> ExperimentStats {
        let closed_trades = self.get_closed_trades();
        let tracer_trades = self.get_tracer_trades();

        let total_trades = closed_trades.len();

        if total_trades == 0 {
            return ExperimentStats::default();
        }

        // Paper PnL statistics
        let paper_pnl_values: Vec<Decimal> = closed_trades.iter()
            .filter_map(|t| t.paper_pnl)
            .collect();

        let total_paper_pnl: Decimal = paper_pnl_values.iter().sum();
        let avg_paper_pnl = if !paper_pnl_values.is_empty() {
            total_paper_pnl / Decimal::from(paper_pnl_values.len() as u64)
        } else {
            Decimal::ZERO
        };

        // Real PnL statistics (from tracers)
        let real_pnl_values: Vec<Decimal> = tracer_trades.iter()
            .filter_map(|t| t.real_pnl)
            .collect();

        let total_real_pnl: Decimal = real_pnl_values.iter().sum();
        let avg_real_pnl = if !real_pnl_values.is_empty() {
            total_real_pnl / Decimal::from(real_pnl_values.len() as u64)
        } else {
            Decimal::ZERO
        };

        // Execution gap statistics
        let execution_gaps: Vec<Decimal> = tracer_trades.iter()
            .filter_map(|t| t.execution_gap)
            .collect();

        let avg_execution_gap = if !execution_gaps.is_empty() {
            let sum: Decimal = execution_gaps.iter().sum();
            sum / Decimal::from(execution_gaps.len() as u64)
        } else {
            Decimal::ZERO
        };

        // Win rate
        let wins = paper_pnl_values.iter().filter(|p| **p > Decimal::ZERO).count();
        let win_rate = if !paper_pnl_values.is_empty() {
            (wins as f64) / (paper_pnl_values.len() as f64)
        } else {
            0.0
        };

        ExperimentStats {
            total_trades,
            tracer_count: tracer_trades.len(),
            total_paper_pnl,
            avg_paper_pnl,
            total_real_pnl,
            avg_real_pnl,
            avg_execution_gap,
            win_rate,
            wins,
            losses: paper_pnl_values.len() - wins,
        }
    }
}

/// Experiment statistics
#[derive(Debug, Clone, Default)]
pub struct ExperimentStats {
    /// Total number of closed trades
    pub total_trades: usize,
    /// Number of tracer trades executed
    pub tracer_count: usize,
    /// Total paper PnL
    pub total_paper_pnl: Decimal,
    /// Average paper PnL per trade
    pub avg_paper_pnl: Decimal,
    /// Total real PnL from tracers
    pub total_real_pnl: Decimal,
    /// Average real PnL per tracer trade
    pub avg_real_pnl: Decimal,
    /// Average execution gap percentage
    pub avg_execution_gap: Decimal,
    /// Win rate (0.0 - 1.0)
    pub win_rate: f64,
    /// Number of winning trades
    pub wins: usize,
    /// Number of losing trades
    pub losses: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_experiment_trade_creation() {
        let trade = ExperimentTrade::new(
            "test_uuid".to_string(),
            "wallet_address".to_string(),
            "token_mint".to_string(),
            "BUY".to_string(),
            "Shield".to_string(),
        );

        assert_eq!(trade.trade_uuid, "test_uuid");
        assert_eq!(trade.signal_side, "BUY");
        assert_eq!(trade.strategy, "Shield");
        assert!(!trade.is_tracer);
    }

    #[test]
    fn test_paper_result_update() {
        let mut trade = ExperimentTrade::new(
            "test_uuid".to_string(),
            "wallet".to_string(),
            "token".to_string(),
            "BUY".to_string(),
            "Spear".to_string(),
        );

        trade.update_paper_result(Decimal::from_str("1.0").unwrap(), 250);

        assert_eq!(trade.paper_fill_price, Some(Decimal::from_str("1.0").unwrap()));
        assert_eq!(trade.entry_latency_ms, Some(250));
    }

    #[test]
    fn test_trade_close() {
        let mut trade = ExperimentTrade::new(
            "test_uuid".to_string(),
            "wallet".to_string(),
            "token".to_string(),
            "BUY".to_string(),
            "Shield".to_string(),
        );

        trade.update_paper_result(Decimal::from_str("1.0").unwrap(), 250);

        let pnl = trade.close_trade(Decimal::from_str("1.10").unwrap()).unwrap();

        assert_eq!(pnl, Decimal::from_str("10.0").unwrap()); // 10% gain
        assert!(trade.exit_time.is_some());
        assert_eq!(trade.paper_pnl, Some(Decimal::from_str("10.0").unwrap()));
    }

    #[test]
    fn test_ledger_statistics() {
        let mut ledger = ExperimentLedger::new();

        // Add 3 trades (2 wins, 1 loss)
        for i in 0..3 {
            let mut trade = ExperimentTrade::new(
                format!("uuid_{}", i),
                "wallet".to_string(),
                "token".to_string(),
                "BUY".to_string(),
                "Shield".to_string(),
            );

            trade.update_paper_result(Decimal::from_str("1.0").unwrap(), 250);

            let exit_price = if i < 2 {
                Decimal::from_str("1.10").unwrap() // Win
            } else {
                Decimal::from_str("0.95").unwrap() // Loss
            };

            trade.close_trade(exit_price).unwrap();
            ledger.record_trade(trade);
        }

        let stats = ledger.calculate_statistics();

        assert_eq!(stats.total_trades, 3);
        assert_eq!(stats.wins, 2);
        assert_eq!(stats.losses, 1);
        assert_eq!(stats.total_paper_pnl, Decimal::from_str("15.0").unwrap()); // 5 + 5 - 5 (0.02 SOL each)
    }
}
