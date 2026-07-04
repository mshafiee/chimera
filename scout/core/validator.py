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
from datetime import datetime, timedelta
from decimal import Decimal
from typing import Dict, List, Optional
import logging

from .models import (
    BacktestConfig,
    HistoricalTrade,
    ValidationResult,
    ValidationStatus,
)
from .backtester import BacktestSimulator
from .liquidity import LiquidityProvider
from .wqs import WalletMetrics, calculate_wqs, calculate_wqs_with_confidence

# Import security client if available
try:
    from config import ScoutConfig
    from .security_client import RugCheckClient
    SECURITY_AVAILABLE = True
except ImportError:
    SECURITY_AVAILABLE = False
    ScoutConfig = None
    RugCheckClient = None

logger = logging.getLogger(__name__)


@dataclass
class PromotionCriteria:
    """Criteria for wallet promotion."""
    # Base WQS threshold (default, can be overridden by archetype-specific thresholds)
    min_wqs_score: float = 75.0
    # Archetype-specific thresholds (None means use base threshold)
    min_wqs_whale: Optional[float] = 70.0  # Lower threshold for high-conviction whale trades
    min_wqs_swing: Optional[float] = 72.0  # Lower threshold for swing traders
    min_wqs_scalper: Optional[float] = None  # Use base threshold
    min_wqs_sniper: Optional[float] = None  # Use base threshold
    min_wqs_insider: Optional[float] = None  # Use base threshold (insiders need high WQS)
    # Momentum boost for wallets with IMPROVING trajectory
    momentum_boost: float = 5.0  # Add this many WQS points for IMPROVING trajectory
    # Minimum raw swap events (basic data sufficiency)
    min_trades: int = 5
    # Minimum ratio of realized closes (SELLs with PnL) to total trades required for promotion.
    # Default 0.4 means at least 40% of trades must be SELLs with computed PnL.
    # This replaces the old fixed min_closes_required (10) which contradicted min_trades (5).
    min_close_ratio: float = 0.4
    max_rejection_rate: float = 0.5  # Max 50% of trades can be rejected
    require_positive_simulated_pnl: bool = True
    max_pnl_reduction_percent: float = 80.0  # Max 80% reduction allowed
    # Max drawdown as fraction of total positive PnL (0.5 = 50% of gains)
    max_drawdown_fraction: float = 0.5

    # Walk-forward validation (reduce overfitting / "lucky wallet" promotion)
    walk_forward_enabled: bool = True
    walk_forward_holdout_fraction: float = 0.3  # validate on most recent 30%
    walk_forward_min_trades: int = 5

    # Penalty applied to effective WQS when walk-forward falls back to
    # full-set validation due to insufficient holdout closes.
    walk_forward_fallback_penalty: float = 15.0

    # Minimum net realized PnL (SOL) required in the walk-forward OOS holdout
    # period. Removes wallets that pass on full-set but fail on the most
    # recent trades.
    min_holdout_pnl_sol: float = 0.01


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
        rugcheck_client: Optional["RugCheckClient"] = None,  # Shared client to avoid duplicate checks
    ):
        """
        Initialize the validator.

        Args:
            liquidity_provider: Provider for liquidity data
            backtest_config: Configuration for backtesting
            promotion_criteria: Criteria for promotion decision
            rugcheck_client: Optional shared RugCheckClient (reuses cache from analyzer)
        """
        self.liquidity = liquidity_provider or LiquidityProvider()
        self.backtest_config = backtest_config or BacktestConfig()
        self.criteria = promotion_criteria or PromotionCriteria()

        self.simulator = BacktestSimulator(self.liquidity, self.backtest_config)

        # Initialize RugCheck client (use shared client if provided, else create new)
        self.rugcheck_client = rugcheck_client
        if self.rugcheck_client is None:
            if SECURITY_AVAILABLE and ScoutConfig and ScoutConfig.get_rugcheck_enabled():
                try:
                    self.rugcheck_client = RugCheckClient()
                except Exception as e:
                    logger.warning(f"Failed to initialize RugCheck client: {e}")

    def _get_archetype_threshold(self, archetype: Optional[str]) -> float:
        """
        Get the WQS threshold for a specific archetype.

        Args:
            archetype: Trader archetype string (SCALPER, SWING, WHALE, SNIPER, INSIDER)

        Returns:
            WQS threshold to use for this archetype
        """
        if archetype is None:
            return self.criteria.min_wqs_score

        archetype_upper = archetype.upper()
        archetype_thresholds = {
            "WHALE": self.criteria.min_wqs_whale,
            "SWING": self.criteria.min_wqs_swing,
            "SCALPER": self.criteria.min_wqs_scalper,
            "SNIPER": self.criteria.min_wqs_sniper,
            "INSIDER": self.criteria.min_wqs_insider,
        }

        threshold = archetype_thresholds.get(archetype_upper)
        return threshold if threshold is not None else self.criteria.min_wqs_score

    def _apply_momentum_boost(self, wqs_score: float, trajectory: Optional[str]) -> float:
        """
        Apply momentum boost for wallets with IMPROVING trajectory.

        Args:
            wqs_score: Current WQS score
            trajectory: Multi-timeframe trajectory state

        Returns:
            Adjusted WQS score with momentum boost applied
        """
        if trajectory == "IMPROVING":
            boosted = wqs_score + self.criteria.momentum_boost
            logger.debug(f"Applied momentum boost: {wqs_score:.1f} -> {boosted:.1f}")
            return boosted
        return wqs_score

    async def validate_for_promotion(
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

        # Step 1: Check WQS score (with archetype-aware thresholds and momentum boost)
        wqs_result = calculate_wqs_with_confidence(metrics, strategy=strategy)
        wqs_score = wqs_result.score
        wqs_confidence = wqs_result.confidence

        # Get archetype-specific threshold
        archetype_threshold = self._get_archetype_threshold(getattr(metrics, 'archetype', None))

        # Apply momentum boost for IMPROVING trajectory
        trajectory = getattr(metrics, 'trajectory', None)
        boosted_wqs_score = self._apply_momentum_boost(wqs_score, trajectory)

        # Log archetype and trajectory info
        if trajectory and getattr(metrics, 'archetype', None):
            logger.info(
                f"Wallet {wallet_address[:8]}: archetype={metrics.archetype}, "
                f"trajectory={trajectory}, threshold={archetype_threshold:.1f}, "
                f"WQS={wqs_score:.1f}{f' (boosted to {boosted_wqs_score:.1f})' if boosted_wqs_score != wqs_score else ''}"
            )

        # Check against archetype-specific threshold
        if boosted_wqs_score < archetype_threshold or wqs_confidence < 0.70:
            reason_parts = []
            if boosted_wqs_score < archetype_threshold:
                reason_parts.append(
                    f"boosted WQS {boosted_wqs_score:.1f} < {archetype_threshold:.1f} "
                    f"(base WQS {wqs_score:.1f}, archetype={getattr(metrics, 'archetype', 'N/A')})"
                )
            if wqs_confidence < 0.70:
                reason_parts.append(f"confidence {wqs_confidence:.2f} < 0.70")
            logger.info(f"Wallet failed WQS check: {'; '.join(reason_parts)}")
            return ValidationResult(
                wallet_address=wallet_address,
                status=ValidationStatus.FAILED_WQS,
                passed=False,
                reason=f"WQS check failed: {'; '.join(reason_parts)}",
                recommended_status="CANDIDATE",
                notes=f"WQS: {wqs_score:.1f} (boosted: {boosted_wqs_score:.1f}), confidence: {wqs_confidence:.2f}, archetype: {getattr(metrics, 'archetype', 'N/A')}",
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
                notes="Need more trade history",
            )

        # Step 2b: RugCheck validation - filter risky tokens
        if self.rugcheck_client:
            risky_tokens = []
            safe_trades = []
            for t in trades:
                token_addr = t.token_address
                if await self.rugcheck_client.is_token_safe(token_addr):
                    safe_trades.append(t)
                else:
                    risky_tokens.append(token_addr)
            
            if risky_tokens:
                unique_risky = list(set(risky_tokens))
                logger.warning(
                    f"Wallet {wallet_address[:8]}... has {len(unique_risky)} risky tokens: {unique_risky[:3]}..."
                )
                # If significant portion of trades involve risky tokens, reject
                risky_ratio = len(risky_tokens) / len(trades) if trades else 0
                if risky_ratio > 0.3:  # More than 30% risky tokens
                    return ValidationResult(
                        wallet_address=wallet_address,
                        status=ValidationStatus.FAILED_LIQUIDITY,  # Reuse status for security failure
                        passed=False,
                        reason=f"High exposure to risky tokens: {len(unique_risky)} risky tokens ({risky_ratio*100:.1f}% of trades)",
                        recommended_status="REJECTED",
                        notes=f"RugCheck flagged {len(unique_risky)} tokens",
                    )
                # Otherwise, use only safe trades for backtesting
                trades = safe_trades
        
        # Step 2c: Check minimum realized closes (SELLs with computed PnL)
        # Uses a ratio threshold rather than a fixed count — a wallet with 15 trades
        # but only 8 SELLs is well-sampled and should not be rejected.
        close_trades = [
            t for t in trades if getattr(t.action, "value", str(t.action)) == "SELL" and t.pnl_sol is not None
        ]
        min_closes = max(3, int(len(trades) * self.criteria.min_close_ratio))
        if len(close_trades) < min_closes:
            logger.info(
                f"Wallet failed close count check: {len(close_trades)} < {min_closes} "
                f"({self.criteria.min_close_ratio*100:.0f}% of {len(trades)} trades)"
            )
            return ValidationResult(
                wallet_address=wallet_address,
                status=ValidationStatus.FAILED_INSUFFICIENT_TRADES,
                passed=False,
                reason=f"Insufficient realized closes: {len(close_trades)} < {min_closes} ({self.criteria.min_close_ratio*100:.0f}% of trades)",
                recommended_status="CANDIDATE",
                notes="Need more realized closes (SELLs) for reliable validation",
            )
        
        # Step 3: Walk-forward split (optional)
        wf_trades = trades
        is_walk_forward = False
        in_sample_trades = None
        wf_notes = None
        if self.criteria.walk_forward_enabled and trades:
            sorted_trades = sorted(trades, key=lambda t: t.timestamp)
            
            # Date-based split: use chronological holdout rather than count-based.
            # Count-based splits can put trades from the same week into both train and test,
            # defeating the purpose of walk-forward validation.
            total_span = (sorted_trades[-1].timestamp - sorted_trades[0].timestamp).total_seconds()
            if total_span >= 7 * 86400:  # 7+ days of data → use date-based split
                holdout_cutoff = sorted_trades[-1].timestamp - timedelta(seconds=total_span * self.criteria.walk_forward_holdout_fraction)
                wf_trades = [t for t in sorted_trades if t.timestamp >= holdout_cutoff]
                in_sample_trades = [t for t in sorted_trades if t.timestamp < holdout_cutoff]
            else:
                # Fall back to count-based for short date ranges
                holdout_n = int(max(1, round(len(sorted_trades) * self.criteria.walk_forward_holdout_fraction)))
                wf_trades = sorted_trades[-holdout_n:]
                in_sample_trades = sorted_trades[:-holdout_n]
            wf_closes = [t for t in wf_trades if getattr(t.action, "value", str(t.action)) == "SELL" and t.pnl_sol is not None]
            if len(wf_closes) < self.criteria.walk_forward_min_trades:
                # If holdout too small, fall back to full set WITH penalty
                wf_trades = trades
                is_walk_forward = False
                wf_notes = (
                    f"Walk-forward skipped: holdout has {len(wf_closes)} closes "
                    f"< {self.criteria.walk_forward_min_trades} required; "
                    f"validated on full trade set ({len(trades)} trades) — "
                    f"WQS threshold raised by {self.criteria.walk_forward_fallback_penalty} points"
                )
                logger.warning(
                    "Wallet %s: walk-forward holdout too small (%d closes < %d min), "
                    "falling back to full-set validation with penalty — result may be overfit",
                    wallet_address[:8], len(wf_closes), self.criteria.walk_forward_min_trades,
                )
                # Apply penalty: require higher WQS when walk-forward is skipped
                effective_min_wqs = self.criteria.min_wqs_score + self.criteria.walk_forward_fallback_penalty
                if wqs_score < effective_min_wqs:
                    logger.info(
                        f"Wallet failed walk-forward fallback WQS check: "
                        f"{wqs_score:.1f} < {effective_min_wqs:.1f} (min {self.criteria.min_wqs_score} + {self.criteria.walk_forward_fallback_penalty} penalty)"
                    )
                    return ValidationResult(
                        wallet_address=wallet_address,
                        status=ValidationStatus.FAILED_WQS,
                        passed=False,
                        reason=f"WQS {wqs_score:.1f} below adjusted threshold {effective_min_wqs:.1f} (walk-forward unavailable, {self.criteria.walk_forward_fallback_penalty}pt penalty applied)",
                        recommended_status="CANDIDATE",
                        notes=f"Walk-forward skipped; WQS must be >= {effective_min_wqs:.1f} without OOS validation",
                    )
            else:
                is_walk_forward = True
                # in_sample_trades already set above (date-based or count-based)
                wf_notes = f"Walk-forward holdout: {len(wf_trades)}/{len(trades)} trades"

        # Step 3b: Validate in-sample period first (prevents curve-fitting on OOS)
        if is_walk_forward and in_sample_trades:
            try:
                is_result = self.simulator.simulate_wallet(
                    wallet_address, in_sample_trades, strategy
                )
                if not is_result.passed:
                    status = self._determine_failure_status(is_result.failure_reason)
                    return ValidationResult(
                        wallet_address=wallet_address,
                        status=status,
                        passed=False,
                        reason=f"Failed in-sample validation: {is_result.failure_reason}",
                        recommended_status="CANDIDATE",
                        notes="In-sample period must be profitable before OOS is evaluated",
                    )
            except Exception as e:
                logger.warning(f"In-sample backtest error (non-fatal): {e}")

        # Step 4: Run backtest simulation (on walk-forward OOS set if enabled)
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

        # Step 5b: Minimum holdout PnL check (D3)
        # The walk-forward OOS period must have net positive PnL of at least
        # min_holdout_pnl_sol SOL to prove the wallet is profitable in the most
        # recent window, not just historically.
        if is_walk_forward and self.criteria.min_holdout_pnl_sol > 0:
            holdout_pnl_sol = float(backtest_result.original_pnl_sol) if backtest_result.original_pnl_sol is not None else 0.0
            if holdout_pnl_sol < self.criteria.min_holdout_pnl_sol:
                logger.info(
                    f"Wallet failed holdout PnL check: {holdout_pnl_sol:.4f} SOL < {self.criteria.min_holdout_pnl_sol} SOL"
                )
                return ValidationResult(
                    wallet_address=wallet_address,
                    status=ValidationStatus.FAILED_NEGATIVE_PNL,
                    backtest_result=backtest_result,
                    passed=False,
                    reason=f"Walk-forward holdout PnL {holdout_pnl_sol:.4f} SOL below minimum {self.criteria.min_holdout_pnl_sol} SOL",
                    recommended_status="CANDIDATE",
                    notes=f"OOS PnL: {holdout_pnl_sol:.4f} SOL",
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

        # 6b. NEW: Check PROFIT FACTOR in Simulator (only when individual trade records available)
        trade_list = getattr(backtest_result, 'trades', []) or []
        if trade_list:
            sim_profit = sum(t.simulated_pnl_sol for t in trade_list if t.simulated_pnl_sol and t.simulated_pnl_sol > 0)
            sim_loss = abs(sum(t.simulated_pnl_sol for t in trade_list if t.simulated_pnl_sol and t.simulated_pnl_sol < 0))

            sim_pf = sim_profit / sim_loss if sim_loss > 0 else (100.0 if sim_profit > 0 else 0.0)

            if sim_pf < 1.1:
                logger.info(f"Wallet failed Simulated Profit Factor: {sim_pf:.2f} (Min 1.1)")
                return ValidationResult(
                    wallet_address=wallet_address,
                    status=ValidationStatus.FAILED_NEGATIVE_PNL,
                    backtest_result=backtest_result,
                    passed=False,
                    reason=f"Simulated Profit Factor too low: {sim_pf:.2f} (Min 1.1)",
                    recommended_status="CANDIDATE",
                    notes=f"Sim PF: {sim_pf:.2f}, Orig PF: {metrics.profit_factor if metrics.profit_factor else 0.0:.2f}",
                )

        # 6c. Max Drawdown Check — reject if peak-to-trough > max_drawdown_fraction of total gains
        from decimal import Decimal as _D
        simulated_equity = [_D("0")]
        current_eq = _D("0")
        total_positive_pnl = _D("0")
        for t in trade_list:
            pnl = t.simulated_pnl_sol
            if pnl:
                pnl_d = _D(str(pnl)) if not isinstance(pnl, _D) else pnl
                current_eq += pnl_d
                simulated_equity.append(current_eq)
                if pnl_d > _D("0"):
                    total_positive_pnl += pnl_d

        if len(simulated_equity) > 1 and total_positive_pnl > _D("0"):
            peak = simulated_equity[0]
            max_dd = _D("0")
            for val in simulated_equity:
                if val > peak:
                    peak = val
                dd = peak - val
                if dd > max_dd:
                    max_dd = dd

            # Scale threshold by trade count: small windows (< 10 trades) are
            # disproportionately affected by a single loss, so use a relaxed threshold.
            num_trades = len(trade_list)
            threshold_multiplier = 1.0
            if num_trades < 10:
                threshold_multiplier = 2.0
            elif num_trades < 20:
                threshold_multiplier = 1.5

            effective_max_dd = self.criteria.max_drawdown_fraction * threshold_multiplier
            threshold = total_positive_pnl * _D(str(effective_max_dd))
            if max_dd > threshold:
                logger.info(
                    f"Wallet failed drawdown check: max_dd={float(max_dd):.4f} SOL "
                    f"> {effective_max_dd*100:.0f}% of gains "
                    f"({float(total_positive_pnl):.4f} SOL, {num_trades} trades, threshold x{threshold_multiplier})"
                )
                return ValidationResult(
                    wallet_address=wallet_address,
                    status=ValidationStatus.FAILED_NEGATIVE_PNL,
                    backtest_result=backtest_result,
                    passed=False,
                    reason=(
                        f"Max drawdown {float(max_dd):.4f} SOL exceeds "
                        f"{effective_max_dd*100:.0f}% of total gains "
                        f"({float(total_positive_pnl):.4f} SOL, {num_trades} trades)"
                    ),
                    recommended_status="CANDIDATE",
                    notes="Wallet has excessive drawdown relative to gains — too volatile to promote",
                )

        # 6d. Token concentration risk — reject wallets with >60% PnL from one token
        # combined with low token diversity (< 5 unique tokens)
        if trade_list:
            token_pnl: Dict[str, Decimal] = {}
            for st in trade_list:
                ot = st.original_trade
                if ot.pnl_sol:
                    pnl_d = ot.pnl_sol if isinstance(ot.pnl_sol, Decimal) else Decimal(str(ot.pnl_sol))
                    token_pnl[ot.token_address] = token_pnl.get(ot.token_address, Decimal('0')) + pnl_d
            if token_pnl:
                total_abs_pnl = sum(abs(v) for v in token_pnl.values())
                if total_abs_pnl > Decimal('0'):
                    max_pnl = max(abs(v) for v in token_pnl.values())
                    concentration = float(max_pnl / total_abs_pnl) if total_abs_pnl > Decimal('0') else 0.0
                    unique_tokens = len(token_pnl)
                    if concentration > 0.60 and unique_tokens < 5:
                        logger.info(
                            f"Wallet failed token concentration check: "
                            f"{concentration*100:.0f}% PnL from one token, only {unique_tokens} unique tokens"
                        )
                        return ValidationResult(
                            wallet_address=wallet_address,
                            status=ValidationStatus.FAILED_NEGATIVE_PNL,
                            backtest_result=backtest_result,
                            passed=False,
                            reason=f"Token concentration risk: {concentration*100:.0f}% of PnL from one token "
                                   f"with only {unique_tokens} unique tokens traded",
                            recommended_status="CANDIDATE",
                            notes="Wallet PnL is not diversified — likely one lucky trade",
                        )

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
        if backtest_result.regime_risk == "BULL":
            logger.info(
                f"Wallet {wallet_address[:8]}... profitable in BULL regime — "
                f"may underperform in bear/crab markets"
            )
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




async def validate_wallet_for_promotion(
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
    return await validator.validate_for_promotion(wallet_address, metrics, trades, strategy)


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
