"""
Unit tests for Advanced Risk Features integration in WQS calculation.

Tests that CVaR, drawdown duration, and ulcer index penalties are correctly
applied to the WQS score when advanced risk features indicate danger.
"""

import pytest
from decimal import Decimal
from scout.core.wqs import (
    WalletMetrics,
    _calculate_raw_score,
    PenaltyCategory,
    calculate_wqs,
)


def create_test_metrics(
    roi_7d=10.0,
    roi_30d=20.0,
    win_rate=0.70,
    profit_factor=1.5,
    max_drawdown_30d=5.0,
    advanced_risk_features=None,
):
    """Helper to create test WalletMetrics."""
    return WalletMetrics(
        address="test_wallet",
        roi_7d=roi_7d,
        roi_30d=roi_30d,
        trade_count_30d=30,
        win_rate=win_rate,
        profit_factor=profit_factor,
        max_drawdown_30d=max_drawdown_30d,
        advanced_risk_features=advanced_risk_features,
    )


class TestAdvancedRiskFeaturesIntegration:
    """Test advanced risk features integration in WQS calculation."""

    def test_cvar_penalty_applied_for_negative_cvar_95(self):
        """Test that negative CVaR (losses) applies penalty."""
        metrics = create_test_metrics(
            advanced_risk_features={
                'extraction_success': True,
                'sample_count': 30,
                'cvar_95': -0.15,  # -15% average loss in worst 5% trades
                'max_drawdown_duration_trades': 10,
                'ulcer_index': 3.0,
            }
        )

        components = _calculate_raw_score(metrics)

        # CVaR penalty should be: abs(-0.15) * 0.2 = 0.03 (0.03 points)
        assert 'cvar' in components.components
        assert components.components['cvar'] < 0
        # CVaR is -0.15, penalty = abs(-0.15) * 0.2 = 0.03
        assert abs(components.components['cvar'] - (-0.03)) < 0.01  # ~0.03 point penalty

    def test_cvar_no_penalty_for_positive_cvar_95(self):
        """Test that positive CVaR (profits) does not penalize."""
        metrics = create_test_metrics(
            advanced_risk_features={
                'extraction_success': True,
                'sample_count': 30,
                'cvar_95': 0.10,  # +10% average profit in worst 5% trades
                'max_drawdown_duration_trades': 5,
                'ulcer_index': 2.0,
            }
        )

        components = _calculate_raw_score(metrics)

        # No CVaR penalty should be applied
        assert 'cvar' not in components.components or components.components.get('cvar', 0) == 0

    def test_drawdown_duration_penalty_applied(self):
        """Test that long drawdown duration applies penalty."""
        metrics = create_test_metrics(
            advanced_risk_features={
                'extraction_success': True,
                'sample_count': 30,
                'max_drawdown_duration_trades': 25,  # 25 trades to recover
                'cvar_95': 0.05,
                'ulcer_index': 3.0,
            }
        )

        components = _calculate_raw_score(metrics)

        # Drawdown duration penalty: 25 * 0.1 = 2.5 points
        assert 'drawdown_duration' in components.components
        assert components.components['drawdown_duration'] < 0
        assert abs(components.components['drawdown_duration'] - (-2.5)) < 0.1

    def test_drawdown_duration_no_penalty_under_threshold(self):
        """Test that drawdown duration under threshold has no penalty."""
        metrics = create_test_metrics(
            advanced_risk_features={
                'extraction_success': True,
                'sample_count': 30,
                'max_drawdown_duration_trades': 5,  # Only 5 trades to recover
                'cvar_95': 0.10,
                'ulcer_index': 2.0,
            }
        )

        components = _calculate_raw_score(metrics)

        # No drawdown duration penalty should be applied
        assert 'drawdown_duration' not in components.components or components.components.get('drawdown_duration', 0) == 0

    def test_ulcer_index_penalty_applied(self):
        """Test that high ulcer index applies penalty."""
        metrics = create_test_metrics(
            advanced_risk_features={
                'extraction_success': True,
                'sample_count': 30,
                'ulcer_index': 8.0,  # Severe, prolonged drawdown
                'cvar_95': -0.05,
                'max_drawdown_duration_trades': 10,
            }
        )

        components = _calculate_raw_score(metrics)

        # Ulcer index penalty: min(20.0, 8.0 * 0.5) = 4.0 points
        assert 'ulcer_index' in components.components
        assert components.components['ulcer_index'] < 0
        assert abs(components.components['ulcer_index'] - (-4.0)) < 0.1

    def test_ulcer_index_penalty_capped_at_20(self):
        """Test that ulcer index penalty is capped at 20 points."""
        metrics = create_test_metrics(
            advanced_risk_features={
                'extraction_success': True,
                'sample_count': 30,
                'ulcer_index': 50.0,  # Extremely high ulcer index
                'cvar_95': -0.10,
                'max_drawdown_duration_trades': 15,
            }
        )

        components = _calculate_raw_score(metrics)

        # Ulcer index penalty should be capped at 20.0
        assert components.components['ulcer_index'] >= -20.0

    def test_ulcer_index_no_penalty_under_threshold(self):
        """Test that ulcer index under threshold has no penalty."""
        metrics = create_test_metrics(
            advanced_risk_features={
                'extraction_success': True,
                'sample_count': 30,
                'ulcer_index': 3.0,  # Mild drawdown
                'cvar_95': 0.05,
                'max_drawdown_duration_trades': 5,
            }
        )

        components = _calculate_raw_score(metrics)

        # No ulcer index penalty should be applied
        assert 'ulcer_index' not in components.components or components.components.get('ulcer_index', 0) == 0

    def test_combined_penalties_reduce_score(self):
        """Test that multiple risk penalties compound to reduce WQS."""
        # Create a wallet with good base metrics but terrible advanced risk features
        metrics = create_test_metrics(
            roi_7d=15.0,
            roi_30d=25.0,
            win_rate=0.75,
            profit_factor=1.8,
            advanced_risk_features={
                'extraction_success': True,
                'sample_count': 30,
                'cvar_95': -0.20,           # Severe tail losses
                'max_drawdown_duration_trades': 30,  # Very slow recovery
                'ulcer_index': 10.0,        # Severe prolonged drawdown
            }
        )

        wqs = calculate_wqs(metrics)

        # With all these risk factors, WQS should be significantly reduced
        # (even though base ROI/win rate are good)
        assert wqs < 70, f"WQS {wqs} should be low due to severe advanced risk penalties"

    def test_no_penalties_when_extraction_failed(self):
        """Test that penalties are not applied when extraction failed."""
        metrics = create_test_metrics(
            advanced_risk_features={
                'extraction_success': False,  # Extraction failed
                'sample_count': 0,
                'cvar_95': 0.0,
                'max_drawdown_duration_trades': 0,
                'ulcer_index': 0.0,
            }
        )

        components = _calculate_raw_score(metrics)

        # No advanced risk penalties should be applied
        assert 'cvar' not in components.components
        assert 'dd_duration' not in components.components
        assert 'ulcer_index' not in components.components

    def test_no_penalties_when_insufficient_sample_size(self):
        """Test that penalties are not applied with insufficient sample size."""
        metrics = create_test_metrics(
            advanced_risk_features={
                'extraction_success': True,
                'sample_count': 3,  # Less than 5 trades
            }
        )

        components = _calculate_raw_score(metrics)

        # No advanced risk penalties should be applied
        assert 'cvar' not in components.components
        assert 'dd_duration' not in components.components
        assert 'ulcer_index' not in components.components

    def test_wqs_without_advanced_risk_features(self):
        """Test that WQS calculation works without advanced risk features."""
        metrics = create_test_metrics(
            advanced_risk_features=None  # No advanced risk features
        )

        components = _calculate_raw_score(metrics)
        wqs = calculate_wqs(metrics)

        # Should calculate normally without advanced risk features
        assert wqs >= 0
        assert wqs <= 100

    def test_spear_strategy_receives_penalties(self):
        """Test that SPEAR strategy also receives advanced risk penalties."""
        metrics = create_test_metrics(
            advanced_risk_features={
                'extraction_success': True,
                'sample_count': 30,
                'cvar_95': -0.15,
                'max_drawdown_duration_trades': 20,
                'ulcer_index': 5.0,
            }
        )

        components = _calculate_raw_score(metrics, strategy="SPEAR")

        # Penalties should still be applied for SPEAR strategy
        assert 'cvar' in components.components
        assert components.components['cvar'] < 0

    def test_good_wallet_no_penalties(self):
        """Test that a wallet with good advanced risk features receives minimal penalties."""
        metrics = create_test_metrics(
            roi_7d=30.0,  # Higher ROI to ensure good WQS despite other factors
            roi_30d=50.0,
            win_rate=0.75,
            profit_factor=2.0,
            max_drawdown_30d=3.0,  # Low drawdown
            advanced_risk_features={
                'extraction_success': True,
                'sample_count': 40,
                'cvar_95': 0.05,           # Positive even in worst 5%
                'max_drawdown_duration_trades': 3,  # Quick recovery
                'ulcer_index': 2.0,        # Low ulcer index
            }
        )

        components = _calculate_raw_score(metrics)
        wqs = calculate_wqs(metrics)

        # Should have reasonable WQS with minimal advanced risk penalties
        # (WQS may be affected by other base metrics, but advanced risk features shouldn't penalize)
        assert wqs > 30, f"Good wallet should have WQS > 30, got {wqs}"
        # Check that no severe advanced risk penalties were applied
        severe_risk_penalties = sum(
            1 for k, v in components.components.items()
            if k in ['cvar', 'drawdown_duration', 'ulcer_index'] and v < -5.0
        )
        assert severe_risk_penalties == 0, "Good wallet should have no severe advanced risk penalties"

    def test_penalty_category_enums_exist(self):
        """Test that new penalty categories are properly defined."""
        assert hasattr(PenaltyCategory, 'CVAR')
        assert hasattr(PenaltyCategory, 'DRAWDOWN_DURATION')
        assert hasattr(PenaltyCategory, 'ULCER_INDEX')

        # Verify they're enum values
        assert isinstance(PenaltyCategory.CVAR, type(PenaltyCategory.MARTINGALE))
        assert isinstance(PenaltyCategory.DRAWDOWN_DURATION, type(PenaltyCategory.MARTINGALE))
        assert isinstance(PenaltyCategory.ULCER_INDEX, type(PenaltyCategory.MARTINGALE))

    def test_string_to_penalty_mapping_updated(self):
        """Test that string mapping includes new penalty categories."""
        from scout.core.wqs import _STRING_TO_PENALTY

        assert 'cvar_penalty' in _STRING_TO_PENALTY
        assert 'drawdown_duration_penalty' in _STRING_TO_PENALTY
        assert 'ulcer_index_penalty' in _STRING_TO_PENALTY

        assert _STRING_TO_PENALTY['cvar_penalty'] == PenaltyCategory.CVAR
        assert _STRING_TO_PENALTY['drawdown_duration_penalty'] == PenaltyCategory.DRAWDOWN_DURATION
        assert _STRING_TO_PENALTY['ulcer_index_penalty'] == PenaltyCategory.ULCER_INDEX


if __name__ == '__main__':
    pytest.main([__file__, '-v'])
