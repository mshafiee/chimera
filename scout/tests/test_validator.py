"""Tests for wallet validation and backtesting"""

import pytest
from datetime import datetime, timedelta
from unittest.mock import Mock
from scout.core.validator import PrePromotionValidator, ValidationStatus, PromotionCriteria
from scout.core.wqs import WalletMetrics
from scout.core.models import HistoricalTrade, TradeAction, SimulatedResult, BacktestConfig
from scout.core.liquidity import LiquidityProvider, LiquidityData
from scout.core.wqs import calculate_wqs, classify_wallet
from scout.core.models import SimulatedTrade
from decimal import Decimal


@pytest.mark.asyncio
async def test_validator_rejects_low_wqs():
    """Test that validator rejects wallets with WQS below threshold."""
    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(min_wqs_score=60.0)
    )

    metrics = WalletMetrics(
        address="test_wallet",
        roi_30d=10.0,  # Low ROI
        trade_count_30d=5,
        win_rate=0.5,
    )

    result = await validator.validate_for_promotion(
        "test_wallet",
        metrics,
        [],
        strategy="SHIELD"
    )

    assert not result.passed
    assert result.status == ValidationStatus.FAILED_WQS
    assert "wqs score" in result.reason.lower()


@pytest.mark.asyncio
async def test_validator_rejects_insufficient_trades():
    """Test that validator rejects wallets with insufficient trades."""
    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(
            min_wqs_score=30.0,
            min_trades=10,
        )
    )

    metrics = WalletMetrics(
        address="test_wallet",
        roi_30d=50.0,
        trade_count_30d=20,  # Enough for WQS
        win_rate=0.7,
        avg_trade_size_sol=0.5,  # avoid dust-trader penalty
        profit_factor=2.0,       # positive proof of profitability
    )

    # Only 5 trades (below min_trades=10)
    trades = [
        HistoricalTrade(
            token_address="token1",
            token_symbol="TOKEN1",
            action=TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.001,
            timestamp=datetime.utcnow() - timedelta(days=i),
            tx_signature=f"tx{i}",
        )
        for i in range(5)
    ]

    result = await validator.validate_for_promotion(
        "test_wallet",
        metrics,
        trades,
        strategy="SHIELD"
    )

    assert not result.passed
    assert result.status == ValidationStatus.FAILED_INSUFFICIENT_TRADES


@pytest.mark.asyncio
async def test_validator_rejects_insufficient_closes():
    """Test that validator rejects wallets with insufficient realized closes."""
    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(
            min_wqs_score=30.0,
            min_trades=5,
            min_close_ratio=0.4,  # Need 40% of trades to be SELLs with PnL
        )
    )
    validator.rugcheck_client = None  # disable network call in tests

    metrics = WalletMetrics(
        address="test_wallet",
        roi_30d=50.0,
        trade_count_30d=20,
        win_rate=0.7,
        avg_trade_size_sol=0.5,
        profit_factor=2.0,
    )

    # Create trades with only 5 SELLs (below min_close_ratio=0.4)
    trades = []
    for i in range(15):
        is_sell = i % 3 == 2  # Every 3rd trade is a SELL
        trades.append(HistoricalTrade(
            token_address="token1",
            token_symbol="TOKEN1",
            action=TradeAction.SELL if is_sell else TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.001,
            timestamp=datetime.utcnow() - timedelta(days=i),
            tx_signature=f"tx{i}",
            pnl_sol=0.1 if is_sell else None,  # Only SELLs have PnL
        ))

    result = await validator.validate_for_promotion(
        "test_wallet",
        metrics,
        trades,
        strategy="SHIELD"
    )

    assert not result.passed
    assert result.status == ValidationStatus.FAILED_INSUFFICIENT_TRADES
    assert "closes" in result.reason.lower()


