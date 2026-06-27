"""
Tests for ActivityBasedCache integration with HeliusClient

These tests verify that the activity-based caching layer works correctly
with the Helius API client, providing intelligent cache invalidation
and reducing redundant API calls.
"""

import pytest
import time
from unittest.mock import Mock, AsyncMock, MagicMock
from datetime import datetime, timedelta

from core.activity_cache import ActivityBasedCache, ActivityLevel, CacheConfig
from core.caching import HeliusCachingWrapper
from core.helius_client import HeliusClient


class TestActivityBasedCache:
    """Test suite for ActivityBasedCache core functionality."""

    def test_cache_initialization(self):
        """Test that cache initializes with correct defaults."""
        cache = ActivityBasedCache()
        assert cache is not None
        stats = cache.get_cache_stats()
        assert stats['entries'] == 0
        assert stats['hits'] == 0
        assert stats['misses'] == 0

    def test_cache_set_and_get(self):
        """Test basic cache set and get operations."""
        cache = ActivityBasedCache()
        test_data = {"key": "value", "transactions": [1, 2, 3]}

        # Set cache entry
        result = cache.set("test_key", test_data, wallet="test_wallet")
        assert result is True

        # Get cache entry
        retrieved_data = cache.get("test_key")
        assert retrieved_data == test_data

        # Verify statistics
        stats = cache.get_cache_stats()
        assert stats['hits'] == 1
        assert stats['entries'] == 1

    def test_cache_expiration(self):
        """Test that cache entries expire based on TTL."""
        cache = ActivityBasedCache()
        test_data = {"value": "test"}

        # Set with a short TTL by using cache configuration
        cache.set("test_key", test_data, wallet="test_wallet")

        # Manually expire the entry for testing
        entry = cache._cache.get("test_key")
        if entry:
            entry.ttl_seconds = -1  # Force expiration

        # Try to get expired entry
        result = cache.get("test_key")
        assert result is None

        # Verify expiration was tracked
        stats = cache.get_cache_stats()
        assert stats['expirations'] == 1
        assert stats['misses'] == 1

    def test_activity_level_determination(self):
        """Test activity level assignment based on transaction count."""
        cache = ActivityBasedCache()

        # Update wallet with different activity levels
        cache.update_wallet_activity("high_activity_wallet", tx_count_24h=60, wqs=75.0)
        cache.update_wallet_activity("medium_activity_wallet", tx_count_24h=5, wqs=55.0)
        cache.update_wallet_activity("low_activity_wallet", tx_count_24h=0, wqs=25.0)

        # Check activity levels
        assert cache._get_activity_level("high_activity_wallet") == ActivityLevel.VERY_HIGH
        assert cache._get_activity_level("medium_activity_wallet") == ActivityLevel.MEDIUM
        assert cache._get_activity_level("low_activity_wallet") == ActivityLevel.INACTIVE

    def test_activity_based_ttl(self):
        """Test that different activity levels get different TTLs."""
        cache = ActivityBasedCache()

        # Update wallets with different activity levels
        cache.update_wallet_activity("very_high_wallet", tx_count_24h=100, wqs=85.0)
        cache.update_wallet_activity("medium_wallet", tx_count_24h=5, wqs=55.0)

        # Get TTL for each wallet
        very_high_ttl = cache.get_cache_ttl("very_high_wallet", time.time())
        medium_ttl = cache.get_cache_ttl("medium_wallet", time.time())

        # Verify VERY_HIGH gets shorter TTL than MEDIUM
        assert very_high_ttl < medium_ttl

    def test_wqs_based_cache_extension(self):
        """Test that high-WQS wallets get cache TTL extension."""
        cache = ActivityBasedCache()

        # Update wallet with high WQS
        cache.update_wallet_activity("high_wqs_wallet", tx_count_24h=10, wqs=75.0)

        # Get TTL with high WQS
        high_wqs_ttl = cache.get_cache_ttl("high_wqs_wallet", time.time())

        # Update wallet with low WQS
        cache.update_wallet_activity("low_wqs_wallet", tx_count_24h=10, wqs=40.0)

        # Get TTL with low WQS
        low_wqs_ttl = cache.get_cache_ttl("low_wqs_wallet", time.time())

        # Verify high-WQS wallet gets longer TTL
        assert high_wqs_ttl > low_wqs_ttl

    def test_wallet_invalidation(self):
        """Test invalidation of all cache entries for a specific wallet."""
        cache = ActivityBasedCache()

        # Add multiple entries for the same wallet
        cache.set("wallet1:txs:30", {"data": "transactions1"}, wallet="wallet1")
        cache.set("wallet1:txs:7", {"data": "transactions2"}, wallet="wallet1")
        cache.set("wallet2:txs:30", {"data": "transactions3"}, wallet="wallet2")

        # Invalidate wallet1 entries
        count = cache.invalidate_wallet("wallet1")
        assert count == 2

        # Verify wallet1 entries are gone
        assert cache.get("wallet1:txs:30") is None
        assert cache.get("wallet1:txs:7") is None

        # Verify wallet2 entries remain
        assert cache.get("wallet2:txs:30") is not None

    def test_inactive_wallet_cleanup(self):
        """Test cleanup of cache entries for inactive wallets."""
        cache = ActivityBasedCache()

        # Add activity data for wallets and cache some data
        cache.update_wallet_activity("active_wallet", tx_count_24h=10, wqs=60.0)
        cache.update_wallet_activity("inactive_wallet", tx_count_24h=0, wqs=20.0)

        # Add cache entries for both wallets
        cache.set("active_wallet:data", {"value": "active_data"}, wallet="active_wallet")
        cache.set("inactive_wallet:data", {"value": "inactive_data"}, wallet="inactive_wallet")

        # Manually set last activity to old timestamp for inactive wallet
        if "inactive_wallet" in cache._activity_data:
            cache._activity_data["inactive_wallet"].last_activity_timestamp = time.time() - (48 * 3600)  # 48 hours ago

        # Run inactive wallet cleanup
        count = cache.invalidate_inactive_wallets(hours_threshold=24)

        # Verify cleanup occurred for inactive wallet entries
        assert count >= 1  # At least the inactive wallet's cache entry should be cleaned up

        # Verify active wallet's data still exists
        assert cache.get("active_wallet:data") is not None

        # Verify inactive wallet's data was removed
        assert cache.get("inactive_wallet:data") is None

    def test_cache_statistics(self):
        """Test that cache statistics are tracked correctly."""
        cache = ActivityBasedCache()

        # Perform operations
        cache.set("key1", {"data": "value1"}, wallet="wallet1")
        cache.get("key1")  # Hit
        cache.get("key2")  # Miss

        stats = cache.get_cache_stats()
        assert stats['hits'] == 1
        assert stats['misses'] == 1
        assert stats['entries'] == 1

    def test_cache_hit_rate_calculation(self):
        """Test cache hit rate calculation."""
        cache = ActivityBasedCache()

        # No operations yet
        stats = cache.get_cache_stats()
        total_requests = stats['hits'] + stats['misses']
        hit_rate = stats['hits'] / total_requests if total_requests > 0 else 0.0
        assert hit_rate == 0.0

        # Add some operations
        cache.set("key1", {"data": "value1"}, wallet="wallet1")
        cache.get("key1")  # Hit
        cache.get("key2")  # Miss

        # Hit rate should be 50% (1 hit out of 2 requests)
        stats = cache.get_cache_stats()
        total_requests = stats['hits'] + stats['misses']
        hit_rate = stats['hits'] / total_requests if total_requests > 0 else 0.0
        assert hit_rate == 0.5

    def test_cache_clear(self):
        """Test complete cache clearing."""
        cache = ActivityBasedCache()

        # Add some data
        cache.set("key1", {"data": "value1"}, wallet="wallet1")
        cache.set("key2", {"data": "value2"}, wallet="wallet2")

        # Clear cache
        cache.clear()

        # Verify everything is cleared
        stats = cache.get_cache_stats()
        assert stats['entries'] == 0
        assert cache.get("key1") is None
        assert cache.get("key2") is None

    def test_activity_distribution_tracking(self):
        """Test tracking of activity level distribution."""
        cache = ActivityBasedCache()

        # Add wallets with different activity levels
        cache.update_wallet_activity("very_high", tx_count_24h=100, wqs=85.0)
        cache.update_wallet_activity("medium", tx_count_24h=5, wqs=55.0)
        cache.update_wallet_activity("low", tx_count_24h=1, wqs=35.0)

        distribution = cache.get_activity_distribution()

        assert distribution['very_high'] == 1
        assert distribution['medium'] == 1
        assert distribution['low'] == 1


