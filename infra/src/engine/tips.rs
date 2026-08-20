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

use chimera_core::config::JitoConfig;
use crate::db_abstraction::Database;
use chimera_core::error::AppResult;
use chimera_core::models::Strategy;
use parking_lot::RwLock;
use rust_decimal::prelude::*;
use std::sync::Arc;

/// Minimum samples required for percentile calculation
const MIN_SAMPLES_FOR_PERCENTILE: u32 = 10;

/// Cold start multiplier (tip_floor * this value)
/// Note: Using a function instead of const because Decimal::from_str is not const
fn cold_start_multiplier() -> Decimal {
    Decimal::from_str("2.0").unwrap_or(Decimal::from(2))
}

/// Compute the percentile-based tip from a tip sample set.
///
/// Pure helper (no DB / TipManager dependency) so the percentile selection —
/// the mechanism that makes seeded paper-mode tips escape cold-start — is
/// directly unit-testable.
fn percentile_tip_from_history(tips: &mut [Decimal], strategy: Strategy, config: &JitoConfig) -> Decimal {
    // Sort ascending for percentile indexing
    tips.sort();

    // Calculate percentile index
    let percentile = match strategy {
        Strategy::Shield => 25, // Conservative: 25th percentile
        Strategy::Spear => config.tip_percentile as usize, // Configured percentile (default 50)
        Strategy::Exit => 75, // Higher: 75th percentile for exits
    };

    // Guard: an empty sample set would underflow `len() - 1` (panic in debug,
    // wrap in release). Fall back to the configured floor.
    if tips.is_empty() {
        return config.tip_floor_sol;
    }

    let index = (tips.len() * percentile / 100).min(tips.len() - 1);
    let percentile_tip = tips[index];

    // For Spear/Exit, use max of percentile and config floor
    match strategy {
        Strategy::Shield => percentile_tip.max(config.tip_floor_sol),
        Strategy::Spear => percentile_tip.max(config.tip_floor_sol),
        Strategy::Exit => {
            let mid = (config.tip_floor_sol + config.tip_ceiling_sol) / Decimal::from(2);
            percentile_tip.max(mid)
        }
    }
}

/// Tip entry for in-memory history
#[derive(Debug, Clone)]
struct TipEntry {
    amount_sol: Decimal,
    #[allow(dead_code)] // Reserved for future strategy-specific tip tracking
    strategy: Strategy,
}

/// Jito Tip Manager
pub struct TipManager {
    /// Jito configuration
    config: JitoConfig,
    /// Database
    db: Arc<dyn Database>,
    /// In-memory tip history (rolling window)
    history: Arc<RwLock<Vec<TipEntry>>>,
    /// Whether we're in cold start mode
    cold_start: Arc<RwLock<bool>>,
    /// Maximum history size
    max_history_size: usize,
}

