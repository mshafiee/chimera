"""
Coverage tests for core.helius_client.HeliusClient.

Mocks aiohttp sessions/responses and Redis, covering all public methods,
helpers, error paths, retries, pagination, and swap parsing.
"""

import asyncio
import importlib
import json
import os
import sys
import time
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch

import aiohttp
import pytest

import core.helius_client as helius_mod
from config import ScoutConfig

W1 = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
W2 = "6ogzHhzdrQr9Pgv6hZ2MNze7UrzBMAFyBBWU5biqCzVz"
W3 = "2weMjPLLybRMMva1fM3U31goWWrCpF59CHWNhnCJ9Vyh"
TOKEN_A = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
TOKEN_B = "EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm"
USDC = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
USDT = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
WSOL = "So11111111111111111111111111111111111111112"


class _FakeRequestInfo:
    real_url = "http://test.invalid"
    url = "http://test.invalid"
    method = "GET"


class _AioResp:
    """aiohttp-style response supporting `async with` + raise_for_status."""

    def __init__(self, status=200, payload=None, headers=None, raise_exc=None):
        self.status = status
        self._payload = payload
        self.headers = headers or {}
        self.request_info = _FakeRequestInfo()
        self.history = None
        self._raise_exc = raise_exc

    async def __aenter__(self):
        return self

    async def __aexit__(self, *a):
        return False

    async def json(self):
        return self._payload

    def raise_for_status(self):
        if self._raise_exc:
            raise self._raise_exc
        if self.status >= 400:
            raise aiohttp.ClientResponseError(
                request_info=self.request_info,
                history=self.history,
                status=self.status,
                message=f"HTTP {self.status}",
            )


class _RaiseResp:
    """Response whose __aenter__ raises (simulates transport errors)."""

    def __init__(self, exc):
        self._exc = exc

    async def __aenter__(self):
        raise self._exc

    async def __aexit__(self, *a):
        return False


def _resp(status=200, payload=None, headers=None, raise_exc=None):
    return _AioResp(status=status, payload=payload, headers=headers, raise_exc=raise_exc)


class _FakeSession:
    """Stub aiohttp session: get()/post() return queued fake responses."""

    def __init__(self, responses):
        self._responses = list(responses) if isinstance(responses, list) else [responses]
        self._idx = 0
        self._loop = None
        self.closed = False

    def _next(self):
        if self._idx < len(self._responses):
            r = self._responses[self._idx]
            self._idx += 1
            return r
        return _resp(status=404)

    def get(self, *a, **k):
        return self._next()

    def post(self, *a, **k):
        return self._next()

    async def close(self):
        self.closed = True


async def _attach(client, responses):
    """Attach a fake session bound to the running loop."""
    fake = _FakeSession(responses)
    fake._loop = asyncio.get_running_loop()
    client._session = fake
    return fake


def _trend_token(i):
    """Unique 34-char base58-valid token address (avoids '0' — not in base58)."""
    letters = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
    return "D" + "1" * 31 + letters[i // 10] + letters[i % 10]


def _patch_config_dir(monkeypatch, tmp_path):
    """Point helius_mod.Path at a temp tree whose ``parent.parent/config`` is tmp_path/config.

    The module resolves config files via ``Path(__file__).parent.parent / "config"``,
    so the fake Path must return a path two levels below the temp root.
    """
    config_dir = tmp_path / "config"
    config_dir.mkdir(exist_ok=True)
    monkeypatch.setattr(helius_mod, "Path", lambda *a, **k: tmp_path / "scout" / "core")
    return config_dir


class _FakeCache:
    """Minimal stand-in for advanced_cache.get_cache() singleton."""

    def __init__(self):
        self.store = {}

    def get(self, prefix, identifier, *args, category=None):
        return self.store.get((prefix, identifier, args))

    def set(self, prefix, identifier, value, *args, category=None):
        self.store[(prefix, identifier, args)] = value


class _FakeTracker:
    """Minimal stand-in for helius_credit_tracker.get_credit_tracker()."""

    def __init__(self, credits=1_000_000):
        self._credits = credits
        self.recorded = []

    def get_snapshot(self):
        return SimpleNamespace(credits_remaining=self._credits)

    def record_request(self, **kwargs):
        self.recorded.append(kwargs)


@pytest.fixture(autouse=True)
def _isolated(monkeypatch):
    """Patch sleeps and singletons for fast, hermetic unit tests."""
    monkeypatch.setattr(helius_mod.asyncio, "sleep", AsyncMock())
    monkeypatch.setattr(helius_mod, "get_cache", lambda: _FakeCache())
    monkeypatch.setattr(helius_mod, "get_credit_tracker", lambda: _FakeTracker(credits=1_000_000))
    yield


@pytest.fixture
def client():
    return helius_mod.HeliusClient(api_key="test_key")


class TestSafeFloat:
    def test_numeric(self):
        assert helius_mod._safe_float(1.5) == 1.5

    def test_numeric_string(self):
        assert helius_mod._safe_float("2.25") == 2.25

    def test_none(self):
        assert helius_mod._safe_float(None) == 0.0

    def test_dict_value(self):
        assert helius_mod._safe_float({"amount": 5}) == 0.0

    def test_bad_string(self):
        assert helius_mod._safe_float("abc", default=-1.0) == -1.0


class TestImportFallbacks:
    @pytest.mark.asyncio
    async def test_import_fallback_branches(self, monkeypatch, tmp_path):
        """Reload the module with imports failing to cover the except branches."""
        blocked = ("core.advanced_cache", "core.caching", "core.helius_credit_tracker", "config")
        saved = {k: sys.modules[k] for k in blocked if k in sys.modules}
        for k in blocked:
            monkeypatch.setitem(sys.modules, k, None)
        mod = importlib.reload(helius_mod)
        assert mod.CACHE_AVAILABLE is False
        assert mod.ACTIVITY_CACHE_AVAILABLE is False
        assert mod.CREDIT_TRACKER_AVAILABLE is False
        assert mod.ScoutConfig is None
        # __init__ fallback paths without ScoutConfig
        monkeypatch.delenv("HELIUS_API_KEY", raising=False)
        monkeypatch.delenv("CHIMERA_RPC__PRIMARY_URL", raising=False)
        monkeypatch.delenv("SOLANA_RPC_URL", raising=False)
        monkeypatch.setenv("SCOUT_HELIUS_RATE_LIMIT_MS", "5")
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        c = mod.HeliusClient(api_key="fallback_key")
        assert len(c.dex_programs) == 3
        assert c.base_url == "https://api.helius.xyz/v0"
        assert c.rate_limit_delay == 0.015
        assert c._adaptive_enabled is False
        assert c._max_api_calls == 500

        # ScoutConfig=None code paths (env-driven config fallbacks)
        c._discovery_cache = {"wallets": [W1]}
        c._discovery_cache_time = time.time()
        assert c._get_discovery_cache(24, 5) == [W1]
        redis = MagicMock()
        redis.is_available.return_value = True
        c._redis = redis
        c._set_discovery_cache([W1], 24, 5)
        redis.set.assert_called_once()
        pipe = MagicMock()
        redis.redis_client.pipeline.return_value = pipe
        c._mark_wallets_seen([W1])
        pipe.execute.assert_called_once()
        monkeypatch.setattr(c, "_validate_wallet_activity", AsyncMock(return_value=True))
        assert await c._batch_validate_activity([W1]) == [W1]
        await _attach(c, [_resp(200, [{"id": 0, "result": {"value": 1000}}])])
        assert await c._filter_by_sol_balance([W1], min_balance_sol=0.0) == [W1]
        await _attach(c, [_resp(200, [{"id": 0, "result": {"value": 1000}}])])
        assert await c.get_wallet_sol_balances([W1]) == {W1: 1e-6}
        monkeypatch.setattr(
            c, "_query_token_transactions",
            AsyncMock(return_value=(TOKEN_A, [{"feePayer": W1}])),
        )
        result = await c._discover_from_active_tokens(
            token_addresses=[TOKEN_A, TOKEN_B],
            hours_back=24, limit_per_token=10, use_parallel=True, max_wallets=100
        )
        assert W1 in result
        monkeypatch.setattr(c, "_discover_from_active_tokens", AsyncMock(return_value={W1: 2}))
        monkeypatch.setattr(c, "_discover_from_recent_blocks", AsyncMock(return_value={}))
        monkeypatch.setattr(c, "_discover_from_dex_programs", AsyncMock(return_value={}))
        monkeypatch.setattr(c, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        monkeypatch.setattr(c, "_filter_by_sol_balance", AsyncMock(return_value=[W1]))
        # clear the in-memory discovery cache populated earlier in this test
        c._discovery_cache = {}
        c._discovery_cache_time = 0.0
        assert await c.discover_wallets_from_recent_swaps(min_trade_count=2) == [W1]

        # Restore real imports BEFORE reloading so the module returns to its
        # production state (otherwise the fallback flags leak into all later
        # tests in this session).
        for k, v in saved.items():
            monkeypatch.setitem(sys.modules, k, v)
        importlib.reload(helius_mod)


class TestInit:
    def test_api_key_param(self):
        c = helius_mod.HeliusClient(api_key="abc")
        assert c.api_key == "abc"

    def test_api_key_env_fallback(self, monkeypatch):
        monkeypatch.setenv("HELIUS_API_KEY", "env_key")
        c = helius_mod.HeliusClient()
        assert c.api_key == "env_key"

    def test_api_key_from_rpc_url(self, monkeypatch):
        monkeypatch.delenv("HELIUS_API_KEY", raising=False)
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "https://rpc.test/?api-key=extracted")
        c = helius_mod.HeliusClient()
        assert c.api_key == "extracted"

    def test_api_key_rpc_url_without_key(self, monkeypatch):
        monkeypatch.delenv("HELIUS_API_KEY", raising=False)
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "https://rpc.test/")
        c = helius_mod.HeliusClient()
        assert c.api_key is None

    def test_session_passthrough(self):
        s = MagicMock()
        c = helius_mod.HeliusClient(api_key="k", session=s)
        assert c._session is s
        assert c._own_session is False

    def test_redis_passthrough(self):
        r = MagicMock()
        c = helius_mod.HeliusClient(api_key="k", redis_client=r)
        assert c._redis is r

    def test_rate_limit_env(self, monkeypatch):
        monkeypatch.setenv("SCOUT_HELIUS_RATE_LIMIT_MS", "100")
        c = helius_mod.HeliusClient(api_key="k")
        assert c.rate_limit_delay == 0.1

    def test_adaptive_disabled_delay(self, monkeypatch):
        monkeypatch.setattr(ScoutConfig, "get_rate_limit_adaptive", staticmethod(lambda: False))
        c = helius_mod.HeliusClient(api_key="k")
        assert c._current_delay == c.rate_limit_delay

    def test_activity_cache_init_failure(self, monkeypatch):
        monkeypatch.setattr(
            helius_mod, "HeliusCachingWrapper", MagicMock(side_effect=RuntimeError("boom"))
        )
        c = helius_mod.HeliusClient(api_key="k")
        assert c._activity_cache is None

    def test_activity_cache_ok(self):
        c = helius_mod.HeliusClient(api_key="k")
        assert c._activity_cache is not None

    def test_no_api_key_ok(self, monkeypatch):
        monkeypatch.delenv("HELIUS_API_KEY", raising=False)
        monkeypatch.delenv("CHIMERA_RPC__PRIMARY_URL", raising=False)
        monkeypatch.delenv("SOLANA_RPC_URL", raising=False)
        c = helius_mod.HeliusClient()
        assert c.api_key is None

    def test_rpc_url_parse_error(self, monkeypatch):
        monkeypatch.delenv("HELIUS_API_KEY", raising=False)
        real_getenv = os.getenv

        def fake_getenv(name, default=""):
            if name == "CHIMERA_RPC__PRIMARY_URL":
                return 12345  # non-str -> urlparse raises -> except branch
            if name == "HELIUS_API_KEY":
                return ""
            return real_getenv(name, default)

        monkeypatch.setattr(helius_mod.os, "getenv", fake_getenv)
        c = helius_mod.HeliusClient()
        assert not c.api_key


class TestRedactApiKey:
    def test_redacts(self):
        s = "https://x/?api-key=SECRET123&other=1"
        assert "SECRET123" not in helius_mod.HeliusClient._redact_api_key(s)
        assert "REDACTED" in helius_mod.HeliusClient._redact_api_key(s)

    def test_unchanged_without_key(self):
        s = "https://x/?a=b"
        assert helius_mod.HeliusClient._redact_api_key(s) == s


class TestSessionManagement:
    @pytest.mark.asyncio
    async def test_get_session_creates(self):
        c = helius_mod.HeliusClient(api_key="k")
        with patch.object(helius_mod.aiohttp, "ClientSession") as cls, patch.object(
            helius_mod.aiohttp, "TCPConnector"
        ) as conn:
            cls.return_value = MagicMock(_loop=asyncio.get_running_loop())
            s = await c._get_session()
        assert c._own_session is True
        assert s is c._session
        conn.assert_called_once()

    @pytest.mark.asyncio
    async def test_get_session_reuses(self, client):
        fake = MagicMock(_loop=asyncio.get_running_loop())
        client._session = fake
        s = await client._get_session()
        assert s is fake
        assert client._own_session is False

    @pytest.mark.asyncio
    async def test_get_session_loop_mismatch(self, client):
        fake = MagicMock(_loop=object())
        client._session = fake
        with patch.object(helius_mod.aiohttp, "ClientSession") as cls:
            cls.return_value = MagicMock(_loop=asyncio.get_running_loop())
            s = await client._get_session()
        assert s is not fake
        assert client._own_session is True

    @pytest.mark.asyncio
    async def test_close_session_owned(self, client):
        fake = _FakeSession([])
        fake._loop = asyncio.get_running_loop()
        client._session = fake
        client._own_session = True
        await client._close_session()
        assert fake.closed is True
        assert client._session is None
        assert client._own_session is False

    @pytest.mark.asyncio
    async def test_close_session_raises(self, client):
        fake = MagicMock(_loop=asyncio.get_running_loop())
        fake.close = AsyncMock(side_effect=Exception("boom"))
        client._session = fake
        client._own_session = True
        await client._close_session()
        assert client._session is None

    @pytest.mark.asyncio
    async def test_close_session_not_owned(self, client):
        fake = _FakeSession([])
        client._session = fake
        client._own_session = False
        await client._close_session()
        assert fake.closed is False

    @pytest.mark.asyncio
    async def test_close(self, client):
        fake = _FakeSession([])
        fake._loop = asyncio.get_running_loop()
        client._session = fake
        client._own_session = True
        await client.close()
        assert fake.closed is True


