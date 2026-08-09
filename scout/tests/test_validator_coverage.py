"""
Coverage-completion tests for core/validator.py.

Targets the remaining uncovered branches: archetype thresholds, momentum
boost, gate-admission feedback, RugCheck filtering paths, walk-forward
fallback, in-sample gate, backtest error paths, drawdown/concentration
gates, quick_check, failure-status mapping, and the import fallback.
"""

import builtins
import importlib
import sys
from datetime import datetime, timedelta
from decimal import Decimal
from unittest.mock import AsyncMock, Mock

import pytest

from core.validator import (
    PrePromotionValidator,
    PromotionCriteria,
    validate_wallet_for_promotion,
)
from core.wqs import WalletMetrics, WqsResult
from core.models import (
    HistoricalTrade,
    TradeAction,
    SimulatedResult,
    SimulatedTrade,
    ValidationStatus,
)
from core.liquidity import LiquidityProvider


def _trades(n, start=None, with_pnl=True):
    """Alternating BUY/SELL trades; default span ~n days (date-based split)."""
    start = start or (datetime.utcnow() - timedelta(days=n))
    trades = []
    for i in range(n):
        is_sell = i % 2 == 1
        trades.append(HistoricalTrade(
            token_address=f"tok{i % 3}",
            token_symbol=f"TOK{i % 3}",
            action=TradeAction.SELL if is_sell else TradeAction.BUY,
            amount_sol=Decimal("1.0"),
            price_at_trade=Decimal("0.5"),
            timestamp=start + timedelta(days=i),
            tx_signature=f"tx{i}",
            pnl_sol=Decimal("0.05") if (is_sell and with_pnl) else None,
            token_amount=Decimal("100"),
            sol_amount=Decimal("1.0"),
        ))
    return trades


def _metrics(**kw):
    base = dict(
        address="cov_valid",
        roi_7d=10.0,
        roi_30d=30.0,
        trade_count_30d=20,
        win_rate=0.6,
        max_drawdown_30d=5.0,
        avg_trade_size_sol=Decimal("0.5"),
        profit_factor=1.5,
        avg_hold_time_hours=5.0,
    )
    base.update(kw)
    return WalletMetrics(**base)


def _sim_result(pnl_list=None, original_pnl=None, trades=None, **kw):
    pnl_list = pnl_list if pnl_list is not None else [0.5] * 6
    dummy = HistoricalTrade(
        token_address="tok0",
        token_symbol="TOK0",
        action=TradeAction.SELL,
        amount_sol=Decimal("1.0"),
        price_at_trade=Decimal("1.0"),
        timestamp=datetime.utcnow(),
        tx_signature="sig_sim",
        pnl_sol=None,  # None -> skipped by the token-concentration gate (6d)
    )
    sim_trades = trades
    if sim_trades is None:
        sim_trades = [
            SimulatedTrade(
                original_trade=dummy,
                current_liquidity_usd=Decimal("50000"),
                liquidity_sufficient=True,
                estimated_slippage_percent=Decimal("0.001"),
                slippage_cost_sol=Decimal("0.001"),
                fee_cost_sol=Decimal("0.001"),
                simulated_pnl_sol=Decimal(str(p)),
                rejected=False,
            )
            for p in pnl_list
        ]
    total = sum((Decimal(str(p)) for p in pnl_list), Decimal("0"))
    defaults = dict(
        wallet_address="cov_valid",
        total_trades=len(sim_trades),
        simulated_trades=len(sim_trades),
        rejected_trades=0,
        original_pnl_sol=original_pnl if original_pnl is not None else total,
        simulated_pnl_sol=total,
        pnl_difference_sol=Decimal("0.01"),
        total_slippage_cost_sol=Decimal("0.01"),
        total_fee_cost_sol=Decimal("0.01"),
        trades=sim_trades,
        passed=total > 0,
        failure_reason=None if total > 0 else "Negative PnL",
    )
    defaults.update(kw)
    return SimulatedResult(**defaults)


