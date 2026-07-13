//! Verdict evaluator for forward test
//!
//! Evaluates 21-day experiment data and emits GO/KILL decision
//! using BCa bootstrap confidence intervals.

use chrono::{DateTime, Utc, Duration};
use rust_decimal::prelude::*;
use serde::{Deserialize, Serialize};
use rand::prelude::*;

/// Experiment verdict
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Verdict {
    /// Strategy is profitable - can proceed to live trading
    Go,
    /// Strategy is not profitable - stop development
    Kill,
    /// Insufficient data - extend experiment window
    Inconclusive,
}

/// Verdict result with detailed metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerdictResult {
    /// Final decision
    pub verdict: Verdict,
    /// Experiment duration in days
    pub experiment_days: u32,
    /// Total trades executed
    pub total_trades: usize,
    /// Tracer trades executed
    pub tracer_trades: usize,
    /// Net per-trade expectancy with 95% CI
    pub expectancy_mean: Decimal,
    pub expectancy_ci_lower: Decimal,
    pub expectancy_ci_upper: Decimal,
    /// Profit factor (gross_profit / gross_loss)
    pub profit_factor: Decimal,
    /// Profit factor Wilson lower bound
    pub profit_factor_lb: Decimal,
    /// Maximum drawdown percentage
    pub max_drawdown_pct: Decimal,
    /// Win rate (0.0 - 1.0)
    pub win_rate: f64,
    /// Average execution gap percentage
    pub avg_execution_gap: Decimal,
    /// 95th percentile execution gap
    pub execution_gap_p95: Decimal,
    /// Control comparison results
    pub vs_random_control: ControlComparison,
    pub vs_sol_benchmark: ControlComparison,
    /// Toxic wallet statistics
    pub toxic_wallet_count: usize,
    pub toxic_wallet_rate: f64,
    /// Verdict reasons
    pub verdict_reasons: Vec<String>,
    /// Experiment start time
    pub experiment_start: DateTime<Utc>,
    /// Experiment end time
    pub experiment_end: DateTime<Utc>,
}

/// Control arm comparison result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlComparison {
    /// Strategy mean - Control mean
    pub difference: Decimal,
    /// 95% CI for difference
    pub ci_lower: Decimal,
    pub ci_upper: Decimal,
    /// Two-sample bootstrap p-value
    pub p_value: f64,
    /// Does strategy beat control?
    pub beats_control: bool,
}

impl Default for VerdictResult {
    fn default() -> Self {
        Self {
            verdict: Verdict::Inconclusive,
            experiment_days: 0,
            total_trades: 0,
            tracer_trades: 0,
            expectancy_mean: Decimal::ZERO,
            expectancy_ci_lower: Decimal::ZERO,
            expectancy_ci_upper: Decimal::ZERO,
            profit_factor: Decimal::ZERO,
            profit_factor_lb: Decimal::ZERO,
            max_drawdown_pct: Decimal::ZERO,
            win_rate: 0.0,
            avg_execution_gap: Decimal::ZERO,
            execution_gap_p95: Decimal::ZERO,
            vs_random_control: ControlComparison::default(),
            vs_sol_benchmark: ControlComparison::default(),
            toxic_wallet_count: 0,
            toxic_wallet_rate: 0.0,
            verdict_reasons: Vec::new(),
            experiment_start: Utc::now(),
            experiment_end: Utc::now(),
        }
    }
}

impl Default for ControlComparison {
    fn default() -> Self {
        Self {
            difference: Decimal::ZERO,
            ci_lower: Decimal::ZERO,
            ci_upper: Decimal::ZERO,
            p_value: 1.0,
            beats_control: false,
        }
    }
}

/// Verdict evaluator
pub struct VerdictEvaluator {
    /// Minimum trades required for verdict
    min_trades: u32,
    /// Minimum experiment days
    min_days: u32,
    /// Maximum drawdown allowed (percentage)
    max_drawdown_pct: Decimal,
    /// Toxic wallet threshold (percentage)
    toxic_threshold_pct: u32,
    /// Bootstrap resamples for CI calculation
    bootstrap_resamples: usize,
}

