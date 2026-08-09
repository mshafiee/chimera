"""
Coverage tests for core.redis_client.RedisClient.

Exercises every branch: Redis-backed operations (mocked), fallback cache,
config-driven init, invalid URLs, connection failures, TTL expiry, eviction.
"""

from datetime import datetime, timedelta
from unittest.mock import MagicMock, patch

import pytest

import core.redis_client as redis_mod
from core.redis_client import RedisClient


def _fake_redis_client():
    """A MagicMock standing in for the redis.Redis instance."""
    return MagicMock()


def _make_redis(url="redis://localhost:6379", enabled=True, client=None):
    """Build a RedisClient whose underlying redis lib is a mock."""
    with patch.object(redis_mod.redis, "Redis") as fake_redis_cls:
        fake_redis_cls.from_url.return_value = client if client is not None else _fake_redis_client()
        c = RedisClient(redis_url=url, enabled=enabled)
    return c


class TestInit:
    def test_disabled_fallback(self):
        with patch.object(redis_mod, "REDIS_AVAILABLE", True):
            c = RedisClient(enabled=False)
        assert c.enabled is False
        assert c.redis_client is None

    def test_redis_library_not_available(self):
        with patch.object(redis_mod, "REDIS_AVAILABLE", False), \
             patch.object(redis_mod, "redis", None):
            c = RedisClient(redis_url="redis://localhost:6379", enabled=True)
        assert c.enabled is True
        assert c.redis_client is None

    def test_config_defaults(self, monkeypatch):
        monkeypatch.setattr(redis_mod.ScoutConfig, "get_redis_enabled", staticmethod(lambda: True))
        monkeypatch.setattr(redis_mod.ScoutConfig, "get_redis_url", staticmethod(lambda: "redis://cfg:6379"))
        with patch.object(redis_mod.redis, "Redis") as fake_redis_cls:
            fake_redis_cls.from_url.return_value = _fake_redis_client()
            c = RedisClient()
        assert c.enabled is True
        assert c.redis_url == "redis://cfg:6379"
        fake_redis_cls.from_url.assert_called_once()

    def test_default_url_when_no_config(self, monkeypatch):
        monkeypatch.setattr(redis_mod, "CONFIG_AVAILABLE", False)
        monkeypatch.setattr(redis_mod, "ScoutConfig", None)
        with patch.object(redis_mod.redis, "Redis"):
            c = RedisClient(enabled=True)
        assert c.redis_url == "redis://localhost:6379"

    def test_invalid_url_format(self):
        c = RedisClient(redis_url="localhost:6379", enabled=True)
        assert c.enabled is False
        assert c.redis_client is None

    def test_connection_failure_falls_back(self):
        client = _fake_redis_client()
        client.ping.side_effect = Exception("connection refused")
        with patch.object(redis_mod.redis, "Redis") as fake_redis_cls:
            fake_redis_cls.from_url.return_value = client
            c = RedisClient(redis_url="redis://localhost:6379", enabled=True)
        assert c.enabled is False
        assert c.redis_client is None

    def test_unix_url_scheme(self):
        with patch.object(redis_mod.redis, "Redis") as fake_redis_cls:
            fake_redis_cls.from_url.return_value = _fake_redis_client()
            c = RedisClient(redis_url="unix:///tmp/redis.sock", enabled=True)
        assert c.enabled is True
        fake_redis_cls.from_url.assert_called_once()


class TestGet:
    def test_redis_hit(self):
        client = _fake_redis_client()
        client.get.return_value = "cached"
        c = _make_redis(client=client)
        assert c.get("k") == "cached"

    def test_redis_miss_uses_fallback(self):
        client = _fake_redis_client()
        client.get.return_value = None
        c = _make_redis(client=client)
        c._fallback_cache["k"] = ("fb", None)
        assert c.get("k") == "fb"

    def test_redis_exception_uses_fallback(self):
        client = _fake_redis_client()
        client.get.side_effect = Exception("boom")
        c = _make_redis(client=client)
        c._fallback_cache["k"] = ("fb", None)
        assert c.get("k") == "fb"

    def test_fallback_no_expiry(self):
        c = RedisClient(enabled=False)
        c._fallback_cache["k"] = ("v", None)
        assert c.get("k") == "v"

    def test_fallback_with_valid_expiry(self):
        c = RedisClient(enabled=False)
        c._fallback_cache["k"] = ("v", datetime.utcnow() + timedelta(seconds=60))
        assert c.get("k") == "v"

    def test_fallback_expired(self):
        c = RedisClient(enabled=False)
        c._fallback_cache["k"] = ("v", datetime.utcnow() - timedelta(seconds=1))
        assert c.get("k") is None
        assert "k" not in c._fallback_cache

    def test_miss_returns_none(self):
        c = RedisClient(enabled=False)
        assert c.get("missing") is None


