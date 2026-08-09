"""Coverage completion tests for core/security_client.py (RugCheck client)."""

import importlib
import sys
from datetime import datetime, timedelta
from unittest.mock import MagicMock, patch

import pytest

import core.advanced_cache as core_advanced_cache
import core.security_client as sc
from core.security_client import RugCheckClient


class FakeResponse:
    def __init__(self, status=200, data=None, error=None):
        self.status = status
        self._data = data or {}
        self._error = error

    async def json(self):
        if self._error is not None:
            raise self._error
        return self._data

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return False


class FakeSession:
    def __init__(self, responses):
        self.responses = list(responses)
        self.requests = []

    def get(self, url, headers=None, timeout=None):
        self.requests.append((url, headers, timeout))
        if self.responses:
            return self.responses.pop(0)
        return FakeResponse(status=500)

    async def close(self):
        self.closed = True


class FakeAdvancedCache:
    """Stand-in for AdvancedCache used by the L2 layer."""

    def __init__(self):
        self._data = {}
        self.set_error = None
        self.get_error = None

    def get(self, prefix, identifier, category=None):
        if self.get_error is not None:
            raise self.get_error
        return self._data.get((prefix, identifier))

    def set(self, prefix, identifier, value, category=None):
        if self.set_error is not None:
            raise self.set_error
        self._data[(prefix, identifier)] = value

    def invalidate_category(self, category):
        self._data = {k: v for k, v in self._data.items() if k[1] != "token_mint"}


@pytest.fixture
def client():
    return RugCheckClient(api_key="test-key")


@pytest.fixture
def session_client():
    return RugCheckClient(api_key="test-key")


class TestInit:
    def test_defaults_without_config(self):
        with patch.object(sc, "CONFIG_AVAILABLE", False):
            c = RugCheckClient()
        assert c.api_key is None
        assert c.fail_mode == "closed"

    def test_init_with_session(self):
        session = MagicMock()
        c = RugCheckClient(session=session)
        assert c._session is session
        assert c._own_session is False

    def test_config_sources_values(self, monkeypatch):
        class FakeConfig:
            @staticmethod
            def get_rugcheck_api_key():
                return "config-key"

            @staticmethod
            def get_rugcheck_fail_mode():
                return "open"

        monkeypatch.setattr(sc, "ScoutConfig", FakeConfig)
        c = RugCheckClient()
        assert c.api_key == "config-key"
        assert c.fail_mode == "open"

    def test_fail_mode_open_default_from_config(self, monkeypatch):
        class FakeConfig:
            @staticmethod
            def get_rugcheck_api_key():
                return None

            @staticmethod
            def get_rugcheck_fail_mode():
                return "open"

        monkeypatch.setattr(sc, "ScoutConfig", FakeConfig)
        assert RugCheckClient().fail_mode == "open"


class TestSessionLifecycle:
    async def test_get_session_creates_own(self, client):
        session = await client._get_session()
        assert client._own_session is True
        assert client._session is session

    async def test_close_session_owned(self, client):
        session = await client._get_session()
        await client._close_session()
        assert client._session is None
        assert client._own_session is False
        assert session.closed

    async def test_close_public(self, client):
        await client.close()

    async def test_close_borrowed_session_not_closed(self):
        session = MagicMock()
        c = RugCheckClient(session=session)
        await c._close_session()
        session.close.assert_not_called()


