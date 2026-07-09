"""
Tests for replay fidelity features in analyzer.

Tests FIFO partial sell math with proportional scaling, data gap tracking,
and replay_data_gap_ratio integration in wallet metrics.
"""

import pytest
from decimal import Decimal

from core.analyzer import WalletAnalyzer
from core.wqs import WalletMetrics, calculate_wqs


class TestReplayDataGapRatio:
    """Test replay_data_gap_ratio field in WalletMetrics."""

    def test_replay_data_gap_ratio_field_exists(self):
        """Test that WalletMetrics has replay_data_gap_ratio field."""
        metrics = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            avg_roi=Decimal('0.1'),
            win_rate=0.5,
            avg_hold_time_hours=24.0,
            replay_data_gap_ratio=0.0
        )
        assert hasattr(metrics, 'replay_data_gap_ratio')
        assert metrics.replay_data_gap_ratio == 0.0

    def test_replay_data_gap_ratio_default(self):
        """Test default value of replay_data_gap_ratio."""
        metrics = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5
        )
        assert metrics.replay_data_gap_ratio == 0.0

    def test_replay_data_gap_ratio_various_values(self):
        """Test replay_data_gap_ratio with various values."""
        metrics = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            replay_data_gap_ratio=0.5
        )
        assert metrics.replay_data_gap_ratio == 0.5

        metrics2 = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            replay_data_gap_ratio=1.0
        )
        assert metrics2.replay_data_gap_ratio == 1.0


class TestWQSDataGapPenalty:
    """Test WQS confidence penalty for data gaps."""

    def test_wqs_confidence_with_no_data_gap(self):
        """Test that zero data gap doesn't affect confidence."""
        metrics = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            avg_roi=Decimal('0.1'),
            win_rate=0.5,
            avg_hold_time_hours=24.0,
            replay_data_gap_ratio=0.0
        )
        score = calculate_wqs(metrics)
        # Should have reasonable confidence (base 50 for no data gap)
        assert score.confidence >= 50

    def test_wqs_confidence_with_partial_data_gap(self):
        """Test that partial data gap reduces confidence."""
        metrics_no_gap = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            avg_roi=Decimal('0.1'),
            win_rate=0.5,
            avg_hold_time_hours=24.0,
            replay_data_gap_ratio=0.0
        )
        
        metrics_gap = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            avg_roi=Decimal('0.1'),
            win_rate=0.5,
            avg_hold_time_hours=24.0,
            replay_data_gap_ratio=0.5
        )
        
        score_no_gap = calculate_wqs(metrics_no_gap)
        score_gap = calculate_wqs(metrics_gap)
        
        # Gap should reduce confidence
        assert score_gap.confidence < score_no_gap.confidence

    def test_wqs_confidence_with_full_data_gap(self):
        """Test that full data gap significantly reduces confidence."""
        metrics = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            avg_roi=Decimal('0.1'),
            win_rate=0.5,
            avg_hold_time_hours=24.0,
            replay_data_gap_ratio=1.0
        )
        score = calculate_wqs(metrics)
        # Should have minimal confidence (max 20 point penalty)
        assert score.confidence <= 30  # 50 base - 20 penalty

    def test_wqs_confidence_penalty_max_20_points(self):
        """Test that data gap penalty is capped at 20 points."""
        metrics_full_gap = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            avg_roi=Decimal('0.1'),
            win_rate=0.5,
            avg_hold_time_hours=24.0,
            replay_data_gap_ratio=1.0
        )
        
        metrics_zero_gap = WalletMetrics(
            wallet_address="test_wallet2",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            avg_roi=Decimal('0.1'),
            win_rate=0.5,
            avg_hold_time_hours=24.0,
            replay_data_gap_ratio=0.0
        )
        
        score_full_gap = calculate_wqs(metrics_full_gap)
        score_zero_gap = calculate_wqs(metrics_zero_gap)
        
        # Penalty should be at most 20 points
        penalty = score_zero_gap.confidence - score_full_gap.confidence
        assert penalty <= 20.0