class TestRedisDiscoveryCache:
    def test_redis_available_false(self, client):
        assert client._redis_available() is False

    def test_redis_available_true(self, client):
        redis = MagicMock()
        redis.is_available.return_value = True
        client._redis = redis
        assert client._redis_available() is True

    def test_get_discovery_cache_redis_hit(self, client):
        redis = MagicMock()
        redis.is_available.return_value = True
        redis.get.return_value = json.dumps([W1, W2])
        client._redis = redis
        result = client._get_discovery_cache(24, 10)
        assert result == [W1, W2]

    def test_get_discovery_cache_redis_trim(self, client):
        redis = MagicMock()
        redis.is_available.return_value = True
        redis.get.return_value = json.dumps([W1, W2, W3])
        client._redis = redis
        assert client._get_discovery_cache(24, 2) == [W1, W2]

    def test_get_discovery_cache_redis_error(self, client):
        redis = MagicMock()
        redis.is_available.return_value = True
        redis.get.side_effect = RuntimeError("down")
        client._redis = redis
        client._discovery_cache = {"wallets": [W1]}
        client._discovery_cache_time = time.time()
        assert client._get_discovery_cache(24, 5) == [W1]

    def test_get_discovery_cache_redis_nonlist(self, client):
        redis = MagicMock()
        redis.is_available.return_value = True
        redis.get.return_value = json.dumps({"not": "list"})
        client._redis = redis
        assert client._get_discovery_cache(24, 5) is None

    def test_get_discovery_cache_memory_fresh(self, client):
        client._discovery_cache = {"wallets": [W1, W2]}
        client._discovery_cache_time = time.time()
        assert client._get_discovery_cache(24, 1) == [W1]

    def test_get_discovery_cache_memory_expired(self, client):
        client._discovery_cache = {"wallets": [W1]}
        client._discovery_cache_time = time.time() - 4000
        assert client._get_discovery_cache(24, 1) is None

    def test_get_discovery_cache_memory_expired_with_redis(self, client):
        redis = MagicMock()
        redis.is_available.return_value = True
        redis.get.return_value = None
        client._redis = redis
        client._discovery_cache = {"wallets": [W1]}
        client._discovery_cache_time = time.time() - 4000
        assert client._get_discovery_cache(24, 1) is None

    def test_set_discovery_cache(self, client):
        redis = MagicMock()
        redis.is_available.return_value = True
        client._redis = redis
        client._set_discovery_cache([W1], 24, 10)
        assert client._discovery_cache == {"wallets": [W1]}
        redis.set.assert_called_once()

    def test_set_discovery_cache_no_redis(self, client):
        client._set_discovery_cache([W1], 24, 10)
        assert client._discovery_cache == {"wallets": [W1]}

    def test_set_discovery_cache_redis_error(self, client):
        redis = MagicMock()
        redis.is_available.return_value = True
        redis.set.side_effect = RuntimeError("down")
        client._redis = redis
        client._set_discovery_cache([W1], 24, 10)
        assert client._discovery_cache == {"wallets": [W1]}

    def test_get_persistent_seen_no_redis(self, client):
        assert client._get_persistent_seen_wallets() == set()

    def test_get_persistent_seen_members(self, client):
        redis = MagicMock()
        redis.is_available.return_value = True
        redis.redis_client.smembers.return_value = {W1, W2}
        client._redis = redis
        assert client._get_persistent_seen_wallets() == {W1, W2}

    def test_get_persistent_seen_error(self, client):
        redis = MagicMock()
        redis.is_available.return_value = True
        redis.redis_client.smembers.side_effect = RuntimeError("down")
        client._redis = redis
        assert client._get_persistent_seen_wallets() == set()

    def test_mark_wallets_seen_no_redis(self, client):
        client._mark_wallets_seen([W1])
        assert client._mark_wallets_seen([]) is None

    def test_mark_wallets_seen_pipe(self, client):
        redis = MagicMock()
        redis.is_available.return_value = True
        pipe = MagicMock()
        redis.redis_client.pipeline.return_value = pipe
        client._redis = redis
        client._mark_wallets_seen([W1, W2])
        pipe.sadd.assert_called_once_with("scout:discovery:seen_wallets", W1, W2)
        pipe.execute.assert_called_once()

    def test_mark_wallets_seen_error(self, client):
        redis = MagicMock()
        redis.is_available.return_value = True
        redis.redis_client.pipeline.side_effect = RuntimeError("down")
        client._redis = redis
        client._mark_wallets_seen([W1])


class TestRateLimitSync:
    def test_rate_limit_no_delay(self, client):
        client.last_request_time = time.time() - 10
        with patch.object(helius_mod.time, "sleep") as slp:
            client._rate_limit()
        slp.assert_not_called()

    def test_rate_limit_with_delay(self, client):
        client.last_request_time = time.time()
        with patch.object(helius_mod.time, "sleep") as slp:
            client._rate_limit()
        assert slp.called


class TestCircuitBreaker:
    def test_check_closed(self, client):
        assert client._check_circuit_breaker() is True

    def test_check_open(self, client):
        client._circuit_breaker_failures = client._circuit_breaker_threshold
        assert client._check_circuit_breaker() is False

    def test_check_reset_after_cooldown(self, client):
        client._circuit_breaker_failures = client._circuit_breaker_threshold
        client._circuit_breaker_reset_time = time.time() - 1
        assert client._check_circuit_breaker() is True
        assert client._circuit_breaker_failures == 0

    def test_record_failure_sync_opens(self, client):
        for _ in range(client._circuit_breaker_threshold):
            client._record_failure_sync()
        assert client._circuit_breaker_reset_time is not None
        assert client._failure_count == client._circuit_breaker_threshold

    def test_record_failure_sync_below_threshold(self, client):
        client._record_failure_sync()
        assert client._circuit_breaker_reset_time is None

    @pytest.mark.asyncio
    async def test_record_failure_async(self, client):
        for _ in range(client._circuit_breaker_threshold):
            await client._record_failure()
        assert client._circuit_breaker_reset_time is not None

    @pytest.mark.asyncio
    async def test_record_success(self, client):
        client._circuit_breaker_failures = 3
        await client._record_success()
        assert client._success_count == 1
        assert client._circuit_breaker_failures == 2

    @pytest.mark.asyncio
    async def test_record_latency_disabled(self, client):
        client._adaptive_enabled = False
        await client._record_latency(50.0)
        assert client._latency_samples == []

    @pytest.mark.asyncio
    async def test_record_latency_enabled(self, client):
        client._adaptive_enabled = True
        client._max_latency_samples = 2
        await client._record_latency(10.0)
        await client._record_latency(20.0)
        await client._record_latency(30.0)
        assert client._latency_samples == [20.0, 30.0]

    @pytest.mark.asyncio
    async def test_get_avg_latency_empty(self, client):
        assert await client._get_avg_latency() is None

    @pytest.mark.asyncio
    async def test_get_avg_latency(self, client):
        client._latency_samples = [10.0, 20.0]
        assert await client._get_avg_latency() == 15.0


class TestAdaptiveRateLimit:
    @pytest.mark.asyncio
    async def test_adjust_disabled(self, client):
        client._adaptive_enabled = False
        await client._adjust_rate_limit()

    @pytest.mark.asyncio
    async def test_adjust_no_samples(self, client):
        client._adaptive_enabled = True
        await client._adjust_rate_limit()

    @pytest.mark.asyncio
    async def test_adjust_no_requests(self, client):
        client._adaptive_enabled = True
        client._latency_samples = [10.0]
        await client._adjust_rate_limit()

    @pytest.mark.asyncio
    async def test_adjust_slow_down(self, client):
        client._adaptive_enabled = True
        client._latency_samples = [300.0]
        client._success_count = 1
        old = client._current_delay
        await client._adjust_rate_limit()
        assert client._current_delay > old

    @pytest.mark.asyncio
    async def test_adjust_slow_down_capped(self, client):
        client._adaptive_enabled = True
        client._latency_samples = [300.0]
        client._success_count = 1
        client._current_delay = client._max_delay
        await client._adjust_rate_limit()
        assert client._current_delay == client._max_delay

    @pytest.mark.asyncio
    async def test_adjust_speed_up(self, client):
        client._adaptive_enabled = True
        client._latency_samples = [20.0]
        client._success_count = 100
        old = client._current_delay
        await client._adjust_rate_limit()
        assert client._current_delay < old

    @pytest.mark.asyncio
    async def test_adjust_speed_up_floored(self, client):
        client._adaptive_enabled = True
        client._latency_samples = [20.0]
        client._success_count = 100
        client._current_delay = client._min_delay
        await client._adjust_rate_limit()
        assert client._current_delay == client._min_delay

    @pytest.mark.asyncio
    async def test_get_rate_limit_stats(self, client):
        client._latency_samples = [10.0, 30.0]
        client._success_count = 9
        client._failure_count = 1
        stats = await client.get_rate_limit_stats()
        assert stats["avg_latency_ms"] == 20.0
        assert stats["success_ratio"] == 0.9
        assert stats["circuit_breaker_open"] is False
        assert stats["current_rps"] > 0

    @pytest.mark.asyncio
    async def test_get_rate_limit_stats_open(self, client):
        client._circuit_breaker_failures = client._circuit_breaker_threshold
        stats = await client.get_rate_limit_stats()
        assert stats["circuit_breaker_open"] is True
        assert stats["avg_latency_ms"] is None


class TestRetryWithBackoff:
    @pytest.mark.asyncio
    async def test_success_first_attempt(self, client):
        async def factory():
            return "ok"

        result = await client._retry_with_backoff(factory, max_retries=3)
        assert result == "ok"
        assert client._success_count == 1

    @pytest.mark.asyncio
    async def test_sync_factory(self, client):
        def factory():
            return "sync-ok"

        assert await client._retry_with_backoff(factory, max_retries=2) == "sync-ok"

    @pytest.mark.asyncio
    async def test_retry_then_success(self, client):
        attempts = {"n": 0}

        async def factory():
            attempts["n"] += 1
            if attempts["n"] < 3:
                raise aiohttp.ClientResponseError(
                    request_info=_FakeRequestInfo(), history=None, status=429, message="rate"
                )
            return "ok"

        result = await client._retry_with_backoff(factory, max_retries=5)
        assert result == "ok"
        assert attempts["n"] == 3
        assert client._failure_count == 0

    @pytest.mark.asyncio
    async def test_non_retryable_raises(self, client):
        async def factory():
            raise aiohttp.ClientResponseError(
                request_info=_FakeRequestInfo(), history=None, status=400, message="bad"
            )

        with pytest.raises(aiohttp.ClientResponseError):
            await client._retry_with_backoff(factory, max_retries=5)
        assert client._failure_count == 1

    @pytest.mark.asyncio
    async def test_exhausted_raises(self, client):
        attempts = {"n": 0}

        async def factory():
            attempts["n"] += 1
            raise aiohttp.ClientResponseError(
                request_info=_FakeRequestInfo(), history=None, status=503, message="down"
            )

        with pytest.raises(aiohttp.ClientResponseError):
            await client._retry_with_backoff(factory, max_retries=2)
        assert attempts["n"] == 2
        assert client._failure_count == 1

    @pytest.mark.asyncio
    async def test_network_error_retryable(self, client):
        attempts = {"n": 0}

        async def factory():
            attempts["n"] += 1
            raise aiohttp.ClientConnectionError("conn refused")

        with pytest.raises(aiohttp.ClientConnectionError):
            await client._retry_with_backoff(factory, max_retries=3)
        assert attempts["n"] == 3

    @pytest.mark.asyncio
    async def test_timeout_retryable(self, client):
        attempts = {"n": 0}

        async def factory():
            attempts["n"] += 1
            raise asyncio.TimeoutError("slow")

        with pytest.raises(asyncio.TimeoutError):
            await client._retry_with_backoff(factory, max_retries=2)
        assert attempts["n"] == 2

    @pytest.mark.asyncio
    async def test_backoff_sleeps(self, client):
        async def factory():
            raise aiohttp.ClientResponseError(
                request_info=_FakeRequestInfo(), history=None, status=429, message="rate"
            )

        with patch.object(helius_mod.asyncio, "sleep", new=AsyncMock()) as slp:
            with pytest.raises(aiohttp.ClientResponseError):
                await client._retry_with_backoff(factory, max_retries=2)
        assert slp.await_count == 1

    @pytest.mark.asyncio
    async def test_max_retries_one(self, client):
        async def factory():
            raise aiohttp.ClientResponseError(
                request_info=_FakeRequestInfo(), history=None, status=429, message="rate"
            )

        with pytest.raises(aiohttp.ClientResponseError):
            await client._retry_with_backoff(factory, max_retries=1)

    @pytest.mark.asyncio
    async def test_zero_retries_returns_none(self, client):
        async def factory():
            return "never-called"

        assert await client._retry_with_backoff(factory, max_retries=0) is None
        assert client._success_count == 0


class TestIsRetryableError:
    def test_non_retryable_statuses(self, client):
        for code in (400, 401, 403, 404, 409, 422):
            assert client._is_retryable_error(code) is False

    def test_retryable_statuses(self, client):
        for code in (408, 429, 500, 502, 503, 504):
            assert client._is_retryable_error(code) is True

    def test_network_error(self, client):
        assert client._is_retryable_error(None, aiohttp.ClientConnectionError()) is True

    def test_timeout_error(self, client):
        assert client._is_retryable_error(None, asyncio.TimeoutError()) is True

    def test_unknown(self, client):
        assert client._is_retryable_error(418) is False


class TestRateLimitAsync:
    @pytest.mark.asyncio
    async def test_waits(self, client):
        client.last_request_time = time.time()
        with patch.object(helius_mod.asyncio, "sleep", new=AsyncMock()) as slp:
            await client._rate_limit_async()
        assert slp.await_count == 1

    @pytest.mark.asyncio
    async def test_no_wait(self, client):
        client.last_request_time = time.time() - 10
        with patch.object(helius_mod.asyncio, "sleep", new=AsyncMock()) as slp:
            await client._rate_limit_async()
        slp.assert_not_called()


class TestMakeRequest:
    @pytest.mark.asyncio
    async def test_no_api_key(self, client):
        client.api_key = None
        assert await client._make_request("/x") is None

    @pytest.mark.asyncio
    async def test_circuit_breaker_open(self, client):
        client._circuit_breaker_failures = client._circuit_breaker_threshold
        assert await client._make_request("/x") is None

    @pytest.mark.asyncio
    async def test_max_api_calls(self, client):
        client._api_calls_made = client._max_api_calls
        assert await client._make_request("/x") is None

    @pytest.mark.asyncio
    async def test_success(self, client):
        await _attach(client, [_resp(200, {"result": "value"})])
        result = await client._make_request("/endpoint", {"a": 1})
        assert result == {"result": "value"}
        assert client._api_calls_made == 1

    @pytest.mark.asyncio
    async def test_no_retry_mode(self, client):
        await _attach(client, [_resp(200, {"ok": True})])
        result = await client._make_request("/endpoint", {}, use_retry=False)
        assert result == {"ok": True}

    @pytest.mark.asyncio
    async def test_429_retry_after_then_success(self, client):
        await _attach(client, [
            _resp(429, headers={"Retry-After": "2"}),
            _resp(200, {"ok": True}),
        ])
        result = await client._make_request("/endpoint", {})
        assert result == {"ok": True}
        assert client._api_calls_made == 1

    @pytest.mark.asyncio
    async def test_429_exhausted(self, client):
        await _attach(client, [_resp(429, headers={"Retry-After": "1"})] * 5)
        assert await client._make_request("/endpoint", {}) is None

    @pytest.mark.asyncio
    async def test_non_retryable_404(self, client):
        await _attach(client, [_resp(404)])
        assert await client._make_request("/endpoint", {}) is None

    @pytest.mark.asyncio
    async def test_timeout(self, client):
        client._session = _FakeSession([_RaiseResp(asyncio.TimeoutError())] * 5)
        client._session._loop = asyncio.get_running_loop()
        assert await client._make_request("/endpoint", {}) is None

    @pytest.mark.asyncio
    async def test_network_error(self, client):
        client._session = _FakeSession([_RaiseResp(aiohttp.ClientConnectionError("refused"))])
        client._session._loop = asyncio.get_running_loop()
        assert await client._make_request("/endpoint", {}) is None


