"""
Coverage tests for core/high_conviction_allocator.py.

Covers rebalancing, performance tracking, state persistence, efficiency
reporting, and the allocation cap/boost branches.
"""

import json
import time

from core.high_conviction_allocator import (
    AllocatorConfig,
    AllocationResult,
    ConvictionLevel,
    HighConvictionAllocator,
    RebalanceResult,
)


def _config(tmp_path):
    return AllocatorConfig(STATE_FILE=str(tmp_path / "state.json"))


def _allocator(tmp_path=None):
    cfg = _config(tmp_path) if tmp_path else AllocatorConfig()
    return HighConvictionAllocator(cfg)


def test_set_total_credits_zero_skips_rebalance():
    allocator = _allocator()
    allocator.set_total_credits(0)
    assert allocator._allocations[ConvictionLevel.VERY_HIGH] == 0


def test_get_conviction_levels():
    allocator = _allocator()
    assert allocator.get_conviction_level(85.0) == ConvictionLevel.VERY_HIGH
    assert allocator.get_conviction_level(75.0) == ConvictionLevel.HIGH
    assert allocator.get_conviction_level(60.0) == ConvictionLevel.MEDIUM
    assert allocator.get_conviction_level(40.0) == ConvictionLevel.EMERGING
    assert allocator.get_conviction_level(10.0) == ConvictionLevel.LOW


def test_calculate_credit_multiplier():
    allocator = _allocator()
    assert allocator.calculate_credit_multiplier(85.0) == 3.0
    assert allocator.calculate_credit_multiplier(75.0) == 2.5
    assert allocator.calculate_credit_multiplier(60.0) == 1.0
    assert allocator.calculate_credit_multiplier(40.0) == 0.3
    assert allocator.calculate_credit_multiplier(10.0) == 0.1


def test_allocate_analysis_credits_with_performance_boost():
    allocator = _allocator()
    allocator.set_total_credits(10000)
    # Seed performance: 20 trades, 80% win rate
    for i in range(20):
        allocator.record_wallet_performance("wallet_star", 80.0, win=True, pnl=1.0)
    result = allocator.allocate_analysis_credits("wallet_star", 80.0, base_credits=100)
    assert result.multiplier_used == 3.0 * 1.5  # VERY_HIGH * boost
    assert "performance boost" in result.reason
    assert result.credits_allocated > 0


def test_allocate_analysis_credits_caps_at_level_budget():
    allocator = _allocator()
    allocator.set_total_credits(1000)  # HIGH alloc = 400
    allocator.allocate_analysis_credits("w1", 75.0, base_credits=1000)
    second = allocator.allocate_analysis_credits("w2", 75.0, base_credits=1000)
    assert second.credits_allocated == 0
    assert allocator._consumed[ConvictionLevel.HIGH] == 400


def test_allocate_analysis_credits_no_boost_reason():
    allocator = _allocator()
    allocator.set_total_credits(10000)
    result = allocator.allocate_analysis_credits("w_plain", 75.0, base_credits=100)
    assert "multiplier" in result.reason
    assert "performance boost" not in result.reason


def test_emerging_and_high_conviction_budget():
    allocator = _allocator()
    allocator.set_total_credits(10000)
    allocator.allocate_analysis_credits("w_em", 40.0, base_credits=1000)
    assert allocator.get_emerging_wallet_budget() < 800
    allocator.allocate_analysis_credits("w_hi", 75.0, base_credits=1000)
    assert allocator.get_high_conviction_budget() < 7000


def test_rebalance_interval_not_reached():
    allocator = _allocator()
    allocator.set_total_credits(10000)
    result = allocator.rebalance_to_high_conviction()
    assert result.credits_moved == 0
    assert result.reason == "Rebalance interval not reached"


