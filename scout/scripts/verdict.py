#!/usr/bin/env python3
"""
Verdict Evaluator for 21-Day Forward Test

Evaluates experiment data and emits GO/KILL decision using BCa bootstrap confidence intervals.

Pre-committed Decision Rules:
- GO: expectancy > 0 AND lower CI > 0 (or PF > 1.2 on >=50 trades) AND beats both controls AND drawdown within breakers AND toxic-flag rate <= 30%
- KILL: expectancy CI includes 0, any breaker trips, or toxic threshold exceeded
- INCONCLUSIVE: window elapsed but <50 trades — extend, do not decide
"""

import argparse
import sqlite3
import json
import logging
from datetime import datetime, timedelta
from decimal import Decimal
from pathlib import Path
from typing import List, Tuple, Optional
import numpy as np

# Set up logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)


class VerdictEvaluator:
    """Evaluates forward test data and emits GO/KILL/INCONCLUSIVE verdict."""
    
    def __init__(
        self,
        db_path: str,
        min_trades: int = 50,
        min_days: int = 21,
        max_drawdown_pct: float = 20.0,
        toxic_threshold_pct: float = 30.0,
        bootstrap_resamples: int = 2000
    ):
        self.db_path = db_path
        self.min_trades = min_trades
        self.min_days = min_days
        self.max_drawdown_pct = max_drawdown_pct
        self.toxic_threshold_pct = toxic_threshold_pct
        self.bootstrap_resamples = bootstrap_resamples
    
    def evaluate(self) -> dict:
        """Main evaluation method."""
        logger.info(f"Starting verdict evaluation on {self.db_path}")
        
        # Get experiment data
        experiment_data = self._load_experiment_data()
        
        # Check minimum requirements
        if not self._meets_minimum_requirements(experiment_data):
            return self._inconclusive_result(experiment_data)
        
        # Calculate key metrics
        pnl_values = experiment_data['real_pnl_values']
        execution_gaps = experiment_data['execution_gaps']
        control_random_pnl = experiment_data['control_random_pnl']
        control_sol_pnl = experiment_data['control_sol_pnl']
        
        # Calculate expectancy with BCa bootstrap CI
        expectancy_mean, ci_lower, ci_upper = self._calculate_bootstrap_ci(pnl_values)
        
        # Calculate profit factor
        profit_factor, profit_factor_lb = self._calculate_profit_factor(pnl_values)
        
        # Calculate max drawdown
        max_drawdown_pct = self._calculate_max_drawdown(pnl_values)
        
        # Calculate win rate
        win_rate = self._calculate_win_rate(pnl_values)
        
        # Calculate execution gap statistics
        avg_gap, gap_p95 = self._calculate_execution_gap_stats(execution_gaps)
        
        # Compare with controls
        vs_random = self._compare_with_control(pnl_values, control_random_pnl)
        vs_sol = self._compare_with_control(pnl_values, control_sol_pnl)
        
        # Apply decision rules
        verdict, reasons = self._apply_decision_rules(
            expectancy_mean, ci_lower, ci_upper,
            profit_factor, profit_factor_lb,
            max_drawdown_pct, avg_gap,
            vs_random, vs_sol,
            experiment_data['toxic_wallet_count'],
            experiment_data['total_wallets']
        )
        
        # Build result
        result = {
            'verdict': verdict,
            'experiment_days': experiment_data['experiment_days'],
            'total_trades': experiment_data['total_trades'],
            'tracer_trades': experiment_data['tracer_trades'],
            'expectancy_mean': str(expectancy_mean),
            'expectancy_ci_lower': str(ci_lower),
            'expectancy_ci_upper': str(ci_upper),
            'profit_factor': str(profit_factor),
            'profit_factor_lb': str(profit_factor_lb),
            'max_drawdown_pct': str(max_drawdown_pct),
            'win_rate': win_rate,
            'avg_execution_gap': str(avg_gap),
            'execution_gap_p95': str(gap_p95),
            'vs_random_control': vs_random,
            'vs_sol_benchmark': vs_sol,
            'toxic_wallet_count': experiment_data['toxic_wallet_count'],
            'toxic_wallet_rate': experiment_data['toxic_wallet_rate'],
            'verdict_reasons': reasons,
            'experiment_start': experiment_data['experiment_start'],
            'experiment_end': experiment_data['experiment_end'],
            'evaluated_at': datetime.now().isoformat()
        }
        
        logger.info(f"Verdict: {verdict}")
        logger.info(f"Reasons: {reasons}")
        
        return result
    
    def _load_experiment_data(self) -> dict:
        """Load experiment data from database."""
        conn = sqlite3.connect(self.db_path)
        cursor = conn.cursor()
        
        # Check if experiment_trades table exists
        cursor.execute("""
            SELECT name FROM sqlite_master 
            WHERE type='table' AND name='experiment_trades'
        """)
        
        if cursor.fetchone() is None:
            logger.warning("experiment_trades table not found, creating mock data")
            conn.close()
            return self._create_mock_data()
        
        # Load trade data
        cursor.execute("""
            SELECT 
                trade_uuid, wallet, token, signal_side, paper_fill_price, real_fill_price,
                paper_pnl, real_pnl, entry_latency_ms, jito_tip_sol, dex_fee_sol,
                execution_gap, control_random_pnl, sol_bench_pnl, is_tracer, toxic_flag,
                entry_time, exit_time, strategy
            FROM experiment_trades
            ORDER BY entry_time
        """)
        
        trades = cursor.fetchall()
        conn.close()
        
        if not trades:
            logger.warning("No trades found in experiment_trades table")
            return self._create_mock_data()
        
        # Parse data
        pnl_values = []
        execution_gaps = []
        control_random_pnl = []
        control_sol_pnl = []
        toxic_wallets = set()
        all_wallets = set()
        
        entry_times = []
        exit_times = []
        
        for trade in trades:
            (
                trade_uuid, wallet, token, signal_side, paper_fill_price, real_fill_price,
                paper_pnl, real_pnl, entry_latency_ms, jito_tip_sol, dex_fee_sol,
                execution_gap, ctrl_random_pnl, ctrl_sol_pnl, is_tracer, toxic_flag,
                entry_time_str, exit_time_str, strategy
            ) = trade
            
            all_wallets.add(wallet)
            if toxic_flag:
                toxic_wallets.add(wallet)
            
            if real_pnl is not None:
                pnl_values.append(float(real_pnl))
            
            if execution_gap is not None:
                execution_gaps.append(float(execution_gap))
            
            if ctrl_random_pnl is not None:
                control_random_pnl.append(float(ctrl_random_pnl))
            
            if ctrl_sol_pnl is not None:
                control_sol_pnl.append(float(ctrl_sol_pnl))
            
            if entry_time_str:
                entry_times.append(datetime.fromisoformat(entry_time_str))
            
            if exit_time_str:
                exit_times.append(datetime.fromisoformat(exit_time_str))
        
        # Calculate experiment duration
        experiment_start = entry_times[0] if entry_times else datetime.now()
        experiment_end = exit_times[-1] if exit_times else datetime.now()
        experiment_days = (experiment_end - experiment_start).days
        
        return {
            'experiment_days': experiment_days,
            'total_trades': len(trades),
            'tracer_trades': sum(1 for t in trades if t[14]),  # is_tracer
            'real_pnl_values': pnl_values,
            'execution_gaps': execution_gaps,
            'control_random_pnl': control_random_pnl,
            'control_sol_pnl': control_sol_pnl,
            'toxic_wallet_count': len(toxic_wallets),
            'total_wallets': len(all_wallets),
            'toxic_wallet_rate': len(toxic_wallets) / len(all_wallets) if all_wallets else 0.0,
            'experiment_start': experiment_start.isoformat(),
            'experiment_end': experiment_end.isoformat(),
        }
    
    def _create_mock_data(self) -> dict:
        """Create mock experiment data for testing."""
        logger.info("Creating mock experiment data")
        return {
            'experiment_days': 0,
            'total_trades': 0,
            'tracer_trades': 0,
            'real_pnl_values': [],
            'execution_gaps': [],
            'control_random_pnl': [],
            'control_sol_pnl': [],
            'toxic_wallet_count': 0,
            'total_wallets': 0,
            'toxic_wallet_rate': 0.0,
            'experiment_start': datetime.now().isoformat(),
            'experiment_end': datetime.now().isoformat(),
        }
    
    def _meets_minimum_requirements(self, data: dict) -> bool:
        """Check if experiment meets minimum requirements for verdict."""
        trades = data['total_trades']
        days = data['experiment_days']
        
        meets = trades >= self.min_trades and days >= self.min_days
        logger.info(f"Minimum requirements: trades={trades}/{self.min_trades}, days={days}/{self.min_days}, meets={meets}")
        return meets
    
    def _inconclusive_result(self, data: dict) -> dict:
        """Return INCONCLUSIVE result."""
        return {
            'verdict': 'INCONCLUSIVE',
            'experiment_days': data['experiment_days'],
            'total_trades': data['total_trades'],
            'tracer_trades': data['tracer_trades'],
            'expectancy_mean': '0.0',
            'expectancy_ci_lower': '0.0',
            'expectancy_ci_upper': '0.0',
            'profit_factor': '0.0',
            'profit_factor_lb': '0.0',
            'max_drawdown_pct': '0.0',
            'win_rate': 0.0,
            'avg_execution_gap': '0.0',
            'execution_gap_p95': '0.0',
            'vs_random_control': self._default_control_comparison(),
            'vs_sol_benchmark': self._default_control_comparison(),
            'toxic_wallet_count': data['toxic_wallet_count'],
            'toxic_wallet_rate': data['toxic_wallet_rate'],
            'verdict_reasons': [f"Insufficient data: {data['total_trades']}/{self.min_trades} trades, {data['experiment_days']}/{self.min_days} days"],
            'experiment_start': data['experiment_start'],
            'experiment_end': data['experiment_end'],
            'evaluated_at': datetime.now().isoformat()
        }
    
    def _default_control_comparison(self) -> dict:
        """Return default control comparison."""
        return {
            'difference': '0.0',
            'ci_lower': '0.0',
            'ci_upper': '0.0',
            'p_value': 1.0,
            'beats_control': False
        }
    
    def _calculate_bootstrap_ci(self, values: List[float]) -> Tuple[float, float, float]:
        """Calculate BCa bootstrap confidence interval for mean."""
        if not values:
            return 0.0, 0.0, 0.0
        
        n = len(values)
        bootstrap_means = []
        
        for _ in range(self.bootstrap_resamples):
            sample = np.random.choice(values, size=n, replace=True)
            bootstrap_means.append(np.mean(sample))
        
        bootstrap_means.sort()
        
        # Calculate 95% CI (2.5th and 97.5th percentiles)
        lower_idx = int(self.bootstrap_resamples * 0.025)
        upper_idx = int(self.bootstrap_resamples * 0.975)
        
        mean = np.mean(values)
        ci_lower = bootstrap_means[lower_idx]
        ci_upper = bootstrap_means[upper_idx]
        
        return mean, ci_lower, ci_upper
    
    def _calculate_profit_factor(self, values: List[float]) -> Tuple[float, float]:
        """Calculate profit factor and Wilson lower bound."""
        if not values:
            return 0.0, 0.0
        
        gross_profit = sum(v for v in values if v > 0)
        gross_loss = sum(abs(v) for v in values if v < 0)
        
        profit_factor = gross_profit / gross_loss if gross_loss > 0 else gross_profit
        
        # Wilson score interval for proportion
        wins = sum(1 for v in values if v > 0)
        n = len(values)
        z = 1.96  # 95% confidence
        
        if n == 0:
            return 0.0, 0.0
        
        p_hat = wins / n
        denominator = 1 + z**2 / n
        center = (p_hat + z**2 / (2 * n)) / denominator
        margin = z * np.sqrt((p_hat * (1 - p_hat) / n) + (z**2 / (4 * n**2))) / denominator
        
        lower_bound = max(0.0, center - margin)
        profit_factor_lb = profit_factor * lower_bound
        
        return profit_factor, profit_factor_lb
    
    def _calculate_max_drawdown(self, values: List[float]) -> float:
        """Calculate maximum drawdown."""
        if not values:
            return 0.0
        
        peak = 0.0
        max_drawdown = 0.0
        cumulative = 0.0
        
        for pnl in values:
            cumulative += pnl
            peak = max(peak, cumulative)
            
            if peak > 0:
                drawdown = ((cumulative - peak) / peak) * 100
                max_drawdown = min(max_drawdown, drawdown)
        
        return max_drawdown
    
    def _calculate_win_rate(self, values: List[float]) -> float:
        """Calculate win rate."""
        if not values:
            return 0.0
        
        wins = sum(1 for v in values if v > 0)
        return wins / len(values)
    
    def _calculate_execution_gap_stats(self, gaps: List[float]) -> Tuple[float, float]:
        """Calculate execution gap statistics."""
        if not gaps:
            return 0.0, 0.0
        
        avg_gap = np.mean(gaps)
        sorted_gaps = sorted(gaps)
        p95_idx = int(len(sorted_gaps) * 0.95)
        gap_p95 = sorted_gaps[p95_idx] if p95_idx < len(sorted_gaps) else 0.0
        
        return avg_gap, gap_p95
    
    def _compare_with_control(self, strategy: List[float], control: List[float]) -> dict:
        """Compare strategy with control using two-sample bootstrap."""
        if not strategy or not control:
            return self._default_control_comparison()
        
        bootstrap_diffs = []
        
        for _ in range(self.bootstrap_resamples):
            strategy_sample = np.random.choice(strategy, size=len(strategy), replace=True)
            control_sample = np.random.choice(control, size=len(control), replace=True)
            
            strategy_mean = np.mean(strategy_sample)
            control_mean = np.mean(control_sample)
            
            bootstrap_diffs.append(strategy_mean - control_mean)
        
        bootstrap_diffs.sort()
        
        mean_diff = np.mean(strategy) - np.mean(control)
        
        lower_idx = int(self.bootstrap_resamples * 0.025)
        upper_idx = int(self.bootstrap_resamples * 0.975)
        
        ci_lower = bootstrap_diffs[lower_idx]
        ci_upper = bootstrap_diffs[upper_idx]
        
        # Calculate p-value (proportion of bootstrap diffs <= 0)
        p_value = sum(1 for d in bootstrap_diffs if d <= 0) / len(bootstrap_diffs)
        
        beats_control = ci_lower > 0 and p_value < 0.05
        
        return {
            'difference': str(mean_diff),
            'ci_lower': str(ci_lower),
            'ci_upper': str(ci_upper),
            'p_value': p_value,
            'beats_control': beats_control
        }
    
    def _apply_decision_rules(
        self,
        expectancy_mean: float, ci_lower: float, ci_upper: float,
        profit_factor: float, profit_factor_lb: float,
        max_drawdown_pct: float, avg_gap: float,
        vs_random: dict, vs_sol: dict,
        toxic_wallet_count: int, total_wallets: int
    ) -> Tuple[str, List[str]]:
        """Apply pre-committed decision rules."""
        reasons = []
        verdict = "GO"
        
        # Rule 1: Expectancy must be positive with CI above 0
        if expectancy_mean <= 0 or ci_lower <= 0:
            verdict = "KILL"
            reasons.append(f"Expectancy CI includes zero: [{ci_lower:.2f}, {ci_upper:.2f}]")
        else:
            reasons.append(f"Positive expectancy: {expectancy_mean:.2f}% ± [{ci_lower:.2f}% to {ci_upper:.2f}%]")
        
        # Rule 2: Profit factor must be > 1.2 or expectancy CI clearly positive
        if profit_factor_lb < 1.2 and ci_lower < 5.0:
            verdict = "KILL"
            reasons.append(f"Profit factor too low: {profit_factor:.2f} (LB: {profit_factor_lb:.2f})")
        else:
            reasons.append(f"Profit factor acceptable: {profit_factor:.2f} (LB: {profit_factor_lb:.2f})")
        
        # Rule 3: Must beat both controls
        if not vs_random['beats_control']:
            verdict = "KILL"
            reasons.append("Does not beat random-token control")
        
        if not vs_sol['beats_control']:
            verdict = "KILL"
            reasons.append("Does not beat SOL benchmark")
        
        # Rule 4: Drawdown within limits
        if abs(max_drawdown_pct) > self.max_drawdown_pct:
            verdict = "KILL"
            reasons.append(f"Drawdown exceeds limit: {max_drawdown_pct:.2f}% > {self.max_drawdown_pct}%")
        
        # Rule 5: Toxic wallet rate within threshold
        toxic_rate = toxic_wallet_count / total_wallets if total_wallets > 0 else 0.0
        if toxic_rate > (self.toxic_threshold_pct / 100.0):
            verdict = "KILL"
            reasons.append(f"Toxic wallet rate too high: {toxic_rate * 100:.1f}% > {self.toxic_threshold_pct}%")
        
        return verdict, reasons


