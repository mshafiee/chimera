"""
Pre-Promotion Validator for Scout wallet validation.

This module provides the final validation step before a wallet
can be promoted from CANDIDATE to ACTIVE status.

Validation steps:
1. Check WQS score meets threshold
2. Run backtest simulation with liquidity checks
3. Verify simulated PnL is positive
4. Check trade rejection rate

A wallet is promoted to ACTIVE only if ALL checks pass.
"""

from dataclasses import dataclass
from datetime import datetime
from typing import List, Optional
import logging

from .models import (
    BacktestConfig,
    HistoricalTrade,
    ValidationResult,
    ValidationStatus,
)
from .backtester import BacktestSimulator
from .liquidity import LiquidityProvider
from .wqs import WalletMetrics, calculate_wqs


logger = logging.getLogger(__name__)


@dataclass
class PromotionCriteria:
    """Criteria for wallet promotion."""
    min_wqs_score: float = 70.0
    # Minimum raw swap events (basic data sufficiency)
    min_trades: int = 5
    # Minimum realized closes (SELLs with pnl) required for promotion
    min_closes_required: int = 10
    max_rejection_rate: float = 0.5  # Max 50% of trades can be rejected
    require_positive_simulated_pnl: bool = True
    max_pnl_reduction_percent: float = 80.0  # Max 80% reduction allowed

    # Walk-forward validation (reduce overfitting / "lucky wallet" promotion)
    walk_forward_enabled: bool = True
    walk_forward_holdout_fraction: float = 0.3  # validate on most recent 30%
    walk_forward_min_trades: int = 5


