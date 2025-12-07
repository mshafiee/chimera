//! Jito Tip Manager
//!
//! Provides dynamic tip calculation based on historical tip data.
//! Handles cold start scenarios and maintains tip history persistence.
//!
//! Strategy:
//! - Shield: Use tip floor (lower risk tolerance)
//! - Spear: Use percentile-based tip for bundle inclusion
//! - Exit: Use higher tip to ensure priority
//!
//! Cold Start:
//! - If < 10 successful tips in history, use tip_floor * 2
//! - After 10 tips, switch to percentile-based calculation

use crate::config::JitoConfig;
use crate::db::{self, DbPool};
use crate::error::AppResult;
use crate::models::Strategy;
use parking_lot::RwLock;
use std::sync::Arc;

/// Minimum samples required for percentile calculation
const MIN_SAMPLES_FOR_PERCENTILE: u32 = 10;

/// Cold start multiplier (tip_floor * this value)
const COLD_START_MULTIPLIER: f64 = 2.0;

/// Tip entry for in-memory history
#[derive(Debug, Clone)]
struct TipEntry {
    amount_sol: f64,
    strategy: Strategy,
}

/// Jito Tip Manager
pub struct TipManager {
    /// Jito configuration
    config: JitoConfig,
    /// Database pool
    db: DbPool,
    /// In-memory tip history (rolling window)
    history: Arc<RwLock<Vec<TipEntry>>>,
    /// Whether we're in cold start mode
    cold_start: Arc<RwLock<bool>>,
    /// Maximum history size
    max_history_size: usize,
}

impl TipManager {
    /// Create a new TipManager
    pub fn new(config: JitoConfig, db: DbPool) -> Self {
        Self {
            config,
            db,
            history: Arc::new(RwLock::new(Vec::new())),
            cold_start: Arc::new(RwLock::new(true)),
            max_history_size: 100,
        }
    }

    /// Initialize from database (load persisted tips)
    pub async fn init(&self) -> AppResult<()> {
        // Load recent tips from database
        let tips = db::get_recent_tips(&self.db, self.max_history_size as u32).await?;

        {
            let mut history = self.history.write();
            for tip in tips {
                history.push(TipEntry {
                    amount_sol: tip,
                    strategy: Strategy::Shield, // Default, not critical for calculation
                });
            }
        }

        // Check if we have enough samples
        let count = db::get_tip_count(&self.db).await?;
        if count >= MIN_SAMPLES_FOR_PERCENTILE {
            *self.cold_start.write() = false;
            tracing::info!(
                tip_count = count,
                "TipManager initialized with sufficient history"
            );
        } else {
            tracing::info!(
                tip_count = count,
                required = MIN_SAMPLES_FOR_PERCENTILE,
                "TipManager in cold start mode"
            );
        }

        Ok(())
    }

    /// Calculate optimal tip for a given strategy and trade size
    pub fn calculate_tip(&self, strategy: Strategy, trade_size_sol: f64) -> f64 {
        let is_cold_start = *self.cold_start.read();

        let base_tip = if is_cold_start {
            self.cold_start_tip(strategy)
        } else {
            self.percentile_tip(strategy)
        };

        // Apply percentage cap
        let max_by_percent = trade_size_sol * self.config.tip_percent_max;

        // Apply ceiling
        let tip = base_tip.min(max_by_percent).min(self.config.tip_ceiling_sol);

        // Ensure minimum
        tip.max(self.config.tip_floor_sol)
    }

    /// Cold start tip calculation
    fn cold_start_tip(&self, strategy: Strategy) -> f64 {
        match strategy {
            Strategy::Shield => self.config.tip_floor_sol * COLD_START_MULTIPLIER,
            Strategy::Spear => self.config.tip_floor_sol * COLD_START_MULTIPLIER * 1.5,
            Strategy::Exit => self.config.tip_ceiling_sol, // Max tip for exits
        }
    }

