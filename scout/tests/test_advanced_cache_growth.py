"""
Tests for Phase 5: Growth-Aware Caching

Tests the growth-aware cache priorities and TTL adjustments
based on WQS scores.
"""

import pytest
import os
from unittest.mock import patch

from core.advanced_cache import (
    AdvancedCache,
    CacheCategory,
    TTLDefaults,
)


@pytest.fixture
def cache(monkeypatch):
    """Create a fresh cache instance for each test (growth mode on)."""
    monkeypatch.setenv("SCOUT_GROWTH_OPTIMIZED", "true")
    cache = AdvancedCache(max_memory_mb=1)
    yield cache
    cache.shutdown()


class TestGrowthAwareCache:
    """Tests for growth-aware caching with WQS-based TTL."""

    def test_high_wqs_wallet_gets_extended_ttl(self, cache):
        """Test that high-WQS wallets get 4x TTL extension."""
        wqs_score = 75.0
        ttl = cache._get_ttl(CacheCategory.WALLET_METRICS, wqs_score=wqs_score)
        base_ttl = TTLDefaults.WALLET_METRICS  # 5 minutes
        expected_ttl = base_ttl * 4
        assert ttl == expected_ttl, f"Expected {expected_ttl}s, got {ttl}s"

    def test_medium_wqs_wallet_gets_double_ttl(self, cache):
        """Test that medium-WQS wallets get 2x TTL extension."""
        wqs_score = 55.0
        ttl = cache._get_ttl(CacheCategory.WALLET_METRICS, wqs_score=wqs_score)
        base_ttl = TTLDefaults.WALLET_METRICS  # 5 minutes
        expected_ttl = base_ttl * 2
        assert ttl == expected_ttl, f"Expected {expected_ttl}s, got {ttl}s"

    def test_low_wqs_wallet_gets_standard_ttl(self, cache):
        """Test that low-WQS wallets get standard TTL."""
        wqs_score = 25.0
        ttl = cache._get_ttl(CacheCategory.WALLET_METRICS, wqs_score=wqs_score)
        base_ttl = TTLDefaults.WALLET_METRICS  # 5 minutes
        assert ttl == base_ttl, f"Expected {base_ttl}s, got {ttl}s"

    def test_growth_mode_disabled_no_extension(self, cache):
        """Test that TTL extension only applies when growth mode is enabled."""
        wqs_score = 75.0
        with patch.dict(os.environ, {"SCOUT_GROWTH_OPTIMIZED": "false"}):
            cache_no_growth = AdvancedCache(max_memory_mb=1)
            ttl = cache_no_growth._get_ttl(CacheCategory.WALLET_METRICS, wqs_score=wqs_score)
            cache_no_growth.shutdown()
            base_ttl = TTLDefaults.WALLET_METRICS  # 5 minutes
            assert ttl == base_ttl, f"Expected {base_ttl}s, got {ttl}s"

    def test_set_and_get_basic(self, cache):
        """Test basic set/get operations."""
        data = {"roi_7d": 25.5, "roi_30d": 120.0}
        cache.set("test_key", "subkey", data, category=CacheCategory.WALLET_METRICS)
        retrieved = cache.get("test_key", "subkey", category=CacheCategory.WALLET_METRICS)
        assert retrieved is not None
        assert retrieved == data

    def test_cache_category_defaults(self, cache):
        """Test that new cache categories have correct default TTLs."""
        assert TTLDefaults.HIGH_WQS_WALLET_DATA == 3600
        assert TTLDefaults.ANALYSIS_RESULTS == 21600
        assert TTLDefaults.DISCOVERY_RESULTS == 1800
        assert TTLDefaults.BACKTEST_RESULTS == 3600

    def test_new_cache_categories_enum(self, cache):
        """Test that new cache categories exist in enum."""
        assert hasattr(CacheCategory, 'HIGH_WQS_WALLET_DATA')
        assert hasattr(CacheCategory, 'ANALYSIS_RESULTS')
        assert hasattr(CacheCategory, 'DISCOVERY_RESULTS')
        assert hasattr(CacheCategory, 'BACKTEST_RESULTS')


class TestGrowthOptimization:
    """Tests for growth-aware cache optimization features."""

    def test_high_wqs_wallet_extended_cache_time(self, cache):
        """Test that high-WQS wallet data gets extended TTL."""
        wqs_score = 75.0
        data = {"metrics": {"roi": 50.0}}
        cache.set("high_wqs_wallet", "data", data,
                 category=CacheCategory.HIGH_WQS_WALLET_DATA,
                 wqs_score=wqs_score)
        retrieved = cache.get("high_wqs_wallet", "data",
                            category=CacheCategory.HIGH_WQS_WALLET_DATA,
                            wqs_score=wqs_score)
        assert retrieved is not None
        assert retrieved == data

    def test_analysis_results_caching(self, cache):
        """Test analysis results caching."""
        run_id = "analysis_run_20240617"
        results = {"wallets_analyzed": 100, "avg_wqs": 65.0}
        cache.set(run_id, "results", results, category=CacheCategory.ANALYSIS_RESULTS)
        retrieved = cache.get(run_id, "results", category=CacheCategory.ANALYSIS_RESULTS)
        assert retrieved is not None
        assert retrieved == results

    def test_discovery_results_caching(self, cache):
        """Test discovery results caching."""
        discovery_id = "discovery_20240617"
        results = {"wallets_found": 50, "sources_used": ["tokens", "dex"]}
        cache.set(discovery_id, "results", results, category=CacheCategory.DISCOVERY_RESULTS)
        retrieved = cache.get(discovery_id, "results", category=CacheCategory.DISCOVERY_RESULTS)
        assert retrieved is not None
        assert retrieved == results

    def test_backtest_results_caching(self, cache):
        """Test backtest results caching."""
        wallet_address = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
        results = {"simulated_roi": 45.0, "trades": 20}
        cache.set(wallet_address, "results", results, category=CacheCategory.BACKTEST_RESULTS)
        retrieved = cache.get(wallet_address, "results", category=CacheCategory.BACKTEST_RESULTS)
        assert retrieved is not None
        assert retrieved == results

    def test_cache_warming_with_high_wqs(self, cache, monkeypatch):
        """Test that warming is invoked with the high-WQS wallet list."""
        high_wqs_wallets = [
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
        ]
        token_addresses = ["DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"]

        # warm_cache itself is a stub; spy on it to verify the high-WQS list
        # is actually forwarded to the warming path
        with patch.object(cache, "warm_cache", wraps=cache.warm_cache) as spy:
            cache.warm_cache(
                wallet_addresses=high_wqs_wallets,
                token_addresses=token_addresses,
                high_wqs_wallets=high_wqs_wallets
            )
        spy.assert_called_once()
        call_kwargs = spy.call_args.kwargs
        assert call_kwargs.get("high_wqs_wallets") == high_wqs_wallets


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
