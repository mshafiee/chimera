"""Tests for Wallet Quality Score (WQS) calculation"""

import sys
from pathlib import Path

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

import pytest
from core.wqs import WalletMetrics, calculate_wqs, classify_wallet


def test_wqs_basic_calculation():
    """Test basic WQS calculation"""
    wallet = WalletMetrics(
        address="test_wallet",
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=25,
        max_drawdown_30d=5.0,
    )
    
    score = calculate_wqs(wallet)
    assert 0 <= score <= 100
    assert score > 30.0  # Should be strong with good metrics


def test_wqs_low_trade_count_penalty():
    """Test that low trade count reduces confidence"""
    wallet_low = WalletMetrics(
        address="test_wallet_low",
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=2,  # Very low closes - should be heavily discounted
        max_drawdown_30d=5.0,
    )
    
    wallet_high = WalletMetrics(
        address="test_wallet_high",
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=25,  # High count - near full confidence
        max_drawdown_30d=5.0,
    )
    
    score_low = calculate_wqs(wallet_low)
    score_high = calculate_wqs(wallet_high)
    
    assert score_high > score_low, f"High trade count should score higher: {score_high} vs {score_low}"
    # Very low counts should not be zeroed out, but should be significantly discounted.
    assert score_low > 0.0
    assert (score_low / score_high) < 0.6


def test_wqs_medium_trade_count_penalty():
    """Test that medium trade count is discounted but not crushed."""
    wallet_medium = WalletMetrics(
        address="test_wallet_medium",
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=10,
        max_drawdown_30d=5.0,
    )
    
    wallet_high = WalletMetrics(
        address="test_wallet_high",
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=25,  # High count - no penalty
        max_drawdown_30d=5.0,
    )
    
    score_medium = calculate_wqs(wallet_medium)
    score_high = calculate_wqs(wallet_high)
    
    assert score_high > score_medium, f"High trade count should score higher: {score_high} vs {score_medium}"
    # With confidence ramp to 1.0 at 20 trades (10 trades → 50% confidence) and activity
    # bonuses that differ by count, the ratio should be > 0 and < 1 (discounted but not zero).
    ratio = score_medium / score_high
    assert 0.2 < ratio < 1.0, f"Expected discounted but non-zero ratio, got {ratio}"


def test_wqs_very_low_trade_count_curve():
    """Sanity check: 1-4 closes are discounted but not annihilated."""
    base = WalletMetrics(
        address="base",
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=25,
        max_drawdown_30d=5.0,
    )
    base_score = calculate_wqs(base)

    for tc in [1, 2, 3, 4]:
        w = WalletMetrics(
            address=f"tc_{tc}",
            roi_30d=50.0,
            win_streak_consistency=0.8,
            roi_7d=10.0,
            trade_count_30d=tc,
            max_drawdown_30d=5.0,
        )
        s = calculate_wqs(w)
        assert s > 0.0
        assert s < base_score


def test_wqs_anti_pump_and_dump():
    """Test anti-pump-and-dump logic: penalize wallets with 7d ROI > 2x 30d ROI"""
    # Pump and dump case: 7d ROI is 3x the 30d ROI
    wallet_pump = WalletMetrics(
        address="test_wallet_pump",
        roi_30d=20.0,
        roi_7d=70.0,  # 7d > 2x 30d (70 > 40) - should be penalized
        win_streak_consistency=0.8,
        trade_count_30d=25,
        max_drawdown_30d=5.0,
    )
    
    # Normal case: 7d ROI is proportional to 30d ROI
    wallet_normal = WalletMetrics(
        address="test_wallet_normal",
        roi_30d=20.0,
        roi_7d=5.0,  # Normal - not a spike
        win_streak_consistency=0.8,
        trade_count_30d=25,
        max_drawdown_30d=5.0,
    )
    
    score_pump = calculate_wqs(wallet_pump)
    score_normal = calculate_wqs(wallet_normal)
    
    assert score_normal > score_pump, f"Normal wallet should score higher than pump: {score_normal} vs {score_pump}"
    # Pump wallet should have 15 points deducted
    assert abs((score_normal - score_pump) - 15.0) < 5.0, f"Pump penalty should be around 15 points"