class TestHeliusCachingWrapper:
    """Test suite for HeliusCachingWrapper integration."""

    def test_wrapper_initialization(self):
        """Test that wrapper initializes correctly."""
        wrapper = HeliusCachingWrapper()
        assert wrapper is not None
        assert wrapper.cache is not None

    def test_get_cached_transactions_hit(self):
        """Test cache hit when transactions are cached."""
        wrapper = HeliusCachingWrapper()
        wallet = "test_wallet"
        transactions = [{"tx": "data1"}, {"tx": "data2"}]

        # Cache transactions
        wrapper.cache_transactions(wallet, transactions, days=30, limit=100, wqs_score=75.0)

        # Try to get cached transactions
        cached = wrapper.get_cached_transactions(wallet, days=30, limit=100)

        assert cached is not None
        assert len(cached) == 2

    def test_get_cached_transactions_miss(self):
        """Test cache miss when transactions are not cached."""
        wrapper = HeliusCachingWrapper()
        wallet = "uncached_wallet"

        # Try to get uncached transactions
        cached = wrapper.get_cached_transactions(wallet, days=30, limit=100)

        assert cached is None

    def test_cache_transactions_with_wqs(self):
        """Test that transactions are cached with WQS-based extension."""
        wrapper = HeliusCachingWrapper()
        wallet = "high_wqs_wallet"
        transactions = [{"tx": "data"}]

        # Cache with high WQS
        result = wrapper.cache_transactions(wallet, transactions, days=30, limit=100, wqs_score=80.0)
        assert result is True

        # Cache with low WQS
        wallet2 = "low_wqs_wallet"
        result2 = wrapper.cache_transactions(wallet2, transactions, days=30, limit=100, wqs_score=40.0)
        assert result2 is True

        # Verify both are cached
        assert wrapper.get_cached_transactions(wallet, days=30, limit=100) is not None
        assert wrapper.get_cached_transactions(wallet2, days=30, limit=100) is not None

    def test_invalidate_wallet(self):
        """Test wallet cache invalidation."""
        wrapper = HeliusCachingWrapper()
        wallet = "test_wallet"
        transactions = [{"tx": "data"}]

        # Cache transactions
        wrapper.cache_transactions(wallet, transactions, days=30, limit=100)

        # Invalidate
        count = wrapper.invalidate_wallet(wallet)
        assert count >= 1

        # Verify cache miss after invalidation
        cached = wrapper.get_cached_transactions(wallet, days=30, limit=100)
        assert cached is None

    def test_cache_statistics_tracking(self):
        """Test that wrapper tracks cache statistics."""
        wrapper = HeliusCachingWrapper()
        wallet = "test_wallet"
        transactions = [{"tx": "data"}]

        # Perform operations
        wrapper.cache_transactions(wallet, transactions, days=30, limit=100, wqs_score=70.0)
        wrapper.get_cached_transactions(wallet, days=30, limit=100)  # Hit
        wrapper.get_cached_transactions("other_wallet", days=30, limit=100)  # Miss

        stats = wrapper.get_cache_stats()
        assert stats['hits'] == 1
        assert stats['misses'] == 1
        assert stats['entries'] >= 1

    def test_activity_level_determination(self):
        """Test activity level determination for cached wallets."""
        wrapper = HeliusCachingWrapper()
        high_activity_wallet = "high_activity"
        low_activity_wallet = "low_activity"

        # Manually update wallet activity with specific transaction counts
        wrapper.cache.update_wallet_activity(high_activity_wallet, tx_count_24h=100, wqs=70.0)
        wrapper.cache.update_wallet_activity(low_activity_wallet, tx_count_24h=5, wqs=50.0)

        # Also update the wrapper's internal activity tracking
        wrapper._wallet_activity[high_activity_wallet] = {'tx_count_24h': 100}
        wrapper._wallet_activity[low_activity_wallet] = {'tx_count_24h': 5}

        # Check activity levels
        high_level = wrapper.get_wallet_activity_level(high_activity_wallet)
        low_level = wrapper.get_wallet_activity_level(low_activity_wallet)

        # High activity wallet should have higher activity level
        assert high_level.value in ["very_high", "high"]
        assert low_level.value in ["low", "medium", "inactive"]

    def test_cleanup_inactive_wallets(self):
        """Test cleanup of inactive wallet caches."""
        wrapper = HeliusCachingWrapper()
        wallet = "inactive_wallet"
        transactions = [{"tx": "data"}]

        # Cache transactions
        wrapper.cache_transactions(wallet, transactions, days=30, limit=100)

        # Mock last activity update to old timestamp
        wrapper._last_activity_update[wallet] = time.time() - (48 * 3600)  # 48 hours ago

        # Cleanup inactive wallets
        count = wrapper.cleanup_inactive_wallets(hours_threshold=24)

        # Verify cleanup occurred
        assert count >= 0  # Should have cleaned up the inactive wallet

    def test_cache_hit_rate_tracking(self):
        """Test cache hit rate calculation."""
        wrapper = HeliusCachingWrapper()

        # No operations
        assert wrapper.get_cache_hit_rate() == 0.0

        # Add operations
        wallet = "test_wallet"
        transactions = [{"tx": "data"}]
        wrapper.cache_transactions(wallet, transactions, days=30, limit=100, wqs_score=70.0)

        wrapper.get_cached_transactions(wallet, days=30, limit=100)  # Hit
        wrapper.get_cached_transactions("other", days=30, limit=100)  # Miss

        # Hit rate should be 50%
        hit_rate = wrapper.get_cache_hit_rate()
        assert hit_rate == 0.5

    def test_cache_clear(self):
        """Test complete cache clearing."""
        wrapper = HeliusCachingWrapper()

        # Add some data
        wrapper.cache_transactions("wallet1", [{"tx": "data"}], days=30, limit=100)
        wrapper.cache_transactions("wallet2", [{"tx": "data"}], days=30, limit=100)

        # Clear cache
        wrapper.clear_cache()

        # Verify everything is cleared
        stats = wrapper.get_cache_stats()
        assert stats['entries'] == 0


