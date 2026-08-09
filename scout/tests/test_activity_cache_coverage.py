"""
Coverage tests for core/activity_cache.py.

Covers the branches not exercised by test_activity_cache_integration.py:
eviction, memory accounting, cleanup, cache policy, and activity levels.
"""

import time

from core.activity_cache import (
    ActivityBasedCache,
    ActivityData,
    ActivityLevel,
    CacheConfig,
    CacheEntry,
)


def _make_entry(cache, key, value="v", ttl=100, last_accessed=None):
    return CacheEntry(
        key=key,
        value=value,
        created_at=time.time(),
        last_accessed=last_accessed or time.time(),
        ttl_seconds=ttl,
        activity_level=ActivityLevel.MEDIUM,
    )


def test_time_until_expiry():
    entry = _make_entry(None, "k", ttl=100, last_accessed=time.time() - 10)
    assert 0 < entry.time_until_expiry() <= 100
    stale = _make_entry(None, "k2", ttl=10, last_accessed=time.time() - 60)
    assert stale.time_until_expiry() == 0


def test_should_cache_wallet_high_activity():
    cache = ActivityBasedCache()
    assert cache.should_cache_wallet("w", ActivityLevel.VERY_HIGH) is True
    assert cache.should_cache_wallet("w", ActivityLevel.HIGH) is True


def test_should_cache_wallet_medium_with_space():
    cache = ActivityBasedCache()
    assert cache.should_cache_wallet("w", ActivityLevel.MEDIUM) is True


def test_should_cache_wallet_medium_full():
    cache = ActivityBasedCache(CacheConfig(MAX_ENTRIES=2))
    cache.set("k1", "v1")
    cache.set("k2", "v2")
    assert cache.should_cache_wallet("w", ActivityLevel.MEDIUM) is False


def test_should_cache_wallet_low_with_high_wqs():
    cache = ActivityBasedCache()
    cache.update_wallet_activity("w", 0, wqs=80.0)
    assert cache.should_cache_wallet("w", ActivityLevel.LOW) is True


def test_should_cache_wallet_low_without_wqs():
    cache = ActivityBasedCache()
    cache.update_wallet_activity("w", 0, wqs=30.0)
    assert cache.should_cache_wallet("w", ActivityLevel.LOW) is False
    assert cache.should_cache_wallet("w", ActivityLevel.INACTIVE) is False
    assert cache.should_cache_wallet("unknown", ActivityLevel.LOW) is False


def test_activity_level_low_branch():
    cache = ActivityBasedCache()
    cache._activity_data["w"] = ActivityData(
        wallet_address="w", tx_count_24h=0.5, last_activity_timestamp=time.time()
    )
    assert cache._get_activity_level("w") == ActivityLevel.LOW
    assert cache._get_activity_level("missing") == ActivityLevel.INACTIVE


def test_set_without_wallet_uses_medium_ttl():
    cache = ActivityBasedCache()
    cache.set("bare", "value")
    entry = cache._cache["bare"]
    assert entry.activity_level == ActivityLevel.MEDIUM
    assert entry.ttl_seconds == cache._config.MEDIUM_TTL


def test_set_overwrite_accounts_memory():
    cache = ActivityBasedCache()
    cache.set("k", "a" * 100)
    cache.set("k", "b" * 50)
    assert cache._cache["k"].size_bytes == 50
    assert cache._total_memory_bytes == 50


def test_evict_lru_when_memory_limit_exceeded():
    config = CacheConfig(MAX_MEMORY_MB=0.001)  # ~1KB
    cache = ActivityBasedCache(config)
    cache.set("big1", "x" * 2000)
    cache.set("big2", "y" * 2000)
    assert cache._stats["evictions"] >= 1
    assert len(cache._cache) == 1


def test_evict_lru_when_entry_limit_exceeded():
    cache = ActivityBasedCache(CacheConfig(MAX_ENTRIES=1))
    cache.set("k1", "v1")
    cache.set("k2", "v2")
    assert len(cache._cache) == 1
    assert cache._stats["evictions"] == 1
    assert "k2" in cache._cache


def test_evict_lru_empty_cache_noop():
    cache = ActivityBasedCache()
    cache._evict_lru()
    assert cache._stats["evictions"] == 0


def test_invalidate_missing_key_returns_false():
    cache = ActivityBasedCache()
    assert cache.invalidate("missing") is False


def test_invalidate_wallet_none_returns_zero():
    cache = ActivityBasedCache()
    assert cache.invalidate_wallet(None) == 0


def test_update_wallet_activity_existing_with_wqs():
    cache = ActivityBasedCache()
    cache.update_wallet_activity("w", 5, wqs=50.0)
    cache.update_wallet_activity("w", 8)  # wqs None -> keep existing
    assert cache._activity_data["w"].tx_count_24h == 8
    assert cache._activity_data["w"].wqs_score == 50.0
    cache.update_wallet_activity("w", 9, wqs=90.0)
    assert cache._activity_data["w"].wqs_score == 90.0


def test_predict_cache_hit_rate():
    cache = ActivityBasedCache()
    assert cache.predict_cache_hit_rate(ActivityLevel.VERY_HIGH) == 0.95
    assert cache.predict_cache_hit_rate(ActivityLevel.HIGH) == 0.85
    assert cache.predict_cache_hit_rate(ActivityLevel.MEDIUM) == 0.70
    assert cache.predict_cache_hit_rate(ActivityLevel.LOW) == 0.50
    assert cache.predict_cache_hit_rate(ActivityLevel.INACTIVE) == 0.20
    assert cache.predict_cache_hit_rate("unknown") == 0.5


def test_check_cleanup_interval_elapsed():
    cache = ActivityBasedCache(CacheConfig(INACTIVE_HOURS_THRESHOLD=24))
    # Force a cleanup pass on next get/set
    cache._last_cleanup = time.time() - cache._config.CLEANUP_INTERVAL_SECONDS - 1
    cache.update_wallet_activity(
        "stale_wallet", 1,
    )
    cache._activity_data["stale_wallet"].last_activity_timestamp = time.time() - (48 * 3600)
    cache.set("stale_k", "v", wallet="stale_wallet")
    # Add an already-expired entry
    cache._cache["expired_k"] = _make_entry(
        cache, "expired_k", ttl=1, last_accessed=time.time() - 10
    )
    cache.get("expired_k")  # triggers _check_cleanup
    assert "stale_wallet" not in cache._activity_data
    assert "expired_k" not in cache._cache
    assert cache._stats["expirations"] >= 1
    # A second cleanup pass takes the expired-keys branch again
    cache._last_cleanup = time.time() - cache._config.CLEANUP_INTERVAL_SECONDS - 1
    cache._cache["expired_k2"] = _make_entry(
        cache, "expired_k2", ttl=1, last_accessed=time.time() - 10
    )
    cache.get("expired_k2")
    assert "expired_k2" not in cache._cache


def test_estimate_size_exception_default():
    class BadStr:
        def __str__(self):
            raise RuntimeError("no str")

    cache = ActivityBasedCache()
    assert cache._estimate_size(BadStr()) == 100


def test_reset_statistics():
    cache = ActivityBasedCache()
    cache.get("missing")
    assert cache._stats["misses"] == 1
    cache.reset_statistics()
    assert cache._stats == {
        "hits": 0, "misses": 0, "evictions": 0, "expirations": 0,
    }