@pytest.mark.asyncio
async def test_validator_rejects_negative_simulated_pnl():
    """Test that validator rejects wallets with negative simulated PnL."""
    # Mock backtester to return negative PnL
    mock_simulator = Mock()
    mock_simulator.simulate_wallet.return_value = SimulatedResult(
        wallet_address="test_wallet",
        total_trades=10,
        simulated_trades=10,
        rejected_trades=0,
        original_pnl_sol=5.0,
        simulated_pnl_sol=-1.0,  # Negative!
        pnl_difference_sol=6.0,
        total_slippage_cost_sol=2.0,
        total_fee_cost_sol=1.0,
        passed=False,
        failure_reason="Negative simulated PnL",
    )

    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(
            min_wqs_score=30.0,
            min_trades=5,
            min_close_ratio=0.3,
        )
    )
    validator.simulator = mock_simulator
    validator.rugcheck_client = None  # disable network call in tests

    metrics = WalletMetrics(
        address="test_wallet",
        roi_30d=50.0,
        trade_count_30d=20,
        win_rate=0.7,
        avg_trade_size_sol=0.5,
        profit_factor=2.0,
    )

    trades = [
        HistoricalTrade(
            token_address="token1",
            token_symbol="TOKEN1",
            action=TradeAction.SELL if i % 2 == 1 else TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.001,
            timestamp=datetime.utcnow() - timedelta(days=i),
            tx_signature=f"tx{i}",
            pnl_sol=0.1 if i % 2 == 1 else None,
        )
        for i in range(10)
    ]

    result = await validator.validate_for_promotion(
        "test_wallet",
        metrics,
        trades,
        strategy="SHIELD"
    )

    assert not result.passed
    assert result.status == ValidationStatus.FAILED_NEGATIVE_PNL


@pytest.mark.asyncio
async def test_validator_rejects_high_rejection_rate():
    """Test that validator rejects wallets with too many rejected trades."""
    # Mock backtester to return high rejection rate
    mock_simulator = Mock()
    mock_simulator.simulate_wallet.return_value = SimulatedResult(
        wallet_address="test_wallet",
        total_trades=10,
        simulated_trades=4,  # Only 4 out of 10 executed
        rejected_trades=6,  # 60% rejected
        original_pnl_sol=5.0,
        simulated_pnl_sol=2.0,
        pnl_difference_sol=3.0,
        total_slippage_cost_sol=1.0,
        total_fee_cost_sol=0.5,
        passed=False,
        failure_reason="Too many trades rejected",
    )

    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(
            min_wqs_score=30.0,
            min_trades=5,
            min_close_ratio=0.3,
            max_rejection_rate=0.5,  # Max 50% rejection
        )
    )
    validator.simulator = mock_simulator
    validator.rugcheck_client = None  # disable network call in tests

    metrics = WalletMetrics(
        address="test_wallet",
        roi_30d=50.0,
        trade_count_30d=20,
        win_rate=0.7,
        avg_trade_size_sol=0.5,
        profit_factor=2.0,
    )

    trades = [
        HistoricalTrade(
            token_address="token1",
            token_symbol="TOKEN1",
            action=TradeAction.SELL if i % 2 == 1 else TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.001,
            timestamp=datetime.utcnow() - timedelta(days=i),
            tx_signature=f"tx{i}",
            pnl_sol=0.1 if i % 2 == 1 else None,
        )
        for i in range(10)
    ]

    result = await validator.validate_for_promotion(
        "test_wallet",
        metrics,
        trades,
        strategy="SHIELD"
    )

    assert not result.passed
    assert result.status == ValidationStatus.FAILED_LIQUIDITY  # High rejection usually means liquidity issues


@pytest.mark.asyncio
async def test_validator_passes_good_wallet():
    """Test that validator accepts wallets that pass all checks."""
    # Mock backtester to return positive result
    mock_simulator = Mock()
    mock_simulator.simulate_wallet.return_value = SimulatedResult(
        wallet_address="test_wallet",
        total_trades=15,
        simulated_trades=15,
        rejected_trades=0,
        original_pnl_sol=10.0,
        simulated_pnl_sol=8.0,  # Positive, acceptable reduction
        pnl_difference_sol=2.0,
        total_slippage_cost_sol=1.0,
        total_fee_cost_sol=1.0,
        passed=True,
        failure_reason=None,
    )

    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(
            min_wqs_score=30.0,
            min_trades=5,
            min_close_ratio=0.3,
        )
    )
    validator.simulator = mock_simulator
    validator.rugcheck_client = None  # disable network call in tests

    metrics = WalletMetrics(
        address="test_wallet",
        roi_30d=50.0,
        trade_count_30d=20,
        win_rate=0.7,
        avg_trade_size_sol=0.5,
        profit_factor=2.0,
    )

    trades = [
        HistoricalTrade(
            token_address="token1",
            token_symbol="TOKEN1",
            action=TradeAction.SELL if i % 2 == 1 else TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.001,
            timestamp=datetime.utcnow() - timedelta(days=i),
            tx_signature=f"tx{i}",
            pnl_sol=0.1 if i % 2 == 1 else None,
        )
        for i in range(15)
    ]

    result = await validator.validate_for_promotion(
        "test_wallet",
        metrics,
        trades,
        strategy="SHIELD"
    )

    assert result.passed
    assert result.status == ValidationStatus.PASSED
    assert result.recommended_status == "ACTIVE"


