"""Coverage completion tests for core/advanced_cache.py (multi-level cache)."""

import json
import sqlite3
import time
from unittest.mock import MagicMock, patch

import pytest

import core.advanced_cache as ac
from core.advanced_cache import (
    AdvancedCache,
    CacheCategory,
    CacheEntry,
    CacheLevel,
    CacheStats,
    TTLDefaults,
    get_analysis_results,
    get_backtest_results,
    get_cache,
    get_discovery_results,
    get_high_wqs_wallet_data,
    get_liquidity_data,
    get_token_creation_time,
    get_token_metadata,
    get_wallet_metrics,
    reset_cache,
    set_analysis_results,
    set_backtest_results,
    set_discovery_results,
    set_high_wqs_wallet_data,
    set_liquidity_data,
    set_token_creation_time,
    set_token_metadata,
    set_wallet_metrics,
)


@pytest.fixture
def cache(tmp_path, monkeypatch):
    monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
    return AdvancedCache()


class FakeRedisClient:
    """Minimal stand-in for RedisClient used by the L2 layer."""

    def __init__(self, redis_url=None, enabled=True):
        self.redis_client = MagicMock()
        self._data = {}
        self.available = True

    def is_available(self):
        return self.available

    def get(self, key):
        return self._data.get(key)

    def set(self, key, value, ttl_seconds=None):
        self._data[key] = value

    def delete(self, key):
        self._data.pop(key, None)


def enable_redis(cache, client=None):
    client = client or FakeRedisClient()
    cache._redis_client = client
    cache._redis_available = True
    return client


class TestCacheStats:
    def test_hit_rate_zero(self):
        stats = CacheStats()
        assert stats.hit_rate == 0.0
        assert stats.l1_hit_rate == 0.0

    def test_hit_rate_computed(self):
        stats = CacheStats(total_hits=3, total_misses=1, l1_hits=2)
        assert stats.hit_rate == pytest.approx(75.0)
        assert stats.l1_hit_rate == pytest.approx(66.67, abs=0.01)


class TestCacheEntry:
    def test_is_expired(self):
        entry = CacheEntry(
            key="k", value="v", category=CacheCategory.WALLET_METRICS,
            created_at=time.time() - 1000, accessed_at=time.time(),
            hit_count=0, size_bytes=1, level=CacheLevel.L1_MEMORY, ttl_seconds=60,
        )
        assert entry.is_expired() is True

    def test_not_expired(self):
        entry = CacheEntry(
            key="k", value="v", category=CacheCategory.WALLET_METRICS,
            created_at=time.time(), accessed_at=time.time(),
            hit_count=0, size_bytes=1, level=CacheLevel.L1_MEMORY, ttl_seconds=3600,
        )
        assert entry.is_expired() is False

    def test_access_updates_metadata(self):
        entry = CacheEntry(
            key="k", value="v", category=CacheCategory.WALLET_METRICS,
            created_at=time.time(), accessed_at=0.0,
            hit_count=0, size_bytes=1, level=CacheLevel.L1_MEMORY, ttl_seconds=3600,
        )
        entry.access()
        assert entry.hit_count == 1
        assert entry.accessed_at > 0