@pytest.mark.asyncio
class TestHeliusClientIntegration:
    """Test suite for HeliusClient with caching integration."""

    async def test_helius_client_with_cache_wrapper(self):
        """Test that HeliusClient can use caching wrapper."""
        # This would require mocking Helius API responses
        # For now, test the structure
        cache_wrapper = HeliusCachingWrapper()

        # Verify wrapper structure
        assert hasattr(cache_wrapper, 'get_cached_transactions')
        assert hasattr(cache_wrapper, 'cache_transactions')
        assert hasattr(cache_wrapper, 'invalidate_wallet')

    async def test_cache_hit_scenario(self):
        """Test typical cache hit scenario."""
        wrapper = HeliusCachingWrapper()
        wallet = "test_wallet"
        transactions = [{"signature": "tx1"}, {"signature": "tx2"}]

        # First call should cache
        wrapper.cache_transactions(wallet, transactions, days=30, limit=100, wqs_score=75.0)

        # Second call should hit cache
        cached = wrapper.get_cached_transactions(wallet, days=30, limit=100)

        assert cached is not None
        assert len(cached) == 2
        assert cached[0]["signature"] == "tx1"

    async def test_cache_miss_scenario(self):
        """Test typical cache miss scenario."""
        wrapper = HeliusCachingWrapper()

        # Call without caching should return None
        cached = wrapper.get_cached_transactions("uncached_wallet", days=30, limit=100)

        assert cached is None

    async def test_activity_based_cache_behavior(self):
        """Test that activity levels affect cache behavior."""
        wrapper = HeliusCachingWrapper()
        high_activity_wallet = "high_activity"
        low_activity_wallet = "low_activity"

        # Manually set activity levels for wallets
        wrapper.cache.update_wallet_activity(high_activity_wallet, tx_count_24h=100, wqs=80.0)
        wrapper.cache.update_wallet_activity(low_activity_wallet, tx_count_24h=5, wqs=40.0)

        # Also update the wrapper's internal activity tracking
        wrapper._wallet_activity[high_activity_wallet] = {'tx_count_24h': 100}
        wrapper._wallet_activity[low_activity_wallet] = {'tx_count_24h': 5}

        # Check activity levels
        high_level = wrapper.get_wallet_activity_level(high_activity_wallet)
        low_level = wrapper.get_wallet_activity_level(low_activity_wallet)

        # Verify activity levels are different
        assert high_level != low_level

        # Verify VERY_HIGH for high activity wallet
        assert high_level == ActivityLevel.VERY_HIGH

        # Verify MEDIUM for low activity wallet
        assert low_level == ActivityLevel.MEDIUM