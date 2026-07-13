//! Live tracer execution hook
//!
//! Executes micro live trades (0.02 SOL) alongside paper trades to measure
//! real execution gap between paper quotes and actual fills.

use chrono::{DateTime, Utc};
use rust_decimal::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Tracer execution state
#[derive(Debug, Clone)]
pub struct TracerState {
    /// Number of tracer trades executed so far
    pub tracer_count: u32,
    /// Timestamp of first tracer trade
    pub first_tracer_time: Option<DateTime<Utc>>,
    /// Current sample rate (decreases after cap)
    pub current_sample_rate: f64,
}

impl Default for TracerState {
    fn default() -> Self {
        Self {
            tracer_count: 0,
            first_tracer_time: None,
            current_sample_rate: 1.0,
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
    tracer_cap: u32,
    initial_sample_rate: f64,
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
            state: Arc::new(Mutex::new(TracerState::default())),
            enabled,
            tracer_cap,
            initial_sample_rate: sample_rate,
            min_live_position_sol,
        }
    }

    /// Check if tracer should fire for this paper trade
    pub async fn should_fire_tracer(&self, paper_trade_uuid: &str) -> bool {
        if !self.enabled {
            return false;
        }

        let mut state = self.state.lock().await;

        // Check if cap reached
        if state.tracer_count >= self.tracer_cap {
            // Taper sample rate after cap
            let taper_factor = (self.tracer_cap as f64) / (state.tracer_count as f64 + 1.0);
            state.current_sample_rate = self.initial_sample_rate * taper_factor;

            // Sample randomly based on tapered rate
            use rand::Rng;
            let mut rng = rand::thread_rng();
            let should_sample: f64 = rng.gen();
            return should_sample < state.current_sample_rate;
        }

        // Random sample at initial rate
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let should_sample: f64 = rng.gen();
        should_sample < self.initial_sample_rate
    }

    /// Record tracer execution and return execution gap
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
    }
}