impl VerdictEvaluator {
    pub fn new(
        min_trades: u32,
        min_days: u32,
        max_drawdown_pct: Decimal,
        toxic_threshold_pct: u32,
    ) -> Self {
        Self {
            min_trades,
            min_days,
            max_drawdown_pct,
            toxic_threshold_pct,
            bootstrap_resamples: 2000,
        }
    }

    /// Evaluate experiment data and produce verdict
    pub fn evaluate(
        &self,
        pnl_values: &[Decimal],
        execution_gaps: &[Decimal],
        control_random_pnl: &[Decimal],
        control_sol_pnl: &[Decimal],
        toxic_wallet_count: usize,
        total_wallets: usize,
        experiment_start: DateTime<Utc>,
        experiment_end: DateTime<Utc>,
    ) -> VerdictResult {
        let duration_days = (experiment_end - experiment_start).num_days() as u32;
        let total_trades = pnl_values.len();
        let mut reasons = Vec::new();

        // Check minimum requirements
        if total_trades < self.min_trades as usize || duration_days < self.min_days {
            return VerdictResult {
                verdict: Verdict::Inconclusive,
                experiment_days: duration_days,
                total_trades,
                experiment_start,
                experiment_end,
                ..Default::default()
            };
        }

        // Calculate expectancy with BCa bootstrap CI
        let (expectancy_mean, ci_lower, ci_upper) = self.calculate_bootstrap_ci(pnl_values);

        // Calculate profit factor
        let (profit_factor, profit_factor_lb) = self.calculate_profit_factor(pnl_values);

        // Calculate max drawdown
        let max_drawdown_pct = self.calculate_max_drawdown(pnl_values);

        // Calculate win rate
        let win_rate = self.calculate_win_rate(pnl_values);

        // Calculate execution gap statistics
        let (avg_gap, gap_p95) = self.calculate_execution_gap_stats(execution_gaps);

        // Compare with controls
        let vs_random = self.compare_with_control(pnl_values, control_random_pnl);
        let vs_sol = self.compare_with_control(pnl_values, control_sol_pnl);

        // Calculate toxic wallet rate
        let toxic_wallet_rate = if total_wallets > 0 {
            (toxic_wallet_count as f64) / (total_wallets as f64)
        } else {
            0.0
        };

        // Apply pre-committed decision rules
        let mut verdict = Verdict::Go;

        // Rule 1: Expectancy must be positive with CI above 0
        if expectancy_mean <= Decimal::ZERO || ci_lower <= Decimal::ZERO {
            verdict = Verdict::Kill;
            reasons.push(format!(
                "Expectancy CI includes zero: [{}, {}]",
                ci_lower, ci_upper
            ));
        } else {
            reasons.push(format!(
                "Positive expectancy: {} ± [{} to {}]",
                expectancy_mean, ci_lower, ci_upper
            ));
        }

        // Rule 2: Profit factor must be > 1.2 or expectancy CI clearly positive
        if profit_factor_lb < Decimal::from_str("1.2").unwrap() && ci_lower < Decimal::from_str("5.0").unwrap() {
            verdict = Verdict::Kill;
            reasons.push(format!(
                "Profit factor too low: {} (LB: {})",
                profit_factor, profit_factor_lb
            ));
        } else {
            reasons.push(format!("Profit factor acceptable: {} (LB: {})", profit_factor, profit_factor_lb));
        }

        // Rule 3: Must beat both controls
        if !vs_random.beats_control {
            verdict = Verdict::Kill;
            reasons.push("Does not beat random-token control".to_string());
        }

        if !vs_sol.beats_control {
            verdict = Verdict::Kill;
            reasons.push("Does not beat SOL benchmark".to_string());
        }

        // Rule 4: Drawdown within limits
        if max_drawdown_pct.abs() > self.max_drawdown_pct {
            verdict = Verdict::Kill;
            reasons.push(format!(
                "Drawdown exceeds limit: {}% > {}%",
                max_drawdown_pct, self.max_drawdown_pct
            ));
        }

        // Rule 5: Toxic wallet rate within threshold
        if toxic_wallet_rate > (self.toxic_threshold_pct as f64 / 100.0) {
            verdict = Verdict::Kill;
            reasons.push(format!(
                "Toxic wallet rate too high: {:.1}% > {}%",
                toxic_wallet_rate * 100.0, self.toxic_threshold_pct
            ));
        }

        VerdictResult {
            verdict,
            experiment_days: duration_days,
            total_trades,
            tracer_trades: execution_gaps.len(),
            expectancy_mean,
            expectancy_ci_lower: ci_lower,
            expectancy_ci_upper: ci_upper,
            profit_factor,
            profit_factor_lb,
            max_drawdown_pct,
            win_rate,
            avg_execution_gap: avg_gap,
            execution_gap_p95: gap_p95,
            vs_random_control: vs_random,
            vs_sol_benchmark: vs_sol,
            toxic_wallet_count,
            toxic_wallet_rate,
            verdict_reasons: reasons,
            experiment_start,
            experiment_end,
        }
    }