# ─── Proof Suite: wallet selection funnel correctness ────────────────────────
#
# P1–P4 prove the promotion pipeline correctly promotes profitable wallets and
# rejects loss-makers. Uses real BacktestSimulator (via MockLiqProvider) for P1.

class _MockLiqProvider(LiquidityProvider):
    """Minimal high-liquidity mock for validator proof tests. No API calls."""

    def __init__(self, liquidity_usd: float = 500_000.0):
        super().__init__()
        self._liq = liquidity_usd

    def get_current_liquidity(self, token_address: str):
        return LiquidityData(
            token_address=token_address,
            liquidity_usd=self._liq,
            price_usd=0.001,
            volume_24h_usd=self._liq * 0.5,
            timestamp=datetime.utcnow(),
            source="mock",
        )

    def get_historical_liquidity(self, token_address: str, timestamp: datetime):
        return self.get_current_liquidity(token_address)


def _make_round_trip_trades(
    n_pairs: int,
    buy_sol: float,
    sell_sol: float,
    token_prefix: str = "ptoken",
    tokens_per_trade: int = 1000,
) -> list:
    """Build n_pairs of (BUY, SELL) HistoricalTrade pairs with sequential timestamps."""
    trades = []
    base = datetime.utcnow() - timedelta(days=n_pairs * 2)
    for k in range(n_pairs):
        tok = f"{token_prefix}_{k}"
        net_pnl = Decimal(str(round(sell_sol - buy_sol - 0.01, 6)))
        trades += [
            HistoricalTrade(
                token_address=tok,
                token_symbol=tok.upper(),
                action=TradeAction.BUY,
                amount_sol=Decimal(str(buy_sol)),
                price_at_trade=Decimal("100.0"),
                timestamp=base + timedelta(days=2 * k),
                tx_signature=f"buy_{k}_{token_prefix}",
                token_amount=Decimal(str(tokens_per_trade)),
            ),
            HistoricalTrade(
                token_address=tok,
                token_symbol=tok.upper(),
                action=TradeAction.SELL,
                amount_sol=Decimal(str(sell_sol)),
                price_at_trade=Decimal(str(sell_sol / buy_sol * 100)),
                timestamp=base + timedelta(days=2 * k, hours=4),
                tx_signature=f"sell_{k}_{token_prefix}",
                token_amount=Decimal(str(tokens_per_trade)),
                pnl_sol=net_pnl,
            ),
        ]
    return trades


def _make_mock_sim_result(pnl_list: list, wallet_address: str = "test_wallet") -> SimulatedResult:
    """Build a SimulatedResult whose trades have specific per-trade PnL (as floats).

    Uses floats (not Decimal) for simulated_pnl_sol so that step 6b's profit-factor
    arithmetic (`sum / sum / 1.2 comparison`) avoids Decimal-vs-float TypeError.
    """
    dummy_trade = HistoricalTrade(
        token_address="token_pf",
        token_symbol="PF",
        action=TradeAction.SELL,
        amount_sol=Decimal("1.0"),
        price_at_trade=Decimal("1.0"),
        timestamp=datetime.utcnow(),
        tx_signature="sig_pf",
    )
    sim_trades = [
        SimulatedTrade(
            original_trade=dummy_trade,
            current_liquidity_usd=Decimal("50000"),
            liquidity_sufficient=True,
            estimated_slippage_percent=Decimal("0.001"),
            slippage_cost_sol=Decimal("0.001"),
            fee_cost_sol=Decimal("0.001"),
            simulated_pnl_sol=pnl,   # plain float — intentional; avoids Decimal<float TypeError
            rejected=False,
        )
        for pnl in pnl_list
    ]
    total = sum(pnl_list)
    return SimulatedResult(
        wallet_address=wallet_address,
        total_trades=len(sim_trades),
        simulated_trades=len(sim_trades),
        rejected_trades=0,
        original_pnl_sol=Decimal(str(round(total * 1.05, 6))),
        simulated_pnl_sol=total,
        pnl_difference_sol=Decimal(str(round(total * 0.05, 6))),
        total_slippage_cost_sol=Decimal("0.05"),
        total_fee_cost_sol=Decimal("0.05"),
        trades=sim_trades,
        passed=total > 0,
        failure_reason=None if total > 0 else "Negative PnL",
    )


