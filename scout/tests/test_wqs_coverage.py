"""
Coverage-completion tests for core/wqs.py.

Targets the remaining uncovered branches: helper functions, ScoreTracker
precedence/confidence, archetype adjustments, advanced penalties, recency
bands, and confidence computation edge cases.
"""

import os
import sys
import json
from datetime import datetime, timedelta, timezone
from decimal import Decimal

import pytest

from core.wqs import (
    ScoreTracker,
    PenaltyCategory,
    _interpret_trajectory,
    _detect_smart_accumulation,
    _detect_market_regime,
    _apply_archetype_adjustments,
    _get_current_weights,
    _compute_wmi,
    _compute_confidence,
    _calculate_raw_score,
    calculate_wqs,
    calculate_wqs_with_confidence,
    classify_wallet,
    WalletMetrics,
    RawScoreComponents,
)


def _metrics(**kw):
    base = dict(
        address="cov_wallet",
        roi_7d=None,
        roi_30d=None,
        trade_count_30d=None,
        win_rate=None,
        max_drawdown_30d=0.0,
        avg_trade_size_sol=Decimal("0.5"),
        last_trade_at=None,
        win_streak_consistency=None,
        avg_entry_delay_seconds=None,
        profit_factor=None,
    )
    base.update(kw)
    return WalletMetrics(**base)


def _ts(days_ago):
    return (datetime.now(timezone.utc) - timedelta(days=days_ago)).isoformat()


class TestTrajectory:
    def test_none_roi(self):
        assert _interpret_trajectory(None, None) == "STABLE"
        assert _interpret_trajectory(None, 10.0) == "STABLE"
        assert _interpret_trajectory(10.0, None) == "STABLE"

    def test_peaked(self):
        assert _interpret_trajectory(1.0, 50.0) == "PEAKED"

    def test_declining(self):
        assert _interpret_trajectory(10.0, 40.0) == "DECLINING"

    def test_improving(self):
        assert _interpret_trajectory(30.0, 40.0) == "IMPROVING"

    def test_stable(self):
        assert _interpret_trajectory(1.0, 1.0) == "STABLE"


class TestSmartAccumulation:
    def test_no_trade_sizes(self):
        assert _detect_smart_accumulation(_metrics()) == 0.0

    def test_short_history(self):
        m = _metrics(trade_sizes=[1.0, 2.0])
        assert _detect_smart_accumulation(m) == 0.0

    def test_growing_trend_positive_roi(self):
        m = _metrics(roi_7d=10.0, trade_sizes=[1.0, 1.5, 2.0, 2.5, 3.0, 3.5])
        score = _detect_smart_accumulation(m)
        assert score > 0.0

    def test_high_variance_penalty(self):
        m = _metrics(roi_7d=10.0, trade_sizes=[1.0, 100.0, 1.0, 90.0, 1.0, 80.0])
        score = _detect_smart_accumulation(m)
        assert 0.0 <= score <= 1.0

    def test_losing_wallet_trending_up(self):
        m = _metrics(roi_7d=-5.0, trade_sizes=[1.0, 1.5, 2.0, 2.5, 3.0, 3.5])
        score = _detect_smart_accumulation(m)
        assert 0.0 <= score <= 1.0

    def test_clamped_zero(self):
        m = _metrics(roi_7d=-5.0, trade_sizes=[5.0, 1.0, 4.0, 1.0, 4.0, 1.0])
        assert 0.0 <= _detect_smart_accumulation(m) <= 1.0