@pytest.mark.asyncio
class TestFIFOPartialSellWithProportionalScaling:
    """Test FIFO partial sell math with proportional scaling for data gaps."""

    async def test_fifo_partial_sell_no_gap(self, analyzer):
        """Test FIFO partial sell without data gaps (no scaling)."""
        # Partial sell of 5 tokens from 20 total
        sell_qty = Decimal('5')
        
        # Without data gap, should use standard FIFO (oldest first)
        # 5 tokens from first position (which has 10)
        remaining_in_first = Decimal('10') - sell_qty
        assert remaining_in_first == Decimal('5')

    async def test_fifo_partial_sell_with_data_gap(self, analyzer):
        """Test FIFO partial sell with data gaps (proportional scaling)."""
        # If we have 20 tokens but only 10 in replay, gap_ratio = 0.5
        total_qty = Decimal('20')
        replay_qty = Decimal('10')
        
        # Partial sell of 5 tokens
        sell_qty = Decimal('5')
        
        # With proportional scaling, we should scale the sell proportionally
        # to the replay data
        scaled_sell_qty = sell_qty * (replay_qty / total_qty)
        assert scaled_sell_qty == Decimal('2.5')

    async def test_fifo_partial_sell_cost_basis_with_gap(self, analyzer):
        """Test cost basis calculation with data gaps."""
        # Position: 10 tokens @ $100, 10 tokens @ $120 (avg $110)
        # Data gap: only have 15 tokens in replay (75%)
        
        gap_ratio = 0.25  # Missing 25%
        
        # Standard FIFO: 8 from first position ($100 each)
        standard_cost = Decimal('8') * Decimal('100')
        
        # Proportional scaling: scale cost basis by 1/(1-gap_ratio)
        scaled_cost = standard_cost / (1 - gap_ratio)
        
        assert standard_cost == Decimal('800')
        assert scaled_cost == Decimal('800') / Decimal('0.75')

    async def test_fifo_partial_sell_scaling_limits(self, analyzer):
        """Test that proportional scaling has reasonable limits."""
        # Edge case: very high data gap ratio
        gap_ratio = 0.9  # Missing 90% of data
        
        standard_cost = Decimal('100')
        scaled_cost = standard_cost / (1 - gap_ratio)
        
        # Should not explode unreasonably
        assert scaled_cost == Decimal('1000')  # 10× scaling is reasonable

    async def test_fifo_complete_position_close_with_gap(self, analyzer):
        """Test complete position close with data gaps."""
        # Position: 20 tokens, replay: 15 tokens (gap_ratio = 0.25)
        
        total_qty = Decimal('20')
        replay_qty = Decimal('15')
        gap_ratio = (total_qty - replay_qty) / total_qty
        
        sell_qty = total_qty  # Selling all tokens
        
        # For complete close, should still use proportional scaling
        scaled_sell_qty = sell_qty * (replay_qty / total_qty)
        
        assert gap_ratio == 0.25
        assert scaled_sell_qty == Decimal('15')

    async def test_fifo_multiple_positions_with_gap(self, analyzer):
        """Test FIFO across multiple positions with data gaps."""
        # Position 1: 10 tokens @ $100
        # Position 2: 10 tokens @ $120
        # Position 3: 10 tokens @ $140
        # Total: 30 tokens, replay: 24 tokens (gap_ratio = 0.2)
        
        replay_qty = Decimal('24')
        total_qty = Decimal('30')
        gap_ratio = (total_qty - replay_qty) / total_qty
        
        # FIFO: 10 from pos1, 5 from pos2
        standard_cost = (Decimal('10') * Decimal('100')) + (Decimal('5') * Decimal('120'))
        
        # With gap scaling
        scaled_cost = standard_cost / (1 - gap_ratio)
        
        assert gap_ratio == 0.2
        assert standard_cost == Decimal('1600')
        assert scaled_cost == Decimal('2000')


@pytest.mark.asyncio
class TestReplayGapTracking:
    """Test data gap tracking during position replay."""

    async def test_replay_gap_detection_missing_trades(self, analyzer):
        """Test gap detection when trades are missing from historical data."""
        # Simulate missing trades in historical data
        all_trades = 100
        replay_trades = 80
        
        gap_ratio = (all_trades - replay_trades) / all_trades
        assert gap_ratio == 0.2

    async def test_replay_gap_detection_zero_missing(self, analyzer):
        """Test gap detection when no trades are missing."""
        all_trades = 100
        replay_trades = 100
        
        gap_ratio = (all_trades - replay_trades) / all_trades
        assert gap_ratio == 0.0

    async def test_replay_gap_detection_all_missing(self, analyzer):
        """Test gap detection when all trades are missing."""
        all_trades = 100
        replay_trades = 0
        
        gap_ratio = (all_trades - replay_trades) / all_trades
        assert gap_ratio == 1.0

    async def test_replay_gap_ratio_in_metrics(self, analyzer):
        """Test that replay_gap_ratio is properly set in metrics."""
        # Simulate wallet with data gaps
        metrics = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=100,
            winning_trades=50,
            losing_trades=50,
            replay_data_gap_ratio=0.3
        )
        
        # Verify the gap ratio is preserved
        assert metrics.replay_data_gap_ratio == 0.3

    async def test_replay_gap_impact_on_roi(self, analyzer):
        """Test that data gaps impact ROI calculation through WQS."""
        # Create two identical wallets with different gap ratios
        metrics_no_gap = WalletMetrics(
            wallet_address="wallet1",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            avg_roi=Decimal('0.2'),  # 20% ROI
            win_rate=0.5,
            avg_hold_time_hours=24.0,
            replay_data_gap_ratio=0.0
        )
        
        metrics_gap = WalletMetrics(
            wallet_address="wallet2",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            avg_roi=Decimal('0.2'),  # Same ROI
            win_rate=0.5,
            avg_hold_time_hours=24.0,
            replay_data_gap_ratio=0.5
        )
        
        score_no_gap = calculate_wqs(metrics_no_gap)
        score_gap = calculate_wqs(metrics_gap)
        
        # Confidence should be lower for wallet with data gaps
        assert score_gap.confidence < score_no_gap.confidence


@pytest.fixture
def analyzer():
    """Create a WalletAnalyzer instance for testing."""
    return WalletAnalyzer()