//! Experiment ledger for recording forward test data
//!
//! Records all paper trades, tracer executions, control arms, and
//! execution gaps for verdict evaluation.
//!
//! NOTE: this ledger is currently in-memory only — data is lost on restart.
//! Persistence to the operator database is not yet implemented.

use chrono::{DateTime, Utc};
use rust_decimal::prelude::*;
use serde::{Deserialize, Serialize};

/// Signal side for a trade
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SignalSide {
    /// Long trade
    Buy,
    /// Short trade
    Sell,
}

impl std::fmt::Display for SignalSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignalSide::Buy => write!(f, "BUY"),
            SignalSide::Sell => write!(f, "SELL"),
        }
    }
}

/// Experiment strategy type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExperimentStrategy {
    /// Conservative strategy
    Shield,
    /// Aggressive strategy
    Spear,
}

impl std::fmt::Display for ExperimentStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExperimentStrategy::Shield => write!(f, "Shield"),
            ExperimentStrategy::Spear => write!(f, "Spear"),
        }
    }
}

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
    pub signal_side: SignalSide,
    /// Paper fill price (per token)
    pub paper_fill_price: Option<Decimal>,
    /// Real fill price from tracer (per token)
    pub real_fill_price: Option<Decimal>,
    /// Paper PnL (percentage return, sign-aware for shorts)
    pub paper_pnl: Option<Decimal>,
    /// Real PnL from tracer (percentage return, sign-aware for shorts)
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
    pub strategy: ExperimentStrategy,
}