def test_rebalance_moves_credits_to_high_conviction(tmp_path):
    allocator = _allocator(tmp_path)
    allocator.set_total_credits(10000)
    allocator._last_rebalance = time.time() - 7200  # past interval
    result = allocator.rebalance_to_high_conviction()
    assert result.credits_moved > 0
    assert "Rebalanced" in result.reason
    assert result.previous_allocations[ConvictionLevel.EMERGING] == 800
    assert allocator._allocations[ConvictionLevel.EMERGING] < 800
    # State saved after rebalance
    state_file = allocator._config.STATE_FILE
    assert json.load(open(state_file))["last_save"]


def test_rebalance_no_deviation():
    allocator = _allocator()
    allocator.set_total_credits(10000)
    # Consume everything so no level has excess
    for level in ConvictionLevel:
        allocator._consumed[level] = allocator._allocations[level]
    allocator._last_rebalance = time.time() - 7200
    result = allocator.rebalance_to_high_conviction()
    assert result.credits_moved == 0
    assert result.reason == "No significant deviation found"


def test_record_wallet_performance_new_and_existing():
    allocator = _allocator()
    allocator.record_wallet_performance("w1", 70.0, win=True, pnl=2.0)
    allocator.record_wallet_performance("w1", 70.0, win=False, pnl=-1.0)
    perf = allocator.get_wallet_performance("w1")
    assert perf["trades"] == 2
    assert perf["wins"] == 1
    assert perf["total_pnl"] == 1.0
    assert perf["win_rate"] == 0.5
    assert allocator.get_wallet_performance("missing") is None


def test_get_allocation_summary():
    allocator = _allocator()
    allocator.set_total_credits(10000)
    allocator.allocate_analysis_credits("w1", 75.0, base_credits=100)
    summary = allocator.get_allocation_summary()
    assert summary["total_allocated"] == 10000
    assert summary["total_consumed"] > 0
    assert summary["allocations_by_level"]["high"]["allocated"] == 4000
    assert summary["high_conviction_budget"] > 0
    assert summary["emerging_wallet_budget"] > 0


def test_get_allocation_efficiency_zero_and_ratio():
    allocator = _allocator()
    # No credits set -> all allocations 0 -> efficiency 0.0
    eff = allocator.get_allocation_efficiency()
    assert all(v == 0.0 for v in eff.values())
    allocator.set_total_credits(10000)
    allocator.allocate_analysis_credits("w1", 75.0, base_credits=100)
    eff2 = allocator.get_allocation_efficiency()
    assert 0 < eff2["high"] <= 1.0


def test_load_state_restores(tmp_path):
    state = {
        "consumed": {"high": 123, "bogus_level": 9},
        "wallet_performance": {"w1": {"trades": 5}},
    }
    state_file = tmp_path / "state.json"
    state_file.write_text(json.dumps(state))
    allocator = _allocator(tmp_path)
    assert allocator._consumed[ConvictionLevel.HIGH] == 123
    assert allocator._wallet_performance["w1"]["trades"] == 5


def test_load_state_corrupt(tmp_path):
    (tmp_path / "state.json").write_text("{corrupt!!")
    allocator = _allocator(tmp_path)  # must not raise
    assert allocator._consumed[ConvictionLevel.HIGH] == 0


def test_save_state_failure_logged(tmp_path):
    # STATE_FILE pointing at a directory -> open() raises OSError
    allocator = HighConvictionAllocator(AllocatorConfig(STATE_FILE=str(tmp_path)))
    allocator._save_state()  # must not raise


def test_reset_consumption():
    allocator = _allocator()
    allocator.set_total_credits(10000)
    allocator.allocate_analysis_credits("w1", 75.0, base_credits=100)
    allocator.reset_consumption()
    assert all(v == 0 for v in allocator._consumed.values())


def test_rebalance_result_and_allocation_result_defaults():
    result = AllocationResult(
        wallet_address="w", wqs_score=70.0, conviction_level=ConvictionLevel.HIGH,
        credits_allocated=100, multiplier_used=2.5, reason="test",
    )
    assert result.timestamp > 0
    rb = RebalanceResult(
        previous_allocations={}, new_allocations={}, credits_moved=0, reason="r"
    )
    assert rb.timestamp > 0