class TestLoadActiveTokens:
    def test_from_env(self, client, monkeypatch):
        monkeypatch.setenv("SCOUT_ACTIVE_TOKENS", "AAA, BBB ,")
        assert client._load_active_tokens() == ["AAA", "BBB"]

    def test_cached(self, client):
        client._token_list_cache = [TOKEN_A]
        client._token_list_cache_time = time.time()
        assert client._load_active_tokens() == [TOKEN_A]

    def test_cache_expired(self, client, monkeypatch, tmp_path):
        client._token_list_cache = [TOKEN_A]
        client._token_list_cache_time = time.time() - 90000
        _patch_config_dir(monkeypatch, tmp_path)
        tokens = client._load_active_tokens()
        assert tokens == client._token_list_cache
        assert client._token_list_cache_time > time.time() - 1

    def test_from_config_file(self, client, monkeypatch, tmp_path):
        config_dir = _patch_config_dir(monkeypatch, tmp_path)
        (config_dir / "active_tokens.txt").write_text(
            "# comment\n%s\n%s # inline\n\n" % (TOKEN_A, TOKEN_B)
        )
        assert client._load_active_tokens() == [TOKEN_A, TOKEN_B]

    def test_config_read_error(self, client, monkeypatch, tmp_path):
        config_dir = _patch_config_dir(monkeypatch, tmp_path)
        (config_dir / "active_tokens.txt").mkdir()  # open() on a directory raises
        tokens = client._load_active_tokens()
        assert tokens  # defaults used


class TestRefreshTokenList:
    @pytest.mark.asyncio
    async def test_no_birdeye_key(self, client, monkeypatch):
        monkeypatch.delenv("BIRDEYE_API_KEY", raising=False)
        assert await client._refresh_token_list() is False

    @pytest.mark.asyncio
    async def test_http_error(self, client, monkeypatch):
        monkeypatch.setenv("BIRDEYE_API_KEY", "key")
        await _attach(client, [_resp(500)])
        assert await client._refresh_token_list() is False

    @pytest.mark.asyncio
    async def test_no_trending(self, client, monkeypatch):
        monkeypatch.setenv("BIRDEYE_API_KEY", "key")
        await _attach(client, [_resp(200, {"trending_tokens": []})])
        assert await client._refresh_token_list() is False

    @pytest.mark.asyncio
    async def test_success(self, client, monkeypatch, tmp_path):
        monkeypatch.setenv("BIRDEYE_API_KEY", "key")
        config_dir = _patch_config_dir(monkeypatch, tmp_path)
        (config_dir / "active_tokens.txt").write_text("# existing\n%s\n%s\n" % (TOKEN_A, TOKEN_B))
        trending = [{"address": _trend_token(i)} for i in range(80)]
        await _attach(client, [_resp(200, {"trending_tokens": trending})])
        assert await client._refresh_token_list() is True
        written = (config_dir / "active_tokens.txt").read_text()
        assert _trend_token(25) in written
        assert (config_dir / "active_tokens.txt.backup").exists()
        assert len(client._token_list_cache) >= 50

    @pytest.mark.asyncio
    async def test_too_few_tokens(self, client, monkeypatch, tmp_path):
        monkeypatch.setenv("BIRDEYE_API_KEY", "key")
        config_dir = _patch_config_dir(monkeypatch, tmp_path)
        (config_dir / "active_tokens.txt").write_text("# existing\n")
        await _attach(client, [
            _resp(200, {"trending_tokens": [{"address": _trend_token(i)} for i in range(10)]})
        ])
        assert await client._refresh_token_list() is False

    @pytest.mark.asyncio
    async def test_invalid_addresses_skipped(self, client, monkeypatch, tmp_path):
        monkeypatch.setenv("BIRDEYE_API_KEY", "key")
        config_dir = _patch_config_dir(monkeypatch, tmp_path)
        (config_dir / "active_tokens.txt").write_text("# existing\n%s\n" % TOKEN_A)
        trending = [{"address": _trend_token(i)} for i in range(60)]
        trending.append({"address": "short"})
        await _attach(client, [_resp(200, {"trending_tokens": trending})])
        assert await client._refresh_token_list() is True

    @pytest.mark.asyncio
    async def test_exception(self, client, monkeypatch):
        monkeypatch.setenv("BIRDEYE_API_KEY", "key")
        client._session = _FakeSession([_RaiseResp(RuntimeError("boom"))])
        client._session._loop = asyncio.get_running_loop()
        assert await client._refresh_token_list() is False


class TestIsValidSolanaAddress:
    def test_valid(self, client):
        assert client._is_valid_solana_address(W1) is True

    def test_empty(self, client):
        assert client._is_valid_solana_address("") is False
        assert client._is_valid_solana_address(None) is False

    def test_too_short(self, client):
        assert client._is_valid_solana_address("abc") is False

    def test_too_long(self, client):
        assert client._is_valid_solana_address(W1 + W1[:2]) is False

    def test_invalid_chars(self, client):
        assert client._is_valid_solana_address("0" * 40) is False

    def test_system_program(self, client):
        assert client._is_valid_solana_address("11111111111111111111111111111111") is False

    def test_exception(self, client):
        assert client._is_valid_solana_address(12345) is False


class TestLoadSeedWallets:
    def test_from_env(self, client, monkeypatch):
        monkeypatch.setenv("SCOUT_SEED_WALLETS", "AAA,BBB,")
        assert client._load_seed_wallets() == ["AAA", "BBB"]

    def test_from_config_file(self, client, monkeypatch, tmp_path):
        config_dir = _patch_config_dir(monkeypatch, tmp_path)
        (config_dir / "seed_wallets.txt").write_text("# c\n%s\n%s\n" % (W1, W2))
        assert client._load_seed_wallets() == [W1, W2]

    def test_config_read_error(self, client, monkeypatch, tmp_path):
        config_dir = _patch_config_dir(monkeypatch, tmp_path)
        (config_dir / "seed_wallets.txt").mkdir()  # open() on a directory raises
        assert client._load_seed_wallets() == []


class TestIsWalletKnown:
    def test_known_cache(self, client):
        client._known_wallets_cache.add(W1)
        assert client._is_wallet_known(W1) is True

    def test_discovered_this_run(self, client):
        client._discovered_this_run.add(W1)
        assert client._is_wallet_known(W1) is True

    def test_db_hit(self, client, fake_db_layer, monkeypatch, tmp_path):
        db_path = tmp_path / "chimera.db"
        db_path.write_text("")
        monkeypatch.setenv("CHIMERA_DB_PATH", str(db_path))
        conn = fake_db_layer
        conn.execute("CREATE TABLE wallets (address TEXT PRIMARY KEY)")
        conn.execute("INSERT INTO wallets (address) VALUES (?)", (W1,))
        conn.commit()
        assert client._is_wallet_known(W1, check_database=True) is True
        assert W1 in client._known_wallets_cache

    def test_db_miss(self, client, fake_db_layer, monkeypatch, tmp_path):
        db_path = tmp_path / "chimera.db"
        db_path.write_text("")
        monkeypatch.setenv("CHIMERA_DB_PATH", str(db_path))
        conn = fake_db_layer
        conn.execute("CREATE TABLE wallets (address TEXT PRIMARY KEY)")
        conn.commit()
        assert client._is_wallet_known(W1, check_database=True) is False

    def test_db_file_missing(self, client, monkeypatch, tmp_path):
        monkeypatch.setenv("CHIMERA_DB_PATH", str(tmp_path / "nope.db"))
        assert client._is_wallet_known(W1, check_database=True) is False

    def test_db_error(self, client, fake_db_layer, monkeypatch, tmp_path):
        db_path = tmp_path / "chimera.db"
        db_path.write_text("")
        monkeypatch.setenv("CHIMERA_DB_PATH", str(db_path))
        assert client._is_wallet_known(W1, check_database=True) is False

    def test_no_check_database(self, client):
        assert client._is_wallet_known(W1, check_database=False) is False


class TestParseUiTokenAmount:
    def test_raw_token_amount_dict(self, client):
        tr = {"rawTokenAmount": {"tokenAmount": "123", "decimals": 6}}
        assert client._parse_ui_token_amount(tr) == 123 / 1e6

    def test_raw_token_amount_no_decimals(self, client):
        tr = {"rawTokenAmount": {"tokenAmount": "123"}}
        assert client._parse_ui_token_amount(tr) == 123.0

    def test_raw_token_amount_bad(self, client):
        tr = {"rawTokenAmount": {"tokenAmount": "abc", "decimals": 6}, "tokenAmount": 7.5}
        assert client._parse_ui_token_amount(tr) == 7.5

    def test_token_amount_ui_amount(self, client):
        tr = {"tokenAmount": {"uiAmount": 5.5}}
        assert client._parse_ui_token_amount(tr) == 5.5

    def test_token_amount_bad_ui_amount_falls_through(self, client):
        tr = {"tokenAmount": {"uiAmount": object(), "uiAmountString": "7.25"}}
        assert client._parse_ui_token_amount(tr) == 7.25

    def test_token_amount_ui_amount_string(self, client):
        tr = {"tokenAmount": {"uiAmount": None, "uiAmountString": "7.25"}}
        assert client._parse_ui_token_amount(tr) == 7.25

    def test_token_amount_amount_decimals(self, client):
        tr = {"tokenAmount": {"amount": "1500", "decimals": 3}}
        assert client._parse_ui_token_amount(tr) == 1.5

    def test_token_amount_bad_dict_fields(self, client):
        tr = {"tokenAmount": {"uiAmount": None, "uiAmountString": None, "amount": "bad"}}
        assert client._parse_ui_token_amount(tr) == 0.0

    def test_token_amount_scalar(self, client):
        tr = {"tokenAmount": "2.5"}
        assert client._parse_ui_token_amount(tr) == 2.5

    def test_token_amount_bad_scalar(self, client):
        tr = {"tokenAmount": object()}
        assert client._parse_ui_token_amount(tr) == 0.0

    def test_missing_all(self, client):
        assert client._parse_ui_token_amount({}) == 0.0


class TestWalletValidationHelpers:
    def test_validate_none(self, client):
        assert client._validate_wallet_address(None) is False
        assert client._validate_wallet_address(123) is False

    def test_validate_short(self, client):
        assert client._validate_wallet_address("abc") is False

    def test_validate_system_account(self, client):
        assert client._validate_wallet_address("11111111111111111111111111111111") is False

    def test_validate_dex_program(self, client):
        prog = client.dex_programs[0]
        assert client._validate_wallet_address(prog) is False

    def test_validate_non_wallet(self, client):
        assert client._validate_wallet_address(USDC) is False

    def test_validate_ends_with_ones(self, client):
        assert client._validate_wallet_address("X" * 12 + "1" * 32) is False

    def test_validate_program_like(self, client):
        assert client._validate_wallet_address("A" * 35 + "111111111") is False

    def test_validate_valid(self, client):
        assert client._validate_wallet_address(W1) is True

    def test_looks_like_program_short(self, client):
        assert helius_mod.HeliusClient._looks_like_program_address("abc") is False

    def test_looks_like_program_prefix(self, client):
        assert helius_mod.HeliusClient._looks_like_program_address("Sysvar" + "A" * 30) is True

    def test_looks_like_program_run(self, client):
        assert helius_mod.HeliusClient._looks_like_program_address("A" * 8 + "B" * 30) is True

    def test_looks_like_program_ones_run(self, client):
        assert helius_mod.HeliusClient._looks_like_program_address("B" * 30 + "1" * 8) is True

    def test_looks_like_program_ends_ones(self, client):
        assert helius_mod.HeliusClient._looks_like_program_address("C" * 36 + "11111111") is True

    def test_looks_like_program_ok(self, client):
        assert helius_mod.HeliusClient._looks_like_program_address(W1) is False

    def test_candidate_invalid(self, client):
        assert client._is_candidate_wallet_address("abc") is False

    def test_candidate_system(self, client):
        assert client._is_candidate_wallet_address(
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        ) is False

    def test_candidate_non_wallet(self, client):
        assert client._is_candidate_wallet_address(
            "JUP4Fb2cqiRUcaTHdrPC8h2gNsA2ETXiPDD33WcGuJB"
        ) is False

    def test_candidate_valid(self, client):
        assert client._is_candidate_wallet_address(W1) is True


class TestExtractWalletsFromTransaction:
    def test_not_dict(self, client):
        assert client._extract_wallets_from_transaction(None) == []

    def test_no_significant_activity(self, client):
        assert client._extract_wallets_from_transaction({}) == []

    def test_native_lamports_significant(self, client):
        tx = {
            "nativeTransfers": [{"amount": 100_000_000}],
            "tokenTransfers": [
                {"fromUserAccount": W1, "toUserAccount": W2, "mint": TOKEN_A},
                {"fromUserAccount": USDC, "toUserAccount": W3, "mint": TOKEN_B},
                {"fromUserAccount": "", "toUserAccount": W2},
                None,
            ],
            "feePayer": W1,
        }
        wallets = client._extract_wallets_from_transaction(tx)
        assert W1 in wallets
        assert W2 in wallets
        assert W3 in wallets
        assert client._discovery_stats["infrastructure_filtered"] == 1

    def test_native_sol_units_significant(self, client):
        tx = {"nativeTransfers": [{"amount": 100}], "tokenTransfers": []}
        assert client._extract_wallets_from_transaction(tx) == []

    def test_token_transfers_only_significant(self, client):
        tx = {"tokenTransfers": [{"fromUserAccount": W1, "toUserAccount": W2}]}
        assert set(client._extract_wallets_from_transaction(tx)) == {W1, W2}