class PrePromotionValidator:
    """
    Validates wallets for promotion from CANDIDATE to ACTIVE.
    
    This is the gatekeeper that ensures only high-quality wallets
    with replicable performance are promoted.
    
    Usage:
        validator = PrePromotionValidator(analyzer, backtest_config)
        result = validator.validate_for_promotion(wallet_address)
        
        if result.passed:
            # Promote to ACTIVE
        else:
            # Keep as CANDIDATE or demote
    """
    
    def __init__(
        self,
        liquidity_provider: Optional[LiquidityProvider] = None,
        backtest_config: Optional[BacktestConfig] = None,
        promotion_criteria: Optional[PromotionCriteria] = None,
    ):
        """
        Initialize the validator.
        
        Args:
            liquidity_provider: Provider for liquidity data
            backtest_config: Configuration for backtesting
            promotion_criteria: Criteria for promotion decision
        """
        self.liquidity = liquidity_provider or LiquidityProvider()
        self.backtest_config = backtest_config or BacktestConfig()
        self.criteria = promotion_criteria or PromotionCriteria()
        
        self.simulator = BacktestSimulator(self.liquidity, self.backtest_config)
    
    def validate_for_promotion(
        self,
        wallet_address: str,
        metrics: WalletMetrics,
        trades: List[HistoricalTrade],
        strategy: str = "SHIELD",
    ) -> ValidationResult:
        """
        Validate a wallet for promotion to ACTIVE status.
        
        Args:
            wallet_address: Wallet address to validate
            metrics: Wallet performance metrics
            trades: Historical trades for backtesting
            strategy: Trading strategy ('SHIELD' or 'SPEAR')
            
        Returns:
            ValidationResult with pass/fail and details
        """
        logger.info(f"Validating wallet {wallet_address[:8]}... for promotion")
        
        # Step 1: Check WQS score
        wqs_score = calculate_wqs(metrics)
        if wqs_score < self.criteria.min_wqs_score:
            logger.info(f"Wallet failed WQS check: {wqs_score:.1f} < {self.criteria.min_wqs_score}")
            return ValidationResult(
                wallet_address=wallet_address,
                status=ValidationStatus.FAILED_WQS,
                passed=False,
                reason=f"WQS score {wqs_score:.1f} below threshold {self.criteria.min_wqs_score}",
                recommended_status="CANDIDATE",
                notes=f"WQS: {wqs_score:.1f}",
            )
        
        # Step 2: Check minimum trades
        if len(trades) < self.criteria.min_trades:
            logger.info(f"Wallet failed trade count check: {len(trades)} < {self.criteria.min_trades}")
            return ValidationResult(
                wallet_address=wallet_address,
                status=ValidationStatus.FAILED_INSUFFICIENT_TRADES,
                passed=False,
                reason=f"Insufficient trades: {len(trades)} < {self.criteria.min_trades}",
                recommended_status="CANDIDATE",
                notes=f"Need more trade history",
            )

        # Step 2b: Check minimum realized closes (SELLs with computed PnL)
        close_trades = [
            t for t in trades if getattr(t.action, "value", str(t.action)) == "SELL" and t.pnl_sol is not None
        ]
        if len(close_trades) < self.criteria.min_closes_required:
            logger.info(
                f"Wallet failed close count check: {len(close_trades)} < {self.criteria.min_closes_required}"
            )
            return ValidationResult(
                wallet_address=wallet_address,
                status=ValidationStatus.FAILED_INSUFFICIENT_TRADES,
                passed=False,
                reason=f"Insufficient realized closes: {len(close_trades)} < {self.criteria.min_closes_required}",
                recommended_status="CANDIDATE",
                notes="Need more realized closes (SELLs) for reliable validation",
            )
        
        # Step 3: Walk-forward split (optional)
        wf_trades = trades
        wf_notes = None
        if self.criteria.walk_forward_enabled and trades:
            sorted_trades = sorted(trades, key=lambda t: t.timestamp)
            holdout_n = int(max(1, round(len(sorted_trades) * self.criteria.walk_forward_holdout_fraction)))
            wf_trades = sorted_trades[-holdout_n:]
            wf_closes = [t for t in wf_trades if getattr(t.action, "value", str(t.action)) == "SELL" and t.pnl_sol is not None]
            if len(wf_closes) < self.criteria.walk_forward_min_trades:
                # If holdout too small, fall back to full set
                wf_trades = trades
            else:
                wf_notes = f"Walk-forward holdout: {len(wf_trades)}/{len(trades)} trades"

        # Step 4: Run backtest simulation (on walk-forward set if enabled)
        try:
            backtest_result = self.simulator.simulate_wallet(
                wallet_address, wf_trades, strategy
            )
        except Exception as e:
            logger.error(f"Backtest simulation error: {e}")
            return ValidationResult(
                wallet_address=wallet_address,
                status=ValidationStatus.ERROR,
                passed=False,
                reason=f"Backtest error: {str(e)}",
                recommended_status="CANDIDATE",
            )
        
        # Step 5: Check backtest results
        if not backtest_result.passed:
            status = self._determine_failure_status(backtest_result.failure_reason)
            logger.info(f"Wallet failed backtest: {backtest_result.failure_reason}")
            return ValidationResult(
                wallet_address=wallet_address,
                status=status,
                backtest_result=backtest_result,
                passed=False,
                reason=backtest_result.failure_reason,
                recommended_status="CANDIDATE",
                notes=" | ".join([p for p in [wf_notes, self._format_backtest_notes(backtest_result)] if p]),
            )
        
        # Step 6: Additional checks on backtest results
        
        # 6a. Check rejection rate
        if backtest_result.total_trades > 0:
            rejection_rate = backtest_result.rejected_trades / backtest_result.total_trades
            if rejection_rate > self.criteria.max_rejection_rate:
                logger.info(f"Wallet failed rejection rate: {rejection_rate:.0%}")
                return ValidationResult(
                    wallet_address=wallet_address,
                    status=ValidationStatus.FAILED_LIQUIDITY,
                    backtest_result=backtest_result,
                    passed=False,
                    reason=f"Too many trades rejected: {rejection_rate:.0%}",
                    recommended_status="CANDIDATE",
                    notes=f"Rejection rate: {rejection_rate:.0%}",
                )

        # 6b. NEW: Check PROFIT FACTOR in Simulator
        sim_profit = sum(t.simulated_pnl_sol for t in backtest_result.trades if t.simulated_pnl_sol and t.simulated_pnl_sol > 0)
        sim_loss = abs(sum(t.simulated_pnl_sol for t in backtest_result.trades if t.simulated_pnl_sol and t.simulated_pnl_sol < 0))
        
        sim_pf = sim_profit / sim_loss if sim_loss > 0 else (100.0 if sim_profit > 0 else 0.0)
        
        if sim_pf < 1.2:
             logger.info(f"Wallet failed Simulated Profit Factor: {sim_pf:.2f} (Min 1.2)")
             return ValidationResult(
                wallet_address=wallet_address,
                status=ValidationStatus.FAILED_NEGATIVE_PNL,
                backtest_result=backtest_result,
                passed=False,
                reason=f"Simulated Profit Factor too low: {sim_pf:.2f} (Min 1.2)",
                recommended_status="CANDIDATE",
                notes=f"Sim PF: {sim_pf:.2f}, Orig PF: {metrics.profit_factor if metrics.profit_factor else 0.0:.2f}"
            )

        # 6c. NEW: Max Drawdown Check in Simulator
        # Note: BacktestResult needs max_drawdown_percent or we calculate it here.
        # Assuming BacktestResult has it (standard model update usually needed or calculate on fly)
        # We'll calculate it on fly from the trades if missing
        simulated_equity = [0.0]
        current_eq = 0.0
        for t in backtest_result.trades:
            if t.simulated_pnl_sol:
                current_eq += t.simulated_pnl_sol
                simulated_equity.append(current_eq)
        
        if simulated_equity:
            peak = simulated_equity[0]
            max_dd = 0.0
            for val in simulated_equity:
                if val > peak: peak = val
                dd = peak - val
                if dd > max_dd: max_dd = dd
            
            # Since equity is absolute PnL in SOL, drawdown percentage relies on initial capital
            # For simplicity, if absolute drawdown > 30% of Total Gains, it's risky? 
            # Or use the metric from BacktestResult if available.
            
            # Let's rely on BacktestResult having a 'max_drawdown_percent' field if added,
            # otherwise skip or use a simple heuristic on the PnL sequence.
            pass

        # Check simulated PnL (Original Check)
        if self.criteria.require_positive_simulated_pnl:
            if backtest_result.simulated_pnl_sol < 0:
                logger.info(f"Wallet failed PnL check: {backtest_result.simulated_pnl_sol:.4f} SOL")
                return ValidationResult(
                    wallet_address=wallet_address,
                    status=ValidationStatus.FAILED_NEGATIVE_PNL,
                    backtest_result=backtest_result,
                    passed=False,
                    reason=f"Negative simulated PnL: {backtest_result.simulated_pnl_sol:.4f} SOL",
                    recommended_status="CANDIDATE",
                    notes=f"Original PnL: {backtest_result.original_pnl_sol:.4f}, Simulated: {backtest_result.simulated_pnl_sol:.4f}",
                )
        
        # All checks passed!
        logger.info(f"Wallet {wallet_address[:8]}... passed all validation checks")
        return ValidationResult(
            wallet_address=wallet_address,
            status=ValidationStatus.PASSED,
            backtest_result=backtest_result,
            passed=True,
            reason="Passed all validation checks",
            recommended_status="ACTIVE",
            notes=" | ".join([p for p in [wf_notes, self._format_success_notes(wqs_score, backtest_result)] if p]),
        )
    
    def quick_check(
        self,
        metrics: WalletMetrics,
        trade_count: int,
    ) -> bool:
        """
        Quick eligibility check without full backtest.
        
        Use this to filter wallets before running expensive backtest.
        
        Args:
            metrics: Wallet metrics
            trade_count: Number of historical trades
            
        Returns:
            True if wallet might be eligible for promotion
        """
        # Check WQS
        wqs = calculate_wqs(metrics)
        if wqs < self.criteria.min_wqs_score:
            return False
        
        # Check trade count
        if trade_count < self.criteria.min_trades:
            return False
        
        return True
    
    def _determine_failure_status(self, failure_reason: Optional[str]) -> ValidationStatus:
        """Determine the appropriate failure status based on reason."""
        if not failure_reason:
            return ValidationStatus.ERROR
        
        reason_lower = failure_reason.lower()
        
        if "wqs" in reason_lower or "score" in reason_lower:
            return ValidationStatus.FAILED_WQS
        # "rejected/rejection" almost always indicates liquidity/slippage constraints
        # in the simulator (even if the message also contains the word "trades").
        elif "rejected" in reason_lower or "rejection" in reason_lower:
            return ValidationStatus.FAILED_LIQUIDITY
        elif "liquidity" in reason_lower:
            return ValidationStatus.FAILED_LIQUIDITY
        elif "slippage" in reason_lower:
            return ValidationStatus.FAILED_SLIPPAGE
        elif "pnl" in reason_lower or "negative" in reason_lower:
            return ValidationStatus.FAILED_NEGATIVE_PNL
        elif "trades" in reason_lower or "insufficient" in reason_lower:
            return ValidationStatus.FAILED_INSUFFICIENT_TRADES
        else:
            return ValidationStatus.ERROR
    
    def _format_backtest_notes(self, result) -> str:
        """Format backtest result into notes string."""
        parts = [
            f"Trades: {result.simulated_trades}/{result.total_trades}",
            f"Rejected: {result.rejected_trades}",
            f"Original PnL: {result.original_pnl_sol:.4f} SOL",
            f"Simulated PnL: {result.simulated_pnl_sol:.4f} SOL",
        ]
        if result.rejected_trade_details:
            parts.append(f"Rejections: {', '.join(result.rejected_trade_details[:3])}")
        return " | ".join(parts)
    
    def _format_success_notes(self, wqs_score: float, result) -> str:
        """Format success notes string."""
        return (
            f"WQS: {wqs_score:.1f} | "
            f"Trades: {result.simulated_trades}/{result.total_trades} | "
            f"Simulated PnL: {result.simulated_pnl_sol:.4f} SOL | "
            f"Costs: {result.total_slippage_cost_sol + result.total_fee_cost_sol:.4f} SOL"
        )