class TestTokenRisk:
    async def test_api_200_safe(self, client):
        session = FakeSession([FakeResponse(status=200, data={
            "score": 100,
            "risks": [{"name": "LowLiquidity", "value": 10}],
        })])
        client._session = session
        result = await client.get_token_risk("mint_1")
        assert result["is_safe"] is True
        assert result["cached"] is False
        assert result["score"] == 100
        assert "mint_1" in client._l1_cache

    async def test_api_200_score_string(self, client):
        session = FakeSession([FakeResponse(status=200, data={
            "score": "1500",
            "risks": [],
        })])
        client._session = session
        result = await client.get_token_risk("mint_2")
        assert result["score"] == 1500

    async def test_api_200_score_invalid(self, client):
        session = FakeSession([FakeResponse(status=200, data={
            "score": "not-a-number",
            "risks": [],
        })])
        client._session = session
        result = await client.get_token_risk("mint_3")
        assert result["score"] == 0

    async def test_api_200_dangerous_flag(self, client):
        session = FakeSession([FakeResponse(status=200, data={
            "score": 100,
            "risks": [{"name": "FreezeAuthority", "value": 1}, {"name": "MutableMetadata", "value": 1}],
        })])
        client._session = session
        result = await client.get_token_risk("mint_4")
        assert result["is_safe"] is False

    async def test_api_200_high_top_holder_concentration(self, client):
        session = FakeSession([FakeResponse(status=200, data={
            "score": 100,
            "risks": [{"name": "TopHoldersPercentage", "value": "95"}],
        })])
        client._session = session
        result = await client.get_token_risk("mint_5")
        assert result["is_safe"] is False

    async def test_api_200_top_holder_value_invalid(self, client):
        session = FakeSession([FakeResponse(status=200, data={
            "score": 100,
            "risks": [{"name": "topHolders", "value": "bad"}],
        })])
        client._session = session
        result = await client.get_token_risk("mint_6")
        assert result["is_safe"] is True

    async def test_api_200_score_over_threshold(self, client):
        session = FakeSession([FakeResponse(status=200, data={
            "score": 5000,
            "risks": [],
        })])
        client._session = session
        result = await client.get_token_risk("mint_7")
        assert result["is_safe"] is False

    async def test_l1_hit(self, client):
        client._l1_cache["mint_8"] = {"data": {"is_safe": True, "score": 5}, "timestamp": datetime.now()}
        result = await client.get_token_risk("mint_8")
        assert result["cached"] is True
        assert result["cache_level"] == "L1"

    async def test_l1_expired_removed(self, client):
        client._l1_cache["mint_9"] = {
            "data": {"is_safe": True},
            "timestamp": datetime.now() - timedelta(hours=3),
        }
        session = FakeSession([FakeResponse(status=200, data={"score": 100, "risks": []})])
        client._session = session
        result = await client.get_token_risk("mint_9")
        assert result["cached"] is False
        # Expired entry removed, then re-added fresh by the API fetch
        assert client._l1_cache["mint_9"]["timestamp"] > datetime.now() - timedelta(minutes=1)

    async def test_l1_capacity_eviction(self, client):
        for i in range(5000):
            client._l1_cache[f"mint_{i}"] = {
                "data": {"is_safe": True},
                "timestamp": datetime.now() - timedelta(hours=5),
            }
        session = FakeSession([FakeResponse(status=200, data={"score": 100, "risks": []})])
        client._session = session
        result = await client.get_token_risk("new_mint")
        assert result["is_safe"] is True
        assert len(client._l1_cache) < 5000

    async def test_404_fail_closed(self, client):
        session = FakeSession([FakeResponse(status=404)])
        client._session = session
        result = await client.get_token_risk("mint_404")
        assert result["is_safe"] is False
        assert result["score"] == 9999

    async def test_404_fail_open(self):
        c = RugCheckClient(api_key="k", fail_mode="open")
        session = FakeSession([FakeResponse(status=404)])
        c._session = session
        result = await c.get_token_risk("mint_404")
        assert result["is_safe"] is True
        assert result["score"] == 0
        assert "mint_404" in c._l1_cache

    async def test_404_fail_open_l1_capacity_evicts(self):
        c = RugCheckClient(api_key="k", fail_mode="open")
        for i in range(5000):
            c._l1_cache[f"old_mint_{i}"] = {
                "data": {"is_safe": True},
                "timestamp": datetime.now() - timedelta(hours=5),
            }
        session = FakeSession([FakeResponse(status=404)])
        c._session = session
        result = await c.get_token_risk("mint_404")
        assert result["is_safe"] is True
        assert len(c._l1_cache) < 5000

    async def test_client_error_fail_open(self):
        import aiohttp

        c = RugCheckClient(api_key="k", fail_mode="open")
        session = FakeSession([FakeResponse(status=200, error=aiohttp.ClientError("conn refused"))])
        c._session = session
        result = await c.get_token_risk("mint_conn_open")
        assert result["is_safe"] is True
        assert result["risks"] == []

    async def test_other_status_fail_closed(self, client):
        session = FakeSession([FakeResponse(status=500)])
        client._session = session
        result = await client.get_token_risk("mint_500")
        assert result["is_safe"] is False

    async def test_other_status_fail_open(self):
        c = RugCheckClient(api_key="k", fail_mode="open")
        session = FakeSession([FakeResponse(status=500)])
        c._session = session
        result = await c.get_token_risk("mint_500")
        assert result["is_safe"] is True

    async def test_client_error_fail_closed(self, client):
        import aiohttp

        session = FakeSession([FakeResponse(status=200, error=aiohttp.ClientError("conn refused"))])
        client._session = session
        result = await client.get_token_risk("mint_conn")
        assert result["is_safe"] is False
        assert result["risks"] == ["RugCheck API error"]

    async def test_generic_exception_fail_closed(self, client):
        session = FakeSession([FakeResponse(status=200, error=ValueError("bad"))])
        client._session = session
        result = await client.get_token_risk("mint_err")
        assert result["is_safe"] is False
        assert result["risks"] == ["RugCheck check error"]

    async def test_generic_exception_fail_open(self):
        c = RugCheckClient(api_key="k", fail_mode="open")
        session = FakeSession([FakeResponse(status=200, error=ValueError("bad"))])
        c._session = session
        result = await c.get_token_risk("mint_err")
        assert result["is_safe"] is True


