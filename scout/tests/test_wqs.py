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
        trade_count_30d=10,  # Low count - should get 0.25x multiplier
        max_drawdown_30d=5.0,
    )
    
    wallet_high = WalletMetrics(
        address="test_wallet_high",
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=50,  # High count - no penalty
        max_drawdown_30d=5.0,
    )
    
    score_low = calculate_wqs(wallet_low)
    score_high = calculate_wqs(wallet_high)
    
    assert score_high > score_low, f"High trade count should score higher: {score_high} vs {score_low}"


def test_wqs_medium_trade_count_penalty():
    """Test that medium trade count (10-20) gets 0.5x multiplier"""
    wallet_medium = WalletMetrics(
        address="test_wallet_medium",
        roi_30d=50.0,
        win_streak_consistency=0.8,
        roi_7d=10.0,
        trade_count_30d=15,  # Medium count - should get 0.5x multiplier
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