class TestMarketRegime:
    def test_no_roi_attrs(self):
        class Bare:
            pass

        assert _detect_market_regime(Bare()) == "NEUTRAL"

    def test_bull(self):
        m = _metrics(roi_7d=25.0, roi_30d=30.0, volatility_30d=10.0)
        assert _detect_market_regime(m) == "BULL"

    def test_bear_negative_30d(self):
        assert _detect_market_regime(_metrics(roi_7d=5.0, roi_30d=-15.0)) == "BEAR"

    def test_bear_mixed(self):
        assert _detect_market_regime(_metrics(roi_7d=-5.0, roi_30d=2.0)) == "BEAR"

    def test_volatile_high_volatility(self):
        m = _metrics(roi_7d=30.0, roi_30d=5.0, volatility_30d=60.0)
        assert _detect_market_regime(m) == "VOLATILE"

    def test_volatile_fading(self):
        assert _detect_market_regime(_metrics(roi_7d=1.0, roi_30d=30.0)) == "VOLATILE"

    def test_bull_recovery(self):
        assert _detect_market_regime(_metrics(roi_7d=15.0, roi_30d=-5.0)) == "BULL"

    def test_neutral(self):
        assert _detect_market_regime(_metrics(roi_7d=2.0, roi_30d=5.0)) == "NEUTRAL"


class TestArchetypeAdjustments:
    def _tracker(self):
        t = ScoreTracker()
        t.positive = 0.0
        return t

    def test_scalper_volatile_bonus(self):
        t = self._tracker()
        _apply_archetype_adjustments(
            t, _metrics(avg_hold_time_hours=0.5, trade_count_30d=150, roi_7d=30.0,
                        roi_30d=5.0, volatility_30d=60.0), "VOLATILE"
        )
        assert t.components.get("regime_adjustment", 0) == 5.0

    def test_swing_volatile_bonus(self):
        t = self._tracker()
        _apply_archetype_adjustments(
            t, _metrics(avg_hold_time_hours=48.0, trade_count_30d=30, roi_7d=1.0,
                        roi_30d=30.0, volatility_30d=60.0), "VOLATILE"
        )
        assert t.components.get("regime_adjustment", 0) == 3.0

    def test_whale_volatile_penalty(self):
        t = self._tracker()
        _apply_archetype_adjustments(
            t, _metrics(avg_hold_time_hours=5.0, trade_count_30d=10,
                        avg_trade_size_sol=Decimal("50.0"), roi_7d=1.0,
                        roi_30d=30.0), "VOLATILE"
        )
        assert t.components.get("regime_adjustment", 0) == -3.0

    def test_swing_bull_bonus(self):
        t = self._tracker()
        _apply_archetype_adjustments(
            t, _metrics(avg_hold_time_hours=48.0, trade_count_30d=30, roi_7d=25.0,
                        roi_30d=30.0, volatility_30d=10.0), "BULL"
        )
        assert t.components.get("regime_adjustment", 0) == 5.0

    def test_whale_bull_bonus(self):
        t = self._tracker()
        _apply_archetype_adjustments(
            t, _metrics(avg_hold_time_hours=5.0, trade_count_30d=10,
                        avg_trade_size_sol=Decimal("50.0"), roi_7d=25.0,
                        roi_30d=30.0, volatility_30d=10.0), "BULL"
        )
        assert t.components.get("regime_adjustment", 0) == 3.0

    def test_day_trader_bull_penalty(self):
        t = self._tracker()
        _apply_archetype_adjustments(
            t, _metrics(avg_hold_time_hours=5.0, trade_count_30d=60, roi_7d=25.0,
                        roi_30d=30.0, volatility_30d=10.0), "BULL"
        )
        assert t.components.get("regime_adjustment", 0) == -2.0

    def test_scalper_bear_bonus(self):
        t = self._tracker()
        _apply_archetype_adjustments(
            t, _metrics(avg_hold_time_hours=0.5, trade_count_30d=150, roi_7d=-5.0,
                        roi_30d=-15.0), "BEAR"
        )
        assert t.components.get("regime_adjustment", 0) == 5.0

    def test_whale_bear_penalty(self):
        t = self._tracker()
        _apply_archetype_adjustments(
            t, _metrics(avg_hold_time_hours=5.0, trade_count_30d=10,
                        avg_trade_size_sol=Decimal("50.0"), roi_7d=-5.0,
                        roi_30d=-15.0), "BEAR"
        )
        assert t.components.get("regime_adjustment", 0) == -5.0

    def test_swing_bear_penalty(self):
        t = self._tracker()
        _apply_archetype_adjustments(
            t, _metrics(avg_hold_time_hours=48.0, trade_count_30d=30, roi_7d=-5.0,
                        roi_30d=-15.0), "BEAR"
        )
        assert t.components.get("regime_adjustment", 0) == -3.0

    def test_general_default(self):
        t = self._tracker()
        _apply_archetype_adjustments(
            t, _metrics(avg_hold_time_hours=100.0, trade_count_30d=5,
                        avg_trade_size_sol=Decimal("1.0")), "NEUTRAL"
        )
        assert "regime_adjustment" not in t.components


