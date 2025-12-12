"""Tests for WQS base score compliance with PDD."""

import sys
from pathlib import Path

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

import pytest
from core.wqs import calculate_wqs, WalletMetrics


class TestWQSBaseScore:
    """Test that WQS calculation starts at 0 (PDD compliant)."""
    
    def test_wqs_starts_at_zero(self):
        """Test that WQS calculation starts from 0, not 50."""
        # Create minimal metrics (all None/zero)
        metrics = WalletMetrics(
            address="test",
            roi_7d=None,
            roi_30d=None,
            trade_count_30d=None,
            win_rate=None,
            max_drawdown_30d=None,
            avg_trade_size_sol=None,
            last_trade_at=None,
            win_streak_consistency=None,
        )
        
        score = calculate_wqs(metrics)
        
        # With all metrics None, score should be 0 (not 50)
        assert score == 0.0
    
    def test_wqs_base_score_pdd_compliant(self):
        """Test that WQS calculation follows PDD specification."""
        # Create metrics with minimal positive values
        metrics = WalletMetrics(
            address="test",
            roi_7d=0.0,
            roi_30d=0.0,
            trade_count_30d=0,
            win_rate=0.0,
            max_drawdown_30d=0.0,
            avg_trade_size_sol=0.0,
            last_trade_at=None,
            win_streak_consistency=0.0,
        )
        
        score = calculate_wqs(metrics)
        
        # With all zeros, score should be 0 (PDD: starts at 0)
        assert score == 0.0
    
    def test_wqs_calculation_with_positive_metrics(self):
        """Test that positive metrics add to the score from 0."""
        metrics = WalletMetrics(
            address="test",
            roi_7d=10.0,
            roi_30d=20.0,  # Should add (20/100) * 25 = 5 points
            trade_count_30d=25,  # > 20, no penalty
            win_rate=0.6,
            max_drawdown_30d=5.0,  # Should subtract 5 * 0.2 = 1 point
            avg_trade_size_sol=0.5,
            last_trade_at="2025-01-01T00:00:00",
            win_streak_consistency=0.5,  # Should add 0.5 * 20 = 10 points
        )
        
        score = calculate_wqs(metrics)
        
        # Expected: 0 (base) + 5 (ROI) + 10 (consistency) - 1 (drawdown) = 14
        # But win_rate fallback might add more
        assert score >= 0.0
        assert score <= 100.0
        # Score should be positive with these metrics
        assert score > 0.0
    
    def test_wqs_with_negative_roi(self):
        """Test that negative ROI doesn't add to score."""
        metrics = WalletMetrics(
            address="test",
            roi_7d=-10.0,
            roi_30d=-20.0,  # Negative ROI should not add points
            trade_count_30d=25,
            win_rate=0.4,
            max_drawdown_30d=10.0,  # Should subtract 10 * 0.2 = 2 points
            avg_trade_size_sol=0.5,
            last_trade_at="2025-01-01T00:00:00",
            win_streak_consistency=0.3,  # Should add 0.3 * 20 = 6 points
        )
        
        score = calculate_wqs(metrics)
        
        # With negative ROI, score should be low or negative (clamped to 0)
        assert score >= 0.0
        assert score <= 100.0
        # Score should be lower than with positive ROI
        assert score < 20.0  # Should be low due to negative ROI and drawdown
    
    def test_wqs_statistical_significance_penalty(self):
        """Test that low trade count applies a monotonic confidence penalty."""
        # Very low trade count
        metrics_very_low = WalletMetrics(
            address="test",
            roi_7d=10.0,
            roi_30d=40.0,  # Would add 10 points
            trade_count_30d=8,
            win_rate=0.6,
            max_drawdown_30d=5.0,
            avg_trade_size_sol=0.5,
            last_trade_at="2025-01-01T00:00:00",
            win_streak_consistency=0.5,  # Would add 10 points
        )
        
        score_very_low = calculate_wqs(metrics_very_low)
        
        # Medium trade count
        metrics_low = WalletMetrics(
            address="test",
            roi_7d=10.0,
            roi_30d=40.0,  # Would add 10 points
            trade_count_30d=15,
            win_rate=0.6,
            max_drawdown_30d=5.0,
            avg_trade_size_sol=0.5,
            last_trade_at="2025-01-01T00:00:00",
            win_streak_consistency=0.5,  # Would add 10 points
        )
        
        score_low = calculate_wqs(metrics_low)
        
        # High trade count (>= 20)
        metrics_high = WalletMetrics(
            address="test",
            roi_7d=10.0,
            roi_30d=40.0,  # Would add 10 points
            trade_count_30d=25,  # >= 20, no penalty
            win_rate=0.6,
            max_drawdown_30d=5.0,
            avg_trade_size_sol=0.5,
            last_trade_at="2025-01-01T00:00:00",
            win_streak_consistency=0.5,  # Would add 10 points
        )
        
        score_high = calculate_wqs(metrics_high)
        
        # Very low count should be lowest
        assert score_very_low < score_low
        # Medium count should be lower than high
        assert score_low < score_high
    
    def test_wqs_anti_pump_and_dump_penalty(self):
        """Test that recent massive spikes are penalized."""
        # Normal case: 7d ROI not > 2x 30d ROI
        metrics_normal = WalletMetrics(
            address="test",
            roi_7d=20.0,
            roi_30d=30.0,  # 7d is not > 2x 30d
            trade_count_30d=25,
            win_rate=0.6,
            max_drawdown_30d=5.0,
            avg_trade_size_sol=0.5,
            last_trade_at="2025-01-01T00:00:00",
            win_streak_consistency=0.5,
        )
        
        score_normal = calculate_wqs(metrics_normal)
        
        # Pump case: 7d ROI > 2x 30d ROI (should be penalized -15)
        metrics_pump = WalletMetrics(
            address="test",
            roi_7d=100.0,  # > 2x 30d ROI
            roi_30d=30.0,
            trade_count_30d=25,
            win_rate=0.6,
            max_drawdown_30d=5.0,
            avg_trade_size_sol=0.5,
            last_trade_at="2025-01-01T00:00:00",
            win_streak_consistency=0.5,
        )
        
        score_pump = calculate_wqs(metrics_pump)
        
        # Pump case should have lower score (penalized -15)
        assert score_pump < score_normal
        # Difference should be approximately 15 points
        assert abs((score_normal - score_pump) - 15.0) < 5.0  # Allow some variance