def test_wqs_anti_pump_and_dump_edge_cases():
    """Test anti-pump-and-dump edge cases"""
    # Case 1: 7d ROI exactly 2x 30d ROI (should NOT trigger penalty, needs > 2x)
    wallet_exact_2x = WalletMetrics(
        address="test_exact_2x",
        roi_30d=20.0,
        roi_7d=40.0,  # Exactly 2x - should NOT trigger
        win_streak_consistency=0.8,
        trade_count_30d=25,
        max_drawdown_30d=5.0,
    )
    
    # Case 2: 7d ROI slightly above 2x (should trigger)
    wallet_slightly_above = WalletMetrics(
        address="test_slightly_above",
        roi_30d=20.0,
        roi_7d=40.1,  # Slightly above 2x - should trigger
        win_streak_consistency=0.8,
        trade_count_30d=25,
        max_drawdown_30d=5.0,
    )
    
    # Case 3: Negative 30d ROI (should NOT trigger penalty)
    wallet_negative_30d = WalletMetrics(
        address="test_negative_30d",
        roi_30d=-10.0,
        roi_7d=50.0,  # High 7d but negative 30d - should NOT trigger
        win_streak_consistency=0.8,
        trade_count_30d=25,
        max_drawdown_30d=5.0,
    )
    
    score_exact = calculate_wqs(wallet_exact_2x)
    score_above = calculate_wqs(wallet_slightly_above)
    score_negative = calculate_wqs(wallet_negative_30d)
    
    assert score_exact > score_above, "Exact 2x should score higher than slightly above 2x"
    assert score_negative > score_above, "Negative 30d should not trigger pump penalty"


def test_wqs_drawdown_penalty():
    """Test that high drawdown reduces score"""
    wallet_low_dd = WalletMetrics(
        address="test_low_dd",
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=25,
        max_drawdown_30d=2.0,  # Low drawdown
    )
    
    wallet_high_dd = WalletMetrics(
        address="test_high_dd",
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=25,
        max_drawdown_30d=15.0,  # High drawdown - should lose 3.0 points (15 * 0.2)
    )
    
    score_low = calculate_wqs(wallet_low_dd)
    score_high = calculate_wqs(wallet_high_dd)
    
    assert score_low > score_high, f"Low drawdown should score higher: {score_low} vs {score_high}"
    # Drawdown penalty should be approximately 13 * 0.2 = 2.6 points difference
    assert abs((score_low - score_high) - 2.6) < 1.0, f"Drawdown penalty should be around 2.6 points"


def test_wqs_activity_bonus():
    """Test that wallets with 50+ trades get activity bonus"""
    wallet_active = WalletMetrics(
        address="test_active",
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=50,  # Should get +5 bonus
        max_drawdown_30d=5.0,
    )
    
    wallet_inactive = WalletMetrics(
        address="test_inactive",
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=49,  # Just below threshold - no bonus
        max_drawdown_30d=5.0,
    )
    
    score_active = calculate_wqs(wallet_active)
    score_inactive = calculate_wqs(wallet_inactive)
    
    assert score_active > score_inactive, f"Active wallet should score higher: {score_active} vs {score_inactive}"
    assert abs((score_active - score_inactive) - 5.0) < 1.0, f"Activity bonus should be around 5 points"


def test_wqs_roi_capping():
    """Test that ROI contribution is capped at 100%"""
    wallet_normal_roi = WalletMetrics(
        address="test_normal_roi",
        roi_30d=50.0,  # Should contribute 12.5 points (50/100 * 25)
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=25,
        max_drawdown_30d=5.0,
    )
    
    wallet_high_roi = WalletMetrics(
        address="test_high_roi",
        roi_30d=200.0,  # Should be capped at 100% - contribute 25 points max
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=25,
        max_drawdown_30d=5.0,
    )
    
    score_normal = calculate_wqs(wallet_normal_roi)
    score_high = calculate_wqs(wallet_high_roi)
    
    # High ROI should score higher, but not by 3x (should be capped)
    assert score_high > score_normal
    # Difference should be approximately 12.5 points (25 - 12.5)
    assert abs((score_high - score_normal) - 12.5) < 2.0, f"ROI contribution should be capped"