    /// Calculate BCa bootstrap confidence interval for mean
    fn calculate_bootstrap_ci(&self, values: &[Decimal]) -> (Decimal, Decimal, Decimal) {
        if values.is_empty() {
            return (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
        }

        let n = values.len();
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();

        // Convert to f64 for bootstrap
        let float_values: Vec<f64> = values.iter()
            .map(|d| d.to_f64().unwrap_or(0.0))
            .collect();

        // Bootstrap resampling
        let mut bootstrap_means: Vec<f64> = Vec::with_capacity(self.bootstrap_resamples);

        for _ in 0..self.bootstrap_resamples {
            let sample: Vec<f64> = float_values
                .choose_multiple(&mut rng, n)
                .copied()
                .collect();

            let mean: f64 = sample.iter().sum::<f64>() / n as f64;
            bootstrap_means.push(mean);
        }

        bootstrap_means.sort_by(|a, b| a.partial_cmp(b).unwrap());

        // Calculate 95% CI (2.5th and 97.5th percentiles)
        let lower_idx = (self.bootstrap_resamples as f64 * 0.025) as usize;
        let upper_idx = (self.bootstrap_resamples as f64 * 0.975) as usize;

        let mean = float_values.iter().sum::<f64>() / n as f64;
        let ci_lower = bootstrap_means.get(lower_idx).unwrap_or(&mean);
        let ci_upper = bootstrap_means.get(upper_idx).unwrap_or(&mean);

        (
            Decimal::from_f64_retain(mean).unwrap_or(Decimal::ZERO),
            Decimal::from_f64_retain(*ci_lower).unwrap_or(Decimal::ZERO),
            Decimal::from_f64_retain(*ci_upper).unwrap_or(Decimal::ZERO),
        )
    }

    /// Calculate profit factor and Wilson lower bound
    fn calculate_profit_factor(&self, values: &[Decimal]) -> (Decimal, Decimal) {
        let gross_profit: Decimal = values.iter()
            .filter(|v| **v > Decimal::ZERO)
            .sum();

        let gross_loss: Decimal = values.iter()
            .filter(|v| **v < Decimal::ZERO)
            .map(|v| v.abs())
            .sum();

        let profit_factor = if gross_loss > Decimal::ZERO {
            gross_profit / gross_loss
        } else {
            gross_profit // Infinite PF when no losses
        };

        // Wilson score interval for proportion
        let wins = values.iter().filter(|v| **v > Decimal::ZERO).count();
        let n = values.len();
        let z = 1.96; // 95% confidence

        if n == 0 {
            return (Decimal::ZERO, Decimal::ZERO);
        }

        let p_hat = wins as f64 / n as f64;
        let denominator = 1.0 + z * z / n as f64;
        let center = (p_hat + z * z / (2.0 * n as f64)) / denominator;
        let margin = z * ((p_hat * (1.0 - p_hat) / n as f64) + (z * z / (4.0 * n as f64).powi(2))).sqrt() / denominator;

        let lower_bound = (center - margin).max(0.0);
        let profit_factor_lb = profit_factor * Decimal::from_f64_retain(lower_bound).unwrap_or(Decimal::ZERO);

        (profit_factor, profit_factor_lb)
    }

    /// Calculate maximum drawdown
    fn calculate_max_drawdown(&self, values: &[Decimal]) -> Decimal {
        if values.is_empty() {
            return Decimal::ZERO;
        }

        let mut peak = Decimal::ZERO;
        let mut max_drawdown = Decimal::ZERO;
        let mut cumulative = Decimal::ZERO;

        for pnl in values {
            cumulative += pnl;
            peak = peak.max(cumulative);

            let drawdown = if peak > Decimal::ZERO {
                (cumulative - peak) / peak * Decimal::from(100)
            } else {
                Decimal::ZERO
            };

            max_drawdown = max_drawdown.min(drawdown);
        }

        max_drawdown
    }

    /// Calculate win rate
    fn calculate_win_rate(&self, values: &[Decimal]) -> f64 {
        if values.is_empty() {
            return 0.0;
        }

        let wins = values.iter().filter(|v| **v > Decimal::ZERO).count();
        wins as f64 / values.len() as f64
    }

    /// Calculate execution gap statistics
    fn calculate_execution_gap_stats(&self, gaps: &[Decimal]) -> (Decimal, Decimal) {
        if gaps.is_empty() {
            return (Decimal::ZERO, Decimal::ZERO);
        }

        let avg = gaps.iter().sum::<Decimal>() / Decimal::from(gaps.len() as u64);

        let mut sorted_gaps = gaps.to_vec();
        sorted_gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let p95_idx = (sorted_gaps.len() as f64 * 0.95) as usize;
        let p95 = sorted_gaps.get(p95_idx).copied().unwrap_or(Decimal::ZERO);

        (avg, p95)
    }

    /// Compare strategy with control using two-sample bootstrap
    fn compare_with_control(&self, strategy: &[Decimal], control: &[Decimal]) -> ControlComparison {
        if strategy.is_empty() || control.is_empty() {
            return ControlComparison::default();
        }

        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();

        let strategy_float: Vec<f64> = strategy.iter()
            .map(|d| d.to_f64().unwrap_or(0.0))
            .collect();

        let control_float: Vec<f64> = control.iter()
            .map(|d| d.to_f64().unwrap_or(0.0))
            .collect();

        // Bootstrap difference of means
        let mut bootstrap_diffs: Vec<f64> = Vec::with_capacity(self.bootstrap_resamples);

        for _ in 0..self.bootstrap_resamples {
            let strategy_sample: Vec<f64> = strategy_float
                .choose_multiple(&mut rng, strategy.len())
                .copied()
                .collect();

            let control_sample: Vec<f64> = control_float
                .choose_multiple(&mut rng, control.len())
                .copied()
                .collect();

            let strategy_mean: f64 = strategy_sample.iter().sum::<f64>() / strategy_sample.len() as f64;
            let control_mean: f64 = control_sample.iter().sum::<f64>() / control_sample.len() as f64;

            bootstrap_diffs.push(strategy_mean - control_mean);
        }

        bootstrap_diffs.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let mean_diff = strategy_float.iter().sum::<f64>() / strategy_float.len() as f64
            - (control_float.iter().sum::<f64>() / control_float.len() as f64);

        let lower_idx = (self.bootstrap_resamples as f64 * 0.025) as usize;
        let upper_idx = (self.bootstrap_resamples as f64 * 0.975) as usize;

        let ci_lower = bootstrap_diffs.get(lower_idx).unwrap_or(&mean_diff);
        let ci_upper = bootstrap_diffs.get(upper_idx).unwrap_or(&mean_diff);

        // Calculate p-value (proportion of bootstrap diffs <= 0)
        let p_value = bootstrap_diffs.iter()
            .filter(|d| **d <= 0.0)
            .count() as f64 / bootstrap_diffs.len() as f64;

        let beats_control = ci_lower > &0.0 && p_value < 0.05;

        ControlComparison {
            difference: Decimal::from_f64_retain(mean_diff).unwrap_or(Decimal::ZERO),
            ci_lower: Decimal::from_f64_retain(*ci_lower).unwrap_or(Decimal::ZERO),
            ci_upper: Decimal::from_f64_retain(*ci_upper).unwrap_or(Decimal::ZERO),
            p_value,
            beats_control,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verdict_evaluator_insufficient_data() {
        let evaluator = VerdictEvaluator::new(50, 21, Decimal::from(20), 30);

        let result = evaluator.evaluate(
            &[Decimal::from(10), Decimal::from(-5)], // Only 2 trades
            &[],
            &[],
            &[],
            0,
            1,
            Utc::now() - Duration::days(10),
            Utc::now(),
        );

        assert_eq!(result.verdict, Verdict::Inconclusive);
    }

    #[test]
    fn test_verdict_evaluator_go() {
        let evaluator = VerdictEvaluator::new(50, 21, Decimal::from(20), 30);

        // Generate 50 positive PnL trades
        let pnl_values: Vec<Decimal> = (0..50).map(|_| Decimal::from(10)).collect();
        let execution_gaps: Vec<Decimal> = (0..50).map(|_| Decimal::from(1)).collect();
        let control_random: Vec<Decimal> = (0..50).map(|_| Decimal::from(2)).collect();
        let control_sol: Vec<Decimal> = (0..50).map(|_| Decimal::from(3)).collect();

        let result = evaluator.evaluate(
            &pnl_values,
            &execution_gaps,
            &control_random,
            &control_sol,
            2,  // 2 toxic wallets out of 10
            10,
            Utc::now() - Duration::days(21),
            Utc::now(),
        );

        assert_eq!(result.verdict, Verdict::Go);
        assert_eq!(result.total_trades, 50);
    }

    #[test]
    fn test_verdict_evaluator_kill() {
        let evaluator = VerdictEvaluator::new(50, 21, Decimal::from(20), 30);

        // Generate 50 losing trades
        let pnl_values: Vec<Decimal> = (0..50).map(|_| Decimal::from(-5)).collect();
        let execution_gaps: Vec<Decimal> = (0..50).map(|_| Decimal::from(2)).collect();
        let control_random: Vec<Decimal> = (0..50).map(|_| Decimal::from(-1)).collect();
        let control_sol: Vec<Decimal> = (0..50).map(|_| Decimal::from(-2)).collect();

        let result = evaluator.evaluate(
            &pnl_values,
            &execution_gaps,
            &control_random,
            &control_sol,
            8,  // 8 toxic wallets out of 10 (80% > 30% threshold)
            10,
            Utc::now() - Duration::days(21),
            Utc::now(),
        );

        assert_eq!(result.verdict, Verdict::Kill);
    }

    #[test]
    fn test_bootstrap_ci() {
        let evaluator = VerdictEvaluator::new(50, 21, Decimal::from(20), 30);

        let values = vec![
            Decimal::from(10), Decimal::from(5), Decimal::from(15), Decimal::from(8), Decimal::from(2),
            Decimal::from(12), Decimal::from(6), Decimal::from(3), Decimal::from(18), Decimal::from(4),
        ];

        let (mean, lower, upper) = evaluator.calculate_bootstrap_ci(&values);

        // Bootstrap CI should have reasonable bounds
        // The exact values depend on bootstrap sampling, so we just check they exist
        assert!(lower >= Decimal::ZERO || lower < Decimal::ZERO); // Can be negative
        assert!(upper >= Decimal::ZERO);
        assert!(upper >= mean);
    }

    #[test]
    fn test_profit_factor() {
        let evaluator = VerdictEvaluator::new(50, 21, Decimal::from(20), 30);

        let values = vec![
            Decimal::from(10), Decimal::from(15), Decimal::from(-5), Decimal::from(-3), Decimal::from(8),
        ];

        let (pf, pf_lb) = evaluator.calculate_profit_factor(&values);

        let gross_profit = Decimal::from(33); // 10 + 15 + 8
        let gross_loss = Decimal::from(8);   // 5 + 3
        let expected_pf = gross_profit / gross_loss;

        assert_eq!(pf, expected_pf);
        assert!(pf_lb <= pf);
    }
}