impl TipManager {
    /// Create a new TipManager
    pub fn new(config: JitoConfig, db: Arc<dyn Database>) -> Self {
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
        let tips = self
            .db
            .get_recent_jito_tips(self.max_history_size as i32)
            .await?;

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
        let count = self.db.get_jito_tip_count().await?;
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

    /// Seed the tip history with realistic mainnet values for PAPER mode.
    ///
    /// Paper trades never record tips (by design — recording simulated tips
    /// would pollute the live percentile data), which means the history stays
    /// empty forever and the TipManager is permanently stuck in cold-start.
    /// In cold-start, the SELL/Exit tip is the config ceiling (e.g. 0.003 SOL),
    /// so every paper exit is charged the maximum modeled cost — a structural
    /// cost drag that makes paper trading unprofitable even when live trading
    /// would clear percentile-based tips.
    ///
    /// Seeding with realistic mainnet tips lets percentile-based calculation
    /// kick in, so paper exits reflect realistic live costs. The seed only
    /// runs when the history is empty; once >= MIN_SAMPLES_FOR_PERCENTILE
    /// rows exist, this is a no-op. Live mode never seeds (it records real
    /// tips), so the percentile data is never polluted.
    pub async fn seed_paper_history_if_empty(&self) -> AppResult<()> {
        let count = self.db.get_jito_tip_count().await?;
        // Only seed a COMPLETELY empty history. Seeding when 1-9 real tips
        // already exist would permanently persist fake seeds into
        // `jito_tip_history` with no distinguishing filter, and later live
        // runs would blend them into percentile calculations — polluting
        // live data exactly as the docs say it won't.
        if count > 0 {
            return Ok(());
        }

        // Realistic mainnet Jito bundle tips for small (< 0.15 SOL) positions.
        // These reflect typical landing tips for low-to-moderate congestion —
        // the exact distribution matters less than escaping cold-start, since
        // percentile selection (Shield: 25th, Exit: 75th) just needs >= 10
        // samples to produce a realistic value well below the ceiling.
        let seed_tips: &[&str] = &[
            "0.0005", "0.0006", "0.0007", "0.0008", "0.0009",
            "0.0010", "0.0011", "0.0012", "0.0014", "0.0016",
            "0.0018", "0.0020",
        ];
        for tip_str in seed_tips {
            let tip = Decimal::from_str(tip_str).map_err(|_| {
                chimera_core::error::AppError::Validation(format!("invalid seed tip: {tip_str}"))
            })?;
            self.db
                .insert_jito_tip(&tip, Some("paper-seed"), Some("SHIELD"), true)
                .await?;
        }

        // Reload history now that rows exist, and exit cold-start.
        let tips = self
            .db
            .get_recent_jito_tips(self.max_history_size as i32)
            .await?;
        {
            let mut history = self.history.write();
            history.clear();
            for tip in tips {
                history.push(TipEntry {
                    amount_sol: tip,
                    strategy: Strategy::Shield,
                });
            }
        }
        *self.cold_start.write() = false;
        tracing::info!(
            seeded = seed_tips.len(),
            "TipManager: seeded paper-mode tip history with realistic values (exiting cold-start)"
        );
        Ok(())
    }

    /// Calculate optimal tip for a given strategy and trade size
    /// Uses Decimal for precision in financial calculations
    pub fn calculate_tip(
        &self,
        strategy: Strategy,
        trade_size_sol: rust_decimal::Decimal,
    ) -> rust_decimal::Decimal {
        let is_cold_start = *self.cold_start.read();

        let base_tip = if is_cold_start {
            self.cold_start_tip(strategy)
        } else {
            self.percentile_tip(strategy)
        };

        // Apply percentage cap using Decimal
        let max_by_percent = trade_size_sol * self.config.tip_percent_max;

        // Apply ceiling using Decimal
        let tip = base_tip
            .min(max_by_percent)
            .min(self.config.tip_ceiling_sol);

        // Ensure minimum
        tip.max(self.config.tip_floor_sol)
    }

    /// Cold start tip calculation
    fn cold_start_tip(&self, strategy: Strategy) -> Decimal {
        match strategy {
            Strategy::Shield => self.config.tip_floor_sol * cold_start_multiplier(),
            Strategy::Spear => {
                self.config.tip_floor_sol
                    * cold_start_multiplier()
                    * Decimal::from_str("1.5").unwrap_or(Decimal::from(3) / Decimal::from(2))
            }
            Strategy::Exit => self.config.tip_ceiling_sol, // Max tip for exits
        }
    }

    /// Percentile-based tip calculation
    fn percentile_tip(&self, strategy: Strategy) -> Decimal {
        let history = self.history.read();

        if history.is_empty() {
            return self.cold_start_tip(strategy);
        }

        let mut tips: Vec<Decimal> = history.iter().map(|e| e.amount_sol).collect();
        percentile_tip_from_history(&mut tips, strategy, &self.config)
    }

    /// Get success rate for a given tip amount range
    /// Returns success rate (0.0-1.0) for tips within ±10% of the given amount
    pub async fn get_tip_success_rate(&self, tip_amount_sol: Decimal) -> AppResult<f64> {
        let total = self.db.get_jito_tip_count().await?;
        if total == 0 {
            // No evidence of landing success — do NOT assume 1.0 (which would
            // wrongly suppress fallback/retry behavior).
            return Ok(0.0);
        }
        let min_tip = tip_amount_sol * Decimal::from_str("0.9").unwrap_or(Decimal::ZERO);
        let max_tip = tip_amount_sol * Decimal::from_str("1.1").unwrap_or(Decimal::ZERO);
        // Approximate success rate by checking what fraction of recent successful tips
        // are within ±10% of our proposed tip amount. This is a heuristic since the
        // Database trait does not expose raw SQL queries for the full success/fail ratio.
        let recent_tips = self.db.get_recent_jito_tips(500).await?;
        let in_range = recent_tips
            .iter()
            .filter(|t| **t >= min_tip && **t <= max_tip)
            .count();
        if recent_tips.is_empty() {
            return Ok(0.0); // No evidence of landing success — don't assume 1.0
        }
        Ok(in_range as f64 / recent_tips.len() as f64)
    }

    /// Check if tip success rate is acceptable (>= 90%)
    pub async fn is_tip_success_rate_acceptable(&self, tip_amount_sol: Decimal) -> AppResult<bool> {
        let rate = self.get_tip_success_rate(tip_amount_sol).await?;
        Ok(rate >= 0.9)
    }

    /// Record a tip (after successful bundle)
    pub async fn record_tip(
        &self,
        tip_amount_sol: Decimal,
        bundle_signature: Option<&str>,
        strategy: Strategy,
        success: bool,
    ) -> AppResult<()> {
        // Persist to database
        let strategy_str = strategy.to_string();
        self.db
            .insert_jito_tip(
                &tip_amount_sol,
                bundle_signature,
                Some(&strategy_str),
                success,
            )
            .await?;

        if success {
            // Update in-memory history (store as Decimal)
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
                    tracing::info!("Exiting cold start mode after {} successful tips", count);
                }
            }
        }

