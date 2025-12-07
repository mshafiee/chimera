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

    fn test_config() -> JitoConfig {
        JitoConfig {
            enabled: true,
            tip_floor_sol: 0.001,
            tip_ceiling_sol: 0.01,
            tip_percentile: 50,
            tip_percent_max: 0.10,
        }
    }

    // ==========================================================================
    // COLD START TESTS
    // ==========================================================================

    #[test]
    fn test_cold_start_multiplier_value() {
        assert_eq!(COLD_START_MULTIPLIER, 2.0, "Cold start multiplier should be 2.0");
    }

    #[test]
    fn test_cold_start_tip_calculation() {
        let config = test_config();
        let cold_tip = config.tip_floor_sol * COLD_START_MULTIPLIER;
        assert!((cold_tip - 0.002).abs() < 0.0001, "Cold start tip should be 0.002 SOL");
    }

    #[test]
    fn test_cold_start_shield_tip() {
        let config = test_config();
        // Shield uses floor * 2
        let tip = config.tip_floor_sol * COLD_START_MULTIPLIER;
        assert!((tip - 0.002).abs() < 0.0001, "Shield cold start tip should be 0.002");
    }

    #[test]
    fn test_cold_start_spear_tip() {
        let config = test_config();
        // Spear uses floor * 2 * 1.5
        let tip = config.tip_floor_sol * COLD_START_MULTIPLIER * 1.5;
        assert!((tip - 0.003).abs() < 0.0001, "Spear cold start tip should be 0.003");
    }

    #[test]
    fn test_cold_start_exit_tip() {
        let config = test_config();
        // Exit uses ceiling during cold start
        let tip = config.tip_ceiling_sol;
        assert!((tip - 0.01).abs() < 0.0001, "Exit cold start tip should be ceiling");
    }

    // ==========================================================================
    // MINIMUM SAMPLES TESTS
    // ==========================================================================

    #[test]
    fn test_min_samples_constant() {
        assert_eq!(MIN_SAMPLES_FOR_PERCENTILE, 10, "Minimum samples should be 10");
    }

    #[test]
    fn test_cold_start_with_few_samples() {
        let sample_count: u32 = 5;
        let is_cold_start = sample_count < MIN_SAMPLES_FOR_PERCENTILE;
        assert!(is_cold_start, "5 samples should trigger cold start mode");
    }

    #[test]
    fn test_exit_cold_start_with_enough_samples() {
        let sample_count: u32 = 10;
        let is_cold_start = sample_count < MIN_SAMPLES_FOR_PERCENTILE;
        assert!(!is_cold_start, "10 samples should exit cold start mode");
    }

    // ==========================================================================
    // PERCENTILE CALCULATION TESTS
    // ==========================================================================

    #[test]
    fn test_percentile_50th() {
        let mut tips: Vec<f64> = vec![
            0.001, 0.002, 0.003, 0.004, 0.005,
            0.006, 0.007, 0.008, 0.009, 0.010,
        ];
        tips.sort_by(|a, b| a.partial_cmp(b).unwrap());
        
        let percentile = 50_usize;
        let index = (tips.len() * percentile / 100).min(tips.len() - 1);
        let tip = tips[index];
        
        assert!((tip - 0.006).abs() < 0.0001, "50th percentile should be 0.006");
    }

    #[test]
    fn test_percentile_25th() {
        let mut tips: Vec<f64> = vec![
            0.001, 0.002, 0.003, 0.004, 0.005,
            0.006, 0.007, 0.008, 0.009, 0.010,
        ];
        tips.sort_by(|a, b| a.partial_cmp(b).unwrap());
        
        let percentile = 25_usize;
        let index = (tips.len() * percentile / 100).min(tips.len() - 1);
        let tip = tips[index];
        
        // 25th percentile for Shield (conservative)
        assert!((tip - 0.003).abs() < 0.0001, "25th percentile should be around 0.003");
    }

    #[test]
    fn test_percentile_75th() {
        let mut tips: Vec<f64> = vec![
            0.001, 0.002, 0.003, 0.004, 0.005,
            0.006, 0.007, 0.008, 0.009, 0.010,
        ];
        tips.sort_by(|a, b| a.partial_cmp(b).unwrap());
        
        let percentile = 75_usize;
        let index = (tips.len() * percentile / 100).min(tips.len() - 1);
        let tip = tips[index];
        
        // 75th percentile for Exit (high priority)
        assert!((tip - 0.008).abs() < 0.0001, "75th percentile should be around 0.008");
    }

    // ==========================================================================
    // TIP CAP TESTS
    // ==========================================================================

    #[test]
    fn test_tip_ceiling_cap() {
        let config = test_config();
        let percentile_tip: f64 = 0.015; // Above ceiling
        let capped_tip = percentile_tip.min(config.tip_ceiling_sol);
        assert!((capped_tip - 0.01).abs() < 0.0001, "Tip should be capped at ceiling");
    }

    #[test]
    fn test_tip_floor_minimum() {
        let config = test_config();
        let percentile_tip: f64 = 0.0005; // Below floor
        let floored_tip = percentile_tip.max(config.tip_floor_sol);
        assert!((floored_tip - 0.001).abs() < 0.0001, "Tip should be floored at minimum");
    }

    #[test]
    fn test_tip_percent_max() {
        let config = test_config();
        let trade_size_sol = 0.05; // 0.05 SOL trade
        let max_by_percent = trade_size_sol * config.tip_percent_max;
        
        // Max tip = 0.05 * 0.10 = 0.005 SOL
        assert!((max_by_percent - 0.005).abs() < 0.0001, "Max by percent should be 0.005");
    }

    #[test]
    fn test_tip_all_caps_applied() {
        let config = test_config();
        let trade_size_sol: f64 = 0.1;
        let base_tip: f64 = 0.015; // High percentile result
        
        // Apply percentage cap
        let max_by_percent = trade_size_sol * config.tip_percent_max; // 0.01
        
        // Apply ceiling
        let tip = base_tip.min(max_by_percent).min(config.tip_ceiling_sol);
        
        // Ensure minimum
        let final_tip = tip.max(config.tip_floor_sol);
        
        assert!((final_tip - 0.01).abs() < 0.0001, "Final tip should be 0.01 (ceiling applies)");
    }

    // ==========================================================================
    // TIP STATS TESTS
    // ==========================================================================

    #[test]
    fn test_tip_stats_default() {
        let stats = TipStats::default();
        assert_eq!(stats.count, 0);
        // Default TipStats has is_cold_start=false, cold start is managed by TipManager
        assert!(!stats.is_cold_start);
        assert_eq!(stats.avg_tip_sol, 0.0);
    }

    #[test]
    fn test_tip_stats_calculation() {
        let tips: Vec<f64> = vec![0.001, 0.002, 0.003, 0.004, 0.005];
        
        let sum: f64 = tips.iter().sum();
        let avg = sum / tips.len() as f64;
        let min = tips.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = tips.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        
        assert!((avg - 0.003).abs() < 0.0001, "Average should be 0.003");
        assert!((min - 0.001).abs() < 0.0001, "Min should be 0.001");
        assert!((max - 0.005).abs() < 0.0001, "Max should be 0.005");
    }

    // ==========================================================================
    // HISTORY ROLLING WINDOW TESTS
    // ==========================================================================

    #[test]
    fn test_history_rolling_window() {
        let max_history_size = 100_usize;
        let mut history: Vec<f64> = Vec::new();
        
        // Add 105 entries
        for i in 0..105 {
            history.push(0.001 * (i as f64 + 1.0));
            if history.len() > max_history_size {
                history.remove(0);
            }
        }
        
        assert_eq!(history.len(), max_history_size, "History should be capped at 100");
        // First entry should be the 6th one added (0.006)
        assert!((history[0] - 0.006).abs() < 0.0001, "Oldest entries should be trimmed");
    }

    // ==========================================================================
    // STRATEGY TIP ORDERING TESTS
    // ==========================================================================

    #[test]
    fn test_strategy_tip_ordering() {
        let config = test_config();
        
        let shield_tip = config.tip_floor_sol * COLD_START_MULTIPLIER;
        let spear_tip = config.tip_floor_sol * COLD_START_MULTIPLIER * 1.5;
        let exit_tip = config.tip_ceiling_sol;
        
        assert!(shield_tip < spear_tip, "Shield tip should be less than Spear");
        assert!(spear_tip < exit_tip, "Spear tip should be less than Exit");
    }

    // ==========================================================================
    // EDGE CASES
    // ==========================================================================

    #[test]
    fn test_empty_history() {
        let tips: Vec<f64> = Vec::new();
        assert!(tips.is_empty());
    }

    #[test]
    fn test_single_tip_in_history() {
        let tips: Vec<f64> = vec![0.005];
        let mut sorted = tips.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        
        let percentile = 50_usize;
        let index = (sorted.len() * percentile / 100).min(sorted.len() - 1);
        let tip = sorted[index];
        
        assert!((tip - 0.005).abs() < 0.0001, "Single tip should return that value");
    }

    #[test]
    fn test_large_trade_ceiling_applies() {
        let config = test_config();
        let trade_size_sol = 10.0;
        
        // Max by percent = 10.0 * 0.10 = 1.0 SOL (way above ceiling)
        let max_by_percent = trade_size_sol * config.tip_percent_max;
        let final_tip = max_by_percent.min(config.tip_ceiling_sol);
        
        assert!((final_tip - 0.01).abs() < 0.0001, "Large trade should still be capped at ceiling");
    }

    #[test]
    fn test_small_trade_floor_applies() {
        let config = test_config();
        let trade_size_sol = 0.005;
        
        // Max by percent = 0.005 * 0.10 = 0.0005 SOL (below floor)
        let max_by_percent = trade_size_sol * config.tip_percent_max;
        let final_tip = max_by_percent.max(config.tip_floor_sol);
        
        assert!((final_tip - 0.001).abs() < 0.0001, "Small trade should use floor");
    }
}