class TestWeightsHelper:
    def test_get_current_weights_import_error(self, monkeypatch):
        monkeypatch.setitem(sys.modules, "core.adaptive_weights", None)
        assert _get_current_weights() == {}

    def test_get_current_weights_default(self):
        weights = _get_current_weights()
        assert isinstance(weights, dict)


class TestRawScoreComponents:
    def test_default_components(self):
        rc = RawScoreComponents()
        assert rc.components == {}

    def test_components_json(self):
        rc = RawScoreComponents(positive=5.0, components={"roi_score": 5.0})
        assert json.loads(rc.components_json) == {"roi_score": 5.0}


class TestScoreTrackerPrecedence:
    def test_single_penalty_early_return(self):
        t = ScoreTracker()
        t.add_neg("drawdown_penalty", 5.0)
        t._apply_penalty_precedence()
        assert t.negative == 5.0

    def test_penalty_confidence_discount(self):
        t = ScoreTracker()
        t.add_neg(PenaltyCategory.SMART_MONEY, 10.0)  # conf 0.5 < 0.8
        t._apply_penalty_confidence()
        assert t.components[PenaltyCategory.SMART_MONEY] == -10.0 * (0.5 + 0.5 * 0.5)


class TestRawScorePenaltyBranches:
    @pytest.fixture(autouse=True)
    def _no_adaptive_weights(self, monkeypatch):
        monkeypatch.setattr("core.wqs._get_current_weights", lambda: {})

    def test_recency_weight_env_fallback(self, monkeypatch):
        # Force `from config import ScoutConfig` to fail inside _calculate_raw_score.
        monkeypatch.setitem(sys.modules, "config", None)
        monkeypatch.setenv("SCOUT_WQS_RECENCY_WEIGHT", "false")
        m = _metrics(roi_7d=5.0, roi_30d=50.0, trade_count_30d=20)
        assert 0.0 <= calculate_wqs(m) <= 100.0

    def test_recovery_fragility_penalty(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, roi_90d=-5.0, trade_count_30d=20)
        comps = _calculate_raw_score(m)
        assert "recovery_fragility" in comps.components

    def test_pumpfun_instant_reject(self):
        m = _metrics(roi_7d=50.0, roi_30d=30.0, trade_count_30d=20, pumpfun_trade_ratio=0.9)
        comps = _calculate_raw_score(m)
        assert comps.is_instant_reject
        assert calculate_wqs(m) == 0.0

    def test_pumpfun_moderate_penalty(self):
        m = _metrics(roi_7d=50.0, roi_30d=30.0, trade_count_30d=20, pumpfun_trade_ratio=0.4)
        comps = _calculate_raw_score(m)
        assert comps.components.get("pumpfun_concentration", 0) == -15.0

    def test_profit_factor_loser_penalty(self):
        m = _metrics(roi_7d=5.0, roi_30d=30.0, trade_count_30d=20, profit_factor=0.4)
        comps = _calculate_raw_score(m)
        assert comps.components.get("pf_score", 0) == -40.0

    def test_parse_rate_continuous_penalty(self):
        m = _metrics(roi_7d=5.0, roi_30d=30.0, trade_count_30d=20, parse_rate=0.4)
        comps = _calculate_raw_score(m)
        assert comps.components.get("martingale", 0) == -(0.60 - 0.4) * 80.0

    def test_is_unproven_penalty(self):
        m = _metrics(roi_7d=5.0, roi_30d=30.0, trade_count_30d=20, is_unproven=True)
        comps = _calculate_raw_score(m)
        assert comps.components.get("martingale", 0) == -20.0

    def test_sortino_high(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, sortino_ratio=3.5,
                    max_drawdown_30d=5.0)
        comps = _calculate_raw_score(m)
        assert comps.components.get("sortino_score", 0) == 20.0

    def test_sortino_medium(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, sortino_ratio=2.5,
                    max_drawdown_30d=10.0)
        comps = _calculate_raw_score(m)
        assert comps.components.get("sortino_score", 0) == 15.0

    def test_sortino_low_positive(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, sortino_ratio=1.5,
                    max_drawdown_30d=20.0)
        comps = _calculate_raw_score(m)
        assert comps.components.get("sortino_score", 0) == 10.0

    def test_sortino_negative_high_dd(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, sortino_ratio=0.3,
                    max_drawdown_30d=50.0)
        comps = _calculate_raw_score(m)
        assert comps.components.get("sortino_score", 0) == -15.0

    def test_sortino_negative(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, sortino_ratio=-0.5,
                    max_drawdown_30d=10.0)
        comps = _calculate_raw_score(m)
        assert comps.components.get("sortino_score", 0) == -10.0

    def test_spear_sortino_bonus(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, sortino_ratio=2.0,
                    max_drawdown_30d=10.0)
        comps = _calculate_raw_score(m, strategy="SPEAR")
        assert comps.components.get("sortino_score", 0) == 20.0

    def test_insider_penalty(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, is_fresh_wallet=True)
        comps = _calculate_raw_score(m)
        assert comps.components.get("insider", 0) == -10.0

    def test_scam_penalty(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, correlated_with_scam=True)
        comps = _calculate_raw_score(m)
        assert comps.components.get("scam", 0) == -20.0

    def test_mev_risk_high(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, mev_risk_score=0.6)
        assert _calculate_raw_score(m).components.get("mev_risk", 0) == -20.0

    def test_mev_risk_medium(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, mev_risk_score=0.3)
        assert _calculate_raw_score(m).components.get("mev_risk", 0) == -12.0

    def test_mev_risk_low(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, mev_risk_score=0.15)
        assert _calculate_raw_score(m).components.get("mev_risk", 0) == -6.4

    def test_dex_diversity_bonus(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, dex_diversity_score=4)
        assert _calculate_raw_score(m).components.get("dex_diversity_score", 0) == 5.0

    def test_limit_orders_bonus(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, uses_limit_orders=True)
        assert _calculate_raw_score(m).components.get("smart_money_score", 0) == 10.0

    def test_token_categories_high(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, unique_token_categories=4)
        assert _calculate_raw_score(m).components.get("token_diversity_score", 0) == 5.0

    def test_token_categories_single(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, unique_token_categories=1)
        assert _calculate_raw_score(m).components.get("token_diversity_score", 0) == -5.0

    def test_remove_bonuses_smart_money(self):
        m = _metrics(roi_7d=10.0, roi_30d=-20.0, trade_count_30d=20,
                    dex_diversity_score=4, uses_limit_orders=True, uses_mev_protection=True)
        comps = _calculate_raw_score(m)
        assert comps.components.get("smart_money", 0) == -7.5

    def test_accumulation_strong_bonus(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                     trade_sizes=[1.0, 2.0, 3.0, 1.0, 4.0, 5.0])
        assert _calculate_raw_score(m).components.get("smart_accumulation", 0) == 8.0

    def test_accumulation_boundary_no_entry(self):
        # _detect_smart_accumulation only yields 0.0/0.2/0.4/0.7; a score of
        # exactly 0.4 must not hit the > 0.4 bonus branch.
        m = _metrics(roi_7d=None, roi_30d=30.0, trade_count_30d=20,
                     trade_sizes=[1.0, 2.0, 3.0, 1.0, 4.0, 5.0])
        assert "smart_accumulation" not in _calculate_raw_score(m).components

    def test_accumulation_low_penalty(self):
        m = _metrics(roi_7d=-5.0, roi_30d=5.0, trade_count_30d=20,
                     trade_sizes=[5.0, 1.0, 4.0, 1.0, 4.0, 1.0])
        comps = _calculate_raw_score(m)
        assert comps.components.get("smart_accumulation", 0) == -5.0

    def test_adaptability_bonus(self):
        m = _metrics(roi_7d=30.0, roi_30d=5.0, trade_count_30d=20, win_rate=0.6,
                     volatility_30d=70.0)
        assert _calculate_raw_score(m).components.get("adaptability", 0) == 5.0

    def test_unrealized_loss_with_realized_profit(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                     total_unrealized_loss_sol=Decimal("2.0"),
                     total_realized_profit_sol=Decimal("1.0"))
        comps = _calculate_raw_score(m)
        assert comps.components.get("martingale", 0) < 0

    def test_unrealized_loss_no_realized(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                     total_unrealized_loss_sol=Decimal("2.0"),
                     total_realized_profit_sol=Decimal("0"))
        assert _calculate_raw_score(m).components.get("martingale", 0) == -20.0

    def test_paper_gains_ratio(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                     total_unrealized_gain_sol=Decimal("9.0"),
                     total_realized_profit_sol=Decimal("1.0"))
        assert _calculate_raw_score(m).components.get("martingale", 0) < 0

    def test_last_trade_at_invalid_type(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, last_trade_at=12345)
        comps = _calculate_raw_score(m)  # must not raise
        assert isinstance(comps.raw_score, float)

    def test_recency_band_5_14_days(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, last_trade_at=_ts(6))
        assert _calculate_raw_score(m).components.get("recency_score", 0) == -8.0

    def test_recency_band_14_21_days(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, last_trade_at=_ts(15))
        assert _calculate_raw_score(m).components.get("recency_score", 0) == -25.0

    def test_recency_band_over_21_days(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, last_trade_at=_ts(30))
        assert _calculate_raw_score(m).components.get("recency_score", 0) == -35.0

    def test_wmi_negative_penalty(self):
        # roi_7d=-1, roi_30d=100, count=20 -> wmi ~ -0.355 (band: -5);
        # bonuses +25 (roi) +10 (roi_7d>-5 & roi_30d>20) then -5 -> 30.0
        m = _metrics(roi_7d=-1.0, roi_30d=100.0, trade_count_30d=20, last_trade_at=_ts(1))
        assert _calculate_raw_score(m).components.get("roi_score", 0) == 30.0

    def test_wmi_very_negative_penalty(self):
        # roi_7d=-100, roi_30d=100, count=1 -> wmi ~ -0.658 (band: -15);
        # roi_reliability=0.1 scales the +2.5 30d bonus, netting -12.5.
        m = _metrics(roi_7d=-100.0, roi_30d=100.0, trade_count_30d=1, last_trade_at=_ts(1))
        assert _calculate_raw_score(m).components.get("roi_score", 0) == -12.5

    def test_adaptive_weights_exception(self, monkeypatch):
        def boom():
            raise RuntimeError("weights failed")

        monkeypatch.setattr("core.wqs._get_current_weights", boom)
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20)
        comps = _calculate_raw_score(m)
        assert comps.positive >= 0

    def test_invalid_penalty_cap_env(self, monkeypatch):
        monkeypatch.setenv("SCOUT_MAX_TOTAL_PENALTY", "not-a-number")
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20)
        comps = _calculate_raw_score(m)
        assert comps.raw_score >= 0.0

    def test_penalty_cap_with_precedence(self):
        # Multiple martingale penalties collapse to most severe, then capped.
        m = _metrics(
            roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
            total_unrealized_loss_sol=Decimal("10.0"),
            total_realized_profit_sol=Decimal("1.0"),
            total_unrealized_gain_sol=Decimal("5.0"),
            parse_rate=0.2,
        )
        comps = _calculate_raw_score(m)
        assert comps.negative <= 80.0