impl ExperimentTrade {
    pub fn new(
        trade_uuid: String,
        wallet: String,
        token: String,
        signal_side: SignalSide,
        strategy: ExperimentStrategy,
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
    ///
    /// PnL is a sign-aware percentage return: shorts (SELL) profit when the
    /// price falls, longs (BUY) profit when it rises. Duplicate closes are
    /// rejected and `exit_time` is only committed after the PnL computation
    /// succeeds, so a failed close never leaves the trade half-closed.
    pub fn close_trade(&mut self, exit_price: Decimal) -> Result<Decimal, String> {
        if self.exit_time.is_some() {
            return Err("Trade already closed".to_string());
        }

        let paper_entry = self
            .paper_fill_price
            .ok_or("Cannot close trade without paper fill price".to_string())?;
        if paper_entry <= Decimal::ZERO {
            return Err(format!("Invalid paper fill price: {}", paper_entry));
        }

        // Calculate paper PnL (sign-aware percentage return)
        let paper_pnl = Self::pnl_percent(paper_entry, exit_price, self.signal_side);
        self.paper_pnl = Some(paper_pnl);

        // Calculate real PnL if tracer executed (sign-aware percentage return)
        if let Some(real_entry) = self.real_fill_price {
            if real_entry > Decimal::ZERO {
                self.real_pnl = Some(Self::pnl_percent(real_entry, exit_price, self.signal_side));
            }
        }

        self.exit_time = Some(Utc::now());

        Ok(paper_pnl)
    }

    /// Sign-aware percentage return for an entry/exit pair.
    fn pnl_percent(entry: Decimal, exit: Decimal, side: SignalSide) -> Decimal {
        let raw = match side {
            SignalSide::Sell => (entry - exit) / entry,
            SignalSide::Buy => (exit - entry) / entry,
        };
        raw * Decimal::from(100)
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
    ///
    /// All statistics are computed over a single well-defined population: the
    /// closed trades. Zero-PnL trades are not counted as losses.
    pub fn calculate_statistics(&self) -> ExperimentStats {
        let closed_trades = self.get_closed_trades();

        if closed_trades.is_empty() {
            return ExperimentStats::default();
        }

        let total_trades = closed_trades.len();
        let tracer_count = closed_trades.iter().filter(|t| t.is_tracer).count();

        // Paper PnL statistics (closed trades only)
        let paper_pnl_values: Vec<Decimal> = closed_trades.iter()
            .filter_map(|t| t.paper_pnl)
            .collect();

        let total_paper_pnl: Decimal = paper_pnl_values.iter().sum();
        let avg_paper_pnl = if !paper_pnl_values.is_empty() {
            total_paper_pnl / Decimal::from(paper_pnl_values.len() as u64)
        } else {
            Decimal::ZERO
        };

        // Real PnL statistics (closed tracer trades only)
        let real_pnl_values: Vec<Decimal> = closed_trades.iter()
            .filter(|t| t.is_tracer)
            .filter_map(|t| t.real_pnl)
            .collect();

        let total_real_pnl: Decimal = real_pnl_values.iter().sum();
        let avg_real_pnl = if !real_pnl_values.is_empty() {
            total_real_pnl / Decimal::from(real_pnl_values.len() as u64)
        } else {
            Decimal::ZERO
        };

        // Execution gap statistics (closed tracer trades only)
        let execution_gaps: Vec<Decimal> = closed_trades.iter()
            .filter(|t| t.is_tracer)
            .filter_map(|t| t.execution_gap)
            .collect();

        let avg_execution_gap = if !execution_gaps.is_empty() {
            let sum: Decimal = execution_gaps.iter().sum();
            sum / Decimal::from(execution_gaps.len() as u64)
        } else {
            Decimal::ZERO
        };

        // Win rate over closed trades with a paper PnL; flat (zero-PnL) trades
        // are neither wins nor losses.
        let wins = paper_pnl_values.iter().filter(|p| **p > Decimal::ZERO).count();
        let flat = paper_pnl_values.iter().filter(|p| **p == Decimal::ZERO).count();
        let losses = paper_pnl_values.len() - wins - flat;
        let win_rate = if !paper_pnl_values.is_empty() {
            (wins as f64) / (paper_pnl_values.len() as f64)
        } else {
            0.0
        };

        ExperimentStats {
            total_trades,
            tracer_count,
            total_paper_pnl,
            avg_paper_pnl,
            total_real_pnl,
            avg_real_pnl,
            avg_execution_gap,
            win_rate,
            wins,
            losses,
        }
    }
}

/// Experiment statistics
#[derive(Debug, Clone, Default)]
pub struct ExperimentStats {
    /// Total number of closed trades
    pub total_trades: usize,
    /// Number of closed tracer trades
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
    use std::str::FromStr;

    #[test]
    fn test_experiment_trade_creation() {
        let trade = ExperimentTrade::new(
            "test_uuid".to_string(),
            "wallet_address".to_string(),
            "token_mint".to_string(),
            SignalSide::Buy,
            ExperimentStrategy::Shield,
        );

        assert_eq!(trade.trade_uuid, "test_uuid");
        assert_eq!(trade.signal_side.to_string(), "BUY");
        assert_eq!(trade.strategy.to_string(), "Shield");
        assert!(!trade.is_tracer);
    }

    #[test]
    fn test_paper_result_update() {
        let mut trade = ExperimentTrade::new(
            "test_uuid".to_string(),
            "wallet".to_string(),
            "token".to_string(),
            SignalSide::Buy,
            ExperimentStrategy::Spear,
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
            SignalSide::Buy,
            ExperimentStrategy::Shield,
        );

        trade.update_paper_result(Decimal::from_str("1.0").unwrap(), 250);

        let pnl = trade.close_trade(Decimal::from_str("1.10").unwrap()).unwrap();

        assert_eq!(pnl, Decimal::from_str("10.0").unwrap()); // 10% gain
        assert!(trade.exit_time.is_some());
        assert_eq!(trade.paper_pnl, Some(Decimal::from_str("10.0").unwrap()));

        // Duplicate close is rejected
        assert!(trade.close_trade(Decimal::from_str("1.20").unwrap()).is_err());
    }

    #[test]
    fn test_trade_close_short() {
        let mut trade = ExperimentTrade::new(
            "test_uuid".to_string(),
            "wallet".to_string(),
            "token".to_string(),
            SignalSide::Sell,
            ExperimentStrategy::Shield,
        );

        trade.update_paper_result(Decimal::from_str("1.0").unwrap(), 250);

        // Short entered at 1.0, exits at 0.90 → +10%
        let pnl = trade.close_trade(Decimal::from_str("0.90").unwrap()).unwrap();

        assert_eq!(pnl, Decimal::from_str("10.0").unwrap());
        assert_eq!(trade.paper_pnl, Some(Decimal::from_str("10.0").unwrap()));
    }

    #[test]
    fn test_trade_close_rejects_zero_entry() {
        let mut trade = ExperimentTrade::new(
            "test_uuid".to_string(),
            "wallet".to_string(),
            "token".to_string(),
            SignalSide::Buy,
            ExperimentStrategy::Shield,
        );

        trade.update_paper_result(Decimal::ZERO, 250);

        assert!(trade.close_trade(Decimal::from_str("1.0").unwrap()).is_err());
        // exit_time must not be committed on failure
        assert!(trade.exit_time.is_none());
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
                SignalSide::Buy,
                ExperimentStrategy::Shield,
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
        assert_eq!(stats.total_paper_pnl, Decimal::from_str("15.0").unwrap()); // 5 + 5 - 5
    }
}