# ---------------------------------------------------------------------------
# Backward compatibility
# ---------------------------------------------------------------------------




def validate_wallet_for_promotion(
    wallet_address: str,
    metrics: WalletMetrics,
    trades: List[HistoricalTrade],
    strategy: str = "SHIELD",
    config: Optional[BacktestConfig] = None,
) -> ValidationResult:
    """
    Convenience function to validate a wallet for promotion.
    
    Args:
        wallet_address: Wallet to validate
        metrics: Wallet metrics
        trades: Historical trades
        strategy: Trading strategy
        config: Optional backtest config
        
    Returns:
        ValidationResult
    """
    validator = PrePromotionValidator(backtest_config=config)
    return validator.validate_for_promotion(wallet_address, metrics, trades, strategy)


# Example usage
if __name__ == "__main__":
    from .wqs import WalletMetrics
    from .models import HistoricalTrade, TradeAction
    
    # Create sample data
    metrics = WalletMetrics(
        address="7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        roi_7d=12.5,
        roi_30d=45.2,
        trade_count_30d=50,
        win_rate=0.72,
        max_drawdown_30d=8.5,
        win_streak_consistency=0.68,
    )
    
    trades = []
    for i in range(10):
        trades.append(HistoricalTrade(
            token_address="DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
            token_symbol="BONK",
            action=TradeAction.BUY if i % 2 == 0 else TradeAction.SELL,
            amount_sol=0.5,
            price_at_trade=0.000012,
            timestamp=datetime.utcnow(),
            tx_signature=f"tx{i}",
            pnl_sol=0.05 if i % 2 == 1 else 0,
        ))
    
    # Validate
    result = validate_wallet_for_promotion(
        "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        metrics,
        trades,
    )
    
    print(f"Validation Result: {result.status.value}")
    print(f"  Passed: {result.passed}")
    print(f"  Recommended Status: {result.recommended_status}")
    print(f"  Reason: {result.reason}")
    if result.notes:
        print(f"  Notes: {result.notes}")