class TestSet:
    def test_set_with_ttl_uses_setex(self):
        client = _fake_redis_client()
        c = _make_redis(client=client)
        c.set("k", "v", ttl_seconds=60)
        client.setex.assert_called_once_with("k", 60, "v")
        assert "k" in c._keys

    def test_set_without_ttl_uses_set(self):
        client = _fake_redis_client()
        c = _make_redis(client=client)
        c.set("k", "v")
        client.set.assert_called_once_with("k", "v")
        assert "k" in c._keys

    def test_redis_exception_falls_back(self):
        client = _fake_redis_client()
        client.set.side_effect = Exception("boom")
        client.setex.side_effect = Exception("boom")
        c = _make_redis(client=client)
        c.set("k", "v", ttl_seconds=10)
        assert c._fallback_cache["k"][0] == "v"
        assert c._fallback_cache["k"][1] is not None

    def test_fallback_without_ttl(self):
        c = RedisClient(enabled=False)
        c.set("k", "v")
        assert c._fallback_cache["k"] == ("v", None)

    def test_fallback_eviction(self):
        c = RedisClient(enabled=False)
        c._fallback_max_size = 2
        c.set("a", "1")
        c.set("b", "2")
        c.set("c", "3")
        assert list(c._fallback_cache.keys()) == ["b", "c"]
        assert "a" not in c._fallback_cache


class TestDelete:
    def test_redis_delete(self):
        client = _fake_redis_client()
        c = _make_redis(client=client)
        c.set("k", "v")
        c.delete("k")
        client.delete.assert_called_once_with("k")
        assert "k" not in c._keys

    def test_redis_delete_exception_falls_back(self):
        client = _fake_redis_client()
        client.delete.side_effect = Exception("boom")
        c = _make_redis(client=client)
        c._fallback_cache["k"] = ("v", None)
        c.delete("k")
        assert "k" not in c._fallback_cache

    def test_fallback_delete(self):
        c = RedisClient(enabled=False)
        c.set("k", "v")
        c.delete("k")
        assert "k" not in c._fallback_cache


class TestClear:
    def test_redis_clear_with_keys(self):
        client = _fake_redis_client()
        c = _make_redis(client=client)
        c._keys = {"a", "b"}
        c.clear()
        client.delete.assert_called_once_with(*{"a", "b"})
        assert c._keys == set()

    def test_redis_clear_without_keys(self):
        client = _fake_redis_client()
        c = _make_redis(client=client)
        c.clear()
        client.delete.assert_not_called()

    def test_redis_clear_exception_falls_back(self):
        client = _fake_redis_client()
        client.delete.side_effect = Exception("boom")
        c = _make_redis(client=client)
        c._keys = {"a"}
        c._fallback_cache["a"] = ("v", None)
        c.clear()
        assert c._fallback_cache == {}
        assert c._keys == set()

    def test_fallback_clear(self):
        c = RedisClient(enabled=False)
        c.set("a", "1")
        c.clear()
        assert c._fallback_cache == {}
        assert c._keys == set()


class TestClose:
    def test_close_redis_client(self):
        client = _fake_redis_client()
        c = _make_redis(client=client)
        c.close()
        client.close.assert_called_once()
        assert c.redis_client is None

    def test_close_redis_exception(self):
        client = _fake_redis_client()
        client.close.side_effect = Exception("boom")
        c = _make_redis(client=client)
        c.close()
        assert c.redis_client is None

    def test_close_no_client(self):
        c = RedisClient(enabled=False)
        c.set("a", "1")
        c.close()
        assert c._fallback_cache == {}


class TestIsAvailable:
    def test_not_enabled(self):
        c = RedisClient(enabled=False)
        assert c.is_available() is False

    def test_no_client(self):
        c = RedisClient(redis_url="localhost:6379", enabled=True)
        assert c.is_available() is False

    def test_ping_ok(self):
        client = _fake_redis_client()
        client.ping.return_value = True
        c = _make_redis(client=client)
        assert c.is_available() is True

    def test_ping_fails(self):
        client = _fake_redis_client()
        c = _make_redis(client=client)
        client.ping.side_effect = Exception("down")
        assert c.is_available() is False


class TestImportFallbacks:
    """Import-time fallbacks when redis/config are unavailable (reload trick)."""

    def _block(self, monkeypatch, blocked):
        import importlib
        import sys
        self._saved = {name: sys.modules.get(name) for name in blocked}
        for name in blocked:
            monkeypatch.setitem(sys.modules, name, None)
        importlib.reload(redis_mod)

    def _restore(self, blocked):
        import importlib
        import sys
        for name in blocked:
            mod = self._saved.get(name)
            if mod is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = mod
        importlib.reload(redis_mod)

    def test_redis_import_fallback(self, monkeypatch):
        self._block(monkeypatch, ["redis"])
        try:
            assert redis_mod.REDIS_AVAILABLE is False
            assert redis_mod.redis is None
        finally:
            self._restore(["redis"])

    def test_config_import_fallback(self, monkeypatch):
        self._block(monkeypatch, ["config"])
        try:
            assert redis_mod.CONFIG_AVAILABLE is False
            assert redis_mod.ScoutConfig is None
        finally:
            self._restore(["config"])