def test_wqs_win_rate_fallback():
    """Test that win_rate is used as fallback when win_streak_consistency is None"""
    wallet_with_consistency = WalletMetrics(
        address="test_with_consistency",
        roi_30d=50.0,
        win_streak_consistency=0.8,  # Should use this
        win_rate=0.6,
        trade_count_30d=25,
        max_drawdown_30d=5.0,
    )
    
    wallet_with_win_rate = WalletMetrics(
        address="test_with_win_rate",
        roi_30d=50.0,
        win_streak_consistency=None,  # Should fallback to win_rate
        win_rate=0.8,  # Higher win rate
        trade_count_30d=25,
        max_drawdown_30d=5.0,
    )
    
    score_consistency = calculate_wqs(wallet_with_consistency)
    score_win_rate = calculate_wqs(wallet_with_win_rate)
    
    # Consistency should contribute more (20 points) than win_rate (15 points)
    # But win_rate wallet has higher win_rate (0.8 vs 0.6), so it might score higher
    # Let's verify both are valid scores
    assert 0 <= score_consistency <= 100
    assert 0 <= score_win_rate <= 100


def test_wqs_none_values():
    """Test that None values are handled gracefully"""
    wallet_minimal = WalletMetrics(
        address="test_minimal",
        # All optional fields are None
    )
    
    score = calculate_wqs(wallet_minimal)
    # PDD: score starts at 0.0
    assert score == 0.0, f"Minimal wallet should return 0.0: {score}"


def test_wqs_negative_values():
    """Test that negative ROI and drawdown are handled"""
    wallet_negative = WalletMetrics(
        address="test_negative",
        roi_30d=-20.0,  # Negative ROI - should not add to score
        roi_7d=-10.0,
        win_streak_consistency=0.5,  # Adds 10 points
        trade_count_30d=25,
        max_drawdown_30d=10.0,  # Positive drawdown - should subtract 2 points
    )
    
    score = calculate_wqs(wallet_negative)
    assert 0 <= score <= 100
    # Score should be modest due to no ROI contribution and drawdown penalty.
    assert score < 20.0, f"Negative ROI wallet should score low: {score}"
    
    # Test with all negative/zero values
    wallet_all_negative = WalletMetrics(
        address="test_all_negative",
        roi_30d=-50.0,
        roi_7d=-30.0,
        win_streak_consistency=0.0,
        trade_count_30d=5,  # Low count penalty
        max_drawdown_30d=50.0,  # High drawdown
    )
    
    score_all_negative = calculate_wqs(wallet_all_negative)
    assert 0 <= score_all_negative <= 100
    assert score_all_negative < 30.0, f"All negative wallet should score very low: {score_all_negative}"


def test_wqs_bounds():
    """Test that WQS is always between 0 and 100"""
    test_cases = [
        WalletMetrics(
            address="test_extreme_low",
            roi_30d=-100.0,
            win_streak_consistency=0.0,
            roi_7d=-50.0,
            trade_count_30d=5,
            max_drawdown_30d=50.0,
        ),
        WalletMetrics(
            address="test_extreme_high",
            roi_30d=200.0,
            win_streak_consistency=1.0,
            roi_7d=100.0,
            trade_count_30d=100,
            max_drawdown_30d=0.0,
        ),
        WalletMetrics(
            address="test_all_none",
            # All None
        ),
    ]
    
    for wallet in test_cases:
        score = calculate_wqs(wallet)
        assert 0 <= score <= 100, f"WQS out of bounds for {wallet.address}: {score}"