def _make_validator(monkeypatch=None, wqs_score=76.0, confidence=0.9, criteria=None):
    validator = PrePromotionValidator(
        promotion_criteria=criteria or PromotionCriteria(min_wqs_score=70.0),
    )
    validator.rugcheck_client = None
    if monkeypatch is not None:

        def fake_wqs(metrics, strategy="SHIELD"):
            # Mirror the real gate-admission instant reject so admission tests
            # behave like production without the full WQS pipeline.
            if (metrics.operator_decision_count is not None
                    and metrics.operator_decision_count >= 10
                    and metrics.operator_admission_rate is not None
                    and metrics.operator_admission_rate <= 0.0):
                return WqsResult(score=0.0, confidence=0.0, adjusted_score=0.0)
            return WqsResult(score=wqs_score, confidence=confidence,
                             adjusted_score=wqs_score * confidence)

        monkeypatch.setattr("core.validator.calculate_wqs_with_confidence", fake_wqs)
    return validator


def _with_sim(validator, results, sim_trades=None):
    """Attach a Mock simulator returning `results` (list = sequential calls)."""
    sim = Mock()
    if isinstance(results, list):
        sim.simulate_wallet.side_effect = results
    else:
        sim.simulate_wallet.return_value = results
    validator.simulator = sim
    return sim


def _all_sell_trades(n, start=None):
    """All-SELL trades so walk-forward holdouts carry >= 5 closes."""
    start = start or (datetime.utcnow() - timedelta(days=n))
    return [
        HistoricalTrade(
            token_address=f"tok{i % 3}",
            token_symbol=f"TOK{i % 3}",
            action=TradeAction.SELL,
            amount_sol=Decimal("1.0"),
            price_at_trade=Decimal("0.5"),
            timestamp=start + timedelta(days=i),
            tx_signature=f"tx{i}",
            pnl_sol=Decimal("0.05"),
            token_amount=Decimal("100"),
            sol_amount=Decimal("1.0"),
        )
        for i in range(n)
    ]


class TestThresholdHelpers:
    def test_get_archetype_threshold(self):
        v = _make_validator()
        c = v.criteria
        assert v._get_archetype_threshold(None) == c.min_wqs_score
        assert v._get_archetype_threshold("WHALE") == c.min_wqs_whale
        assert v._get_archetype_threshold("SWING") == c.min_wqs_swing
        assert v._get_archetype_threshold("SCALPER") == c.min_wqs_score
        assert v._get_archetype_threshold("SNIPER") == c.min_wqs_score
        assert v._get_archetype_threshold("unknown_arch") == c.min_wqs_score

    def test_get_close_ratio_threshold(self):
        v = _make_validator()
        c = v.criteria
        assert v._get_close_ratio_threshold(None) == c.min_close_ratio
        assert v._get_close_ratio_threshold("WHALE") == c.min_close_ratio_whale
        assert v._get_close_ratio_threshold("SWING") == c.min_close_ratio_swing
        assert v._get_close_ratio_threshold("SCALPER") == c.min_close_ratio_scalper
        assert v._get_close_ratio_threshold("SNIPER") == c.min_close_ratio

    def test_apply_momentum_boost(self):
        v = _make_validator()
        assert v._apply_momentum_boost(70.0, "IMPROVING") == 75.0
        assert v._apply_momentum_boost(70.0, "STABLE") == 70.0
        assert v._apply_momentum_boost(70.0, None) == 70.0


class TestArchetypeGate:
    def test_tg_bot_user_blocked(self):
        v = _make_validator()
        result = v.validate_archetype_for_promotion(
            "w1", _metrics(is_tg_bot_user=True)
        )
        assert not result.passed
        assert "Telegram bot" in result.reason

    def test_low_churn_disabled(self):
        v = PrePromotionValidator(promotion_criteria=PromotionCriteria(
            enforce_low_churn=False,
        ))
        result = v.validate_archetype_for_promotion("w1", _metrics(archetype="SNIPER"))
        assert result.passed

    def test_forbidden_archetype(self):
        v = _make_validator()
        result = v.validate_archetype_for_promotion("w1", _metrics(archetype="SNIPER"))
        assert not result.passed
        assert "excluded" in result.reason

    def test_short_hold_time(self):
        v = _make_validator()
        result = v.validate_archetype_for_promotion(
            "w1", _metrics(archetype="SWING", avg_hold_time_hours=1.0)
        )
        assert not result.passed
        assert "Avg hold time" in result.reason

    def test_pass(self):
        v = _make_validator()
        result = v.validate_archetype_for_promotion(
            "w1", _metrics(archetype="SWING", avg_hold_time_hours=5.0)
        )
        assert result.passed