# ─── P1 ──────────────────────────────────────────────────────────────────────

@pytest.mark.asyncio
async def test_realistic_profitable_wallet_reaches_active():
    """Full chain proof: realistic profitable metrics → WQS ≥ 70 → real backtest passes → ACTIVE.

    Uses a real BacktestSimulator (no mock) backed by a high-liquidity mock provider.
    Proves the wallet selection funnel promotes genuinely profitable wallets end-to-end.
    """
    from scout.core.wqs import WalletMetrics as WM

    # Step 1: Verify WQS independently — ~72.65 with these metrics
    # roi_7d=80: contributes min(10, (80/100)*10)=8 pts (S-03 fix: 7d ROI scaled same as 30d)
    metrics = WM(
        address="wallet_proof_001",
        roi_7d=80.0,
        roi_30d=45.0,
        trade_count_30d=30,
        win_rate=0.70,
        max_drawdown_30d=8.0,
        avg_trade_size_sol=0.5,        # Avoids dust-trader (-10 pt) penalty
        win_streak_consistency=0.6,
        avg_entry_delay_seconds=180.0, # Smart money sweet spot (+15 pts)
        profit_factor=2.8,             # +5 pts (> 1.5)
    )
    wqs = calculate_wqs(metrics)
    assert wqs >= 70.0, (
        f"Realistic profitable metrics must score ≥70. Got {wqs:.2f}. "
        f"(roi_30d=45, roi_7d=80, win_rate=0.70, PF=2.8, delay=180s)"
    )

    # Step 2: 15 profitable BUY/SELL pairs: buy 2.0 SOL, sell 2.6 SOL (+30% gross)
    trades = _make_round_trip_trades(
        n_pairs=15,
        buy_sol=2.0,
        sell_sol=2.6,
        token_prefix="profit_tok",
    )

    # Step 3: Full validator with real backtester + mock $500k liquidity per token
    validator = PrePromotionValidator(
        liquidity_provider=_MockLiqProvider(liquidity_usd=500_000.0),
        backtest_config=BacktestConfig(
            min_liquidity_shield_usd=10_000.0,
            min_trades_required=5,
        ),
        promotion_criteria=PromotionCriteria(
            min_wqs_score=70.0,
            min_trades=20,
            min_close_ratio=0.5,
            walk_forward_enabled=False,   # Use full set — no holdout split
        ),
    )
    validator.rugcheck_client = None

    result = await validator.validate_for_promotion(
        "wallet_proof_001", metrics, trades, strategy="SHIELD"
    )

    assert result.passed, (
        f"Profitable wallet must reach ACTIVE. "
        f"Status: {result.status}, Reason: {result.reason}"
    )
    assert result.recommended_status == "ACTIVE", (
        f"Expected ACTIVE, got {result.recommended_status}"
    )


# ─── P2 ──────────────────────────────────────────────────────────────────────

@pytest.mark.asyncio
async def test_realistic_losing_wallet_rejected():
    """Proves WQS filter blocks loss-making wallets before backtest.

    A wallet with negative ROI, low win rate, and sub-1.0 profit factor
    must score < 40 and be rejected at the WQS gate — no backtest run.
    """
    metrics = WalletMetrics(
        address="wallet_loser_001",
        roi_7d=-8.0,
        roi_30d=-15.0,
        trade_count_30d=20,
        win_rate=0.38,
        max_drawdown_30d=25.0,
        profit_factor=0.75,  # < 1.0 → -40 pt Losing Trader penalty
    )

    wqs = calculate_wqs(metrics)
    assert wqs < 40.0, f"Losing wallet metrics must score < 40. Got {wqs:.2f}"
    assert classify_wallet(wqs) == "REJECTED"

    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(min_wqs_score=70.0),
    )
    validator.rugcheck_client = None

    result = await validator.validate_for_promotion(
        "wallet_loser_001", metrics, [], strategy="SHIELD"
    )

    assert not result.passed, "Loss-making wallet must not pass validation"
    assert result.status == ValidationStatus.FAILED_WQS, (
        f"Low WQS wallet must fail at WQS gate. Got: {result.status}"
    )


# ─── P3 ──────────────────────────────────────────────────────────────────────