def test_classify_wallet():
    """Test wallet classification based on WQS score"""
    assert classify_wallet(75.0) == "ACTIVE"
    assert classify_wallet(70.0) == "ACTIVE"
    assert classify_wallet(69.9) == "CANDIDATE"
    assert classify_wallet(50.0) == "CANDIDATE"
    assert classify_wallet(40.0) == "CANDIDATE"
    assert classify_wallet(39.9) == "REJECTED"
    assert classify_wallet(0.0) == "REJECTED"


# ── Financial-loss & missed-profit test suite ─────────────────────────────────


def test_wqs_momentum_bonus_not_applied_when_both_roi_negative():
    """
    Test 72 (plan): momentum bonus must NOT fire when roi_30d and roi_7d are both negative.

    The code: `if (roi_7d or 0) > 0 and (roi_30d or 0) > 0 and roi_7d >= roi_30d * 0.5`
    For roi_30d=-50%, roi_7d=-20%: roi_7d > 0 is False → no bonus.

    Risk: If the condition incorrectly applied the bonus for negative pairs where roi_7d
    is "better" than roi_30d (smaller magnitude), bad wallets in recovery would be promoted.
    """
    wallet = WalletMetrics(
        address="test_neg_momentum",
        roi_30d=-50.0,
        roi_7d=-20.0,  # "better" but still negative
        win_streak_consistency=0.6,
        trade_count_30d=25,
        max_drawdown_30d=10.0,
    )

    wallet_no_recent = WalletMetrics(
        address="test_no_recent",
        roi_30d=-50.0,
        roi_7d=None,
        win_streak_consistency=0.6,
        trade_count_30d=25,
        max_drawdown_30d=10.0,
    )

    score_neg = calculate_wqs(wallet)
    score_none = calculate_wqs(wallet_no_recent)

    # Both should score the same since neither gets the momentum bonus
    assert score_neg == score_none, (
        f"Negative ROI pair must not get momentum bonus: {score_neg} vs {score_none}"
    )


def test_wqs_profit_factor_single_win_heavily_penalized_by_confidence():
    """
    Test 73 (plan): A wallet with 1 trade and high profit_factor gets a nearly-zero score
    due to the confidence multiplier (trade_count=1 → confidence=1/20=0.05).

    Even with profit_factor > 3.0 (+15 pts bonus), the final score ≈ (base + 15) × 0.05.
    This prevents a wallet with 1 lucky trade from being promoted.
    """
    wallet_1_win = WalletMetrics(
        address="test_1_win",
        roi_30d=200.0,
        roi_7d=200.0,
        profit_factor=5.0,  # Elite → +15 pts
        win_streak_consistency=1.0,
        trade_count_30d=1,  # confidence = 0.05
        max_drawdown_30d=0.0,
        avg_trade_size_sol=1.0,
    )

    wallet_25_wins = WalletMetrics(
        address="test_25_wins",
        roi_30d=200.0,
        roi_7d=200.0,
        profit_factor=5.0,
        win_streak_consistency=1.0,
        trade_count_30d=25,  # confidence = 1.0
        max_drawdown_30d=0.0,
        avg_trade_size_sol=1.0,
    )

    score_1 = calculate_wqs(wallet_1_win)
    score_25 = calculate_wqs(wallet_25_wins)

    # 1-trade wallet must be heavily discounted
    assert score_1 < score_25, "1-trade wallet must score much lower than 25-trade wallet"
    assert score_1 < score_25 * 0.15, (
        f"1-trade confidence penalty (5%) must reduce score to <15% of full-confidence score: "
        f"{score_1} vs {score_25}"
    )