    /// Percentile-based tip calculation
    fn percentile_tip(&self, strategy: Strategy) -> f64 {
        let history = self.history.read();

        if history.is_empty() {
            return self.cold_start_tip(strategy);
        }

        // Get all tip amounts sorted
        let mut tips: Vec<f64> = history.iter().map(|e| e.amount_sol).collect();
        tips.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Calculate percentile index
        let percentile = match strategy {
            Strategy::Shield => 25,  // Conservative: 25th percentile
            Strategy::Spear => self.config.tip_percentile as usize, // Configured percentile (default 50)
            Strategy::Exit => 75,    // Higher: 75th percentile for exits
        };

        let index = (tips.len() * percentile / 100).min(tips.len() - 1);
        let percentile_tip = tips[index];

        // For Spear/Exit, use max of percentile and config floor
        match strategy {
            Strategy::Shield => percentile_tip.max(self.config.tip_floor_sol),
            Strategy::Spear => percentile_tip.max(self.config.tip_floor_sol),
            Strategy::Exit => percentile_tip.max(
                (self.config.tip_floor_sol + self.config.tip_ceiling_sol) / 2.0
            ),
        }
    }

    /// Record a tip (after successful bundle)
    pub async fn record_tip(
        &self,
        tip_amount_sol: f64,
        bundle_signature: Option<&str>,
        strategy: Strategy,
        success: bool,
    ) -> AppResult<()> {
        // Persist to database
        db::insert_jito_tip(
            &self.db,
            tip_amount_sol,
            bundle_signature,
            &strategy.to_string(),
            success,
        )
        .await?;

        if success {
            // Update in-memory history
            {
                let mut history = self.history.write();
                history.push(TipEntry {
                    amount_sol: tip_amount_sol,
                    strategy,
                });

                // Trim to max size (remove oldest)
                if history.len() > self.max_history_size {
                    history.remove(0);
                }
            }

            // Check if we can exit cold start
            if *self.cold_start.read() {
                let count = self.history.read().len() as u32;
                if count >= MIN_SAMPLES_FOR_PERCENTILE {
                    *self.cold_start.write() = false;
                    tracing::info!(
                        "Exiting cold start mode after {} successful tips",
                        count
                    );
                }
            }
        }

        Ok(())
    }

    /// Get current tip statistics
    pub fn stats(&self) -> TipStats {
        let history = self.history.read();

        if history.is_empty() {
            return TipStats::default();
        }

        let tips: Vec<f64> = history.iter().map(|e| e.amount_sol).collect();
        let sum: f64 = tips.iter().sum();
        let avg = sum / tips.len() as f64;
        let min = tips.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = tips.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        TipStats {
            count: tips.len(),
            avg_tip_sol: avg,
            min_tip_sol: min,
            max_tip_sol: max,
            is_cold_start: *self.cold_start.read(),
        }
    }

    /// Check if in cold start mode
    pub fn is_cold_start(&self) -> bool {
        *self.cold_start.read()
    }
}

/// Tip statistics
#[derive(Debug, Clone, Default)]
pub struct TipStats {
    /// Number of tips in history
    pub count: usize,
    /// Average tip amount
    pub avg_tip_sol: f64,
    /// Minimum tip
    pub min_tip_sol: f64,
    /// Maximum tip
    pub max_tip_sol: f64,
    /// Whether in cold start mode
    pub is_cold_start: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cold_start_tip() {
        let config = JitoConfig {
            enabled: true,
            tip_floor_sol: 0.001,
            tip_ceiling_sol: 0.01,
            tip_percentile: 50,
            tip_percent_max: 0.10,
        };

        // Cold start tip should be floor * 2
        let cold_tip = config.tip_floor_sol * COLD_START_MULTIPLIER;
        assert!((cold_tip - 0.002).abs() < 0.0001);
    }

    #[test]
    fn test_tip_stats_default() {
        let stats = TipStats::default();
        assert_eq!(stats.count, 0);
        assert!(stats.is_cold_start);
    }
}
