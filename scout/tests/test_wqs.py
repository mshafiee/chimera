"""Tests for Wallet Quality Score (WQS) calculation"""

import pytest
from scout.core.wqs import calculate_wqs


def test_wqs_basic_calculation():
    """Test basic WQS calculation"""
    wallet = {
        'roi_30d': 50.0,
        'win_streak_consistency': 0.8,
        'roi_7d': 10.0,
        'trade_count_30d': 25,
        'max_drawdown_30d': 5.0,
    }
    
    score = calculate_wqs(wallet)
    assert 0 <= score <= 100


def test_wqs_low_trade_count_penalty():
    """Test that low trade count reduces confidence"""
    wallet_low = {
        'roi_30d': 50.0,
        'win_streak_consistency': 0.8,
        'roi_7d': 10.0,
        'trade_count_30d': 10,  # Low count
        'max_drawdown_30d': 5.0,
    }
    
    wallet_high = {
        'roi_30d': 50.0,
        'win_streak_consistency': 0.8,
        'roi_7d': 10.0,
        'trade_count_30d': 50,  # High count
        'max_drawdown_30d': 5.0,
    }
    
    score_low = calculate_wqs(wallet_low)
    score_high = calculate_wqs(wallet_high)
    
    assert score_high > score_low


def test_wqs_spike_penalty():
    """Test that recent spikes are penalized"""
    wallet_spike = {
        'roi_30d': 50.0,
        'win_streak_consistency': 0.8,
        'roi_7d': 120.0,  # 7d > 2x 30d (spike)
        'trade_count_30d': 25,
        'max_drawdown_30d': 5.0,
    }
    
    wallet_normal = {
        'roi_30d': 50.0,
        'win_streak_consistency': 0.8,
        'roi_7d': 12.0,  # Normal
        'trade_count_30d': 25,
        'max_drawdown_30d': 5.0,
    }
    
    score_spike = calculate_wqs(wallet_spike)
    score_normal = calculate_wqs(wallet_normal)
    
    assert score_normal > score_spike


def test_wqs_drawdown_penalty():
    """Test that high drawdown reduces score"""
    wallet_low_dd = {
        'roi_30d': 50.0,
        'win_streak_consistency': 0.8,
        'roi_7d': 10.0,
        'trade_count_30d': 25,
        'max_drawdown_30d': 2.0,  # Low drawdown
    }
    
    wallet_high_dd = {
        'roi_30d': 50.0,
        'win_streak_consistency': 0.8,
        'roi_7d': 10.0,
        'trade_count_30d': 25,
        'max_drawdown_30d': 15.0,  # High drawdown
    }
    
    score_low = calculate_wqs(wallet_low_dd)
    score_high = calculate_wqs(wallet_high_dd)
    
    assert score_low > score_high


def test_wqs_bounds():
    """Test that WQS is always between 0 and 100"""
    test_cases = [
        {'roi_30d': -100, 'win_streak_consistency': 0, 'roi_7d': -50, 'trade_count_30d': 5, 'max_drawdown_30d': 50},
        {'roi_30d': 200, 'win_streak_consistency': 1.0, 'roi_7d': 100, 'trade_count_30d': 100, 'max_drawdown_30d': 0},
    ]
    
    for wallet in test_cases:
        score = calculate_wqs(wallet)
        assert 0 <= score <= 100, f"WQS out of bounds: {score}"