class TestIsTokenSafe:
    async def test_is_token_safe(self, client):
        session = FakeSession([FakeResponse(status=200, data={"score": 100, "risks": []})])
        client._session = session
        assert await client.is_token_safe("mint_safe") is True

    async def test_is_token_safe_dangerous(self, client):
        session = FakeSession([FakeResponse(status=200, data={"score": 9999, "risks": []})])
        client._session = session
        assert await client.is_token_safe("mint_danger") is False


class TestEvictionAndClear:
    def test_evict_expired_l1(self, client):
        client._l1_cache["old_mint"] = {"data": {}, "timestamp": datetime.now() - timedelta(hours=3)}
        client._l1_cache["new_mint"] = {"data": {}, "timestamp": datetime.now()}
        client._evict_expired_l1()
        assert "old_mint" not in client._l1_cache
        assert "new_mint" in client._l1_cache

    def test_clear_cache(self, client):
        client._l1_cache["mint"] = {"data": {}, "timestamp": datetime.now()}
        client.clear_cache()
        assert client._l1_cache == {}


class TestCacheEnabled:
    """Runs after registering core.advanced_cache as the importable module."""

    @pytest.fixture(autouse=True)
    def _reload_with_cache(self, monkeypatch):
        monkeypatch.setitem(sys.modules, "advanced_cache", core_advanced_cache)
        importlib.reload(sc)
        yield
        monkeypatch.undo()
        importlib.reload(sc)

    def test_cache_available(self):
        assert sc.CACHE_AVAILABLE is True

    async def test_l2_hit(self):
        fake_cache = FakeAdvancedCache()
        fake_cache._data[("token_security", "mint_l2")] = {"is_safe": True, "score": 5}
        with patch.object(sc, "AdvancedCache", lambda: fake_cache):
            c = RugCheckClient()
            result = await c.get_token_risk("mint_l2")
        assert result["cached"] is True
        assert result["cache_level"] == "L2"

    async def test_l2_get_exception(self):
        fake_cache = FakeAdvancedCache()
        fake_cache.get_error = RuntimeError("cache down")
        with patch.object(sc, "AdvancedCache", lambda: fake_cache):
            c = RugCheckClient()
            session = FakeSession([FakeResponse(status=200, data={"score": 100, "risks": []})])
            c._session = session
            result = await c.get_token_risk("mint_l2err")
        assert result["cached"] is False

    async def test_l2_set_exception(self):
        fake_cache = FakeAdvancedCache()
        fake_cache.set_error = RuntimeError("cache down")
        with patch.object(sc, "AdvancedCache", lambda: fake_cache):
            c = RugCheckClient()
            session = FakeSession([FakeResponse(status=200, data={"score": 100, "risks": []})])
            c._session = session
            result = await c.get_token_risk("mint_l2set")
        assert result["is_safe"] is True

    async def test_l2_set_exception_on_404(self):
        fake_cache = FakeAdvancedCache()
        fake_cache.set_error = RuntimeError("cache down")
        with patch.object(sc, "AdvancedCache", lambda: fake_cache):
            c = RugCheckClient(api_key="k", fail_mode="open")
            session = FakeSession([FakeResponse(status=404)])
            c._session = session
            result = await c.get_token_risk("mint_l2set404")
        assert result["is_safe"] is True

    async def test_clear_all_caches(self):
        fake_cache = FakeAdvancedCache()
        fake_cache._data[("token_security", "token_mint")] = {"is_safe": True}
        with patch.object(sc, "AdvancedCache", lambda: fake_cache):
            c = RugCheckClient()
            await c.clear_all_caches()
        assert fake_cache._data == {}

    async def test_clear_all_caches_exception(self):
        class BrokenCache(FakeAdvancedCache):
            def invalidate_category(self, category):
                raise RuntimeError("cache down")

        with patch.object(sc, "AdvancedCache", BrokenCache):
            c = RugCheckClient()
            await c.clear_all_caches()


class TestImportFallbacks:
    """Poison both optional imports and reload — must run last in the file."""

    def test_fallbacks_when_imports_missing(self, monkeypatch):
        monkeypatch.setitem(sys.modules, "config", None)
        monkeypatch.setitem(sys.modules, "advanced_cache", None)
        importlib.reload(sc)
        try:
            assert sc.CONFIG_AVAILABLE is False
            assert sc.ScoutConfig is None
            assert sc.CACHE_AVAILABLE is False
            assert sc.AdvancedCache is None
            c = RugCheckClient()
            assert c.fail_mode == "closed"
        finally:
            monkeypatch.undo()
            importlib.reload(sc)
            assert sc.CONFIG_AVAILABLE is True