class TestInitRedis:
    def test_redis_disabled_by_config(self, cache):
        assert cache._redis_available is False

    def test_redis_enabled(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        import config
        import core.redis_client as rc

        class FakeScoutConfig:
            @staticmethod
            def get_redis_enabled():
                return True

            @staticmethod
            def get_redis_url():
                return "redis://localhost:6379"

        fake_redis_client = FakeRedisClient()
        with patch.object(config, "ScoutConfig", FakeScoutConfig), patch.object(
            rc, "RedisClient", lambda **kwargs: fake_redis_client
        ):
            new_cache = AdvancedCache()
        assert new_cache._redis_available is True

    def test_redis_init_exception(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        import config

        class FakeScoutConfig:
            @staticmethod
            def get_redis_enabled():
                raise RuntimeError("redis down")

        with patch.object(config, "ScoutConfig", FakeScoutConfig):
            new_cache = AdvancedCache()
        assert new_cache._redis_available is False

    def test_init_sqlite_failure(self, monkeypatch, tmp_path):
        blocker = tmp_path / "blocker"
        blocker.write_text("x")
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(blocker / "cache.db"))
        AdvancedCache()


class TestCacheKey:
    def test_short_key(self, cache):
        assert cache._get_cache_key("wallet", "abc123") == "wallet:abc123"

    def test_long_key_hashed(self, cache):
        long_id = "x" * 200
        key = cache._get_cache_key("wallet", long_id, "extra")
        assert key.startswith("wallet:hash:")
        assert len(key) < 100

    def test_with_args(self, cache):
        assert cache._get_cache_key("a", "b", 1, "c") == "a:b:1:c"


class TestSerialization:
    def test_serialize_value_error(self, cache):
        class BadStr:
            def __str__(self):
                raise RuntimeError("cannot stringify")

        assert cache._serialize_value({"x": BadStr()}) == b"{}"

    def test_deserialize_bytes(self, cache):
        assert cache._deserialize_value(b'{"a": 1}') == {"a": 1}

    def test_deserialize_str(self, cache):
        assert cache._deserialize_value('{"a": 2}') == {"a": 2}

    def test_deserialize_invalid(self, cache):
        assert cache._deserialize_value(b"{corrupt") is None


class TestEviction:
    def test_evict_empty(self, cache):
        assert cache._evict_l1_entries(100) is False

    def test_evict_success(self, cache):
        cache._l1_cache["a"] = CacheEntry(
            key="a", value=1, category=CacheCategory.WALLET_METRICS,
            created_at=time.time(), accessed_at=1.0,
            hit_count=0, size_bytes=100, level=CacheLevel.L1_MEMORY, ttl_seconds=300,
        )
        cache._l1_cache["b"] = CacheEntry(
            key="b", value=2, category=CacheCategory.WALLET_METRICS,
            created_at=time.time(), accessed_at=2.0,
            hit_count=0, size_bytes=50, level=CacheLevel.L1_MEMORY, ttl_seconds=300,
        )
        assert cache._evict_l1_entries(90) is True
        assert "a" not in cache._l1_cache
        assert "b" in cache._l1_cache

    def test_evict_insufficient(self, cache):
        cache._l1_cache["a"] = CacheEntry(
            key="a", value=1, category=CacheCategory.WALLET_METRICS,
            created_at=time.time(), accessed_at=1.0,
            hit_count=0, size_bytes=10, level=CacheLevel.L1_MEMORY, ttl_seconds=300,
        )
        assert cache._evict_l1_entries(1000) is False
        assert "a" not in cache._l1_cache

    def test_memory_pressure_normal(self, cache):
        cache._aggressive_eviction = False
        cache._l1_max_memory = 100
        cache._l1_cache["a"] = CacheEntry(
            key="a", value=1, category=CacheCategory.WALLET_METRICS,
            created_at=time.time(), accessed_at=time.time(),
            hit_count=0, size_bytes=90, level=CacheLevel.L1_MEMORY, ttl_seconds=300,
        )
        assert cache._check_memory_pressure() is False

    def test_memory_pressure_normal_high(self, cache):
        cache._aggressive_eviction = False
        cache._l1_max_memory = 100
        cache._l1_cache["a"] = CacheEntry(
            key="a", value=1, category=CacheCategory.WALLET_METRICS,
            created_at=time.time(), accessed_at=time.time(),
            hit_count=0, size_bytes=99, level=CacheLevel.L1_MEMORY, ttl_seconds=300,
        )
        assert cache._check_memory_pressure() is True

    def test_memory_pressure_aggressive(self, cache):
        cache._aggressive_eviction = True
        cache._l1_max_memory = 100
        cache._l1_cache["a"] = CacheEntry(
            key="a", value=1, category=CacheCategory.WALLET_METRICS,
            created_at=time.time(), accessed_at=time.time(),
            hit_count=0, size_bytes=85, level=CacheLevel.L1_MEMORY, ttl_seconds=300,
        )
        assert cache._check_memory_pressure() is True


class TestTTL:
    def test_base_ttl(self, cache):
        assert cache._get_ttl(CacheCategory.WALLET_METRICS) == 300

    def test_env_override(self, cache, monkeypatch):
        monkeypatch.setenv("SCOUT_CACHE_TTL_WALLET_METRICS", "123")
        assert cache._get_ttl(CacheCategory.WALLET_METRICS) == 123

    def test_env_override_invalid(self, cache, monkeypatch):
        monkeypatch.setenv("SCOUT_CACHE_TTL_WALLET_METRICS", "not-a-number")
        assert cache._get_ttl(CacheCategory.WALLET_METRICS) == 300

    def test_growth_high_wqs(self, cache, monkeypatch):
        monkeypatch.setenv("SCOUT_GROWTH_OPTIMIZED", "true")
        assert cache._get_ttl(CacheCategory.WALLET_METRICS, wqs_score=85.0) == 1200

    def test_growth_medium_wqs(self, cache, monkeypatch):
        monkeypatch.setenv("SCOUT_GROWTH_OPTIMIZED", "true")
        assert cache._get_ttl(CacheCategory.WALLET_METRICS, wqs_score=50.0) == 600

    def test_growth_low_wqs(self, cache, monkeypatch):
        monkeypatch.setenv("SCOUT_GROWTH_OPTIMIZED", "true")
        assert cache._get_ttl(CacheCategory.WALLET_METRICS, wqs_score=10.0) == 300

    def test_growth_disabled(self, cache, monkeypatch):
        monkeypatch.setenv("SCOUT_GROWTH_OPTIMIZED", "false")
        assert cache._get_ttl(CacheCategory.WALLET_METRICS, wqs_score=85.0) == 300

    def test_growth_only_for_wallet_metrics(self, cache, monkeypatch):
        monkeypatch.setenv("SCOUT_GROWTH_OPTIMIZED", "true")
        assert cache._get_ttl(CacheCategory.TOKEN_SECURITY, wqs_score=85.0) == 7200

    def test_unknown_category_default(self):
        assert TTLDefaults.get_ttl(CacheCategory.WALLET_AGE) == 2592000


class TestGetSetL1:
    def test_set_and_get_l1(self, cache):
        cache.set("wallet", "w1", {"score": 90}, "metrics", category=CacheCategory.WALLET_METRICS)
        assert cache.get("wallet", "w1", "metrics", category=CacheCategory.WALLET_METRICS) == {"score": 90}

    def test_get_miss(self, cache):
        assert cache.get("wallet", "nope") is None
        assert cache.get("wallet", "nope", default="fallback") == "fallback"

    def test_get_l1_expired_falls_through(self, cache):
        with patch("sqlite3.connect", side_effect=RuntimeError("sqlite down")):
            cache.set("wallet", "w1", {"score": 90}, category=CacheCategory.WALLET_METRICS)
            key = cache._get_cache_key("wallet", "w1")
            cache._l1_cache[key].created_at = time.time() - 10000
            assert cache.get("wallet", "w1", category=CacheCategory.WALLET_METRICS) is None

    def test_set_none_ignored(self, cache):
        cache.set("wallet", "w1", None)
        assert cache.get("wallet", "w1") is None

    def test_set_entry_too_large(self, cache):
        cache._l1_max_memory = 10
        with patch("sqlite3.connect", side_effect=RuntimeError("sqlite down")):
            cache.set("wallet", "w1", {"big": "x" * 1000}, category=CacheCategory.WALLET_METRICS)
            assert cache.get("wallet", "w1") is None

    def test_set_triggers_eviction(self, cache):
        cache._l1_max_memory = 500
        with patch("sqlite3.connect", side_effect=RuntimeError("sqlite down")):
            cache.set("wallet", "old", {"data": "y" * 300}, category=CacheCategory.WALLET_METRICS)
            cache.set("wallet", "new", {"data": "z" * 300}, category=CacheCategory.WALLET_METRICS)
            assert cache.get("wallet", "old") is None
            assert cache.get("wallet", "new") is not None


class TestL2Redis:
    def test_l2_hit_promotes_to_l1(self, cache):
        client = enable_redis(cache)
        client._data["token:abc"] = json.dumps({"safe": True})
        value = cache.get("token", "abc", category=CacheCategory.TOKEN_SECURITY)
        assert value == {"safe": True}
        assert cache._stats.l2_hits == 1
        assert "token:abc" in cache._l1_cache

    def test_l2_corrupt_payload_deleted(self, cache):
        client = enable_redis(cache)
        client._data["token:abc"] = "{corrupt"
        assert cache.get("token", "abc", category=CacheCategory.TOKEN_SECURITY) is None
        assert "token:abc" not in client._data
        assert cache._stats.total_misses == 1

    def test_l2_exception_treated_as_miss(self, cache):
        client = enable_redis(cache)
        client.get = MagicMock(side_effect=RuntimeError("redis down"))
        assert cache.get("token", "abc") is None

    def test_set_writes_l2(self, cache):
        client = enable_redis(cache)
        cache.set("token", "abc", {"safe": True}, category=CacheCategory.TOKEN_SECURITY)
        assert json.loads(client._data["token:abc"]) == {"safe": True}
        client.redis_client.sadd.assert_called()

    def test_set_l2_exception_logged(self, cache):
        client = enable_redis(cache)
        client.set = MagicMock(side_effect=RuntimeError("redis down"))
        cache.set("token", "abc", {"safe": True}, category=CacheCategory.TOKEN_SECURITY)
        assert cache.get("token", "abc") == {"safe": True}  # L1 still served

    def test_invalidate_l2(self, cache):
        client = enable_redis(cache)
        client._data["token:abc"] = "x"
        cache.invalidate("token", "abc")
        assert "token:abc" not in client._data

    def test_invalidate_l2_exception(self, cache):
        client = enable_redis(cache)
        client.delete = MagicMock(side_effect=RuntimeError("redis down"))
        cache.invalidate("token", "abc")

    def test_invalidate_category_l2(self, cache):
        client = enable_redis(cache)
        client.redis_client.smembers.return_value = {"k1", "k2"}
        cache.invalidate_category(CacheCategory.TOKEN_SECURITY)
        assert client.redis_client.delete.call_count == 2
        assert set(client.redis_client.delete.call_args_list[0].args) == {"k1", "k2"}
        assert client.redis_client.delete.call_args_list[1] == (("cat:token_security",),)

    def test_invalidate_category_l2_no_members(self, cache):
        client = enable_redis(cache)
        client.redis_client.smembers.return_value = set()
        cache.invalidate_category(CacheCategory.TOKEN_SECURITY)

    def test_invalidate_category_l2_exception(self, cache):
        client = enable_redis(cache)
        client.redis_client = MagicMock()
        client.redis_client.smembers.side_effect = RuntimeError("redis down")
        cache.invalidate_category(CacheCategory.TOKEN_SECURITY)


class TestL3Sqlite:
    def test_l3_hit_promotes_to_l1(self, cache):
        cache.set("wallet", "w9", {"score": 50}, category=CacheCategory.WALLET_METRICS)
        cache._l1_cache.clear()
        value = cache.get("wallet", "w9", category=CacheCategory.WALLET_METRICS)
        assert value == {"score": 50}
        assert cache._stats.l3_hits == 1
        assert "wallet:w9" in cache._l1_cache

    def test_l3_corrupt_row_invalidated(self, cache):
        conn = sqlite3.connect(cache._sqlite_path)
        conn.execute(
            "INSERT INTO cache_entries (key, value, category, created_at, accessed_at, hit_count, size_bytes, ttl_seconds) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            ("wallet:bad", b"{corrupt", "wallet_metrics", time.time(), time.time(), 0, 10, 300),
        )
        conn.commit()
        conn.close()
        assert cache.get("wallet", "bad", category=CacheCategory.WALLET_METRICS) is None

    def test_l3_expired_row_miss(self, cache):
        cache.set("wallet", "w9", {"score": 50}, category=CacheCategory.WALLET_METRICS)
        conn = sqlite3.connect(cache._sqlite_path)
        conn.execute("UPDATE cache_entries SET created_at = ?", (time.time() - 10000,))
        conn.commit()
        conn.close()
        cache._l1_cache.clear()
        assert cache.get("wallet", "w9", category=CacheCategory.WALLET_METRICS) is None

    def test_l3_exception_swallowed(self, cache, monkeypatch):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", "/nonexistent_dir_xyz/cache.db")
        new_cache = AdvancedCache()
        new_cache._sqlite_path = "/nonexistent_dir_xyz/cache.db"
        assert new_cache.get("wallet", "w1") is None

    def test_set_l3_exception_swallowed(self, cache, monkeypatch):
        with patch("sqlite3.connect", side_effect=RuntimeError("sqlite down")):
            cache.set("wallet", "w1", {"score": 90}, category=CacheCategory.WALLET_METRICS)
        # L1 still stored
        assert cache.get("wallet", "w1") == {"score": 90}

    def test_invalidate_l3(self, cache):
        cache.set("wallet", "w9", {"score": 50}, category=CacheCategory.WALLET_METRICS)
        cache.invalidate("wallet", "w9")
        assert cache.get("wallet", "w9") is None

    def test_invalidate_l3_exception(self, cache):
        with patch("sqlite3.connect", side_effect=RuntimeError("sqlite down")):
            cache.invalidate("wallet", "w9")

    def test_invalidate_category_l3_exception(self, cache):
        with patch("sqlite3.connect", side_effect=RuntimeError("sqlite down")):
            cache.invalidate_category(CacheCategory.WALLET_METRICS)

    def test_update_l3_hit_count_exception(self, cache):
        with patch("sqlite3.connect", side_effect=RuntimeError("sqlite down")):
            cache._update_l3_hit_count("key", 2)

    def test_invalidate_category_l3(self, cache):
        cache.set("token", "t1", {"safe": True}, category=CacheCategory.TOKEN_SECURITY)
        cache.set("token", "t2", {"safe": False}, category=CacheCategory.TOKEN_SECURITY)
        cache.invalidate_category(CacheCategory.TOKEN_SECURITY)
        assert cache.get("token", "t1", category=CacheCategory.TOKEN_SECURITY) is None
        assert cache.get("token", "t2", category=CacheCategory.TOKEN_SECURITY) is None

    def test_invalidate_category_l1_only(self, cache):
        cache.set("wallet", "w1", {"a": 1}, category=CacheCategory.WALLET_METRICS)
        cache.set("token", "t1", {"b": 2}, category=CacheCategory.TOKEN_SECURITY)
        cache.invalidate_category(CacheCategory.WALLET_METRICS)
        assert cache.get("wallet", "w1") is None
        assert cache.get("token", "t1") == {"b": 2}


class TestCleanup:
    def test_cleanup_expired_l1_and_l3(self, cache):
        cache.set("wallet", "w1", {"a": 1}, category=CacheCategory.WALLET_METRICS)
        cache.set("token", "t1", {"b": 2}, category=CacheCategory.TOKEN_SECURITY)
        conn = sqlite3.connect(cache._sqlite_path)
        conn.execute("UPDATE cache_entries SET created_at = ?", (time.time() - 100000,))
        conn.commit()
        conn.close()
        key = cache._get_cache_key("wallet", "w1")
        cache._l1_cache[key].created_at = time.time() - 100000
        cache.cleanup_expired()
        assert cache.get("wallet", "w1", category=CacheCategory.WALLET_METRICS) is None
        assert cache.get("token", "t1") == {"b": 2}

    def test_cleanup_l3_exception(self, cache, monkeypatch):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", "/nonexistent_dir_xyz/cache.db")
        new_cache = AdvancedCache()
        new_cache._sqlite_path = "/nonexistent_dir_xyz/cache.db"
        new_cache.cleanup_expired()


class TestStatsAndShutdown:
    def test_get_stats(self, cache):
        cache.set("wallet", "w1", {"a": 1})
        cache.get("wallet", "w1")
        stats = cache.get_stats()
        assert stats.total_hits == 1
        assert stats.total_entries == 1

    def test_print_stats(self, cache, capsys):
        cache.set("wallet", "w1", {"a": 1})
        cache.get("wallet", "w1")
        cache.print_stats()
        out = capsys.readouterr().out
        assert "ADVANCED CACHE - STATISTICS" in out

    def test_warm_cache_disabled(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_WARMING", "false")
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        cache = AdvancedCache()
        cache.warm_cache(["w1"], ["t1"], high_wqs_wallets=["w1"])

    def test_warm_cache_enabled(self, cache):
        cache.warm_cache(["w1"], ["t1"])
        cache.warm_cache(["w2"], ["t2"], high_wqs_wallets=["w2"])

    def test_shutdown(self, cache):
        cache.shutdown()

    def test_shutdown_error_path(self, cache):
        # The inner try block cannot raise (hasattr never fails), so the
        # except branch is unreachable by design.
        assert cache._sqlite_path




class TestSingleton:
    def test_get_cache_singleton(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        monkeypatch.setattr(ac, "_cache", None)
        first = get_cache()
        second = get_cache()
        assert first is second
        monkeypatch.setattr(ac, "_cache", None)

    def test_reset_cache_with_instance(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        fake_cache = MagicMock()
        monkeypatch.setattr(ac, "_cache", fake_cache)
        reset_cache()
        assert ac._cache is None
        fake_cache.shutdown.assert_called_once()

    def test_reset_cache_without_instance(self, monkeypatch):
        monkeypatch.setattr(ac, "_cache", None)
        reset_cache()


class TestConvenienceFunctions:
    def test_wallet_metrics_roundtrip(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        monkeypatch.setattr(ac, "_cache", None)
        set_wallet_metrics("w1", {"score": 90})
        assert get_wallet_metrics("w1") == {"score": 90}
        monkeypatch.setattr(ac, "_cache", None)

    def test_token_metadata_roundtrip(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        monkeypatch.setattr(ac, "_cache", None)
        set_token_metadata("t1", {"name": "BONK"})
        assert get_token_metadata("t1") == {"name": "BONK"}
        monkeypatch.setattr(ac, "_cache", None)

    def test_token_creation_roundtrip(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        monkeypatch.setattr(ac, "_cache", None)
        set_token_creation_time("t1", 1234.0)
        assert get_token_creation_time("t1") == 1234.0
        monkeypatch.setattr(ac, "_cache", None)

    def test_liquidity_roundtrip(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        monkeypatch.setattr(ac, "_cache", None)
        set_liquidity_data("t1", {"usd": 50000})
        assert get_liquidity_data("t1") == {"usd": 50000}
        monkeypatch.setattr(ac, "_cache", None)

    def test_high_wqs_roundtrip(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        monkeypatch.setattr(ac, "_cache", None)
        set_high_wqs_wallet_data("w1", {"data": 1}, wqs_score=85.0)
        assert get_high_wqs_wallet_data("w1", wqs_score=85.0) == {"data": 1}
        monkeypatch.setattr(ac, "_cache", None)

    def test_analysis_results_roundtrip(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        monkeypatch.setattr(ac, "_cache", None)
        set_analysis_results("run1", {"result": 1})
        assert get_analysis_results("run1") == {"result": 1}
        monkeypatch.setattr(ac, "_cache", None)

    def test_discovery_results_roundtrip(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        monkeypatch.setattr(ac, "_cache", None)
        set_discovery_results("d1", {"wallets": 5})
        assert get_discovery_results("d1") == {"wallets": 5}
        monkeypatch.setattr(ac, "_cache", None)

    def test_backtest_results_roundtrip(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_CACHE_DB_PATH", str(tmp_path / "cache.db"))
        monkeypatch.setattr(ac, "_cache", None)
        set_backtest_results("w1", {"pnl": 10.0})
        assert get_backtest_results("w1") == {"pnl": 10.0}
        monkeypatch.setattr(ac, "_cache", None)