class TestGetWalletFirstTransaction:
    @pytest.mark.asyncio
    async def test_no_api_key(self, client):
        client.api_key = None
        assert await client.get_wallet_first_transaction(W1) is None

    @pytest.mark.asyncio
    async def test_single_page(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [
            _resp(200, {"result": [{"signature": "s1", "blockTime": 12345}]})
        ])
        assert await client.get_wallet_first_transaction(W1) == 12345.0

    @pytest.mark.asyncio
    async def test_http_error(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(500)])
        assert await client.get_wallet_first_transaction(W1) is None

    @pytest.mark.asyncio
    async def test_empty_result(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(200, {"result": []})])
        assert await client.get_wallet_first_transaction(W1) is None

    @pytest.mark.asyncio
    async def test_no_block_time_then_missing_signature(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(200, {"result": [{"blockTime": None}]})])
        assert await client.get_wallet_first_transaction(W1) is None

    @pytest.mark.asyncio
    async def test_two_page(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        page1 = [{"signature": "sig%d" % i, "blockTime": 1000 + i} for i in range(1000)]
        page2 = [{"signature": "old", "blockTime": 500}]
        await _attach(client, [_resp(200, {"result": page1}), _resp(200, {"result": page2})])
        assert await client.get_wallet_first_transaction(W1) == 500.0

    @pytest.mark.asyncio
    async def test_two_page_empty_second(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        page1 = [{"signature": "sig%d" % i, "blockTime": 1000 + i} for i in range(1000)]
        await _attach(client, [_resp(200, {"result": page1}), _resp(200, {"result": []})])
        assert await client.get_wallet_first_transaction(W1) == 1999.0

    @pytest.mark.asyncio
    async def test_two_page_http_error(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        page1 = [{"signature": "sig%d" % i, "blockTime": 1000 + i} for i in range(1000)]
        await _attach(client, [_resp(200, {"result": page1}), _resp(500)])
        assert await client.get_wallet_first_transaction(W1) is None

    @pytest.mark.asyncio
    async def test_exception(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        client._session = _FakeSession([_RaiseResp(RuntimeError("boom"))])
        client._session._loop = asyncio.get_running_loop()
        assert await client.get_wallet_first_transaction(W1) is None

    @pytest.mark.asyncio
    async def test_rpc_url_fallback(self, client, monkeypatch):
        monkeypatch.delenv("CHIMERA_RPC__PRIMARY_URL", raising=False)
        monkeypatch.delenv("SOLANA_RPC_URL", raising=False)
        await _attach(client, [_resp(200, {"result": [{"blockTime": 42}]})])
        assert await client.get_wallet_first_transaction(W1) == 42.0


class TestGetWalletFunder:
    @pytest.mark.asyncio
    async def test_no_api_key(self, client):
        client.api_key = None
        assert await client.get_wallet_funder(W1) is None

    @pytest.mark.asyncio
    async def test_funder_found(self, client, monkeypatch):
        monkeypatch.setattr(
            client,
            "_make_request",
            AsyncMock(side_effect=[
                [{"signature": "sig1"}],
                {"nativeTransfers": [
                    {"toUserAccount": W1, "fromUserAccount": W2}
                ]},
            ]),
        )
        assert await client.get_wallet_funder(W1) == W2

    @pytest.mark.asyncio
    async def test_not_list(self, client, monkeypatch):
        monkeypatch.setattr(client, "_make_request", AsyncMock(return_value=None))
        assert await client.get_wallet_funder(W1) is None

    @pytest.mark.asyncio
    async def test_missing_signature(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_make_request", AsyncMock(side_effect=[[{"blockTime": 1}], {}])
        )
        assert await client.get_wallet_funder(W1) is None

    @pytest.mark.asyncio
    async def test_tx_not_dict(self, client, monkeypatch):
        monkeypatch.setattr(
            client,
            "_make_request",
            AsyncMock(side_effect=[[{"signature": "s1"}], None]),
        )
        assert await client.get_wallet_funder(W1) is None

    @pytest.mark.asyncio
    async def test_no_matching_transfer(self, client, monkeypatch):
        monkeypatch.setattr(
            client,
            "_make_request",
            AsyncMock(side_effect=[
                [{"signature": "s1"}],
                {"nativeTransfers": [{"toUserAccount": W2, "fromUserAccount": W3}]},
            ]),
        )
        assert await client.get_wallet_funder(W1) is None

    @pytest.mark.asyncio
    async def test_exception(self, client, monkeypatch):
        monkeypatch.setattr(client, "_make_request", AsyncMock(side_effect=RuntimeError("boom")))
        assert await client.get_wallet_funder(W1) is None


class TestGetTokenFirstTxTimestamp:
    @pytest.mark.asyncio
    async def test_no_api_key(self, client):
        client.api_key = None
        assert await client.get_token_first_tx_timestamp(TOKEN_A) is None
        assert await client.get_token_first_tx_timestamp("") is None

    @pytest.mark.asyncio
    async def test_returns_timestamp(self, client, monkeypatch):
        monkeypatch.setattr(client, "_make_request", AsyncMock(return_value=[{"timestamp": 123}]))
        assert await client.get_token_first_tx_timestamp(TOKEN_A) == 123

    @pytest.mark.asyncio
    async def test_not_list(self, client, monkeypatch):
        monkeypatch.setattr(client, "_make_request", AsyncMock(return_value={}))
        assert await client.get_token_first_tx_timestamp(TOKEN_A) is None

    @pytest.mark.asyncio
    async def test_exception(self, client, monkeypatch):
        monkeypatch.setattr(client, "_make_request", AsyncMock(side_effect=RuntimeError("boom")))
        assert await client.get_token_first_tx_timestamp(TOKEN_A) is None


class TestGetWalletSolBalance:
    @pytest.mark.asyncio
    async def test_no_rpc_url(self, client, monkeypatch):
        monkeypatch.delenv("CHIMERA_RPC__PRIMARY_URL", raising=False)
        monkeypatch.delenv("SOLANA_RPC_URL", raising=False)
        assert await client._get_wallet_sol_balance(W1) == 0.0

    @pytest.mark.asyncio
    async def test_success(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(200, {"result": {"value": 1000000000}})])
        assert await client._get_wallet_sol_balance(W1) == 1.0

    @pytest.mark.asyncio
    async def test_non_200(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(500)])
        assert await client._get_wallet_sol_balance(W1) == 0.0

    @pytest.mark.asyncio
    async def test_exception(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        client._session = _FakeSession([_RaiseResp(RuntimeError("boom"))])
        client._session._loop = asyncio.get_running_loop()
        assert await client._get_wallet_sol_balance(W1) == 0.0


class TestValidateWalletActivity:
    def _txs(self, n, ts, tx_type="SWAP"):
        return [{"timestamp": ts, "type": tx_type, "signature": "sig%d" % i} for i in range(n)]

    @pytest.mark.asyncio
    async def test_cache_hit(self, client, monkeypatch):
        cache = _FakeCache()
        cache.store[("wallet_validation", W1, (f"{W1}:3:7",))] = True
        monkeypatch.setattr(helius_mod, "get_cache", lambda: cache)
        assert await client._validate_wallet_activity(W1) is True

    @pytest.mark.asyncio
    async def test_validation_disabled(self, client, monkeypatch):
        monkeypatch.setenv("SCOUT_VALIDATE_WALLET_ACTIVITY", "false")
        assert await client._validate_wallet_activity(W1) is True

    @pytest.mark.asyncio
    async def test_too_few_trades(self, client, monkeypatch):
        monkeypatch.setattr(client, "get_wallet_transactions", AsyncMock(return_value=[]))
        assert await client._validate_wallet_activity(W1, min_trades=3) is False

    @pytest.mark.asyncio
    async def test_same_day_trades(self, client, monkeypatch):
        now = time.time()
        monkeypatch.setattr(
            client, "get_wallet_transactions",
            AsyncMock(return_value=self._txs(5, now - 3600)),
        )
        assert await client._validate_wallet_activity(W1, min_trades=5) is False

    @pytest.mark.asyncio
    async def test_no_recent_trade(self, client, monkeypatch):
        old = time.time() - 10 * 86400
        monkeypatch.setattr(
            client, "get_wallet_transactions",
            AsyncMock(return_value=self._txs(3, old)),
        )
        assert await client._validate_wallet_activity(W1, min_trades=3) is False

    @pytest.mark.asyncio
    async def test_low_balance(self, client, monkeypatch):
        now = time.time()
        monkeypatch.setattr(
            client, "get_wallet_transactions",
            AsyncMock(return_value=self._txs(3, now)),
        )
        monkeypatch.setattr(client, "_get_wallet_sol_balance", AsyncMock(return_value=0.0))
        assert await client._validate_wallet_activity(W1, min_trades=3) is False

    @pytest.mark.asyncio
    async def test_balance_error_continues(self, client, monkeypatch):
        now = time.time()
        monkeypatch.setattr(
            client, "get_wallet_transactions",
            AsyncMock(return_value=self._txs(3, now)),
        )
        monkeypatch.setattr(
            client, "_get_wallet_sol_balance", AsyncMock(side_effect=RuntimeError("rpc"))
        )
        assert await client._validate_wallet_activity(W1, min_trades=3) is True

    @pytest.mark.asyncio
    async def test_no_swap_types(self, client, monkeypatch):
        now = time.time()
        monkeypatch.setattr(
            client, "get_wallet_transactions",
            AsyncMock(return_value=self._txs(3, now, tx_type="TRANSFER")),
        )
        monkeypatch.setattr(client, "_get_wallet_sol_balance", AsyncMock(return_value=1.0))
        assert await client._validate_wallet_activity(W1, min_trades=3) is False

    @pytest.mark.asyncio
    async def test_no_recent_trades_high_threshold(self, client, monkeypatch):
        mid = time.time() - 30 * 3600
        txs = self._txs(3, mid - 86400) + self._txs(2, mid)
        monkeypatch.setattr(client, "get_wallet_transactions", AsyncMock(return_value=txs))
        monkeypatch.setattr(client, "_get_wallet_sol_balance", AsyncMock(return_value=1.0))
        assert await client._validate_wallet_activity(W1, min_trades=5) is False

    @pytest.mark.asyncio
    async def test_success(self, client, monkeypatch):
        now = time.time()
        txs = self._txs(3, now - 3600) + self._txs(2, now - 2 * 86400)
        monkeypatch.setattr(client, "get_wallet_transactions", AsyncMock(return_value=txs))
        monkeypatch.setattr(client, "_get_wallet_sol_balance", AsyncMock(return_value=1.0))
        assert await client._validate_wallet_activity(W1, min_trades=5) is True

    @pytest.mark.asyncio
    async def test_success_no_balance_check(self, client, monkeypatch):
        now = time.time()
        monkeypatch.setenv("SCOUT_MIN_SOL_BALANCE", "0")
        monkeypatch.setattr(
            client, "get_wallet_transactions",
            AsyncMock(return_value=self._txs(3, now)),
        )
        assert await client._validate_wallet_activity(W1, min_trades=3) is True

    @pytest.mark.asyncio
    async def test_exception_fails_closed(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "get_wallet_transactions", AsyncMock(side_effect=RuntimeError("boom"))
        )
        assert await client._validate_wallet_activity(W1) is False


class TestBatchValidateActivity:
    @pytest.mark.asyncio
    async def test_empty(self, client):
        assert await client._batch_validate_activity([]) == []

    @pytest.mark.asyncio
    async def test_mixed_results(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_validate_wallet_activity",
            AsyncMock(side_effect=[True, False, True]),
        )
        result = await client._batch_validate_activity([W1, W2, W3])
        assert result == [W1, W3]

    @pytest.mark.asyncio
    async def test_exception_result(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_validate_wallet_activity",
            AsyncMock(side_effect=[True, RuntimeError("boom")]),
        )
        assert await client._batch_validate_activity([W1, W2]) == [W1]

    @pytest.mark.asyncio
    async def test_max_wallets(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_validate_wallet_activity", AsyncMock(return_value=True)
        )
        result = await client._batch_validate_activity([W1, W2, W3], max_wallets=2)
        assert len(result) == 2


class TestAggressiveWalletFilter:
    @pytest.mark.asyncio
    async def test_empty(self, client):
        assert await client._aggressive_wallet_filter([]) == []

    @pytest.mark.asyncio
    async def test_validation_disabled(self, client):
        result = await client._aggressive_wallet_filter(
            [W1], validation_config={"validation_enabled": False}
        )
        assert result == [W1]

    @pytest.mark.asyncio
    async def test_no_valid_format(self, client):
        assert await client._aggressive_wallet_filter(["abc"]) == []

    @pytest.mark.asyncio
    async def test_no_balance_filter(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_batch_validate_activity", AsyncMock(return_value=[W1, W2])
        )
        result = await client._aggressive_wallet_filter(
            [W1, W2], validation_config={"min_sol_balance": 0.0, "min_trades": 2}
        )
        assert result == [W1, W2]

    @pytest.mark.asyncio
    async def test_with_balance_filter(self, client, monkeypatch):
        monkeypatch.setattr(client, "_filter_by_sol_balance", AsyncMock(return_value=[W1]))
        monkeypatch.setattr(client, "_batch_validate_activity", AsyncMock(return_value=[W1]))
        result = await client._aggressive_wallet_filter(
            [W1, W2], validation_config={"min_sol_balance": 0.5, "min_trades": 2}
        )
        assert result == [W1]

    @pytest.mark.asyncio
    async def test_no_balance_survivors(self, client, monkeypatch):
        monkeypatch.setattr(client, "_filter_by_sol_balance", AsyncMock(return_value=[]))
        result = await client._aggressive_wallet_filter(
            [W1], validation_config={"min_sol_balance": 0.5, "min_trades": 2}
        )
        assert result == []

    @pytest.mark.asyncio
    async def test_env_config(self, client, monkeypatch):
        monkeypatch.setenv("SCOUT_VALIDATE_WALLET_ACTIVITY", "true")
        monkeypatch.setattr(client, "_filter_by_sol_balance", AsyncMock(return_value=[W1]))
        monkeypatch.setattr(client, "_batch_validate_activity", AsyncMock(return_value=[W1]))
        assert await client._aggressive_wallet_filter([W1]) == [W1]


class TestFilterBySolBalance:
    @pytest.mark.asyncio
    async def test_empty(self, client):
        assert await client._filter_by_sol_balance([]) == []

    @pytest.mark.asyncio
    async def test_no_rpc_url(self, client, monkeypatch):
        monkeypatch.delenv("CHIMERA_RPC__PRIMARY_URL", raising=False)
        monkeypatch.delenv("SOLANA_RPC_URL", raising=False)
        assert await client._filter_by_sol_balance([W1]) == [W1]

    @pytest.mark.asyncio
    async def test_list_results(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(200, [
            {"id": 0, "result": {"value": 5_000_000_000}},
            {"id": 1, "result": {"value": 1}},
            {"id": 5, "result": {"value": 2_000_000_000}},
        ])])
        result = await client._filter_by_sol_balance([W1, W2, W3], min_balance_sol=0.1)
        assert result == [W1]

    @pytest.mark.asyncio
    async def test_dict_result(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(200, {"result": {"value": 3_000_000_000}})])
        assert await client._filter_by_sol_balance([W1], min_balance_sol=0.1) == [W1]

    @pytest.mark.asyncio
    async def test_http_error_fail_open(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(500)])
        result = await client._filter_by_sol_balance([W1, W2], min_balance_sol=0.1)
        assert result == [W1, W2]

    @pytest.mark.asyncio
    async def test_http_error_fail_closed(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        monkeypatch.setenv("SCOUT_BALANCE_FAIL_MODE", "closed")
        await _attach(client, [_resp(500)])
        assert await client._filter_by_sol_balance([W1], min_balance_sol=0.1) == []

    @pytest.mark.asyncio
    async def test_exception_fail_open(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        client._session = _FakeSession([_RaiseResp(RuntimeError("boom"))])
        client._session._loop = asyncio.get_running_loop()
        assert await client._filter_by_sol_balance([W1], min_balance_sol=0.1) == [W1]

    @pytest.mark.asyncio
    async def test_exception_fail_closed(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        monkeypatch.setenv("SCOUT_BALANCE_FAIL_MODE", "closed")
        client._session = _FakeSession([_RaiseResp(RuntimeError("boom"))])
        client._session._loop = asyncio.get_running_loop()
        assert await client._filter_by_sol_balance([W1], min_balance_sol=0.1) == []

    @pytest.mark.asyncio
    async def test_stats_recorded(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(200, [{"id": 0, "result": {"value": 0}}])])
        await client._filter_by_sol_balance([W1], min_balance_sol=0.1)
        stats = client.get_discovery_stats()
        assert stats["balance_checked"] == 1
        assert stats["balance_filtered"] == 1


class TestWalletCreationTimestampsBatch:
    @pytest.mark.asyncio
    async def test_results(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "get_wallet_first_transaction",
            AsyncMock(side_effect=[123.0, None, RuntimeError("boom")]),
        )
        result = await client._get_wallet_creation_timestamps_batch([W1, W2, W3])
        assert result == {W1: 123.0, W2: None, W3: None}


class TestFilterByWalletAge:
    @pytest.mark.asyncio
    async def test_disabled(self, client):
        assert await client._filter_by_wallet_age([W1], min_age_days=0) == [W1]

    @pytest.mark.asyncio
    async def test_empty(self, client):
        assert await client._filter_by_wallet_age([], min_age_days=7) == []

    @pytest.mark.asyncio
    async def test_filters_young(self, client, monkeypatch):
        now = time.time()
        monkeypatch.setattr(
            client, "_get_wallet_creation_timestamps_batch",
            AsyncMock(return_value={W1: now - 30 * 86400, W2: None, W3: now - 86400}),
        )
        result = await client._filter_by_wallet_age([W1, W2, W3], min_age_days=7)
        assert result == [W1, W2]


class TestGetWalletSolBalances:
    @pytest.mark.asyncio
    async def test_empty(self, client):
        assert await client.get_wallet_sol_balances([]) == {}

    @pytest.mark.asyncio
    async def test_no_rpc_url(self, client, monkeypatch):
        monkeypatch.delenv("CHIMERA_RPC__PRIMARY_URL", raising=False)
        monkeypatch.delenv("SOLANA_RPC_URL", raising=False)
        assert await client.get_wallet_sol_balances([W1]) == {W1: 0.0}

    @pytest.mark.asyncio
    async def test_list_results(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(200, [
            {"id": 0, "result": {"value": 5_000_000_000}},
            {"id": 1, "result": {"value": 1}},
        ])])
        result = await client.get_wallet_sol_balances([W1, W2])
        assert result[W1] == 5.0
        assert result[W2] == 1e-9

    @pytest.mark.asyncio
    async def test_dict_result(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(200, {"result": {"value": 3_000_000_000}})])
        assert await client.get_wallet_sol_balances([W1]) == {W1: 3.0}

    @pytest.mark.asyncio
    async def test_exception_defaults(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        client._session = _FakeSession([_RaiseResp(RuntimeError("boom"))])
        client._session._loop = asyncio.get_running_loop()
        assert await client.get_wallet_sol_balances([W1, W2]) == {W1: 0.0, W2: 0.0}

    @pytest.mark.asyncio
    async def test_missing_defaults(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(200, [{"id": 0, "result": {"value": 1000}}])])
        result = await client.get_wallet_sol_balances([W1, W2])
        assert result == {W1: 1e-6, W2: 0.0}