class TestConfidence:
    def test_is_unproven_cap(self):
        assert _compute_confidence(20, metrics=None, is_unproven=True) == 0.70

    def test_parse_rate_cap(self):
        m = _metrics(parse_rate=0.5)
        conf = _compute_confidence(20, metrics=m)
        assert conf == 0.30 + 0.5 * 0.70

    def test_size_factor_discount(self):
        m = _metrics(avg_trade_size_sol=Decimal("0.1"))
        conf = _compute_confidence(20, metrics=m)
        assert conf < 1.0

    def test_profit_factor_floor(self):
        conf = _compute_confidence(3, profit_factor=3.0)
        assert conf == 0.80


class TestWqsConfidenceInstantReject:
    def test_instant_reject_returns_zero(self):
        m = _metrics(roi_7d=50.0, roi_30d=30.0, trade_count_30d=20, pumpfun_trade_ratio=0.9)
        result = calculate_wqs_with_confidence(m)
        assert result.score == 0.0
        assert result.confidence == 0.0
        assert result.adjusted_score == 0.0


class TestClassifyWallet:
    def test_active_with_low_confidence(self):
        assert classify_wallet(80.0, confidence=0.5, min_confidence=0.7) == "CANDIDATE"

    def test_active_high_confidence(self):
        assert classify_wallet(80.0, confidence=0.9, min_confidence=0.7) == "ACTIVE"

    def test_candidate(self):
        assert classify_wallet(60.0) == "CANDIDATE"

    def test_rejected(self):
        assert classify_wallet(30.0) == "REJECTED"


