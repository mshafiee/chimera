"""
Coverage tests for core/caching.py (HeliusCachingWrapper).

Existing test_activity_cache_integration.py covers the underlying
ActivityBasedCache; this file covers the wrapper's own branches.
"""

import time

from core.activity_cache import ActivityLevel
from core.caching import HeliusCachingWrapper


def _txs(n=5, ts=None):
    ts = ts or time.time()
    return [{"signature": f"sig{i}", "timestamp": ts} for i in range(n)]


def test_cache_transactions_empty_returns_false():
    w = HeliusCachingWrapper()
    assert w.cache_transactions("wallet1", [], days=30) is False


def test_cache_miss_returns_none():
    w = HeliusCachingWrapper()
    assert w.get_cached_transactions("wallet1") is None


def test_cache_roundtrip_hit():
    w = HeliusCachingWrapper()
    txs = _txs()
    assert w.cache_transactions("wallet1", txs, days=30, wqs_score=75.0) is True
    cached = w.get_cached_transactions("wallet1", days=30)
    assert cached == txs


def test_estimate_24h_tx_count_recent():
    w = HeliusCachingWrapper()
    now = time.time()
    txs = [{"timestamp": now - 60} for _ in range(4)] + [{"timestamp": now - 99999}]
    assert w._estimate_24h_tx_count(txs, 30) == 4


def test_estimate_24h_tx_count_falls_back_to_average():
    w = HeliusCachingWrapper()
    txs = [{"timestamp": 0} for _ in range(10)]
    assert w._estimate_24h_tx_count(txs, 30) == 0  # avg_daily = 10/30 = 0


def test_estimate_24h_tx_count_zero_days():
    w = HeliusCachingWrapper()
    assert w._estimate_24h_tx_count([{"timestamp": 0}], 0) == 0
    assert w._estimate_24h_tx_count([], 30) == 0


def test_invalidate_wallet_clears_tracking():
    w = HeliusCachingWrapper()
    txs = _txs()
    w.cache_transactions("wallet1", txs, days=30)
    assert w.invalidate_wallet("wallet1") == 1
    assert "wallet1" not in w._wallet_activity
    assert "wallet1" not in w._last_activity_update
    assert w.get_cached_transactions("wallet1", days=30) is None


def test_get_cache_stats_includes_tracking():
    w = HeliusCachingWrapper()
    w.cache_transactions("wallet1", _txs(), days=30, wqs_score=80.0)
    stats = w.get_cache_stats()
    assert stats["wallets_tracked"] == 1
    assert "activity_distribution" in stats
    assert stats["activity_distribution"][ActivityLevel.MEDIUM.value] == 1


def test_cleanup_inactive_wallets():
    w = HeliusCachingWrapper()
    w.cache_transactions("old_wallet", _txs(), days=30)
    # Backdate both the wrapper tracking and the underlying cache activity
    w._last_activity_update["old_wallet"] = time.time() - (48 * 3600)
    w._wallet_activity["old_wallet"]["last_updated"] = time.time() - (48 * 3600)
    w.cache._activity_data["old_wallet"].last_activity_timestamp = time.time() - (48 * 3600)
    count = w.cleanup_inactive_wallets(hours_threshold=24)
    assert count == 1
    assert "old_wallet" not in w._wallet_activity


def test_get_wallet_activity_level_no_data():
    w = HeliusCachingWrapper()
    assert w.get_wallet_activity_level("unknown") == ActivityLevel.INACTIVE


def test_get_wallet_activity_level_high():
    w = HeliusCachingWrapper()
    w.cache_transactions("wallet_hi", [{"timestamp": time.time()} for _ in range(30)], days=1, wqs_score=0.0)
    assert w.get_wallet_activity_level("wallet_hi") == ActivityLevel.HIGH


def test_get_wallet_activity_level_very_high():
    w = HeliusCachingWrapper()
    w.cache_transactions("wallet_vh", [{"timestamp": time.time()} for _ in range(60)], days=1)
    assert w.get_wallet_activity_level("wallet_vh") == ActivityLevel.VERY_HIGH


def test_get_wallet_activity_level_medium():
    w = HeliusCachingWrapper()
    w.cache_transactions("wallet_med", [{"timestamp": time.time()} for _ in range(5)], days=1)
    assert w.get_wallet_activity_level("wallet_med") == ActivityLevel.MEDIUM


def test_get_wallet_activity_level_inactive_tracking():
    w = HeliusCachingWrapper()
    w._wallet_activity["wallet_zero"] = {"tx_count_24h": 0, "wqs_score": None, "last_updated": time.time()}
    assert w.get_wallet_activity_level("wallet_zero") == ActivityLevel.INACTIVE


def test_get_cache_hit_rate_and_reset_statistics():
    w = HeliusCachingWrapper()
    txs = _txs()
    w.cache_transactions("wallet1", txs, days=30)
    w.get_cached_transactions("wallet1", days=30)  # one hit
    assert w.get_cache_hit_rate() > 0
    w.reset_statistics()
    assert w.get_cache_hit_rate() == 0.0


def test_clear_cache():
    w = HeliusCachingWrapper()
    w.cache_transactions("wallet1", _txs(), days=30)
    w.clear_cache()
    assert len(w._wallet_activity) == 0
    assert len(w._last_activity_update) == 0
    assert w.get_cached_transactions("wallet1", days=30) is None