class TestStatsAccessors:
    def test_get_discovery_stats(self, client):
        client._discovery_stats["balance_checked"] = 5
        assert client.get_discovery_stats() == {
            "infrastructure_filtered": 0, "balance_checked": 5, "balance_filtered": 0
        }

    def test_get_cache_stats_empty(self, client):
        client._activity_cache = None
        assert client.get_cache_stats() == {}

    def test_get_cache_stats(self, client):
        mock = MagicMock()
        mock.get_cache_stats.return_value = {"hits": 10}
        client._activity_cache = mock
        assert client.get_cache_stats() == {"hits": 10}


class TestQueryTokenTransactions:
    @pytest.mark.asyncio
    async def test_list_data(self, client, monkeypatch):
        monkeypatch.setattr(client, "_make_request", AsyncMock(return_value=[
            {"timestamp": 1000, "type": "SWAP"},
            {"timestamp": 500, "type": "SWAP"},
            {"timestamp": None, "type": "SWAP"},
        ]))
        token, txs = await client._query_token_transactions(TOKEN_A, cutoff_time=900, limit_per_token=2)
        assert token == TOKEN_A
        assert [t["timestamp"] for t in txs] == [1000, None]

    @pytest.mark.asyncio
    async def test_dict_data(self, client, monkeypatch):
        monkeypatch.setattr(client, "_make_request", AsyncMock(return_value={
            "transactions": [{"timestamp": 1}]
        }))
        token, txs = await client._query_token_transactions(TOKEN_A, cutoff_time=0, limit_per_token=0)
        assert len(txs) == 1

    @pytest.mark.asyncio
    async def test_no_data(self, client, monkeypatch):
        monkeypatch.setattr(client, "_make_request", AsyncMock(return_value=None))
        token, txs = await client._query_token_transactions(TOKEN_A, 0, 0)
        assert (token, txs) == (TOKEN_A, [])

    @pytest.mark.asyncio
    async def test_exception(self, client, monkeypatch):
        monkeypatch.setattr(client, "_make_request", AsyncMock(side_effect=RuntimeError("boom")))
        token, txs = await client._query_token_transactions(TOKEN_A, 0, 0)
        assert (token, txs) == (TOKEN_A, [])


class TestDiscoverFromActiveTokens:
    def _tx(self, fee_payer=None, with_transfers=False):
        tx = {"feePayer": fee_payer}
        if with_transfers:
            tx["tokenTransfers"] = [{"fromUserAccount": W2, "toUserAccount": W3}]
        return tx

    @pytest.mark.asyncio
    async def test_parallel_fee_payer(self, client, monkeypatch):
        monkeypatch.setenv("SCOUT_ACTIVE_TOKENS", "%s,%s" % (TOKEN_A, TOKEN_B))
        monkeypatch.setattr(
            client, "_query_token_transactions",
            AsyncMock(side_effect=[
                (TOKEN_A, [self._tx(W1)]),
                (TOKEN_B, [self._tx(W2)]),
            ]),
        )
        result = await client._discover_from_active_tokens(
            hours_back=24, limit_per_token=10, use_parallel=True, max_wallets=100
        )
        assert result == {W1: 1, W2: 1}

    @pytest.mark.asyncio
    async def test_parallel_extraction_fallback(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_query_token_transactions",
            AsyncMock(return_value=(TOKEN_A, [self._tx(None, with_transfers=True)])),
        )
        result = await client._discover_from_active_tokens(
            token_addresses=[TOKEN_A, TOKEN_B],
            hours_back=24, limit_per_token=10, use_parallel=True, max_wallets=100
        )
        assert W2 in result and W3 in result

    @pytest.mark.asyncio
    async def test_parallel_early_termination(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_query_token_transactions",
            AsyncMock(return_value=(TOKEN_A, [self._tx(W1), self._tx(W1)])),
        )
        result = await client._discover_from_active_tokens(
            token_addresses=[TOKEN_A, TOKEN_B],
            hours_back=24, limit_per_token=10, use_parallel=True, max_wallets=1
        )
        assert result == {W1: 2}

    @pytest.mark.asyncio
    async def test_parallel_coro_exception(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_query_token_transactions",
            AsyncMock(side_effect=[
                RuntimeError("boom"),
                (TOKEN_B, [self._tx(W2)]),
            ]),
        )
        result = await client._discover_from_active_tokens(
            token_addresses=[TOKEN_A, TOKEN_B],
            hours_back=24, limit_per_token=10, use_parallel=True, max_wallets=100
        )
        assert result == {W2: 1}

    @pytest.mark.asyncio
    async def test_parallel_api_limit_break(self, client, monkeypatch):
        def side_effect(token, cutoff, limit):
            client._api_calls_made = client._max_api_calls
            return token, [self._tx(W1)]

        monkeypatch.setattr(client, "_query_token_transactions", AsyncMock(side_effect=side_effect))
        result = await client._discover_from_active_tokens(
            token_addresses=[TOKEN_A, TOKEN_B],
            hours_back=24, limit_per_token=10, use_parallel=True, max_wallets=100
        )
        assert W1 in result

    @pytest.mark.asyncio
    async def test_parallel_no_tasks(self, client, monkeypatch):
        client._api_calls_made = client._max_api_calls
        result = await client._discover_from_active_tokens(
            token_addresses=[TOKEN_A, TOKEN_B],
            hours_back=24, limit_per_token=10, use_parallel=True, max_wallets=100
        )
        assert result == {}

    @pytest.mark.asyncio
    async def test_sequential(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_query_token_transactions",
            AsyncMock(side_effect=[
                (TOKEN_A, [self._tx(W1)]),
                (TOKEN_B, [self._tx(None, with_transfers=True)]),
            ]),
        )
        result = await client._discover_from_active_tokens(
            token_addresses=[TOKEN_A, TOKEN_B],
            hours_back=24, limit_per_token=10, use_parallel=False, max_wallets=100
        )
        assert W1 in result and W2 in result

    @pytest.mark.asyncio
    async def test_sequential_early_termination(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_query_token_transactions",
            AsyncMock(return_value=(TOKEN_A, [self._tx(W1)])),
        )
        result = await client._discover_from_active_tokens(
            token_addresses=[TOKEN_A, TOKEN_B],
            hours_back=24, limit_per_token=10, use_parallel=False, max_wallets=1
        )
        assert result == {W1: 1}

    @pytest.mark.asyncio
    async def test_sequential_api_limit(self, client, monkeypatch):
        client._api_calls_made = client._max_api_calls
        result = await client._discover_from_active_tokens(
            token_addresses=[TOKEN_A, TOKEN_B],
            hours_back=24, limit_per_token=10, use_parallel=False, max_wallets=100
        )
        assert result == {}

    @pytest.mark.asyncio
    async def test_default_tokens(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_query_token_transactions", AsyncMock(return_value=(TOKEN_A, [self._tx(W1)]))
        )
        result = await client._discover_from_active_tokens(
            token_addresses=None, hours_back=24, limit_per_token=10,
            use_parallel=False, max_wallets=100
        )
        assert W1 in result