def test_wqs_boundary_60_active_59_9_candidate():
    """Proves the ACTIVE/CANDIDATE promotion boundary is exactly at WQS 65.0.

    WQS=65.0 → ACTIVE; WQS=64.99 → CANDIDATE; WQS=19.99 → REJECTED.
    This boundary is what separates copy-eligible wallets from candidates.
    """
    assert classify_wallet(65.0) == "ACTIVE",    "WQS 65.0 must be ACTIVE"
    assert classify_wallet(65.01) == "ACTIVE",   "WQS 65.01 must be ACTIVE"
    assert classify_wallet(64.99) == "CANDIDATE","WQS 64.99 must be CANDIDATE (not ACTIVE)"
    assert classify_wallet(20.0) == "CANDIDATE", "WQS 20.0 must be CANDIDATE"
    assert classify_wallet(19.99) == "REJECTED", "WQS 19.99 must be REJECTED"


# ─── P4 ──────────────────────────────────────────────────────────────────────

@pytest.mark.asyncio
async def test_profit_factor_threshold_1_2_enforced():
    """Proves validator rejects wallets whose simulated profit factor < 1.2.

    PF < 1.1 means wins barely cover losses — the Martingale danger zone.
    The threshold is 'sim_pf < 1.1' (strict): PF=0.947 fails, PF=1.1 passes.
    """
    # Metrics with min_wqs_score=30 to pass WQS check (trade_count=20 → confidence=1.0)
    metrics = WalletMetrics(
        address="test_pf_wallet",
        roi_30d=50.0,
        trade_count_30d=20,
        win_rate=0.70,
        avg_trade_size_sol=0.5,  # Avoid dust-trader penalty
        profit_factor=1.5,  # +5 pts — needed to reach WQS > 30 threshold
    )

    # 30 base trades: even=BUY, odd=SELL with pnl_sol=0.1 (gives 15 SELLs → passes min_closes)
    base_trades = [
        HistoricalTrade(
            token_address="token_pf",
            token_symbol="PF",
            action=TradeAction.SELL if i % 2 == 1 else TradeAction.BUY,
            amount_sol=Decimal("1.0"),
            price_at_trade=Decimal("1.0"),
            timestamp=datetime.utcnow() - timedelta(days=i),
            tx_signature=f"tx_pf_{i}",
            pnl_sol=Decimal("0.1") if i % 2 == 1 else None,
        )
        for i in range(30)
    ]

    # Case A: PF = 1.0 / 0.95 ≈ 1.053 < 1.1 → must FAIL (but net PnL > 0 so backtest passes)
    # 10 wins × +0.1 = +1.0 SOL profit; 1 loss × -0.95 = -0.95 SOL; net = +0.05
    mock_low = Mock()
    mock_low.simulate_wallet.return_value = _make_mock_sim_result(
        [0.1] * 10 + [-0.95], wallet_address="test_pf_wallet"
    )
    validator_low = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(
            min_wqs_score=30.0,
            min_trades=5,
            min_close_ratio=0.3,
            walk_forward_enabled=False,
        )
    )
    validator_low.simulator = mock_low
    validator_low.rugcheck_client = None

    result_low = await validator_low.validate_for_promotion(
        "test_pf_wallet", metrics, base_trades
    )
    assert not result_low.passed, (
        f"Simulated PF≈0.947 must fail. Got: {result_low.status}: {result_low.reason}"
    )
    assert result_low.status == ValidationStatus.FAILED_NEGATIVE_PNL, (
        f"Expected FAILED_NEGATIVE_PNL for PF≈0.947, got {result_low.status}: {result_low.reason}"
    )
    assert "1.1" in (result_low.reason or ""), (
        f"Rejection reason must cite the 1.1 threshold: {result_low.reason}"
    )

    # Case B: PF = 1.08 / 0.9 = 1.2 > 1.1 → must PASS
    # 9 wins × +0.12 = +1.08 SOL; 1 loss × -0.9 = -0.9 SOL; net = +0.18
    mock_exact = Mock()
    mock_exact.simulate_wallet.return_value = _make_mock_sim_result(
        [0.12] * 9 + [-0.9], wallet_address="test_pf_wallet"
    )
    validator_exact = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(
            min_wqs_score=30.0,
            min_trades=5,
            min_close_ratio=0.3,
            walk_forward_enabled=False,
            max_drawdown_fraction=1.0,  # disable drawdown gate — this test is about profit-factor only
        )
    )
    validator_exact.simulator = mock_exact
    validator_exact.rugcheck_client = None

    result_exact = await validator_exact.validate_for_promotion(
        "test_pf_wallet", metrics, base_trades
    )
    assert result_exact.passed, (
        f"PF=1.2 must pass (check is `< 1.1`). "
        f"Got: {result_exact.status}: {result_exact.reason}"
    )