        Ok(())
    }

    /// Get current tip statistics
    pub fn stats(&self) -> TipStats {
        let history = self.history.read();

        if history.is_empty() {
            // Report the REAL cold-start flag instead of the default
            // (is_cold_start: false) — cold start is exactly when the history
            // tends to be empty, and misreporting it misleads operators.
            return TipStats {
                is_cold_start: *self.cold_start.read(),
                ..TipStats::default()
            };
        }

        let tips: Vec<Decimal> = history.iter().map(|e| e.amount_sol).collect();
        let sum: Decimal = tips.iter().sum();
        let count = Decimal::from(tips.len());
        let avg = sum / count;
        let min = tips.iter().cloned().min().unwrap_or(Decimal::ZERO);
        let max = tips.iter().cloned().max().unwrap_or(Decimal::ZERO);

        TipStats {
            count: tips.len(),
            avg_tip_sol: avg,
            min_tip_sol: min,
            max_tip_sol: max,
            is_cold_start: *self.cold_start.read(),
        }
    }

    /// Calculate tip with dynamic scaling based on recent failure rates
    /// If failure rate > 30%, scale tip up to compete for block inclusion
    pub async fn calculate_dynamic_tip_with_load(
        &self,
        strategy: Strategy,
        trade_size_sol: Decimal,
        failure_rate: f64,
    ) -> Decimal {
        let base_tip = self.calculate_tip(strategy, trade_size_sol);

        // Scale tip if block landing rate is poor (>30% failure rate)
        if failure_rate > 0.3 {
            let multiplier = Decimal::from_f64_retain(1.0 + (failure_rate * 0.5))
                .unwrap_or(Decimal::ONE);
            (base_tip * multiplier).min(self.config.tip_ceiling_sol)
        } else {
            base_tip
        }
    }

    /// Get recent bundle failure rate for tip calculation
    pub async fn get_recent_failure_rate(&self) -> AppResult<f64> {
        let total = self.db.get_jito_tip_count().await?;
        if total == 0 {
            return Ok(0.0); // No history = assume 0% failure
        }

        // Both counts come from the same source (successful tips only), so this
        // is really 'fraction of history inside the recent window'. Using the
        // all-time `total` on one side and the 100-row window on the other
        // would converge the estimate to 1.0 as history grows; clamp to the
        // window on both sides instead.
        let recent_tips = self.db.get_recent_jito_tips(100).await?;
        let recent_count = recent_tips.len() as f64;
        let window_total = total.min(100) as f64;
        let failure_rate = 1.0 - (recent_count / window_total.max(1.0));
        Ok(failure_rate.clamp(0.0, 1.0))
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
    /// Average tip amount (using Decimal for precision)
    pub avg_tip_sol: Decimal,
    /// Minimum tip (using Decimal for precision)
    pub min_tip_sol: Decimal,
    /// Maximum tip (using Decimal for precision)
    pub max_tip_sol: Decimal,
    /// Whether in cold start mode
    pub is_cold_start: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::kelly_sizer::tests::MockDatabase;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn test_config() -> JitoConfig {
        JitoConfig {
            enabled: true,
            tip_floor_sol: Decimal::from_str("0.001").unwrap(),
            tip_ceiling_sol: Decimal::from_str("0.01").unwrap(),
            tip_percentile: 50,
            tip_percent_max: Decimal::from_str("0.10").unwrap(),
            helius_fallback: false,
            searcher_endpoint: None,
            min_failures_before_fallback: 10,
            disable_fallback: false,
            max_retries: 5,
            helius_staked_exits: true,
        }
    }

    // ==========================================================================
    // COLD START TESTS
    // ==========================================================================

    #[test]
    fn test_cold_start_multiplier_value() {
        let expected = Decimal::from_str("2.0").unwrap();
        assert_eq!(
            cold_start_multiplier(),
            expected,
            "Cold start multiplier should be 2.0"
        );
    }

    #[test]
    fn test_cold_start_tip_calculation() {
        let config = test_config();
        let cold_tip = config.tip_floor_sol * cold_start_multiplier();
        let expected = Decimal::from_str("0.002").unwrap();
        assert!(
            (cold_tip - expected).abs() < Decimal::from_str("0.0001").unwrap(),
            "Cold start tip should be 0.002 SOL"
        );
    }

    #[test]
    fn test_cold_start_shield_tip() {
        let config = test_config();
        // Shield uses floor * 2
        let tip = config.tip_floor_sol * cold_start_multiplier();
        let expected = Decimal::from_str("0.002").unwrap();
        assert!(
            (tip - expected).abs() < Decimal::from_str("0.0001").unwrap(),
            "Shield cold start tip should be 0.002"
        );
    }

    #[test]
    fn test_cold_start_spear_tip() {
        let config = test_config();
        // Spear uses floor * 2 * 1.5
        let tip =
            config.tip_floor_sol * cold_start_multiplier() * Decimal::from_str("1.5").unwrap();
        let expected = Decimal::from_str("0.003").unwrap();
        assert!(
            (tip - expected).abs() < Decimal::from_str("0.0001").unwrap(),
            "Spear cold start tip should be 0.003"
        );
    }

    #[test]
    fn test_cold_start_exit_tip() {
        let config = test_config();
        // Exit uses ceiling during cold start
        let tip = config.tip_ceiling_sol;
        assert!(
            (tip - Decimal::from_str("0.01").unwrap()).abs() < Decimal::from_str("0.0001").unwrap(),
            "Exit cold start tip should be ceiling"
        );
    }

    // ==========================================================================
    // MINIMUM SAMPLES TESTS
    // ==========================================================================

    #[test]
    fn test_min_samples_constant() {
        assert_eq!(
            MIN_SAMPLES_FOR_PERCENTILE, 10,
            "Minimum samples should be 10"
        );
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
    fn test_paper_seed_escapes_cold_start_and_lowers_exit_tip() {
        // Simulate the seeded paper-mode tip history (values from seed_paper_history_if_empty)
        let config = test_config();
        let mut seeded: Vec<Decimal> = [
            "0.0005", "0.0006", "0.0007", "0.0008", "0.0009",
            "0.0010", "0.0011", "0.0012", "0.0014", "0.0016",
            "0.0018", "0.0020",
        ]
        .iter()
        .map(|s| Decimal::from_str(s).unwrap())
        .collect();

        // Exit cold-start tip would be the ceiling (0.01 in test config).
        // With seeded history, the Exit tip uses max(75th percentile, mid) where
        // mid = (floor+ceiling)/2 — must be strictly below the cold-start ceiling.
        let seeded_exit_tip = percentile_tip_from_history(&mut seeded, Strategy::Exit, &config);
        assert!(
            seeded_exit_tip < Decimal::from_str("0.01").unwrap(),
            "Seeded exit tip should be below the cold-start ceiling, got {seeded_exit_tip}"
        );
        // The 75th percentile of the seed set ≈ 0.0016; the Exit mid-floor
        // (floor+ceiling)/2 = 0.0055 in the test config raises it, but it must
        // still be strictly less than the cold-start ceiling.
        assert!(
            seeded_exit_tip < config.tip_ceiling_sol,
            "Seeded Exit tip must beat cold-start ceiling"
        );

        // Shield BUY tip uses 25th percentile ≈ 0.0007; must be below floor*2 (0.002)
        let seeded_buy_tip = percentile_tip_from_history(&mut seeded, Strategy::Shield, &config);
        assert!(
            seeded_buy_tip < Decimal::from_str("0.002").unwrap(),
            "Seeded Shield tip should be below cold-start floor*2, got {seeded_buy_tip}"
        );
    }

    #[test]
    fn test_percentile_50th() {
        let mut tips: Vec<f64> = vec![
            0.001, 0.002, 0.003, 0.004, 0.005, 0.006, 0.007, 0.008, 0.009, 0.010,
        ];
        tips.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let percentile = 50_usize;
        let index = (tips.len() * percentile / 100).min(tips.len() - 1);
        let tip = tips[index];

        assert!(
            (tip - 0.006).abs() < 0.0001,
            "50th percentile should be 0.006"
        );
    }

    #[test]
    fn test_percentile_25th() {
        let mut tips: Vec<f64> = vec![
            0.001, 0.002, 0.003, 0.004, 0.005, 0.006, 0.007, 0.008, 0.009, 0.010,
        ];
        tips.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let percentile = 25_usize;
        let index = (tips.len() * percentile / 100).min(tips.len() - 1);
        let tip = tips[index];

        // 25th percentile for Shield (conservative)
        assert!(
            (tip - 0.003).abs() < 0.0001,
            "25th percentile should be around 0.003"
        );
    }

    #[test]
    fn test_percentile_75th() {
        let mut tips: Vec<f64> = vec![
            0.001, 0.002, 0.003, 0.004, 0.005, 0.006, 0.007, 0.008, 0.009, 0.010,
        ];
        tips.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let percentile = 75_usize;
        let index = (tips.len() * percentile / 100).min(tips.len() - 1);
        let tip = tips[index];

        // 75th percentile for Exit (high priority)
        assert!(
            (tip - 0.008).abs() < 0.0001,
            "75th percentile should be around 0.008"
        );
    }

    // ==========================================================================
    // TIP CAP TESTS
    // ==========================================================================

    #[test]
    fn test_tip_ceiling_cap() {
        let config = test_config();
        let percentile_tip = Decimal::from_str("0.015").unwrap(); // Above ceiling
        let capped_tip = percentile_tip.min(config.tip_ceiling_sol);
        assert!(
            (capped_tip - Decimal::from_str("0.01").unwrap()).abs()
                < Decimal::from_str("0.0001").unwrap(),
            "Tip should be capped at ceiling"
        );
    }

    #[test]
    fn test_tip_floor_minimum() {
        let config = test_config();
        let percentile_tip = Decimal::from_str("0.0005").unwrap(); // Below floor
        let floored_tip = percentile_tip.max(config.tip_floor_sol);
        assert!(
            (floored_tip - Decimal::from_str("0.001").unwrap()).abs()
                < Decimal::from_str("0.0001").unwrap(),
            "Tip should be floored at minimum"
        );
    }

    #[test]
    fn test_tip_percent_max() {
        let config = test_config();
        let trade_size_sol = Decimal::from_str("0.05").unwrap(); // 0.05 SOL trade
        let max_by_percent = trade_size_sol * config.tip_percent_max;

        // Max tip = 0.05 * 0.10 = 0.005 SOL
        assert!(
            (max_by_percent - Decimal::from_str("0.005").unwrap()).abs()
                < Decimal::from_str("0.0001").unwrap(),
            "Max by percent should be 0.005"
        );
    }

    #[test]
    fn test_tip_all_caps_applied() {
        let config = test_config();
        let trade_size_sol = Decimal::from_str("0.1").unwrap();
        let base_tip = Decimal::from_str("0.015").unwrap(); // High percentile result

        // Apply percentage cap
        let max_by_percent = trade_size_sol * config.tip_percent_max; // 0.01

        // Apply ceiling
        let tip = base_tip.min(max_by_percent).min(config.tip_ceiling_sol);

        // Ensure minimum
        let final_tip = tip.max(config.tip_floor_sol);

        assert!(
            (final_tip - Decimal::from_str("0.01").unwrap()).abs()
                < Decimal::from_str("0.0001").unwrap(),
            "Final tip should be 0.01 (ceiling applies)"
        );
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
        assert_eq!(stats.avg_tip_sol, Decimal::ZERO);
    }

    #[test]
    fn test_tip_stats_calculation() {
        let tips: Vec<Decimal> = vec![
            Decimal::from_str("0.001").unwrap(),
            Decimal::from_str("0.002").unwrap(),
            Decimal::from_str("0.003").unwrap(),
            Decimal::from_str("0.004").unwrap(),
            Decimal::from_str("0.005").unwrap(),
        ];

        let sum: Decimal = tips.iter().sum();
        let count = Decimal::from(tips.len());
        let avg = sum / count;
        let min = tips.iter().cloned().min().unwrap_or(Decimal::ZERO);
        let max = tips.iter().cloned().max().unwrap_or(Decimal::ZERO);

        assert_eq!(
            avg,
            Decimal::from_str("0.003").unwrap(),
            "Average should be 0.003"
        );
        assert_eq!(
            min,
            Decimal::from_str("0.001").unwrap(),
            "Min should be 0.001"
        );
        assert_eq!(
            max,
            Decimal::from_str("0.005").unwrap(),
            "Max should be 0.005"
        );
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

        assert_eq!(
            history.len(),
            max_history_size,
            "History should be capped at 100"
        );
        // First entry should be the 6th one added (0.006)
        assert!(
            (history[0] - 0.006).abs() < 0.0001,
            "Oldest entries should be trimmed"
        );
    }

    // ==========================================================================
    // STRATEGY TIP ORDERING TESTS
    // ==========================================================================

    #[test]
    fn test_strategy_tip_ordering() {
        let config = test_config();

        let shield_tip = config.tip_floor_sol * cold_start_multiplier();
        let spear_tip =
            config.tip_floor_sol * cold_start_multiplier() * Decimal::from_str("1.5").unwrap();
        let exit_tip = config.tip_ceiling_sol;

        assert!(
            shield_tip < spear_tip,
            "Shield tip should be less than Spear"
        );
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

        assert!(
            (tip - 0.005).abs() < 0.0001,
            "Single tip should return that value"
        );
    }

    #[test]
    fn test_large_trade_ceiling_applies() {
        let config = test_config();
        let trade_size_sol = Decimal::from_str("10.0").unwrap();

        // Max by percent = 10.0 * 0.10 = 1.0 SOL (way above ceiling)
        let max_by_percent = trade_size_sol * config.tip_percent_max;
        let final_tip = max_by_percent.min(config.tip_ceiling_sol);

        assert!(
            (final_tip - Decimal::from_str("0.01").unwrap()).abs()
                < Decimal::from_str("0.0001").unwrap(),
            "Large trade should still be capped at ceiling"
        );
    }

    #[test]
    fn test_small_trade_floor_applies() {
        let config = test_config();
        let trade_size_sol = Decimal::from_str("0.005").unwrap();

        // Max by percent = 0.005 * 0.10 = 0.0005 SOL (below floor)
        let max_by_percent = trade_size_sol * config.tip_percent_max;
        let final_tip = max_by_percent.max(config.tip_floor_sol);

        assert!(
            (final_tip - Decimal::from_str("0.001").unwrap()).abs()
                < Decimal::from_str("0.0001").unwrap(),
            "Small trade should use floor"
        );
    }

    // ==========================================================================
    // DATABASE-BACKED TipManager TESTS
    // ==========================================================================

    fn manager_with(tips: Vec<Decimal>, count: u32) -> (TipManager, Arc<MockDatabase>) {
        let mock = Arc::new(MockDatabase {
            recent_jito_tips: parking_lot::RwLock::new(tips),
            jito_tip_count: parking_lot::RwLock::new(count),
            ..Default::default()
        });
        let manager = TipManager::new(test_config(), mock.clone());
        (manager, mock)
    }

    #[tokio::test]
    async fn test_init_exits_cold_start_with_enough_history() {
        let tips: Vec<Decimal> = (0..12).map(|i| Decimal::from_str(&format!("0.00{}", i)).unwrap()).collect();
        let (manager, _db) = manager_with(tips, 12);
        manager.init().await.unwrap();

        assert!(!manager.is_cold_start());
        // History loaded from DB.
        assert_eq!(manager.history.read().len(), 12);
        let stats = manager.stats();
        assert_eq!(stats.count, 12);
    }

    #[tokio::test]
    async fn test_init_stays_in_cold_start_with_few_samples() {
        let tips = vec![Decimal::from_str("0.001").unwrap(); 5];
        let (manager, _db) = manager_with(tips, 5);
        manager.init().await.unwrap();

        assert!(manager.is_cold_start());
    }

    #[tokio::test]
    async fn test_seed_paper_history_when_empty() {
        let (manager, db) = manager_with(Vec::new(), 0);
        manager.seed_paper_history_if_empty().await.unwrap();

        assert!(!manager.is_cold_start());
        assert_eq!(manager.history.read().len(), 12);
        // Seed rows were inserted via the mock DB.
        let inserted = db.inserted_tips.read().clone();
        assert_eq!(inserted.len(), 12);
        assert!(inserted.iter().all(|(_, sig, _, success)| {
            sig.as_deref() == Some("paper-seed") && *success
        }));
    }

    #[tokio::test]
    async fn test_seed_paper_history_noop_when_rows_exist() {
        let (manager, db) = manager_with(vec![Decimal::from_str("0.001").unwrap()], 1);
        manager.seed_paper_history_if_empty().await.unwrap();

        // count > 0 -> no seeding
        assert!(manager.is_cold_start());
        assert_eq!(db.inserted_tips.read().len(), 0);
    }

    #[tokio::test]
    async fn test_record_tip_accumulates_and_exits_cold_start() {
        let (manager, db) = manager_with(Vec::new(), 0);
        for i in 0..10 {
            manager
                .record_tip(
                    Decimal::from_str(&format!("0.001{}", i)).unwrap(),
                    Some(&format!("sig-{i}")),
                    chimera_core::models::Strategy::Shield,
                    true,
                )
                .await
                .unwrap();
        }
        assert_eq!(manager.history.read().len(), 10);
        assert!(!manager.is_cold_start());

        // Failed tip is persisted but not added to history.
        manager
            .record_tip(
                Decimal::from_str("0.002").unwrap(),
                None,
                chimera_core::models::Strategy::Spear,
                false,
            )
            .await
            .unwrap();
        assert_eq!(manager.history.read().len(), 10);
        let inserted = db.inserted_tips.read().clone();
        assert_eq!(inserted.len(), 11);
        assert_eq!(inserted[10].2.as_deref(), Some("SPEAR"));
        assert!(!inserted[10].3);
    }

    #[tokio::test]
    async fn test_record_tip_trims_history_at_max() {
        let (manager, _db) = manager_with(Vec::new(), 0);
        for i in 0..105 {
            manager
                .record_tip(
                    Decimal::from_str(&format!("0.001{i}")).unwrap(),
                    None,
                    chimera_core::models::Strategy::Shield,
                    true,
                )
                .await
                .unwrap();
        }
        assert_eq!(manager.history.read().len(), 100);
    }

    #[tokio::test]
    async fn test_calculate_tip_cold_start_strategies() {
        let (manager, _db) = manager_with(Vec::new(), 0);
        // Shield: floor * 2 = 0.002
        let tip = manager.calculate_tip(chimera_core::models::Strategy::Shield, Decimal::from_str("0.05").unwrap());
        assert_eq!(tip, Decimal::from_str("0.002").unwrap());
        // Spear: floor * 2 * 1.5 = 0.003
        let tip = manager.calculate_tip(chimera_core::models::Strategy::Spear, Decimal::from_str("0.05").unwrap());
        assert_eq!(tip, Decimal::from_str("0.003").unwrap());
        // Exit: ceiling = 0.01 (trade size 1.0 -> 10% cap of 0.1 doesn't clamp).
        let tip = manager.calculate_tip(chimera_core::models::Strategy::Exit, Decimal::from_str("1.0").unwrap());
        assert_eq!(tip, Decimal::from_str("0.01").unwrap());
    }

    #[tokio::test]
    async fn test_calculate_tip_percentile_with_history() {
        // 12 tips loaded via init -> percentile path (not cold start).
        let tips: Vec<Decimal> = (1..=12).map(|i| Decimal::from_str(&format!("0.00{:02}", i)).unwrap()).collect();
        let (manager, _db) = manager_with(tips, 12);
        manager.init().await.unwrap();
        assert!(!manager.is_cold_start());

        // Shield percentile (25th of sorted 0.0001..0.0012) = index 3 = 0.0004 ->
        // max(0.0004, floor 0.001) = 0.001; percent cap 0.1*0.1 = 0.01.
        let tip = manager.calculate_tip(chimera_core::models::Strategy::Shield, Decimal::from_str("0.1").unwrap());
        assert_eq!(tip, Decimal::from_str("0.001").unwrap());
    }

    #[tokio::test]
    async fn test_calculate_tip_ceiling_and_floor() {
        let tips: Vec<Decimal> = (1..=12).map(|i| Decimal::from_str(&format!("0.00{:02}", i)).unwrap()).collect();
        let (manager, _db) = manager_with(tips, 12);
        manager.init().await.unwrap();

        // Exit percentile 75th of 0.0001..0.0012 = index 9 = 0.0010 ->
        // max(0.0010, mid 0.0055) = 0.0055; percent cap 1.0*0.1 = 0.1 -> 0.0055.
        let tip = manager.calculate_tip(chimera_core::models::Strategy::Exit, Decimal::from_str("1.0").unwrap());
        assert_eq!(tip, Decimal::from_str("0.0055").unwrap());

        // Tiny trade: percent cap 0.004*0.1 = 0.0004; min(percentile 0.004, 0.0004, ceiling)
        // = 0.0004 -> max(floor 0.001) = 0.001.
        let tip = manager.calculate_tip(chimera_core::models::Strategy::Shield, Decimal::from_str("0.004").unwrap());
        assert_eq!(tip, Decimal::from_str("0.001").unwrap());
    }

    #[tokio::test]
    async fn test_get_tip_success_rate() {
        // total == 0 -> 0.0
        let (manager, _db) = manager_with(Vec::new(), 0);
        assert_eq!(manager.get_tip_success_rate(Decimal::from_str("0.001").unwrap()).await.unwrap(), 0.0);

        // Recent tips: 4 of 5 within ±10% of 0.001 (range 0.0009-0.0011).
        let tips = vec![
            Decimal::from_str("0.00095").unwrap(),
            Decimal::from_str("0.001").unwrap(),
            Decimal::from_str("0.00105").unwrap(),
            Decimal::from_str("0.0011").unwrap(),
            Decimal::from_str("0.002").unwrap(),
        ];
        let (manager, _db) = manager_with(tips, 5);
        let rate = manager.get_tip_success_rate(Decimal::from_str("0.001").unwrap()).await.unwrap();
        assert_eq!(rate, 0.8);

        // Empty recent tips but total > 0 -> 0.0 (no evidence of landing).
        let (manager, _db) = manager_with(Vec::new(), 3);
        assert_eq!(manager.get_tip_success_rate(Decimal::from_str("0.001").unwrap()).await.unwrap(), 0.0);
    }

    #[tokio::test]
    async fn test_is_tip_success_rate_acceptable() {
        let tips = vec![Decimal::from_str("0.001").unwrap(); 10];
        let (manager, _db) = manager_with(tips, 10);
        assert!(manager.is_tip_success_rate_acceptable(Decimal::from_str("0.001").unwrap()).await.unwrap());
        assert!(!manager.is_tip_success_rate_acceptable(Decimal::from_str("0.01").unwrap()).await.unwrap());
    }

    #[tokio::test]
    async fn test_get_recent_failure_rate() {
        // No history -> 0.
        let (manager, _db) = manager_with(Vec::new(), 0);
        assert_eq!(manager.get_recent_failure_rate().await.unwrap(), 0.0);

        // total 100, recent window 100 rows -> failure 0.
        let tips: Vec<Decimal> = vec![Decimal::from_str("0.001").unwrap(); 100];
        let (manager, _db) = manager_with(tips, 100);
        assert_eq!(manager.get_recent_failure_rate().await.unwrap(), 0.0);

        // total 100 but only 25 recent rows -> 75% "failure".
        let tips: Vec<Decimal> = vec![Decimal::from_str("0.001").unwrap(); 25];
        let (manager, _db) = manager_with(tips, 100);
        assert_eq!(manager.get_recent_failure_rate().await.unwrap(), 0.75);
    }

    #[tokio::test]
    async fn test_calculate_dynamic_tip_with_load() {
        let (manager, _db) = manager_with(Vec::new(), 0);
        let base = manager.calculate_tip(chimera_core::models::Strategy::Shield, Decimal::from_str("0.05").unwrap());
        // High failure rate -> scaled up (capped at ceiling).
        let scaled = manager
            .calculate_dynamic_tip_with_load(
                chimera_core::models::Strategy::Shield,
                Decimal::from_str("0.05").unwrap(),
                0.5,
            )
            .await;
        assert_eq!(scaled, (base * Decimal::from_str("1.25").unwrap()).min(manager.config.tip_ceiling_sol));
        // Low failure rate -> unchanged.
        let unchanged = manager
            .calculate_dynamic_tip_with_load(
                chimera_core::models::Strategy::Shield,
                Decimal::from_str("0.05").unwrap(),
                0.1,
            )
            .await;
        assert_eq!(unchanged, base);
    }

    #[tokio::test]
    async fn test_stats_empty_history_reports_cold_start() {
        let (manager, _db) = manager_with(Vec::new(), 0);
        let stats = manager.stats();
        assert_eq!(stats.count, 0);
        assert!(stats.is_cold_start);
    }
}