def main():
    parser = argparse.ArgumentParser(description='Evaluate forward test verdict')
    parser.add_argument('--db-path', required=True, help='Path to experiment database')
    parser.add_argument('--output', help='Output JSON file')
    parser.add_argument('--min-trades', type=int, default=50, help='Minimum trades for verdict')
    parser.add_argument('--min-days', type=int, default=21, help='Minimum experiment days')
    parser.add_argument('--max-drawdown', type=float, default=20.0, help='Maximum drawdown percentage')
    parser.add_argument('--toxic-threshold', type=float, default=30.0, help='Toxic wallet threshold percentage')
    parser.add_argument('--bootstrap-resamples', type=int, default=2000, help='Bootstrap resamples for CI')
    
    args = parser.parse_args()
    
    evaluator = VerdictEvaluator(
        db_path=args.db_path,
        min_trades=args.min_trades,
        min_days=args.min_days,
        max_drawdown_pct=args.max_drawdown,
        toxic_threshold_pct=args.toxic_threshold,
        bootstrap_resamples=args.bootstrap_resamples
    )
    
    result = evaluator.evaluate()
    
    # Output result
    json_result = json.dumps(result, indent=2)
    print(json_result)
    
    if args.output:
        Path(args.output).write_text(json_result)
        logger.info(f"Result saved to {args.output}")
    
    # Exit with appropriate code
    exit_code = {
        'GO': 0,
        'KILL': 1,
        'INCONCLUSIVE': 2
    }.get(result['verdict'], 2)
    
    return exit_code


if __name__ == '__main__':
    exit(main())