class TestDiscoverFromRecentBlocks:
    @pytest.mark.asyncio
    async def test_no_rpc_no_key(self, client, monkeypatch):
        client.api_key = None
        monkeypatch.delenv("CHIMERA_RPC__PRIMARY_URL", raising=False)
        monkeypatch.delenv("SOLANA_RPC_URL", raising=False)
        assert await client._discover_from_recent_blocks() == {}

    @pytest.mark.asyncio
    async def test_rpc_url_from_api_key(self, client, monkeypatch):
        monkeypatch.delenv("CHIMERA_RPC__PRIMARY_URL", raising=False)
        monkeypatch.delenv("SOLANA_RPC_URL", raising=False)
        await _attach(client, [_resp(200, {"result": None})])
        assert await client._discover_from_recent_blocks(hours_back=1, limit=1) == {}

    @pytest.mark.asyncio
    async def test_slot_http_error(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(500)])
        assert await client._discover_from_recent_blocks(hours_back=1, limit=1) == {}

    @pytest.mark.asyncio
    async def test_invalid_slot(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(200, {"result": None})])
        assert await client._discover_from_recent_blocks(hours_back=1, limit=1) == {}

    @pytest.mark.asyncio
    async def test_success_and_skips(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")

        def block(txs):
            return _resp(200, {"result": {"transactions": txs}})

        failed_tx = {"transaction": {"meta": {"err": {"InstructionError": [0]}}}}
        empty_tx = {}
        transfer_tx = {"transaction": {"meta": {"err": None}, "message": {
            "instructions": [{"parsed": {"type": "transfer"}}],
            "accountKeys": [W1, W2, W3],
        }}}
        dex_tx = {"transaction": {"meta": {"err": None}, "message": {
            "instructions": [{"programId": client.dex_programs[0]}],
            "accountKeys": [W1, "short", W3],
        }}}
        await _attach(client, [
            _resp(200, {"result": 1000}),
            block([failed_tx]),
            _resp(500),
            _resp(200, {"result": None}),
            block([transfer_tx]),
            block([dex_tx]),
            block([empty_tx]),
        ])
        result = await client._discover_from_recent_blocks(hours_back=1, limit=1000)
        assert W1 in result
        assert W3 in result
        assert "short" not in result

    @pytest.mark.asyncio
    async def test_rpc_url_fallback(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [_resp(200, {"result": 1000})])
        assert await client._discover_from_recent_blocks(hours_back=1, limit=1) == {}

    @pytest.mark.asyncio
    async def test_slot_limit_break(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")

        def block(txs):
            return _resp(200, {"result": {"transactions": txs}})

        def tx(k):
            return {"transaction": {"meta": {"err": None}, "message": {
                "instructions": [{"programId": client.dex_programs[0]}],
                "accountKeys": [k],
            }}}

        # two txs in one block with limit=1 -> inner tx loop breaks after the first
        await _attach(client, [
            _resp(200, {"result": 1000}),
            block([tx(W1), tx(W2)]),
        ])
        result = await client._discover_from_recent_blocks(hours_back=1, limit=1)
        assert result == {W1: 1}

    @pytest.mark.asyncio
    async def test_inner_exception(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        await _attach(client, [
            _resp(200, {"result": 1000}),
            _RaiseResp(RuntimeError("boom")),
        ])
        assert await client._discover_from_recent_blocks(hours_back=1, limit=1) == {}

    @pytest.mark.asyncio
    async def test_outer_exception(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://rpc.test/")
        client._session = _FakeSession([_RaiseResp(RuntimeError("boom"))])
        client._session._loop = asyncio.get_running_loop()
        assert await client._discover_from_recent_blocks() == {}


class TestDiscoverFromDexPrograms:
    @pytest.mark.asyncio
    async def test_no_helius_rpc(self, client, monkeypatch):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "http://plain-rpc.test/")
        assert await client._discover_from_dex_programs() == {}

    @pytest.mark.asyncio
    async def test_success(self, client, monkeypatch):
        monkeypatch.setenv(
            "CHIMERA_RPC__PRIMARY_URL",
            "https://mainnet.helius-rpc.com/?api-key=abc",
        )
        tx = {"tokenTransfers": [{"fromUserAccount": W1, "toUserAccount": W2}]}
        await _attach(client, [_resp(200, {"result": {"data": [tx]}})])
        result = await client._discover_from_dex_programs(limit=50)
        assert W1 in result and W2 in result

    @pytest.mark.asyncio
    async def test_error_envelope(self, client, monkeypatch):
        monkeypatch.setenv(
            "CHIMERA_RPC__PRIMARY_URL",
            "https://mainnet.helius-rpc.com/?api-key=abc",
        )
        await _attach(client, [_resp(200, {"result": {"error": "bad"}})])
        assert await client._discover_from_dex_programs(limit=50) == {}

    @pytest.mark.asyncio
    async def test_429_retry(self, client, monkeypatch):
        monkeypatch.setenv(
            "CHIMERA_RPC__PRIMARY_URL",
            "https://mainnet.helius-rpc.com/?api-key=abc",
        )
        tx = {"tokenTransfers": [{"fromUserAccount": W1, "toUserAccount": W2}]}
        await _attach(client, [
            _resp(429, headers={"Retry-After": "1"}),
            _resp(200, {"result": {"data": [tx]}}),
        ])
        result = await client._discover_from_dex_programs(limit=50)
        assert W1 in result

    @pytest.mark.asyncio
    async def test_exception_continues(self, client, monkeypatch):
        monkeypatch.setenv(
            "CHIMERA_RPC__PRIMARY_URL",
            "https://mainnet.helius-rpc.com/?api-key=abc",
        )
        client._session = _FakeSession([_RaiseResp(RuntimeError("boom"))])
        client._session._loop = asyncio.get_running_loop()
        assert await client._discover_from_dex_programs(limit=50) == {}

    @pytest.mark.asyncio
    async def test_api_limit(self, client, monkeypatch):
        monkeypatch.setenv(
            "CHIMERA_RPC__PRIMARY_URL",
            "https://mainnet.helius-rpc.com/?api-key=abc",
        )
        client._api_calls_made = client._max_api_calls
        assert await client._discover_from_dex_programs(limit=50) == {}


class TestDiscoverFromSeedWallets:
    @pytest.mark.asyncio
    async def test_no_seed_wallets(self, client, monkeypatch, tmp_path):
        monkeypatch.delenv("SCOUT_SEED_WALLETS", raising=False)
        monkeypatch.setattr(helius_mod, "Path", lambda *a, **k: tmp_path)
        assert await client._discover_from_seed_wallets() == {}

    @pytest.mark.asyncio
    async def test_success(self, client, monkeypatch):
        monkeypatch.setenv("SCOUT_SEED_WALLETS", W1)
        tx = {"tokenTransfers": [
            {"fromUserAccount": W1, "toUserAccount": W3},
            {"fromUserAccount": W3, "toUserAccount": W1},
        ]}
        monkeypatch.setattr(
            client, "get_wallet_transactions", AsyncMock(return_value=[tx])
        )
        result = await client._discover_from_seed_wallets()
        assert result == {W3: 1}

    @pytest.mark.asyncio
    async def test_exception_continues(self, client, monkeypatch):
        monkeypatch.setenv("SCOUT_SEED_WALLETS", "%s,%s" % (W1, W2))
        monkeypatch.setattr(
            client, "get_wallet_transactions", AsyncMock(side_effect=RuntimeError("boom"))
        )
        assert await client._discover_from_seed_wallets() == {}

    @pytest.mark.asyncio
    async def test_api_limit(self, client, monkeypatch):
        monkeypatch.setenv("SCOUT_SEED_WALLETS", "%s,%s" % (W1, W2))
        client._api_calls_made = client._max_api_calls
        assert await client._discover_from_seed_wallets() == {}


class TestDiscoverFromTopPerformingTokens:
    @pytest.mark.asyncio
    async def test_cached(self, client):
        client._cached_active_token_wallets = {W1: 5, W2: 3, W3: 1}
        result = await client.discover_from_top_performing_tokens()
        assert result == [W1, W2, W3]

    @pytest.mark.asyncio
    async def test_discover_and_cache(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_discover_from_active_tokens",
            AsyncMock(return_value={W1: 5, W2: 3, W3: 1}),
        )
        result = await client.discover_from_top_performing_tokens()
        assert result == [W1, W2, W3]
        assert client._cached_active_token_wallets == {W1: 5, W2: 3, W3: 1}

    @pytest.mark.asyncio
    async def test_exception(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_discover_from_active_tokens",
            AsyncMock(side_effect=RuntimeError("boom")),
        )
        assert await client.discover_from_top_performing_tokens() == []


class TestDiscoverWallets:
    @pytest.mark.asyncio
    async def test_wrapper(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "discover_wallets_from_recent_swaps", AsyncMock(return_value=[W1, W2])
        )
        result = await client.discover_wallets(hours_back=48, max_wallets=5)
        assert result == {W1: 2, W2: 1}


class TestDiscoverWalletsFromRecentSwaps:
    @pytest.mark.asyncio
    async def test_no_api_key(self, client, monkeypatch):
        client.api_key = None
        assert await client.discover_wallets_from_recent_swaps() == []

    @pytest.mark.asyncio
    async def test_no_api_key_strict(self, client, monkeypatch):
        client.api_key = None
        with pytest.raises(helius_mod.DiscoveryError):
            await client.discover_wallets_from_recent_swaps(strict=True)

    @pytest.mark.asyncio
    async def test_credit_cap(self, client, monkeypatch):
        monkeypatch.setattr(
            helius_mod, "get_credit_tracker", lambda: _FakeTracker(credits=0)
        )
        assert await client.discover_wallets_from_recent_swaps() == []

    @pytest.mark.asyncio
    async def test_discovery_cache_hit(self, client):
        client._discovery_cache = {"wallets": [W1, W2]}
        client._discovery_cache_time = time.time()
        assert await client.discover_wallets_from_recent_swaps() == [W1, W2]

    @pytest.mark.asyncio
    async def test_full_pipeline(self, client, monkeypatch):
        counts = {W1: 5, W2: 4, W3: 3, "11111111111111111111111111111111": 10}
        monkeypatch.setattr(
            client, "_discover_from_active_tokens", AsyncMock(return_value=counts)
        )
        monkeypatch.setattr(client, "_discover_from_recent_blocks", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_dex_programs", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "discover_from_top_performing_tokens", AsyncMock(return_value=[]))
        monkeypatch.setattr(client, "_filter_by_sol_balance", AsyncMock(return_value=[W1]))
        result = await client.discover_wallets_from_recent_swaps(
            max_wallets=10, hours_back=24, min_trade_count=2
        )
        assert result == [W1]

    @pytest.mark.asyncio
    async def test_fallback_strategies(self, client, monkeypatch):
        monkeypatch.setattr(client, "_discover_from_active_tokens", AsyncMock(return_value={}))
        monkeypatch.setattr(
            client, "_discover_from_recent_blocks", AsyncMock(return_value={W1: 1})
        )
        monkeypatch.setattr(
            client, "_discover_from_dex_programs", AsyncMock(return_value={W2: 2})
        )
        monkeypatch.setattr(
            client, "_discover_from_seed_wallets", AsyncMock(return_value={W3: 1})
        )
        monkeypatch.setattr(client, "discover_from_top_performing_tokens", AsyncMock(return_value=[]))
        result = await client.discover_wallets_from_recent_swaps(
            max_wallets=10, min_trade_count=1
        )
        assert set(result) == {W1, W2, W3}

    @pytest.mark.asyncio
    async def test_strategy_failures_and_timeouts(self, client, monkeypatch):
        monkeypatch.setattr(client, "_discover_from_active_tokens", AsyncMock(return_value={}))
        monkeypatch.setattr(
            client, "_discover_from_recent_blocks",
            AsyncMock(side_effect=asyncio.TimeoutError("slow")),
        )
        monkeypatch.setattr(
            client, "_discover_from_dex_programs",
            AsyncMock(side_effect=RuntimeError("boom")),
        )
        monkeypatch.setattr(client, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        result = await client.discover_wallets_from_recent_swaps()
        assert result == []

    @pytest.mark.asyncio
    async def test_strategy1_raises(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_discover_from_active_tokens", AsyncMock(side_effect=RuntimeError("boom"))
        )
        monkeypatch.setattr(client, "_discover_from_recent_blocks", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_dex_programs", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        assert await client.discover_wallets_from_recent_swaps() == []

    @pytest.mark.asyncio
    async def test_trending_strategy5(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_discover_from_active_tokens", AsyncMock(return_value={W1: 1})
        )
        monkeypatch.setattr(client, "_discover_from_recent_blocks", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_dex_programs", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        monkeypatch.setattr(
            client, "discover_from_top_performing_tokens", AsyncMock(return_value=[W2])
        )
        result = await client.discover_wallets_from_recent_swaps(
            max_wallets=10, min_trade_count=1
        )
        assert W2 in result

    @pytest.mark.asyncio
    async def test_balance_validation_skipped(self, client, monkeypatch):
        monkeypatch.setenv("SCOUT_VALIDATE_WALLET_BALANCE", "false")
        monkeypatch.setattr(
            client, "_discover_from_active_tokens", AsyncMock(return_value={W1: 2})
        )
        monkeypatch.setattr(client, "_discover_from_recent_blocks", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_dex_programs", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        result = await client.discover_wallets_from_recent_swaps(min_trade_count=2)
        assert result == [W1]

    @pytest.mark.asyncio
    async def test_balance_validation_error(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_discover_from_active_tokens", AsyncMock(return_value={W1: 2})
        )
        monkeypatch.setattr(client, "_discover_from_recent_blocks", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_dex_programs", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        monkeypatch.setattr(
            client, "_filter_by_sol_balance", AsyncMock(side_effect=RuntimeError("boom"))
        )
        result = await client.discover_wallets_from_recent_swaps(min_trade_count=2)
        assert result == [W1]

    @pytest.mark.asyncio
    async def test_wallet_age_filter(self, client, monkeypatch):
        monkeypatch.setenv("SCOUT_MIN_WALLET_AGE_DAYS", "7")
        monkeypatch.setattr(
            client, "_discover_from_active_tokens", AsyncMock(return_value={W1: 2, W2: 2})
        )
        monkeypatch.setattr(client, "_discover_from_recent_blocks", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_dex_programs", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_filter_by_sol_balance", AsyncMock(return_value=[W1, W2]))
        monkeypatch.setattr(client, "_filter_by_wallet_age", AsyncMock(return_value=[W1]))
        result = await client.discover_wallets_from_recent_swaps(min_trade_count=2)
        assert result == [W1]

    @pytest.mark.asyncio
    async def test_wallet_age_filter_error(self, client, monkeypatch):
        monkeypatch.setenv("SCOUT_MIN_WALLET_AGE_DAYS", "7")
        monkeypatch.setattr(
            client, "_discover_from_active_tokens", AsyncMock(return_value={W1: 2})
        )
        monkeypatch.setattr(client, "_discover_from_recent_blocks", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_dex_programs", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_filter_by_sol_balance", AsyncMock(return_value=[W1]))
        monkeypatch.setattr(
            client, "_filter_by_wallet_age", AsyncMock(side_effect=RuntimeError("boom"))
        )
        result = await client.discover_wallets_from_recent_swaps(min_trade_count=2)
        assert result == [W1]

    @pytest.mark.asyncio
    async def test_activity_validation(self, client, monkeypatch):
        monkeypatch.setenv("SCOUT_VALIDATE_WALLET_ACTIVITY", "true")
        monkeypatch.setattr(
            client, "_discover_from_active_tokens", AsyncMock(return_value={W1: 2, W2: 2})
        )
        monkeypatch.setattr(client, "_discover_from_recent_blocks", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_dex_programs", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_filter_by_sol_balance", AsyncMock(return_value=[W1, W2]))
        monkeypatch.setattr(client, "_batch_validate_activity", AsyncMock(return_value=[W1]))
        result = await client.discover_wallets_from_recent_swaps(min_trade_count=2)
        assert result == [W1]

    @pytest.mark.asyncio
    async def test_dedup(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_discover_from_active_tokens", AsyncMock(return_value={W1: 3, W2: 2})
        )
        monkeypatch.setattr(client, "_discover_from_recent_blocks", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_dex_programs", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_filter_by_sol_balance", AsyncMock(return_value=[W1, W2]))
        monkeypatch.setattr(client, "_get_persistent_seen_wallets", lambda: {W1})
        result = await client.discover_wallets_from_recent_swaps(min_trade_count=1)
        assert result == [W2]

    @pytest.mark.asyncio
    async def test_adaptive_stats(self, client, monkeypatch):
        client._adaptive_enabled = True
        client._latency_samples = [10.0]
        client._circuit_breaker_failures = client._circuit_breaker_threshold
        monkeypatch.setattr(
            client, "_discover_from_active_tokens", AsyncMock(return_value={W1: 2})
        )
        monkeypatch.setattr(client, "_discover_from_recent_blocks", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_dex_programs", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_filter_by_sol_balance", AsyncMock(return_value=[W1]))
        result = await client.discover_wallets_from_recent_swaps(min_trade_count=2)
        assert result == [W1]

    @pytest.mark.asyncio
    async def test_trending_strategy_raises(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_discover_from_active_tokens", AsyncMock(return_value={W1: 1})
        )
        monkeypatch.setattr(client, "_discover_from_recent_blocks", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_dex_programs", AsyncMock(return_value={}))
        monkeypatch.setattr(client, "_discover_from_seed_wallets", AsyncMock(return_value={}))
        monkeypatch.setattr(
            client, "discover_from_top_performing_tokens",
            AsyncMock(side_effect=RuntimeError("boom")),
        )
        result = await client.discover_wallets_from_recent_swaps(
            max_wallets=10, min_trade_count=1
        )
        assert result == [W1]


class TestGetWalletTransactions:
    @pytest.mark.asyncio
    async def test_no_api_key(self, client):
        client.api_key = None
        assert await client.get_wallet_transactions(W1) == []

    @pytest.mark.asyncio
    async def test_invalid_address(self, client):
        assert await client.get_wallet_transactions("short") == []

    @pytest.mark.asyncio
    async def test_credit_cap(self, client, monkeypatch):
        monkeypatch.setattr(
            helius_mod, "get_credit_tracker", lambda: _FakeTracker(credits=0)
        )
        assert await client.get_wallet_transactions(W1) == []

    @pytest.mark.asyncio
    async def test_activity_cache_hit(self, client):
        mock = MagicMock()
        mock.get_cached_transactions.return_value = [{"type": "SWAP"}]
        client._activity_cache = mock
        result = await client.get_wallet_transactions(W1)
        assert result == [{"type": "SWAP"}]

    @pytest.mark.asyncio
    async def test_basic_cache_hit(self, client, monkeypatch):
        client._activity_cache = None
        cache = _FakeCache()
        cache.store[("wallet_txs", W1, (f"{W1}:7:100",))] = [{"type": "SWAP"}]
        monkeypatch.setattr(helius_mod, "get_cache", lambda: cache)
        result = await client.get_wallet_transactions(W1, days=7, limit=100)
        assert result == [{"type": "SWAP"}]

    @pytest.mark.asyncio
    async def test_pagination_swap(self, client, monkeypatch):
        client._activity_cache = None
        now = int(time.time())
        pages = [
            [{"timestamp": now, "type": "SWAP", "signature": "s%d" % i} for i in range(100)],
            [{"timestamp": now, "type": "SWAP", "signature": "t%d" % i} for i in range(100)],
            [{"timestamp": now, "type": "SWAP", "signature": "u%d" % i} for i in range(100)],
        ]
        calls = []

        async def fake_make_request(endpoint, params, use_retry=True):
            calls.append(dict(params))
            return pages[min(len(calls) - 1, 2)]

        monkeypatch.setattr(client, "_make_request", fake_make_request)
        # limit=150 canonicalizes to 200 (nearest 100 bucket)
        result = await client.get_wallet_transactions(W1, days=30, limit=150)
        assert len(result) == 200
        assert calls[0]["type"] == "SWAP"

    @pytest.mark.asyncio
    async def test_pagination_reached_cutoff(self, client, monkeypatch):
        client._activity_cache = None
        old = int(time.time()) - 40 * 86400
        page1 = [{"timestamp": old, "type": "SWAP", "signature": "s%d" % i} for i in range(100)]

        async def fake_make_request(endpoint, params, use_retry=True):
            return page1

        monkeypatch.setattr(client, "_make_request", fake_make_request)
        result = await client.get_wallet_transactions(W1, days=30, limit=100)
        assert result == []

    @pytest.mark.asyncio
    async def test_swap_fallback_unfiltered(self, client, monkeypatch):
        client._activity_cache = None
        now = int(time.time())
        txs = [
            {"timestamp": now, "type": "SWAP", "signature": "s1"},
            {"timestamp": now, "type": "TRANSFER", "signature": "s2"},
        ]
        calls = {"n": 0}

        async def fake_make_request(endpoint, params, use_retry=True):
            if params.get("type") == "SWAP":
                return []
            calls["n"] += 1
            return txs if calls["n"] == 1 else []

        monkeypatch.setattr(client, "_make_request", fake_make_request)
        result = await client.get_wallet_transactions(W1, days=7, limit=100)
        assert [t["signature"] for t in result] == ["s1"]

    @pytest.mark.asyncio
    async def test_dict_batch_and_no_signature(self, client, monkeypatch):
        client._activity_cache = None
        now = int(time.time())
        page = {"transactions": [{"timestamp": now, "type": "SWAP"}]}

        async def fake_make_request(endpoint, params, use_retry=True):
            return page

        monkeypatch.setattr(client, "_make_request", fake_make_request)
        result = await client.get_wallet_transactions(W1, days=7, limit=100)
        assert len(result) == 1

    @pytest.mark.asyncio
    async def test_no_data_breaks(self, client, monkeypatch):
        client._activity_cache = None
        monkeypatch.setattr(client, "_make_request", AsyncMock(return_value=None))
        assert await client.get_wallet_transactions(W1, days=7, limit=100) == []

    @pytest.mark.asyncio
    async def test_empty_batch_breaks(self, client, monkeypatch):
        client._activity_cache = None
        monkeypatch.setattr(client, "_make_request", AsyncMock(return_value=[]))
        assert await client.get_wallet_transactions(W1, days=7, limit=100) == []

    @pytest.mark.asyncio
    async def test_max_pages(self, client, monkeypatch):
        client._activity_cache = None
        monkeypatch.setenv("SCOUT_WALLET_TX_MAX_PAGES", "1")
        now = int(time.time())
        page = [{"timestamp": now, "type": "SWAP", "signature": "s%d" % i} for i in range(100)]

        async def fake_make_request(endpoint, params, use_retry=True):
            return page

        monkeypatch.setattr(client, "_make_request", fake_make_request)
        result = await client.get_wallet_transactions(W1, days=7, limit=100)
        assert len(result) == 100

    @pytest.mark.asyncio
    async def test_max_pages_break_before_target(self, client, monkeypatch):
        client._activity_cache = None
        monkeypatch.setenv("SCOUT_WALLET_TX_MAX_PAGES", "1")
        now = int(time.time())
        page = [{"timestamp": now, "type": "SWAP", "signature": "s%d" % i} for i in range(50)]

        async def fake_make_request(endpoint, params, use_retry=True):
            return page

        monkeypatch.setattr(client, "_make_request", fake_make_request)
        result = await client.get_wallet_transactions(W1, days=7, limit=100)
        assert len(result) == 50

    @pytest.mark.asyncio
    async def test_empty_dict_batch_breaks(self, client, monkeypatch):
        client._activity_cache = None
        monkeypatch.setattr(client, "_make_request", AsyncMock(return_value={"transactions": []}))
        assert await client.get_wallet_transactions(W1, days=7, limit=100) == []

    @pytest.mark.asyncio
    async def test_tx_without_timestamp_kept(self, client, monkeypatch):
        client._activity_cache = None
        now = int(time.time())
        page = [
            {"timestamp": now, "type": "SWAP", "signature": "s1"},
            {"type": "SWAP", "signature": "s2"},
        ]
        calls = {"n": 0}

        async def fake_make_request(endpoint, params, use_retry=True):
            calls["n"] += 1
            return page if calls["n"] == 1 else []

        monkeypatch.setattr(client, "_make_request", fake_make_request)
        result = await client.get_wallet_transactions(W1, days=7, limit=100)
        assert [t["signature"] for t in result] == ["s1", "s2"]

    @pytest.mark.asyncio
    async def test_caches_result(self, client, monkeypatch):
        mock = MagicMock()
        mock.get_cached_transactions.return_value = None
        client._activity_cache = mock
        now = int(time.time())
        txs = [{"timestamp": now, "type": "SWAP", "signature": "s1"}]
        calls = {"n": 0}

        async def fake_make_request(endpoint, params, use_retry=True):
            calls["n"] += 1
            return txs if calls["n"] == 1 else []

        monkeypatch.setattr(client, "_make_request", fake_make_request)
        result = await client.get_wallet_transactions(W1, days=7, limit=100)
        assert result == txs
        mock.cache_transactions.assert_called_once()


class TestParseDefiTransaction:
    def test_transfer_in(self, client):
        tx = {"type": "TRANSFER", "source": W2, "destination": W1,
              "amount": 100, "mint": TOKEN_A}
        result = client.parse_defi_transaction(tx, W1)
        assert result["direction"] == "IN"
        assert result["token"] == TOKEN_A

    def test_transfer_out(self, client):
        tx = {"type": "TRANSFER", "source": W1, "destination": W2,
              "amount": 100, "mint": TOKEN_A}
        result = client.parse_defi_transaction(tx, W1)
        assert result["direction"] == "OUT"

    def test_transfer_not_involved(self, client):
        tx = {"type": "TRANSFER", "source": W2, "destination": W3,
              "amount": 100, "mint": TOKEN_A}
        assert client.parse_defi_transaction(tx, W1) is None

    def test_transfer_missing_amount(self, client):
        tx = {"type": "TRANSFER", "source": W2, "destination": W1}
        assert client.parse_defi_transaction(tx, W1) is None

    def test_add_liquidity(self, client):
        tx = {
            "type": "ADD_LIQUIDITY",
            "signature": "sig1",
            "timestamp": 123,
            "tokenTransfers": [
                {"mint": TOKEN_A, "tokenAmount": 10, "toUserAccount": W1},
                {"mint": TOKEN_B, "tokenAmount": 5, "fromUserAccount": W1},
            ],
            "nativeTransfers": [
                {"amount": 1_000_000_000, "toUserAccount": W1},
                {"amount": 500_000_000, "fromUserAccount": W1},
            ],
        }
        result = client.parse_defi_transaction(tx, W1)
        assert result["type"] == "LIQUIDITY_EVENT"
        assert result["sol_delta"] == 0.5
        assert len(result["tokens_in"]) == 1

    def test_remove_liquidity(self, client):
        tx = {"type": "REMOVE_LIQUIDITY", "tokenTransfers": [], "nativeTransfers": []}
        assert client.parse_defi_transaction(tx, W1)["type"] == "LIQUIDITY_EVENT"

    def test_stake(self, client):
        tx = {"type": "STAKE_TOKEN", "tokenTransfers": [], "nativeTransfers": []}
        assert client.parse_defi_transaction(tx, W1)["type"] == "STAKE_EVENT"

    def test_unknown_type(self, client):
        assert client.parse_defi_transaction({"type": "OTHER"}, W1) is None

    def test_exception(self, client):
        tx = {"type": "ADD_LIQUIDITY", "tokenTransfers": [None], "nativeTransfers": []}
        assert client.parse_defi_transaction(tx, W1) is None


class TestIsWalletInvolved:
    def test_fee_payer(self, client):
        assert client._is_wallet_involved({"feePayer": W1}, W1) is True

    def test_signature(self, client):
        assert client._is_wallet_involved({"signatures": [W1, "abc"]}, W1) is True

    def test_token_transfer(self, client):
        tx = {"tokenTransfers": [{"fromUserAccount": W1}]}
        assert client._is_wallet_involved(tx, W1) is True

    def test_native_transfer(self, client):
        tx = {"nativeTransfers": [{"toUserAccount": W1}]}
        assert client._is_wallet_involved(tx, W1) is True

    def test_account_data_token_changes(self, client):
        tx = {"accountData": [{"account": W1, "tokenBalanceChanges": [{}]}]}
        assert client._is_wallet_involved(tx, W1) is True

    def test_account_data_native_change(self, client):
        tx = {"accountData": [{"account": W1, "nativeBalanceChange": 5}]}
        assert client._is_wallet_involved(tx, W1) is True

    def test_instructions(self, client):
        tx = {"instructions": [{"accounts": [W2, W1]}]}
        assert client._is_wallet_involved(tx, W1) is True

    def test_not_involved(self, client):
        assert client._is_wallet_involved({}, W1) is False


class TestParseSwapTransaction:
    def test_not_dict(self, client):
        assert client.parse_swap_transaction(None, W1) is None

    def test_no_wallet(self, client):
        assert client.parse_swap_transaction({"type": "SWAP"}, None) is None

    def test_not_involved(self, client):
        assert client.parse_swap_transaction({"type": "SWAP"}, W1) is None

    def test_delta_strategy(self, client):
        tx = {
            "signature": "sig1",
            "timestamp": 100,
            "nativeTransfers": [{"fromUserAccount": W1, "toUserAccount": W2, "amount": 100_000_000}],
            "tokenTransfers": [{"mint": TOKEN_A, "tokenAmount": 10, "toUserAccount": W1}],
            "feePayer": W1,
        }
        result = client.parse_swap_transaction(tx, W1)
        assert result is not None
        assert result["direction"] == "BUY"

    def test_events_strategy_fallback(self, client):
        tx = {
            "signature": "sig1",
            "timestamp": 100,
            "feePayer": W1,
            "events": {"swap": {"nativeInput": 50_000_000, "tokenOutputs": [
                {"mint": TOKEN_A, "tokenAmount": 5}
            ]}},
        }
        result = client.parse_swap_transaction(tx, W1)
        assert result["direction"] == "BUY"

    def test_account_data_strategy_fallback(self, client):
        tx = {
            "signature": "sig1",
            "timestamp": 100,
            "feePayer": W1,
            "accountData": [{"account": W1, "nativeBalanceChange": -100_000_000,
                             "tokenBalanceChanges": [
                                 {"userAccount": W1, "mint": TOKEN_A,
                                  "rawTokenAmount": {"tokenAmount": "5000000000", "decimals": 6}}
                             ]}],
        }
        result = client.parse_swap_transaction(tx, W1)
        assert result is not None

    def test_all_strategies_fail(self, client):
        tx = {"signature": "sig1", "feePayer": W1}
        assert client.parse_swap_transaction(tx, W1) is None

    def test_strategy_exceptions_logged_and_continue(self, client, monkeypatch):
        monkeypatch.setattr(
            client, "_parse_swap_from_deltas",
            MagicMock(side_effect=RuntimeError("deltas boom")),
        )
        monkeypatch.setattr(
            client, "_parse_swap_from_events",
            MagicMock(side_effect=RuntimeError("events boom")),
        )
        monkeypatch.setattr(
            client, "_parse_swap_from_account_data",
            MagicMock(side_effect=RuntimeError("account boom")),
        )
        tx = {"signature": "sig1", "feePayer": W1}
        assert client.parse_swap_transaction(tx, W1) is None


class TestParseSwapFromDeltas:
    """Covers _parse_swap_from_deltas strategy-1 parsing paths.

    NOTE: token_deltas is provably always zero (every transfer counterparty of
    a wallet-touching transfer is added to wallet_owned_accounts, so both sides
    of each transfer cancel). The stablecoin/multi-token/wSOL branches in the
    source are therefore unreachable and marked ``# pragma: no cover``.
    """

    def _native(self, frm, to, amt):
        return {"fromUserAccount": frm, "toUserAccount": to, "amount": amt}

    def _token(self, mint, amt, frm=None, to=None):
        tr = {"mint": mint, "tokenAmount": amt}
        if frm:
            tr["fromUserAccount"] = frm
        if to:
            tr["toUserAccount"] = to
        return tr

    def test_buy_sol_to_token(self, client):
        tx = {
            "signature": "s1",
            "nativeTransfers": [self._native(W1, W2, 100_000_000)],
            "tokenTransfers": [self._token(TOKEN_A, 10.0, to=W1)],
        }
        result = client._parse_swap_from_deltas(tx, W1)
        assert result["direction"] == "BUY"
        assert result["token_mint"] == TOKEN_A
        assert result["sol_amount"] == 0.1
        assert result["quote_mint"] == WSOL
        assert result["net_sol_delta"] == -0.1

    def test_sell_token_to_sol(self, client):
        tx = {
            "signature": "s1",
            "nativeTransfers": [self._native(W2, W1, 200_000_000)],
            "tokenTransfers": [self._token(TOKEN_A, 50.0, frm=W1)],
        }
        result = client._parse_swap_from_deltas(tx, W1)
        assert result["direction"] == "SELL"
        assert result["token_amount"] == 50.0
        assert result["sol_amount"] == 0.2
        assert result["net_sol_delta"] == 0.2

    def test_no_transfers_returns_none(self, client):
        assert client._parse_swap_from_deltas({"signature": "s1"}, W1) is None

    def test_no_primary_largest_transfer(self, client):
        tx = {
            "signature": "s1",
            "tokenTransfers": [
                self._token(TOKEN_A, 10.0, frm="X" * 44, to=W1),
                self._token(TOKEN_A, 10.0, frm=W1, to="Y" * 44),
                self._token(WSOL, 0.5, to=W1),  # wSOL key exercises the sol_mint skip in the inflow loop
            ],
        }
        result = client._parse_swap_from_deltas(tx, W1)
        assert result is None  # cancelling deltas -> primary via largest -> still no SOL leg

    def test_instruction_level_fallback(self, client):
        tx = {
            "signature": "s1",
            "nativeTransfers": [self._native(W1, W2, 100_000_000)],
            "tokenTransfers": [
                self._token(TOKEN_A, 10.0, frm="X" * 44, to=W1),
                self._token(TOKEN_A, 10.0, frm=W1, to="Y" * 44),
            ],
            "instructions": [{"programId": "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
                              "parsed": {"type": "swap"}, "accounts": [W1]}],
        }
        result = client._parse_swap_from_deltas(tx, W1)
        assert result is None

    def test_non_dict_entries_skipped(self, client):
        tx = {
            "signature": "s1",
            "nativeTransfers": [None, self._native(W1, W2, 100_000_000), "junk"],
            "tokenTransfers": [None, self._token(TOKEN_A, 10.0, to=W1), 42],
        }
        result = client._parse_swap_from_deltas(tx, W1)
        assert result["direction"] == "BUY"

    def test_bad_native_amount_skipped(self, client):
        tx = {
            "signature": "s1",
            "nativeTransfers": [
                {"fromUserAccount": W1, "toUserAccount": W2, "amount": "not-a-number"}
            ],
            "tokenTransfers": [self._token(TOKEN_A, 10.0, to=W1)],
        }
        # int() fails -> native leg skipped -> sol_delta 0 -> no SOL/stable path
        assert client._parse_swap_from_deltas(tx, W1) is None

    def test_mint_shapes_skipped_and_wsol_keys(self, client):
        """Covers no-mint/wSOL/stable skip branches in the delta loops."""
        tx = {
            "signature": "s1",
            "nativeTransfers": [self._native(W1, W2, 100_000_000)],
            "tokenTransfers": [
                {"tokenAmount": 10.0, "toUserAccount": W1},  # no mint -> skipped
                self._token(WSOL, 0.5, to=W1),  # wSOL: merged into sol path
                self._token(USDC, 1000.0, frm=W1),  # stable: skipped for primary
                self._token(TOKEN_A, 10.0, to=W1),
            ],
        }
        result = client._parse_swap_from_deltas(tx, W1)
        assert result["direction"] == "BUY"
        assert result["token_mint"] == TOKEN_A

    def test_unsatisfiable_sol_to_token_without_primary(self, client):
        tx = {
            "signature": "s1",
            "nativeTransfers": [self._native(W1, W2, 100_000_000)],
            "tokenTransfers": [self._token(TOKEN_A, 10.0, frm="X" * 44, to="Y" * 44)],
        }
        assert client._parse_swap_from_deltas(tx, W1) is None


class TestParseFromInstructionLevel:
    def test_no_instructions(self, client):
        assert client._parse_from_instruction_level({}, W1, {}) is None

    def test_non_dict_instruction(self, client):
        tx = {"instructions": [None, "junk"]}
        assert client._parse_from_instruction_level(tx, W1, {}) is None

    def test_unknown_program(self, client):
        tx = {"instructions": [{"programId": "UNKNOWN123", "parsed": {"type": "swap"}}]}
        assert client._parse_from_instruction_level(tx, W1, {}) is None

    def test_wrong_instruction_type(self, client):
        tx = {"instructions": [{"programId": "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4",
                                "parsed": {"type": "transfer"}}]}
        assert client._parse_from_instruction_level(tx, W1, {}) is None

    def test_no_accounts(self, client):
        tx = {"instructions": [{"programId": "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4",
                                "parsed": {"type": "swap"}}]}
        assert client._parse_from_instruction_level(tx, W1, {}) is None

    def test_info_not_dict(self, client):
        tx = {"instructions": [{"programId": "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4",
                                "parsed": {"type": "swap", "info": None},
                                "accounts": [W1]}]}
        assert client._parse_from_instruction_level(tx, W1, {}) is None

    def test_wallet_not_in_accounts(self, client):
        tx = {"instructions": [{"programId": "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4",
                                "parsed": {"type": "swap",
                                           "info": {"tokenIn": USDC, "tokenOut": TOKEN_A}},
                                "accounts": [W2]}]}
        assert client._parse_from_instruction_level(tx, W1, {}) is None

    def test_buy_with_stable_quote(self, client):
        deltas = {USDC: -1000.0, TOKEN_A: 5000.0}
        tx = {"signature": "s1", "timestamp": 123,
              "instructions": [{"programId": "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4",
                                "parsed": {"type": "swap",
                                           "info": {"tokenIn": USDC, "tokenOut": TOKEN_A}},
                                "accounts": [W1]}]}
        result = client._parse_from_instruction_level(tx, W1, deltas)
        assert result["direction"] == "BUY"
        assert result["token_mint"] == TOKEN_A
        assert result["price_usd"] == 0.2
        assert result["quote_mint"] == USDC
        assert result["swap_type"] == "instruction_jupiter"

    def test_buy_without_stable_quote(self, client):
        deltas = {TOKEN_B: -100.0, TOKEN_A: 5000.0}
        tx = {"instructions": [{"programId": "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",
                                "parsed": {"type": "swapBaseIn",
                                           "info": {"inputMint": TOKEN_B, "outputMint": TOKEN_A}},
                                "accounts": [W1]}]}
        result = client._parse_from_instruction_level(tx, W1, deltas)
        assert result["direction"] == "BUY"
        assert result["price_usd"] is None
        assert result["usd_amount"] is None
        assert result["swap_type"] == "instruction_orca"

    def test_direction_mismatch_returns_none(self, client):
        deltas = {USDC: 100.0, TOKEN_A: 5000.0}
        tx = {"instructions": [{"programId": "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4",
                                "parsed": {"type": "swap",
                                           "info": {"tokenIn": USDC, "tokenOut": TOKEN_A}},
                                "accounts": [W1]}]}
        assert client._parse_from_instruction_level(tx, W1, deltas) is None


class TestParseSwapFromEvents:
    def test_events_not_dict(self, client):
        tx = {"events": "junk"}
        assert client._parse_swap_from_events(tx, W1) is None

    def test_swap_empty(self, client):
        tx = {"events": {}}
        assert client._parse_swap_from_events(tx, W1) is None

    def test_buy_native_input_dict(self, client):
        tx = {"signature": "s1", "events": {"swap": {
            "nativeInput": {"amount": "50000000"},
            "tokenOutputs": [{"mint": TOKEN_A, "tokenAmount": 100.0}],
        }}}
        result = client._parse_swap_from_events(tx, W1)
        assert result["direction"] == "BUY"
        assert result["sol_amount"] == 0.05

    def test_sell_native_output_scalar(self, client):
        tx = {"signature": "s1", "events": {"swap": {
            "nativeOutput": "20000000",
            "tokenInputs": [{"mint": TOKEN_A, "tokenAmount": 50.0}],
        }}}
        result = client._parse_swap_from_events(tx, W1)
        assert result["direction"] == "SELL"
        assert result["sol_amount"] == 0.02

    def test_native_output_dict_bad_amount(self, client):
        tx = {"signature": "s1", "events": {"swap": {
            "nativeOutput": {"amount": "abc"},
            "tokenInputs": [{"mint": TOKEN_A, "tokenAmount": 50.0}],
        }}}
        # dict-form amount parses to 0.0 -> no SOL leg and no token outputs -> None
        assert client._parse_swap_from_events(tx, W1) is None

    def test_net_sol_in_greater(self, client):
        tx = {"signature": "s1", "events": {"swap": {
            "nativeInput": 100_000_000,
            "nativeOutput": 20_000_000,
            "tokenOutputs": [{"mint": TOKEN_A, "tokenAmount": 100.0}],
        }}}
        result = client._parse_swap_from_events(tx, W1)
        assert result["direction"] == "BUY"
        assert result["sol_amount"] == 0.08

    def test_net_sol_out_greater(self, client):
        tx = {"signature": "s1", "events": {"swap": {
            "nativeInput": 20_000_000,
            "nativeOutput": 100_000_000,
            "tokenInputs": [{"mint": TOKEN_A, "tokenAmount": 100.0}],
        }}}
        result = client._parse_swap_from_events(tx, W1)
        assert result["direction"] == "SELL"
        assert result["sol_amount"] == 0.08

    def test_token_to_token_events(self, client):
        tx = {"signature": "s1", "events": {"swap": {
            "tokenInputs": [{"mint": USDC, "tokenAmount": 100.0}],
            "tokenOutputs": [{"mint": TOKEN_A, "tokenAmount": 5000.0}],
        }}}
        result = client._parse_swap_from_events(tx, W1)
        assert result["direction"] == "BUY"
        assert result["token_mint"] == TOKEN_A

    def test_token_inputs_dict_normalized(self, client):
        tx = {"signature": "s1", "events": {"swap": {
            "tokenIn": {"mint": USDC, "tokenAmount": 100.0},
            "tokenOut": {"mint": TOKEN_A, "tokenAmount": 5000.0},
        }}}
        result = client._parse_swap_from_events(tx, W1)
        assert result["direction"] == "BUY"

    def test_no_sol_leg_no_tokens_returns_none(self, client):
        tx = {"events": {"swap": {"tokenInputs": [], "tokenOutputs": []}}}
        assert client._parse_swap_from_events(tx, W1) is None

    def test_non_dict_entries_skipped(self, client):
        tx = {"signature": "s1", "events": {"swap": {
            "nativeInput": {"amount": "abc"},
            "tokenInputs": [None, "junk", {"mint": TOKEN_A, "tokenAmount": 50.0}],
            "tokenOutputs": [None, {"mint": TOKEN_A, "tokenAmount": 100.0}],
        }}}
        result = client._parse_swap_from_events(tx, W1)
        assert result is not None
        assert result["direction"] == "BUY"  # token-to-token leg (non-dict entries skipped)

    def test_token_outputs_missing_mint(self, client):
        tx = {"signature": "s1", "events": {"swap": {
            "nativeInput": 50_000_000,
            "tokenOutputs": [{"tokenAmount": 5.0}],
        }}}
        assert client._parse_swap_from_events(tx, W1) is None


class TestParseSwapFromAccountData:
    def test_no_account_data(self, client):
        assert client._parse_swap_from_account_data({}, W1) is None

    def test_buy_from_raw_token_amount(self, client):
        tx = {"signature": "s1", "accountData": [
            {"account": W1, "nativeBalanceChange": -100_000_000,
             "tokenBalanceChanges": [
                 {"userAccount": W1, "mint": TOKEN_A,
                  "rawTokenAmount": {"tokenAmount": "5000000000", "decimals": 6}}
             ]},
        ]}
        result = client._parse_swap_from_account_data(tx, W1)
        assert result["direction"] == "BUY"
        assert result["token_amount"] == 5000.0
        assert result["sol_amount"] == 0.1
        assert result["price_sol"] == 0.1 / 5000.0

    def test_raw_token_amount_no_decimals(self, client):
        tx = {"signature": "s1", "accountData": [
            {"account": W1,
             "tokenBalanceChanges": [
                 {"userAccount": W1, "mint": TOKEN_A,
                  "rawTokenAmount": {"tokenAmount": "123"}}
             ]},
        ]}
        result = client._parse_swap_from_account_data(tx, W1)
        assert result["token_amount"] == 123.0

    def test_raw_token_amount_bad_decimals(self, client):
        tx = {"signature": "s1", "accountData": [
            {"account": W1,
             "tokenBalanceChanges": [
                 {"userAccount": W1, "mint": TOKEN_A,
                  "rawTokenAmount": {"tokenAmount": "5", "decimals": "abc"}},
                 {"userAccount": W1, "mint": TOKEN_A,
                  "rawTokenAmount": {"tokenAmount": "10000000", "decimals": 6}},
             ]},
        ]}
        result = client._parse_swap_from_account_data(tx, W1)
        assert result["token_amount"] == 10.0

    def test_scalar_before_after(self, client):
        tx = {"signature": "s1", "accountData": [
            {"account": W1,
             "tokenBalanceChanges": [
                 {"userAccount": W1, "mint": TOKEN_A,
                  "rawTokenAmountBefore": "1000000", "rawTokenAmountAfter": "6000000",
                  "decimals": 6}
             ]},
        ]}
        result = client._parse_swap_from_account_data(tx, W1)
        assert result["token_amount"] == 5.0

    def test_scalar_bad_decimals(self, client):
        tx = {"signature": "s1", "accountData": [
            {"account": W1,
             "tokenBalanceChanges": [
                 {"userAccount": W1, "mint": TOKEN_A,
                  "rawTokenAmountBefore": 1.0, "rawTokenAmountAfter": 6.0,
                  "decimals": "x"}
             ]},
        ]}
        result = client._parse_swap_from_account_data(tx, W1)
        assert result["token_amount"] == 5.0

    def test_zero_delta_skipped(self, client):
        tx = {"signature": "s1", "accountData": [
            {"account": W1,
             "tokenBalanceChanges": [
                 {"userAccount": W1, "mint": TOKEN_A,
                  "rawTokenAmountBefore": "5", "rawTokenAmountAfter": "5"}
             ]},
        ]}
        assert client._parse_swap_from_account_data(tx, W1) is None

    def test_user_account_mismatch_skipped(self, client):
        tx = {"signature": "s1", "accountData": [
            {"account": W1,
             "tokenBalanceChanges": [
                 {"userAccount": W2, "mint": TOKEN_A,
                  "rawTokenAmount": {"tokenAmount": "100", "decimals": 0}}
             ]},
        ]}
        assert client._parse_swap_from_account_data(tx, W1) is None

    def test_missing_mint_skipped(self, client):
        tx = {"signature": "s1", "accountData": [
            {"account": W1,
             "tokenBalanceChanges": [
                 {"userAccount": W1,
                  "rawTokenAmount": {"tokenAmount": "100", "decimals": 0}},
                 {"userAccount": W1, "mint": TOKEN_A,
                  "rawTokenAmount": {"tokenAmount": "200", "decimals": 0}},
             ]},
        ]}
        result = client._parse_swap_from_account_data(tx, W1)
        assert result["token_amount"] == 200.0

    def test_sol_only_change(self, client):
        tx = {"signature": "s1", "accountData": [
            {"account": W1, "nativeBalanceChange": "not-int",
             "tokenBalanceChanges": [
                 {"userAccount": W1, "mint": WSOL,
                  "rawTokenAmount": {"tokenAmount": "1000000000", "decimals": 9}}
             ]},
        ]}
        result = client._parse_swap_from_account_data(tx, W1)
        assert result["token_mint"] == WSOL
        assert result["sol_amount"] == 1.0

    def test_native_change_int(self, client):
        tx = {"signature": "s1", "accountData": [
            {"account": W1, "nativeBalanceChange": 100_000_000,
             "tokenBalanceChanges": [
                 {"userAccount": W1, "mint": TOKEN_A,
                  "rawTokenAmount": {"tokenAmount": "-5000000000", "decimals": 6}}
             ]},
        ]}
        result = client._parse_swap_from_account_data(tx, W1)
        assert result["direction"] == "SELL"
        assert result["sol_amount"] == 0.1

