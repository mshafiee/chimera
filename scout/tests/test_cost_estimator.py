"""Tests for core/cost_estimator.py - Fee estimation and caching."""

from unittest.mock import patch, MagicMock
from core.cost_estimator import CostEstimator


@patch("core.cost_estimator.CostEstimator._load_fee_cache")
def test_cost_estimator_init(mock_load):
    estimator = CostEstimator()
    assert estimator is not None
    mock_load.assert_called_once()


@patch("core.cost_estimator.CostEstimator._load_fee_cache")
def test_cost_estimator_init_with_key(mock_load):
    estimator = CostEstimator(helius_api_key="test-key")
    assert estimator is not None
    assert "test-key" in estimator._api_key