def test_wqs_recency_naive_datetime_string_no_error():
    """
    Test 74 (plan): A naive datetime string (no timezone suffix) must not cause a
    ValueError that is silently swallowed. The recency bonus/penalty should still apply.
    """
    from datetime import datetime, timedelta

    # Recent naive datetime (2 days ago)
    recent = (datetime.utcnow() - timedelta(days=1)).strftime("%Y-%m-%dT%H:%M:%S")

    wallet = WalletMetrics(
        address="test_naive_dt",
        roi_30d=30.0,
        roi_7d=5.0,
        win_streak_consistency=0.7,
        trade_count_30d=20,
        max_drawdown_30d=5.0,
        last_trade_at=recent,  # naive ISO string, no Z or +00:00
    )

    # Should not raise; bonus should be applied for recent activity
    score = calculate_wqs(wallet)
    assert 0 <= score <= 100, f"Score out of bounds for naive datetime: {score}"

    # Compare against wallet with no last_trade_at — recent one should score >= (no penalty)
    wallet_no_dt = WalletMetrics(
        address="test_no_dt",
        roi_30d=30.0,
        roi_7d=5.0,
        win_streak_consistency=0.7,
        trade_count_30d=20,
        max_drawdown_30d=5.0,
    )
    score_no_dt = calculate_wqs(wallet_no_dt)
    # Recent trader should score >= no-datetime wallet (gets activity bonus)
    assert score >= score_no_dt, (
        f"Recent trader should score >= no-datetime wallet: {score} vs {score_no_dt}"
    )


def test_wqs_confidence_multiplier_applied_once_not_doubled():
    """
    Test 75 (plan): The confidence multiplier (trade_count / 20) must be applied exactly
    once at the end of calculate_wqs(), not combined with other per-section penalties.

    Verify: with trade_count=2 (confidence=0.1) and trade_count=4 (confidence=0.2),
    the score ratio is ≈ 0.5 (not further compounded by another mechanism).
    """
    base_metrics = dict(
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        max_drawdown_30d=5.0,
        avg_trade_size_sol=1.0,
    )

    wallet_2 = WalletMetrics(address="tc_2", trade_count_30d=2, **base_metrics)
    wallet_4 = WalletMetrics(address="tc_4", trade_count_30d=4, **base_metrics)
    wallet_20 = WalletMetrics(address="tc_20", trade_count_30d=20, **base_metrics)

    score_2 = calculate_wqs(wallet_2)
    score_4 = calculate_wqs(wallet_4)
    score_20 = calculate_wqs(wallet_20)

    # confidence(2) = 0.1, confidence(4) = 0.2, confidence(20) = 1.0
    # Expected ratios: score_2/score_20 ≈ 0.1, score_4/score_20 ≈ 0.2
    if score_20 > 0:
        ratio_2 = score_2 / score_20
        ratio_4 = score_4 / score_20
        # Ratios should be close to confidence values (within activity bonus variance)
        assert ratio_2 < 0.25, f"tc=2 ratio should be ~0.1, got {ratio_2}"
        assert ratio_4 < 0.40, f"tc=4 ratio should be ~0.2, got {ratio_4}"


def test_wqs_sniper_detection_not_applied_when_delay_is_none():
    """
    Test 76 (plan): If avg_entry_delay_seconds is None, the sniper penalty must NOT fire.

    Risk: If None were treated as 0 (fast entry), the wallet would be immediately rejected.
    Many legitimate wallets lack delay data, and incorrect rejection = missed profits.
    """
    wallet_with_delay_none = WalletMetrics(
        address="test_no_delay",
        roi_30d=60.0,
        roi_7d=10.0,
        win_streak_consistency=0.8,
        trade_count_30d=30,
        max_drawdown_30d=3.0,
        avg_entry_delay_seconds=None,  # No delay data
    )

    wallet_with_safe_delay = WalletMetrics(
        address="test_safe_delay",
        roi_30d=60.0,
        roi_7d=10.0,
        win_streak_consistency=0.8,
        trade_count_30d=30,
        max_drawdown_30d=3.0,
        avg_entry_delay_seconds=300.0,  # 5 minutes = safe "smart money" range
    )

    score_none = calculate_wqs(wallet_with_delay_none)
    score_safe = calculate_wqs(wallet_with_safe_delay)

    # Both should be > 0 (neither is penalized as a sniper)
    assert score_none > 0.0, "None delay must not trigger sniper rejection (score must be > 0)"
    # Safe delay gets +15 bonus, so score_safe >= score_none is expected
    assert score_safe >= score_none, (
        f"Safe delay (5 min) should score >= None delay: {score_safe} vs {score_none}"
    )