class TestWmiBranches:
    def test_ratio_02_05_band(self):
        # roi_ratio in (0.2, 0.5] -> (ratio - 0.2) / 0.3 * 0.5
        assert _compute_wmi(3.0, 10.0, None) > _compute_wmi(0.0, 10.0, None)

    def test_roi30_negative_roi7_negative(self):
        # roi_30d <= 0 and roi_7d < 0 -> roi_trend = -0.5
        wmi = _compute_wmi(-5.0, -10.0, None)
        assert wmi < 0

    def test_roi30_negative_roi7_positive(self):
        # roi_30d <= 0 and roi_7d >= 0 -> roi_trend = 0.0
        assert _compute_wmi(5.0, -10.0, None) == 0.0


class TestEnhancedMomentumBranches:
    def test_ratio_06_08(self):
        m = _metrics(roi_7d=25.0, roi_30d=35.0, trade_count_30d=20)
        comps = _calculate_raw_score(m)
        assert comps.components.get("enhanced_momentum", 0) == 5.0

    def test_ratio_04_06(self):
        m = _metrics(roi_7d=60.0, roi_30d=120.0, trade_count_30d=20)
        comps = _calculate_raw_score(m)
        assert comps.components.get("enhanced_momentum", 0) == 5.0

    def test_roi7_50_100(self):
        m = _metrics(roi_7d=60.0, roi_30d=100.0, trade_count_30d=20)
        comps = _calculate_raw_score(m)
        assert comps.components.get("enhanced_momentum", 0) == 5.0

    def test_roi7_20_50(self):
        m = _metrics(roi_7d=30.0, roi_30d=33.0, trade_count_30d=20)
        comps = _calculate_raw_score(m)
        assert comps.components.get("enhanced_momentum", 0) == 5.0

    def test_roi30_negative_roi7_positive(self):
        m = _metrics(roi_7d=15.0, roi_30d=-10.0, trade_count_30d=20)
        comps = _calculate_raw_score(m)
        # +0.3 raw momentum is below the +5 bonus threshold
        assert "enhanced_momentum" not in comps.components

    def test_roi7_deeply_below_roi30(self):
        m = _metrics(roi_7d=-5.0, roi_30d=0.0, trade_count_30d=20)
        comps = _calculate_raw_score(m)
        assert comps.components.get("enhanced_momentum", 0) == -3.0


