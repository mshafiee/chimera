//! Live tracer execution hook
//!
//! Executes micro live trades (0.02 SOL) alongside paper trades to measure
//! real execution gap between paper quotes and actual fills.

use chrono::{DateTime, Utc};
use rust_decimal::prelude::*;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Tracer execution state
#[derive(Debug, Clone)]
pub struct TracerState {
    /// Number of tracer trades executed so far
    pub tracer_count: u32,
    /// Timestamp of first tracer trade
    pub first_tracer_time: Option<DateTime<Utc>>,
    /// Current sample rate (starts at the configured rate)
    pub current_sample_rate: f64,
    /// Paper trade UUIDs that already fired a tracer (dedup — at most one
    /// tracer per paper trade)
    pub fired_paper_trades: HashSet<String>,
}

impl Default for TracerState {
    fn default() -> Self {
        Self {
            tracer_count: 0,
            first_tracer_time: None,
            current_sample_rate: 1.0,
            fired_paper_trades: HashSet::new(),
        }
    }
}

/// Execution gap measurement
#[derive(Debug, Clone)]
pub struct ExecutionGap {
    /// Paper fill price (per token)
    pub paper_fill_price: Decimal,
    /// Real fill price (per token)
    pub real_fill_price: Decimal,
    /// Execution gap as percentage: (real - paper) / paper
    pub gap_pct: Decimal,
    /// Trade side (entry/exit)
    pub side: String,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

impl ExecutionGap {
    pub fn new(
        paper_fill_price: Decimal,
        real_fill_price: Decimal,
        side: String,
    ) -> Self {
        let gap_pct = if paper_fill_price > Decimal::ZERO {
            (real_fill_price - paper_fill_price) / paper_fill_price * Decimal::from(100)
        } else {
            Decimal::ZERO
        };

        Self {
            paper_fill_price,
            real_fill_price,
            gap_pct,
            side,
            timestamp: Utc::now(),
        }
    }
}

/// Tracer hook for executing micro live trades
pub struct TracerHook {
    state: Arc<Mutex<TracerState>>,
    enabled: bool,
    /// Hard bound on live-trade exposure: no tracer fires once reached.
    tracer_cap: u32,
    initial_sample_rate: f64,
    #[allow(dead_code)] // Reserved for position-size gating of tracer orders
    min_live_position_sol: Decimal,
}

impl TracerHook {
    pub fn new(
        enabled: bool,
        tracer_cap: u32,
        sample_rate: f64,
        min_live_position_sol: Decimal,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(TracerState {
                tracer_count: 0,
                first_tracer_time: None,
                current_sample_rate: sample_rate,
                fired_paper_trades: HashSet::new(),
            })),
            enabled,
            tracer_cap,
            initial_sample_rate: sample_rate,
            min_live_position_sol,
        }
    }

    /// Check if tracer should fire for this paper trade
    ///
    /// `tracer_cap` is a HARD stop: once reached, no further tracers fire
    /// (the old tapering behavior never reached zero and left live-trade
    /// exposure unbounded).
    pub async fn should_fire_tracer(&self, paper_trade_uuid: &str) -> bool {
        if !self.enabled {
            return false;
        }

        let state = self.state.lock().await;

        if state.fired_paper_trades.contains(paper_trade_uuid) {
            return false;
        }

        if state.tracer_count >= self.tracer_cap {
            return false;
        }

        // Random sample at initial rate
        use rand::Rng;
        rand::rng().random::<f64>() < self.initial_sample_rate
    }

    /// Atomically decide and record a tracer fire.
    ///
    /// Checks dedup, the cap, and samples under a single lock, so concurrent
    /// callers can never both pass the cap check and over-fire. Returns the
    /// execution gap when the tracer fires, `None` otherwise.
    pub async fn try_record_tracer(
        &self,
        paper_trade_uuid: &str,
        paper_fill_price: Decimal,
        real_fill_price: Decimal,
        side: String,
    ) -> Option<ExecutionGap> {
        if !self.enabled {
            return None;
        }

        let mut state = self.state.lock().await;

        if state.fired_paper_trades.contains(paper_trade_uuid) {
            return None;
        }

        if state.tracer_count >= self.tracer_cap {
            return None;
        }

        use rand::Rng;
        if rand::rng().random::<f64>() >= self.initial_sample_rate {
            return None;
        }

        state.fired_paper_trades.insert(paper_trade_uuid.to_string());
        if state.first_tracer_time.is_none() {
            state.first_tracer_time = Some(Utc::now());
        }
        state.tracer_count += 1;

        Some(ExecutionGap::new(paper_fill_price, real_fill_price, side))
    }

    /// Record tracer execution and return execution gap
    ///
    /// Manual path: increments the count unconditionally. Prefer
    /// [`TracerHook::try_record_tracer`] for the decision+record flow.
    pub async fn record_tracer(
        &self,
        paper_fill_price: Decimal,
        real_fill_price: Decimal,
        side: String,
    ) -> ExecutionGap {
        let mut state = self.state.lock().await;

        // Update state
        if state.first_tracer_time.is_none() {
            state.first_tracer_time = Some(Utc::now());
        }
        state.tracer_count += 1;

        // Create execution gap measurement
        ExecutionGap::new(paper_fill_price, real_fill_price, side)
    }

    /// Get current tracer statistics
    pub async fn get_stats(&self) -> TracerState {
        self.state.lock().await.clone()
    }

    /// Check if tracer cap has been reached
    pub async fn cap_reached(&self) -> bool {
        let state = self.state.lock().await;
        state.tracer_count >= self.tracer_cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execution_gap_calculation() {
        let gap = ExecutionGap::new(
            Decimal::from_str("1.0").unwrap(),
            Decimal::from_str("1.05").unwrap(),
            "entry".to_string(),
        );

        assert_eq!(gap.gap_pct, Decimal::from_str("5.0").unwrap());
        assert_eq!(gap.side, "entry");
    }

    #[tokio::test]
    async fn test_should_fire_tracer() {
        let hook = TracerHook::new(true, 10, 1.0, Decimal::from_str("0.02").unwrap());

        // With 100% sample rate, should always fire
        for _ in 0..5 {
            assert!(hook.should_fire_tracer("test").await);
        }

        // With 0% sample rate, should never fire
        let hook_disabled = TracerHook::new(true, 10, 0.0, Decimal::from_str("0.02").unwrap());
        for _ in 0..5 {
            assert!(!hook_disabled.should_fire_tracer("test").await);
        }
    }

    #[tokio::test]
    async fn test_tracer_cap() {
        let hook = TracerHook::new(true, 2, 1.0, Decimal::from_str("0.02").unwrap());

        // Record 2 tracers to reach cap
        hook.record_tracer(Decimal::ONE, Decimal::ONE, "entry".to_string()).await;
        hook.record_tracer(Decimal::ONE, Decimal::ONE, "entry".to_string()).await;

        // Should cap after 2 tracers
        assert!(hook.cap_reached().await);
        // Hard stop: no further fires past the cap
        assert!(!hook.should_fire_tracer("another").await);
    }

    #[tokio::test]
    async fn test_try_record_tracer_dedup() {
        let hook = TracerHook::new(true, 10, 1.0, Decimal::from_str("0.02").unwrap());

        // 100% rate: first call fires
        assert!(hook
            .try_record_tracer("paper-1", Decimal::ONE, Decimal::ONE, "entry".to_string())
            .await
            .is_some());
        // Same paper trade cannot fire twice
        assert!(hook
            .try_record_tracer("paper-1", Decimal::ONE, Decimal::ONE, "entry".to_string())
            .await
            .is_none());
        // A different paper trade can
        assert!(hook
            .try_record_tracer("paper-2", Decimal::ONE, Decimal::ONE, "entry".to_string())
            .await
            .is_some());
    }
}
