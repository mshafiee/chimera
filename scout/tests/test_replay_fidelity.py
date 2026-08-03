"""
Tests for replay fidelity features in analyzer.

Tests FIFO partial sell math with proportional scaling, data gap tracking,
and replay_data_gap_ratio integration in wallet metrics.
"""


from core.wqs import WalletMetrics, calculate_wqs_with_confidence


def _metrics(gap_ratio, address="test_wallet"):
    """Build WalletMetrics with real field names and a data gap ratio."""
    return WalletMetrics(
        address=address,
        roi_30d=10.0,
        trade_count_30d=10,
        win_rate=0.5,
        avg_hold_time_hours=24.0,
        replay_data_gap_ratio=gap_ratio,
    )


class TestReplayDataGapRatio:
    """Test replay_data_gap_ratio field in WalletMetrics."""

    def test_replay_data_gap_ratio_field_exists(self):
        """Test that WalletMetrics has replay_data_gap_ratio field."""
        metrics = _metrics(0.0)
        assert hasattr(metrics, 'replay_data_gap_ratio')
        assert metrics.replay_data_gap_ratio == 0.0

    def test_replay_data_gap_ratio_default(self):
        """Test default value of replay_data_gap_ratio."""
        metrics = WalletMetrics(address="test_wallet", trade_count_30d=10, win_rate=0.5)
        assert metrics.replay_data_gap_ratio is None

    def test_replay_data_gap_ratio_various_values(self):
        """Test replay_data_gap_ratio with various values."""
        assert _metrics(0.5).replay_data_gap_ratio == 0.5
        assert _metrics(1.0).replay_data_gap_ratio == 1.0


class TestWQSDataGapPenalty:
    """Test WQS penalty for data gaps."""

    def test_wqs_score_with_no_data_gap(self):
        """Test that zero data gap doesn't apply a penalty."""
        result = calculate_wqs_with_confidence(_metrics(0.0))
        assert result.score > 0

    def test_wqs_score_with_partial_data_gap(self):
        """Test that a partial data gap reduces the score."""
        score_no_gap = calculate_wqs_with_confidence(_metrics(0.0))
        score_gap = calculate_wqs_with_confidence(_metrics(0.5))

        assert score_gap.score < score_no_gap.score

    def test_wqs_score_with_full_data_gap(self):
        """Test that a full data gap heavily penalizes the score."""
        score_full_gap = calculate_wqs_with_confidence(_metrics(1.0))
        score_zero_gap = calculate_wqs_with_confidence(_metrics(0.0))

        assert score_full_gap.score < score_zero_gap.score

    def test_wqs_gap_penalty_capped_at_20_points(self):
        """Test that the data gap penalty is capped (bounded, non-negative)."""
        score_full_gap = calculate_wqs_with_confidence(_metrics(1.0))
        score_zero_gap = calculate_wqs_with_confidence(_metrics(0.0))

        penalty = score_zero_gap.score - score_full_gap.score
        assert 0.0 <= penalty <= 20.0, f"Gap penalty must be bounded, got {penalty:.2f}"
        assert score_full_gap.score < score_zero_gap.score


class TestReplayGapImpactOnMetrics:
    """Test data gap integration in wallet metrics."""

    def test_replay_gap_ratio_preserved_in_metrics(self):
        """Test that the gap ratio is preserved through WQS computation."""
        metrics = _metrics(0.3)
        result = calculate_wqs_with_confidence(metrics)

        assert metrics.replay_data_gap_ratio == 0.3
        assert result.score is not None

    def test_replay_gap_impact_on_score(self):
        """Test that data gaps lower the WQS score (not confidence)."""
        score_no_gap = calculate_wqs_with_confidence(_metrics(0.0, "wallet1"))
        score_gap = calculate_wqs_with_confidence(_metrics(0.5, "wallet2"))

        # Confidence reflects sample size and is unaffected by gap ratio;
        # the score carries the replay-data-gap penalty.
        assert score_gap.confidence == score_no_gap.confidence
        assert score_gap.score < score_no_gap.score
