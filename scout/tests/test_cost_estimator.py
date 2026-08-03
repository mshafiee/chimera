"""Tests for core/cost_estimator.py - Fee estimation and caching."""

from unittest.mock import patch

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


@patch("core.cost_estimator.CostEstimator._load_fee_cache")
def test_build_rpc_url_with_key(mock_load):
    """An API key yields the default Helius RPC URL."""
    estimator = CostEstimator(helius_api_key="test-key")
    url = estimator._build_rpc_url()
    assert url == "https://mainnet.helius-rpc.com/"


@patch("core.cost_estimator.CostEstimator._load_fee_cache")
def test_build_rpc_url_prefers_env_url(mock_load, monkeypatch):
    """CHIMERA_RPC__PRIMARY_URL takes precedence over the API-key default."""
    monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "https://rpc.example.com")
    estimator = CostEstimator(helius_api_key="test-key")
    url = estimator._build_rpc_url()
    assert url == "https://rpc.example.com"


@patch("core.cost_estimator.CostEstimator._load_fee_cache")
def test_build_rpc_url_empty_key_fallback(mock_load, monkeypatch):
    """An empty-string key falls back to env vars and yields None when unset."""
    monkeypatch.delenv("CHIMERA_RPC__PRIMARY_URL", raising=False)
    monkeypatch.delenv("SOLANA_RPC_URL", raising=False)
    estimator = CostEstimator(helius_api_key="")
    assert estimator._build_rpc_url() is None