class TestMoreRawScoreBranches:
    @pytest.fixture(autouse=True)
    def _no_adaptive_weights(self, monkeypatch):
        monkeypatch.setattr("core.wqs._get_current_weights", lambda: {})

    def test_pump_spike_negative_roi30(self):
        m = _metrics(roi_7d=60.0, roi_30d=-10.0, trade_count_30d=20)
        assert _calculate_raw_score(m).components.get("pump_spike", 0) == -25.0

    def test_pump_spike_tiny_positive_roi30(self):
        # roi_30d in (0, 1) with roi_7d > 10 is always a pump spike
        m = _metrics(roi_7d=20.0, roi_30d=0.5, trade_count_30d=20)
        assert _calculate_raw_score(m).components.get("pump_spike", 0) == -25.0

    def test_replay_data_gap_penalty(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                    replay_data_gap_ratio=0.5)
        assert _calculate_raw_score(m).components.get("replay_data_gap", 0) == -10.0

    def test_win_rate_tiers(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, win_rate=0.95)
        comps = _calculate_raw_score(m)
        assert comps.components.get("win_rate_score", 0) == 20.0

    def test_win_rate_070(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, win_rate=0.70)
        assert _calculate_raw_score(m).components.get("win_rate_score", 0) == 10.0

    def test_activity_high_counts(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=100)
        assert _calculate_raw_score(m).components.get("activity_score", 0) == 20.0

    def test_activity_count_50(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=50)
        assert _calculate_raw_score(m).components.get("activity_score", 0) == 15.0

    def test_dust_trader_penalty(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                    avg_trade_size_sol=Decimal("0.01"))
        assert _calculate_raw_score(m).components.get("pump_spike", 0) == -10.0

    def test_operator_admission_zero_reject(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                    operator_admission_rate=0.0, operator_decision_count=10)
        comps = _calculate_raw_score(m)
        assert comps.is_instant_reject
        assert calculate_wqs(m) == 0.0

    def test_win_streak_bonus(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                    win_streak_consistency=0.6)
        assert _calculate_raw_score(m).components.get("consistency_score", 0) == 5.0

    def test_sniper_instant_reject(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                    avg_entry_delay_seconds=5.0)
        assert _calculate_raw_score(m).is_instant_reject

    def test_sniper_moderate_penalty(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                    avg_entry_delay_seconds=30.0)
        assert _calculate_raw_score(m).components.get("sniper", 0) == -15.0

    def test_entry_delay_bonus(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                    avg_entry_delay_seconds=300.0)
        assert _calculate_raw_score(m).components.get("entry_delay_score", 0) == 15.0

    def test_pf_gt_3(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, profit_factor=3.5)
        assert _calculate_raw_score(m).components.get("pf_score", 0) == 15.0

    def test_pf_gt_15(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, profit_factor=1.8)
        assert _calculate_raw_score(m).components.get("pf_score", 0) == 5.0

    def test_pf_ge_12(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, profit_factor=1.2)
        assert _calculate_raw_score(m).components.get("pf_score", 0) == 2.0

    def test_pf_ge_11(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, profit_factor=1.1)
        assert _calculate_raw_score(m).components.get("pf_score", 0) == -3.0

    def test_pf_ge_10(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, profit_factor=1.0)
        assert _calculate_raw_score(m).components.get("pf_score", 0) == -6.0

    def test_pf_ge_05(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, profit_factor=0.5)
        assert _calculate_raw_score(m).components.get("pf_score", 0) == -25.0

    def test_martingale_wr_pf_mismatch(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, win_rate=0.8,
                    profit_factor=1.0)
        assert _calculate_raw_score(m).components.get("martingale", 0) == -15.0

    def test_pf_wr_penalty(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, win_rate=0.8,
                    profit_factor=0.9)
        comps = _calculate_raw_score(m)
        # PF_WR confidence 0.7 -> -20 * (0.5 + 0.5*0.7) = -17
        assert comps.components.get("pf_wr", 0) == -17.0

    def test_low_win_rate_removes_bonuses(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, win_rate=0.3)
        comps = _calculate_raw_score(m)
        assert comps.components.get("smart_money", 0) < 0 or "smart_money" not in comps.components

    def test_momentum_score_over_07(self):
        m = _metrics(roi_7d=150.0, roi_30d=100.0, trade_count_30d=20)
        assert _calculate_raw_score(m).components.get("enhanced_momentum", 0) == 10.0

    def test_momentum_score_07(self):
        m = _metrics(roi_7d=90.0, roi_30d=100.0, trade_count_30d=20)
        assert _calculate_raw_score(m).components.get("enhanced_momentum", 0) == 7.0

    def test_momentum_score_03_05(self):
        m = _metrics(roi_7d=25.0, roi_30d=35.0, trade_count_30d=20)
        assert _calculate_raw_score(m).components.get("enhanced_momentum", 0) == 5.0

    def test_bull_regime_bonus(self):
        m = _metrics(roi_7d=25.0, roi_30d=30.0, trade_count_30d=20, volatility_30d=10.0)
        assert _calculate_raw_score(m).components.get("market_regime", 0) == 3.0

    def test_last_trade_at_datetime_object(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                    last_trade_at=datetime.now(timezone.utc) - timedelta(hours=1))
        comps = _calculate_raw_score(m)
        assert comps.components.get("recency_score", 0) == 10.0

    def test_last_trade_at_naive(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                    last_trade_at=(datetime.now(timezone.utc) - timedelta(days=3)).replace(tzinfo=None))
        comps = _calculate_raw_score(m)
        assert comps.components.get("recency_score", 0) == 5.0

    def test_wmi_positive_high(self):
        m = _metrics(roi_7d=60.0, roi_30d=100.0, trade_count_30d=100, last_trade_at=_ts(1))
        comps = _calculate_raw_score(m)
        assert comps.components.get("roi_score", 0) > 0

    def test_wmi_positive_mid(self):
        m = _metrics(roi_7d=30.0, roi_30d=40.0, trade_count_30d=40, last_trade_at=_ts(1))
        comps = _calculate_raw_score(m)
        assert comps.components.get("roi_score", 0) > 0

    def test_adaptive_weights_applied(self, monkeypatch):
        monkeypatch.setattr("core.wqs._get_current_weights",
                            lambda: {"roi_score": 2.0, "not_a_component": 5.0})
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20)
        comps = _calculate_raw_score(m)
        assert comps.components["roi_score"] > 0
        assert "not_a_component" not in comps.components

    def test_advanced_risk_features_penalties(self):
        arf = {
            "extraction_success": True,
            "sample_count": 10,
            "cvar_95": -5.0,
            "max_drawdown_duration_trades": 15,
            "ulcer_index": 8.0,
            "extraction_errors": None,
        }
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                     advanced_risk_features=arf)
        comps = _calculate_raw_score(m)
        assert comps.components.get("cvar", 0) < 0
        assert comps.components.get("drawdown_duration", 0) < 0
        assert comps.components.get("ulcer_index", 0) < 0

    def test_advanced_risk_features_incomplete(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                     advanced_risk_features={"extraction_success": False})
        comps = _calculate_raw_score(m)  # must not raise
        assert comps.raw_score >= 0.0

    def test_smart_accumulation_cv_penalty(self):
        # Trend in (0.3, 0.8) + roi>0 but variance > 2x average -> -0.3
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                     trade_sizes=[1.0, 2.0, 100.0, 1.0, 2.0, 200.0])
        # 0.7 raw score minus 0.3 variance penalty lands at 0.4 -> no entry
        assert "smart_accumulation" not in _calculate_raw_score(m).components

    def test_penalty_cap_scaling_with_uncappable(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20,
                    max_drawdown_30d=2000.0, correlated_with_scam=True)
        comps = _calculate_raw_score(m)
        assert comps.negative <= 80.0

    def test_calculate_wqs_with_confidence_normal(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, win_rate=0.6)
        result = calculate_wqs_with_confidence(m)
        assert result.score > 0
        assert result.confidence > 0
        assert 0 <= result.adjusted_score <= 100

    def test_calculate_wqs_arbitrage(self):
        m = _metrics(roi_7d=10.0, roi_30d=30.0, trade_count_30d=20, archetype="ARBITRAGE")
        assert calculate_wqs(m) == 0.0
        result = calculate_wqs_with_confidence(m)
        assert result.score == 0.0

    def test_confidence_10_19(self):
        assert _compute_confidence(10) >= 0.90

    def test_confidence_0_2(self):
        assert _compute_confidence(0) == 0.0
        assert _compute_confidence(2) < 0.55