class TestGateAdmission:
    @pytest.mark.asyncio
    async def test_zero_admission_instant_reject(self, monkeypatch):
        monkeypatch.setattr(
            "core.db.execute_and_fetchone",
            lambda sql, params: {"total": 10, "admitted": 0},
        )
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        result = await v.validate_for_promotion("w_adm", _metrics(), _trades(6))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_WQS

    @pytest.mark.asyncio
    async def test_no_decision_records(self, monkeypatch):
        monkeypatch.setattr(
            "core.db.execute_and_fetchone",
            lambda sql, params: {"total": 0, "admitted": 0},
        )
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        _with_sim(v, _sim_result([0.5] * 7, passed=True, failure_reason=None))
        result = await v.validate_for_promotion("w_adm", _metrics(), _trades(6))
        assert result.passed

    @pytest.mark.asyncio
    async def test_admission_lookup_exception(self, monkeypatch):
        def boom(sql, params):
            raise RuntimeError("db down")

        monkeypatch.setattr("core.db.execute_and_fetchone", boom)
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        _with_sim(v, _sim_result([0.5] * 7, passed=True, failure_reason=None))
        result = await v.validate_for_promotion("w_adm", _metrics(), _trades(6))
        assert result.passed


class TestWqsGate:
    @pytest.mark.asyncio
    async def test_below_threshold_fails(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=50.0, confidence=0.9)
        result = await v.validate_for_promotion("w1", _metrics(), _trades(6))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_WQS
        assert "WQS" in result.reason

    @pytest.mark.asyncio
    async def test_low_confidence_fails(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=90.0, confidence=0.3)
        v.criteria.min_confidence = 0.5
        result = await v.validate_for_promotion("w1", _metrics(), _trades(6))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_WQS
        assert "confidence" in result.reason

    @pytest.mark.asyncio
    async def test_momentum_boost_logs(self, monkeypatch, caplog):
        caplog.set_level(10, logger="core.validator")
        v = _make_validator(monkeypatch, wqs_score=74.0, confidence=0.9)
        v.criteria.min_wqs_score = 70.0
        _with_sim(v, _sim_result([0.5] * 7, passed=True, failure_reason=None))
        metrics = _metrics(archetype="SWING", trajectory="IMPROVING")
        result = await v.validate_for_promotion("w1", metrics, _trades(6))
        assert result.passed
        assert any("archetype=SWING" in r.message for r in caplog.records)


class TestRugCheckGates:
    @pytest.mark.asyncio
    async def test_high_risky_ratio_rejected(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        rug = AsyncMock()
        rug.is_token_safe.side_effect = [True] * 6 + [False] * 4
        v.rugcheck_client = rug
        result = await v.validate_for_promotion("w1", _metrics(), _trades(10))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_LIQUIDITY
        assert result.recommended_status == "REJECTED"

    @pytest.mark.asyncio
    async def test_all_safe_tokens(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        rug = AsyncMock()
        rug.is_token_safe.side_effect = [True] * 6
        v.rugcheck_client = rug
        _with_sim(v, _sim_result([0.5] * 7, passed=True, failure_reason=None))
        result = await v.validate_for_promotion("w1", _metrics(), _trades(6))
        assert result.passed

    @pytest.mark.asyncio
    async def test_fast_track_revoked_after_filter(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=85.0, confidence=0.9)
        v.criteria.min_wqs_score = 10.0
        v.criteria.min_confidence = 0.1
        v.criteria.min_trades = 8
        v.criteria.fast_track_wqs_threshold = 80.0
        v.criteria.fast_track_min_trades = 8
        v.criteria.enforce_low_churn = False
        rug = AsyncMock()
        rug.is_token_safe.side_effect = [True] * 7 + [False] * 3
        v.rugcheck_client = rug
        result = await v.validate_for_promotion("w1", _metrics(), _trades(10))
        # fast-track revoked (7 safe < 8) and safe trades < min_trades -> reject
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_INSUFFICIENT_TRADES


class TestWalkForward:
    def _wf_validator(self, monkeypatch, criteria=None):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9,
                            criteria=criteria)
        return v

    @pytest.mark.asyncio
    async def test_walk_forward_success(self, monkeypatch):
        v = self._wf_validator(monkeypatch)
        sim = Mock()
        _with_sim(v, [
            _sim_result([0.5] * 7, passed=True, failure_reason=None),  # in-sample
            _sim_result([0.5] * 7, passed=True, failure_reason=None),  # holdout
        ])
        trades = _all_sell_trades(20)
        result = await v.validate_for_promotion("w1", _metrics(), trades)
        assert result.passed
        assert v.simulator.simulate_wallet.call_count == 2

    @pytest.mark.asyncio
    async def test_in_sample_fails(self, monkeypatch):
        v = self._wf_validator(monkeypatch)
        _with_sim(v, _sim_result(
            [-0.5] * 6, passed=False, failure_reason="Trades rejected: liquidity",
        ))
        result = await v.validate_for_promotion("w1", _metrics(), _all_sell_trades(20))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_LIQUIDITY
        assert "in-sample" in result.reason.lower()

    @pytest.mark.asyncio
    async def test_in_sample_exception(self, monkeypatch):
        v = self._wf_validator(monkeypatch)
        _with_sim(v, RuntimeError("simulator crash"))
        result = await v.validate_for_promotion("w1", _metrics(), _all_sell_trades(20))
        assert not result.passed
        assert result.status == ValidationStatus.ERROR
        assert "In-sample validation error" in result.reason

    @pytest.mark.asyncio
    async def test_count_based_split(self, monkeypatch):
        v = self._wf_validator(monkeypatch)
        _with_sim(v, [
            _sim_result([0.5] * 6, passed=True, failure_reason=None),
            _sim_result([0.5] * 6, passed=True, failure_reason=None),
        ])
        # Same-day trades (span < 7 days) force the count-based split
        trades = _all_sell_trades(24, start=datetime.utcnow() - timedelta(hours=12))
        result = await v.validate_for_promotion("w1", _metrics(), trades)
        assert result.passed

    @pytest.mark.asyncio
    async def test_wf_fallback_wqs_penalty_fails(self, monkeypatch):
        v = self._wf_validator(monkeypatch, criteria=PromotionCriteria(
            min_wqs_score=75.0, walk_forward_min_trades=5,
        ))
        v.simulator = Mock()
        # Alternating trades: holdout (last ~30%) has < 5 closes -> fallback
        trades = _trades(20)
        result = await v.validate_for_promotion("w1", _metrics(), trades)
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_WQS
        assert "penalty applied" in result.reason

    @pytest.mark.asyncio
    async def test_wf_fallback_wqs_penalty_passes(self, monkeypatch):
        v = self._wf_validator(monkeypatch, criteria=PromotionCriteria(
            min_wqs_score=70.0, walk_forward_min_trades=5,
        ))
        _with_sim(v, _sim_result([0.5] * 6, passed=True, failure_reason=None))
        trades = _trades(20)
        result = await v.validate_for_promotion("w1", _metrics(), trades)
        assert result.passed
        assert "Walk-forward skipped" in result.notes


class TestBacktestGates:
    @pytest.mark.asyncio
    async def test_backtest_exception(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.walk_forward_enabled = False
        sim = Mock()
        sim.simulate_wallet.side_effect = RuntimeError("backtest crashed")
        v.simulator = sim
        result = await v.validate_for_promotion("w1", _metrics(), _trades(6))
        assert not result.passed
        assert result.status == ValidationStatus.ERROR
        assert "Backtest error" in result.reason

    @pytest.mark.asyncio
    async def test_backtest_failed_result(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.walk_forward_enabled = False
        sim = Mock()
        sim.simulate_wallet.return_value = _sim_result(
            [-0.5] * 6, passed=False, failure_reason="Liquidity insufficient",
        )
        v.simulator = sim
        result = await v.validate_for_promotion("w1", _metrics(), _trades(6))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_LIQUIDITY
        assert result.notes

    @pytest.mark.asyncio
    async def test_holdout_pnl_below_minimum(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.min_holdout_pnl_sol = 0.01
        _with_sim(v, [
            _sim_result([0.5] * 7, passed=True, failure_reason=None),  # in-sample
            _sim_result([0.5] * 7, original_pnl=Decimal("0.0"),
                        passed=True, failure_reason=None),  # holdout
        ])
        result = await v.validate_for_promotion("w1", _metrics(), _all_sell_trades(20))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_NEGATIVE_PNL
        assert "holdout PnL" in result.reason

    @pytest.mark.asyncio
    async def test_drawdown_gate_fails_10_19_trades(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.walk_forward_enabled = False
        v.criteria.max_drawdown_fraction = 0.5
        sim = Mock()
        # 10 trades: pf = 5/4.5 = 1.11 >= 1.1; equity peak 5, dd 4.5
        # multiplier 1.5 -> threshold = 5*0.75 = 3.75 -> 4.5 > 3.75 FAIL
        sim.simulate_wallet.return_value = _sim_result(
            [1.0] * 5 + [-0.9] * 5, passed=True, failure_reason=None,
        )
        v.simulator = sim
        result = await v.validate_for_promotion("w1", _metrics(), _trades(10))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_NEGATIVE_PNL
        assert "drawdown" in result.reason.lower()

    @pytest.mark.asyncio
    async def test_drawdown_gate_passes_under_10_trades(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.walk_forward_enabled = False
        # 8 trades with a deep mid-dip: dd == total_pos (2.0x multiplier
        # threshold is total_pos, so the gate passes for < 10 trades).
        sim = Mock()
        sim.simulate_wallet.return_value = _sim_result(
            [1.0] * 4 + [-4.0] + [1.0] * 3, passed=True, failure_reason=None,
        )
        v.simulator = sim
        result = await v.validate_for_promotion("w1", _metrics(), _trades(8))
        assert result.passed

    @pytest.mark.asyncio
    async def test_token_concentration_fails(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.walk_forward_enabled = False
        v.criteria.max_drawdown_fraction = 1.0  # disable drawdown gate
        dummy_trades = []
        for i in range(10):
            is_sell = i % 2 == 1
            dummy_trades.append(HistoricalTrade(
                token_address="tokA" if is_sell else "tokB",
                token_symbol="A" if is_sell else "B",
                action=TradeAction.SELL if is_sell else TradeAction.BUY,
                amount_sol=Decimal("1.0"),
                price_at_trade=Decimal("1.0"),
                timestamp=datetime.utcnow() - timedelta(days=i),
                tx_signature=f"stx{i}",
                pnl_sol=Decimal("1.0") if is_sell else None,
            ))
        sim_trades = [
            SimulatedTrade(
                original_trade=dt,
                current_liquidity_usd=Decimal("50000"),
                liquidity_sufficient=True,
                estimated_slippage_percent=Decimal("0.001"),
                slippage_cost_sol=Decimal("0.001"),
                fee_cost_sol=Decimal("0.001"),
                simulated_pnl_sol=Decimal("1.0") if i % 2 == 1 else Decimal("-0.1"),
                rejected=False,
            )
            for i, dt in enumerate(dummy_trades)
        ]
        sim = Mock()
        sim.simulate_wallet.return_value = _sim_result(
            [1.0 if i % 2 == 1 else -0.1 for i in range(10)],
            trades=sim_trades, passed=True, failure_reason=None,
        )
        v.simulator = sim
        result = await v.validate_for_promotion("w1", _metrics(), _trades(10))
        assert not result.passed
        assert "concentration" in result.reason.lower()

    @pytest.mark.asyncio
    async def test_token_concentration_passes(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.walk_forward_enabled = False
        v.criteria.max_drawdown_fraction = 1.0
        dummy_trades = []
        for i in range(10):
            dummy_trades.append(HistoricalTrade(
                token_address=f"tok{i % 5}",
                token_symbol=f"T{i % 5}",
                action=TradeAction.SELL,
                amount_sol=Decimal("1.0"),
                price_at_trade=Decimal("1.0"),
                timestamp=datetime.utcnow() - timedelta(days=i),
                tx_signature=f"stx{i}",
                pnl_sol=Decimal("0.5"),
            ))
        sim_trades = [
            SimulatedTrade(
                original_trade=dt,
                current_liquidity_usd=Decimal("50000"),
                liquidity_sufficient=True,
                estimated_slippage_percent=Decimal("0.001"),
                slippage_cost_sol=Decimal("0.001"),
                fee_cost_sol=Decimal("0.001"),
                simulated_pnl_sol=Decimal("0.5"),
                rejected=False,
            )
            for dt in dummy_trades
        ]
        sim = Mock()
        sim.simulate_wallet.return_value = _sim_result(
            [0.5] * 10, trades=sim_trades, passed=True, failure_reason=None,
        )
        v.simulator = sim
        result = await v.validate_for_promotion("w1", _metrics(), _trades(10))
        assert result.passed

    @pytest.mark.asyncio
    async def test_bull_regime_logs(self, monkeypatch, caplog):
        caplog.set_level(10, logger="core.validator")
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.walk_forward_enabled = False
        _with_sim(v, _sim_result(
            [0.5] * 6, passed=True, failure_reason=None, regime_risk="BULL",
        ))
        result = await v.validate_for_promotion("w1", _metrics(), _trades(6))
        assert result.passed
        assert any("BULL regime" in r.message for r in caplog.records)


class TestQuickCheck:
    def test_forbidden_archetype(self):
        v = _make_validator(criteria=PromotionCriteria(
            enforce_low_churn=True, forbidden_archetypes={"SNIPER"},
        ))
        assert not v.quick_check(_metrics(archetype="SNIPER"), trade_count=30)

    def test_short_hold_time(self):
        v = _make_validator(criteria=PromotionCriteria(
            enforce_low_churn=True, min_avg_hold_time_hours=2.0,
        ))
        assert not v.quick_check(
            _metrics(archetype="SWING", avg_hold_time_hours=1.0), trade_count=30
        )

    def test_low_wqs(self):
        v = _make_validator(criteria=PromotionCriteria(
            enforce_low_churn=False, min_wqs_score=80.0,
        ))
        assert not v.quick_check(_metrics(), trade_count=30)

    def test_low_trade_count(self):
        v = _make_validator(criteria=PromotionCriteria(
            enforce_low_churn=False, min_wqs_score=1.0, min_trades=10,
        ))
        assert not v.quick_check(_metrics(), trade_count=3)

    def test_passes(self):
        v = _make_validator(criteria=PromotionCriteria(
            enforce_low_churn=False, min_wqs_score=1.0, min_trades=1,
        ))
        assert v.quick_check(_metrics(), trade_count=30)


class TestFailureStatus:
    def test_all_branches(self):
        v = _make_validator()
        assert v._determine_failure_status(None) == ValidationStatus.ERROR
        assert v._determine_failure_status("WQS score below threshold") == ValidationStatus.FAILED_WQS
        assert v._determine_failure_status("score too low") == ValidationStatus.FAILED_WQS
        assert v._determine_failure_status("Trades rejected: liquidity") == ValidationStatus.FAILED_LIQUIDITY
        assert v._determine_failure_status("high rejection rate") == ValidationStatus.FAILED_LIQUIDITY
        assert v._determine_failure_status("Liquidity insufficient") == ValidationStatus.FAILED_LIQUIDITY
        assert v._determine_failure_status("Slippage too high") == ValidationStatus.FAILED_SLIPPAGE
        assert v._determine_failure_status("Negative PnL") == ValidationStatus.FAILED_NEGATIVE_PNL
        assert v._determine_failure_status("pnl below zero") == ValidationStatus.FAILED_NEGATIVE_PNL
        assert v._determine_failure_status("Insufficient trades") == ValidationStatus.FAILED_INSUFFICIENT_TRADES
        assert v._determine_failure_status("only 2 trades") == ValidationStatus.FAILED_INSUFFICIENT_TRADES
        assert v._determine_failure_status("mystery failure") == ValidationStatus.ERROR


class TestFormatNotes:
    def test_backtest_notes_with_rejections(self):
        v = _make_validator()
        result = _sim_result([0.5] * 6)
        result.rejected_trade_details = ["liquidity", "slippage", "fee", "other"]
        notes = v._format_backtest_notes(result)
        assert "Rejections:" in notes
        assert "liquidity" in notes

    def test_backtest_notes_without_rejections(self):
        v = _make_validator()
        notes = v._format_backtest_notes(_sim_result([0.5] * 6))
        assert "Original PnL" in notes


class TestRugCheckInitFailure:
    def test_client_init_exception(self, monkeypatch):
        class BrokenRugCheck:
            def __init__(self):
                raise RuntimeError("no API")

        monkeypatch.setattr("core.validator.ScoutConfig.get_rugcheck_enabled",
                            lambda: True)
        monkeypatch.setattr("core.validator.RugCheckClient", BrokenRugCheck)
        validator = PrePromotionValidator()
        assert validator.rugcheck_client is None


@pytest.mark.asyncio
async def test_validate_wallet_for_promotion_convenience():
    result = await validate_wallet_for_promotion(
        "conv_wallet",
        WalletMetrics(address="conv_wallet", roi_30d=-20.0),
        [],
    )
    assert not result.passed


class TestImportFallback:
    def test_security_import_guards(self, monkeypatch):
        import core.validator as v

        real_import = builtins.__import__

        def blocked(name, *args, **kwargs):
            if name == "config":
                raise ImportError("blocked for test")
            return real_import(name, *args, **kwargs)

        monkeypatch.setattr(builtins, "__import__", blocked)
        try:
            importlib.reload(v)
            assert v.SECURITY_AVAILABLE is False
            assert v.ScoutConfig is None
            assert v.RugCheckClient is None
            validator = v.PrePromotionValidator()
            assert validator.rugcheck_client is None
        finally:
            monkeypatch.setattr(builtins, "__import__", real_import)
            importlib.reload(v)
            assert v.SECURITY_AVAILABLE is True


class TestRemainingGates:
    @pytest.mark.asyncio
    async def test_low_churn_fail_logged(self, monkeypatch, caplog):
        caplog.set_level(10, logger="core.validator")
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        metrics = _metrics(archetype="SNIPER")
        result = await v.validate_for_promotion("w1", metrics, _trades(6))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_WQS
        assert any("gate=low_churn" in r.message and "FAIL" in r.message
                   for r in caplog.records)

    @pytest.mark.asyncio
    async def test_min_trades_gate_fails(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.min_trades = 10
        result = await v.validate_for_promotion("w1", _metrics(), _trades(6))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_INSUFFICIENT_TRADES
        assert "Insufficient trades" in result.reason

    @pytest.mark.asyncio
    async def test_fast_track_success(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=85.0, confidence=0.9)
        v.criteria.min_wqs_score = 10.0
        v.criteria.min_confidence = 0.1
        v.criteria.fast_track_wqs_threshold = 80.0
        result = await v.validate_for_promotion("w1", _metrics(), _trades(6))
        assert result.passed
        assert "Fast-track" in result.reason
        assert result.recommended_status == "ACTIVE"

    @pytest.mark.asyncio
    async def test_close_ratio_gate_fails(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.fast_track_wqs_threshold = 90.0
        all_buys = [
            HistoricalTrade(
                token_address=f"tok{i}",
                token_symbol=f"TOK{i}",
                action=TradeAction.BUY,
                amount_sol=Decimal("1.0"),
                price_at_trade=Decimal("0.5"),
                timestamp=datetime.utcnow() - timedelta(days=i),
                tx_signature=f"buy{i}",
                token_amount=Decimal("100"),
                sol_amount=Decimal("1.0"),
            )
            for i in range(6)
        ]
        result = await v.validate_for_promotion("w1", _metrics(), all_buys)
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_INSUFFICIENT_TRADES
        assert "realized closes" in result.reason

    @pytest.mark.asyncio
    async def test_rejection_rate_gate_fails(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.walk_forward_enabled = False
        _with_sim(v, _sim_result(
            [0.5] * 4, passed=True, failure_reason=None,
            total_trades=10, simulated_trades=4, rejected_trades=6,
        ))
        result = await v.validate_for_promotion("w1", _metrics(), _trades(6))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_LIQUIDITY
        assert "rejected" in result.reason

    @pytest.mark.asyncio
    async def test_sim_profit_factor_gate_fails(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.walk_forward_enabled = False
        _with_sim(v, _sim_result(
            [0.5] * 6 + [-1.0] * 3, passed=True, failure_reason=None,
        ))
        result = await v.validate_for_promotion("w1", _metrics(), _trades(9))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_NEGATIVE_PNL
        assert "Profit Factor" in result.reason

    @pytest.mark.asyncio
    async def test_negative_simulated_pnl_fails(self, monkeypatch):
        v = _make_validator(monkeypatch, wqs_score=76.0, confidence=0.9)
        v.criteria.walk_forward_enabled = False
        _with_sim(v, _sim_result(
            [-0.5] * 6, passed=True, failure_reason=None, trades=[],
            total_trades=6, simulated_trades=6, rejected_trades=0,
        ))
        result = await v.validate_for_promotion("w1", _metrics(), _trades(6))
        assert not result.passed
        assert result.status == ValidationStatus.FAILED_NEGATIVE_PNL
        assert "Negative simulated PnL" in result.reason
