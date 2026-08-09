"""
Coverage tests for core.liquidity_sources.dexscreener_client and
core.liquidity_sources.jupiter_client.

Both clients are tested with mocked HTTP layers (requests / aiohttp).
"""

import asyncio
import time
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, Mock, patch

import aiohttp
import pytest
import requests

import core.liquidity_sources.dexscreener_client as dexscreener_mod
import core.liquidity_sources.jupiter_client as jupiter_mod
from core.liquidity_sources.dexscreener_client import DexScreenerClient
from core.liquidity_sources.jupiter_client import JupiterLiquidityClient

VALID = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"


class _FakeTime:
    """Stub for the time module so rate limiting never actually sleeps."""

    def __init__(self):
        self.now = 1000.0

    def time(self):
        return self.now

    def sleep(self, s):
        self.now += s


@pytest.fixture
def fake_time(monkeypatch):
    ft = _FakeTime()
    monkeypatch.setattr(dexscreener_mod, "time", ft)
    return ft


# ---------------------------------------------------------------------------
# DexScreenerClient
# ---------------------------------------------------------------------------


def _resp_json(status=200, payload=None, raise_exc=None):
    r = MagicMock()
    r.status_code = status
    r.raise_for_status = MagicMock(side_effect=raise_exc) if raise_exc else MagicMock()
    r.json.return_value = payload
    return r


@pytest.fixture
def dex():
    return DexScreenerClient(api_key="test_key")


class TestDexScreenerInit:
    def test_api_key_param(self):
        assert DexScreenerClient(api_key="abc").api_key == "abc"

    def test_api_key_env(self, monkeypatch):
        monkeypatch.setenv("DEXSCREENER_API_KEY", "env_key")
        assert DexScreenerClient().api_key == "env_key"

    def test_defaults(self):
        c = DexScreenerClient()
        assert c.base_url == "https://api.dexscreener.com/latest/dex"


class TestDexScreenerRateLimit:
    def test_no_delay(self, fake_time):
        c = DexScreenerClient()
        c._rate_limit()
        assert fake_time.now == 1000.0

    def test_with_delay(self, fake_time):
        c = DexScreenerClient()
        c.last_request_time = fake_time.now - 0.1
        c._rate_limit()
        assert fake_time.now >= 1000.4


class TestDexScreenerValidation:
    def test_empty_address(self):
        assert DexScreenerClient()._validate_solana_address("") is False

    def test_valid_address(self):
        assert DexScreenerClient()._validate_solana_address(VALID) is True

    def test_invalid_chars(self):
        assert DexScreenerClient()._validate_solana_address("0OIl" + "A" * 30) is False

    def test_too_short(self):
        assert DexScreenerClient()._validate_solana_address("abc") is False

    def test_safe_url_encode(self):
        c = DexScreenerClient()
        assert c._safe_url_encode("A/B") == "A%2FB"


class TestDexScreenerGetCurrentLiquidity:
    def test_empty_token(self, fake_time):
        assert DexScreenerClient().get_current_liquidity("") is None
        assert DexScreenerClient().get_current_liquidity("   ") is None

    def test_invalid_address(self, fake_time):
        assert DexScreenerClient().get_current_liquidity("bad!addr") is None

    def test_success_with_api_key(self, fake_time):
        payload = {
            "pairs": [
                {"liquidity": {"usd": 100}, "priceUsd": "1.5", "volume": {"h24": 50}},
                {"liquidity": {"usd": 500}, "priceUsd": 2.0, "volume": {"h24": 60}},
            ]
        }
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(payload=payload)) as m:
            liq = DexScreenerClient(api_key="k").get_current_liquidity(VALID)
        assert liq is not None
        assert liq.liquidity_usd == 500
        assert liq.price_usd == 2.0
        assert liq.volume_24h_usd == 60
        assert liq.source == "dexscreener"
        _, kwargs = m.call_args
        assert kwargs["headers"] == {"X-API-KEY": "k"}

    def test_success_without_api_key(self, fake_time):
        payload = {"pairs": [{"liquidity": {"usd": 10}, "priceUsd": 1.0, "volume": {"h24": 5}}]}
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(payload=payload)) as m:
            liq = DexScreenerClient().get_current_liquidity(VALID)
        assert liq is not None
        assert liq.liquidity_usd == 10

    def test_no_pairs(self, fake_time):
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(payload={})):
            assert DexScreenerClient().get_current_liquidity(VALID) is None

    def test_pairs_not_list(self, fake_time):
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(payload={"pairs": "x"})):
            assert DexScreenerClient().get_current_liquidity(VALID) is None

    def test_pairs_empty(self, fake_time):
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(payload={"pairs": []})):
            assert DexScreenerClient().get_current_liquidity(VALID) is None

    def test_bad_liquidity_usd_string(self, fake_time):
        payload = {"pairs": [{"liquidity": {"usd": "oops"}, "priceUsd": 1.0, "volume": {"h24": 1}}]}
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(payload=payload)):
            assert DexScreenerClient().get_current_liquidity(VALID) is None

    def test_non_dict_liquidity_raises_value_error(self, fake_time):
        payload = {"pairs": [{"liquidity": 5, "priceUsd": 1.0, "volume": {"h24": 1}}]}
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(payload=payload)):
            assert DexScreenerClient().get_current_liquidity(VALID) is None

    def test_bad_price_type(self, fake_time):
        payload = {"pairs": [{"liquidity": {"usd": 1}, "priceUsd": [], "volume": {"h24": 1}}]}
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(payload=payload)):
            assert DexScreenerClient().get_current_liquidity(VALID) is None

    def test_bad_volume_type(self, fake_time):
        payload = {"pairs": [{"liquidity": {"usd": 1}, "priceUsd": 1.0, "volume": 5}]}
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(payload=payload)):
            assert DexScreenerClient().get_current_liquidity(VALID) is None

    def test_zero_liquidity_returns_none(self, fake_time):
        payload = {"pairs": [{"liquidity": {"usd": 0}, "priceUsd": 1.0, "volume": {"h24": 1}}]}
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(payload=payload)):
            assert DexScreenerClient().get_current_liquidity(VALID) is None

    def test_request_exception(self, fake_time):
        exc = requests.exceptions.RequestException("network down")
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(raise_exc=exc)):
            assert DexScreenerClient().get_current_liquidity(VALID) is None

    def test_http_error(self, fake_time):
        exc = requests.exceptions.HTTPError("404")
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(status=404, raise_exc=exc)):
            assert DexScreenerClient().get_current_liquidity(VALID) is None

    def test_parsing_error(self, fake_time):
        with patch.object(dexscreener_mod.requests, "get", return_value=_resp_json(payload={"pairs": "x"})):
            assert DexScreenerClient().get_current_liquidity(VALID) is None


# ---------------------------------------------------------------------------
# JupiterLiquidityClient
# ---------------------------------------------------------------------------


class _FakeRequestInfo:
    real_url = "http://test.invalid"
    url = "http://test.invalid"
    method = "GET"


def _jresp(status=200, payload=None, raise_exc=None):
    return _JAioResp(status=status, payload=payload, raise_exc=raise_exc)


class _JAioResp:
    """aiohttp-style response supporting `async with` + raise_for_status."""

    def __init__(self, status=200, payload=None, raise_exc=None):
        self.status = status
        self._payload = payload
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


class _JSession:
    def __init__(self, responses):
        self._responses = list(responses) if isinstance(responses, list) else [responses]
        self._idx = 0
        self.closed = False
        self.detached = False
        self._loop = None

    def _next(self):
        if self._idx < len(self._responses):
            r = self._responses[self._idx]
            self._idx += 1
            return r
        return _jresp(status=404)

    def get(self, *a, **k):
        return self._next()

    async def close(self):
        self.closed = True

    def detach(self):
        self.detached = True


@pytest.fixture
def jup():
    return JupiterLiquidityClient(api_url="https://api.jup.ag/price")


async def _jattach(client, responses):
    fake = _JSession(responses)
    fake._loop = asyncio.get_running_loop()
    client._session = fake
    return fake


class TestJupiterInit:
    def test_defaults(self, monkeypatch):
        monkeypatch.delenv("CHIMERA_JUPITER__API_KEY", raising=False)
        c = JupiterLiquidityClient()
        assert c.api_url == "https://api.jup.ag/price"
        assert c.api_key is None

    def test_api_key_from_env(self, monkeypatch):
        monkeypatch.setenv("CHIMERA_JUPITER__API_KEY", "jup_key")
        c = JupiterLiquidityClient()
        assert c.api_key == "jup_key"

    def test_session_passthrough(self):
        s = MagicMock()
        c = JupiterLiquidityClient(session=s)
        assert c._session is s


class TestJupiterSession:
    @pytest.mark.asyncio
    async def test_get_session_creates(self, jup):
        with patch.object(jupiter_mod.aiohttp, "ClientSession") as cls:
            cls.return_value = MagicMock(_loop=asyncio.get_running_loop(), closed=False)
            s = await jup._get_session()
        assert s is jup._session
        assert jup._own_session is True

    @pytest.mark.asyncio
    async def test_get_session_reuses(self, jup):
        fake = MagicMock(_loop=asyncio.get_running_loop(), closed=False)
        jup._session = fake
        assert await jup._get_session() is fake

    @pytest.mark.asyncio
    async def test_loop_mismatch_closes_old(self, jup):
        old_loop = asyncio.new_event_loop()
        old = MagicMock(_loop=old_loop, closed=False)
        old.close = AsyncMock()
        jup._session = old
        with patch.object(jupiter_mod.aiohttp, "ClientSession") as cls:
            cls.return_value = MagicMock(_loop=asyncio.get_running_loop(), closed=False)
            s = await jup._get_session()
        old.close.assert_awaited_once()
        assert s is not old
        old_loop.close()

    @pytest.mark.asyncio
    async def test_loop_mismatch_detaches_dead_loop(self, jup):
        dead_loop = asyncio.new_event_loop()
        dead_loop.close()
        old = MagicMock(_loop=dead_loop, closed=False)
        old.close = AsyncMock()
        jup._session = old
        with patch.object(jupiter_mod.aiohttp, "ClientSession") as cls:
            cls.return_value = MagicMock(_loop=asyncio.get_running_loop(), closed=False)
            await jup._get_session()
        old.detach.assert_called_once()
        old.close.assert_not_awaited()

    @pytest.mark.asyncio
    async def test_loop_mismatch_old_closed_no_close(self, jup):
        old = MagicMock(_loop=object(), closed=True)
        old.close = AsyncMock()
        jup._session = old
        with patch.object(jupiter_mod.aiohttp, "ClientSession") as cls:
            cls.return_value = MagicMock(_loop=asyncio.get_running_loop(), closed=False)
            await jup._get_session()
        old.close.assert_not_awaited()

    @pytest.mark.asyncio
    async def test_loop_mismatch_close_raises_detaches(self, jup):
        old_loop = asyncio.new_event_loop()
        old = MagicMock(_loop=old_loop, closed=False)
        old.close = AsyncMock(side_effect=Exception("boom"))
        jup._session = old
        with patch.object(jupiter_mod.aiohttp, "ClientSession") as cls:
            cls.return_value = MagicMock(_loop=asyncio.get_running_loop(), closed=False)
            await jup._get_session()
        old.detach.assert_called_once()
        old_loop.close()

    @pytest.mark.asyncio
    async def test_loop_mismatch_close_and_detach_raise(self, jup):
        old_loop = asyncio.new_event_loop()
        old = MagicMock(_loop=old_loop, closed=False)
        old.close = AsyncMock(side_effect=Exception("boom"))
        old.detach = Mock(side_effect=Exception("detach boom"))
        jup._session = old
        with patch.object(jupiter_mod.aiohttp, "ClientSession") as cls:
            cls.return_value = MagicMock(_loop=asyncio.get_running_loop(), closed=False)
            await jup._get_session()
        old_loop.close()

    @pytest.mark.asyncio
    async def test_close_session(self, jup):
        fake = _JSession([])
        fake._loop = asyncio.get_running_loop()
        jup._session = fake
        jup._own_session = True
        await jup._close_session()
        assert fake.closed is True
        assert jup._session is None

    @pytest.mark.asyncio
    async def test_close_session_raises(self, jup):
        fake = MagicMock(_loop=asyncio.get_running_loop())
        fake.close = AsyncMock(side_effect=Exception("boom"))
        jup._session = fake
        jup._own_session = True
        await jup._close_session()
        assert jup._session is None

    @pytest.mark.asyncio
    async def test_close_public(self, jup):
        fake = _JSession([])
        fake._loop = asyncio.get_running_loop()
        jup._session = fake
        jup._own_session = True
        await jup.close()
        assert fake.closed is True

    @pytest.mark.asyncio
    async def test_async_context_manager(self, jup):
        fake = _JSession([])
        fake._loop = asyncio.get_running_loop()
        jup._session = fake
        jup._own_session = True
        async with jup as c:
            assert c is jup
        assert fake.closed is True


class TestJupiterRateLimit:
    @pytest.mark.asyncio
    async def test_no_delay(self, jup):
        with patch("core.liquidity_sources.jupiter_client.asyncio.sleep", new=AsyncMock()) as slp:
            await jup._rate_limit()
        slp.assert_not_awaited()

    @pytest.mark.asyncio
    async def test_with_delay(self, jup):
        jup.last_request_time = time.time()
        with patch("core.liquidity_sources.jupiter_client.asyncio.sleep", new=AsyncMock()) as slp:
            await jup._rate_limit()
        assert slp.await_count == 1


class TestJupiterGetCurrentLiquidity:
    @pytest.mark.asyncio
    async def test_success(self, jup):
        await _jattach(jup, _jresp(status=200, payload={VALID: {"usdPrice": 2.5}}))
        liq = await jup.get_current_liquidity(VALID)
        assert liq is not None
        assert liq.price_usd == 2.5
        assert liq.liquidity_usd == 0.0
        assert liq.source == "jupiter_v3"

    @pytest.mark.asyncio
    async def test_missing_token_key(self, jup):
        await _jattach(jup, _jresp(status=200, payload={"OTHER": {"usdPrice": 1.0}}))
        assert await jup.get_current_liquidity(VALID) is None

    @pytest.mark.asyncio
    async def test_missing_price(self, jup):
        await _jattach(jup, _jresp(status=200, payload={VALID: {"foo": 1}}))
        assert await jup.get_current_liquidity(VALID) is None

    @pytest.mark.asyncio
    async def test_zero_price(self, jup):
        await _jattach(jup, _jresp(status=200, payload={VALID: {"usdPrice": 0}}))
        assert await jup.get_current_liquidity(VALID) is None

    @pytest.mark.asyncio
    async def test_negative_price(self, jup):
        await _jattach(jup, _jresp(status=200, payload={VALID: {"usdPrice": -1}}))
        assert await jup.get_current_liquidity(VALID) is None

    @pytest.mark.asyncio
    async def test_price_not_dict(self, jup):
        await _jattach(jup, _jresp(status=200, payload={VALID: "string"}))
        assert await jup.get_current_liquidity(VALID) is None

    @pytest.mark.asyncio
    async def test_price_non_float(self, jup):
        await _jattach(jup, _jresp(status=200, payload={VALID: {"usdPrice": "abc"}}))
        assert await jup.get_current_liquidity(VALID) is None

    @pytest.mark.asyncio
    async def test_http_client_error(self, jup):
        exc = aiohttp.ClientResponseError(
            request_info=_FakeRequestInfo(), history=None, status=500, message="x",
        )
        await _jattach(jup, _jresp(status=500, raise_exc=exc))
        assert await jup.get_current_liquidity(VALID) is None

    @pytest.mark.asyncio
    async def test_timeout_error(self, jup):
        await _jattach(jup, _jresp(
            status=200,
            raise_exc=asyncio.TimeoutError(),
        ))
        assert await jup.get_current_liquidity(VALID) is None

    @pytest.mark.asyncio
    async def test_api_key_header(self, jup, monkeypatch):
        monkeypatch.setenv("CHIMERA_JUPITER__API_KEY", "jup_key")
        jup.api_key = "jup_key"
        await _jattach(jup, _jresp(status=200, payload={VALID: {"usdPrice": 1.0}}))
        await jup.get_current_liquidity(VALID)


class TestJupiterGetSolPrice:
    @pytest.mark.asyncio
    async def test_success(self, jup):
        await _jattach(jup, _jresp(status=200, payload={"So11111111111111111111111111111111111111112": {"usdPrice": 180.0}}))
        assert await jup.get_sol_price_usd() == 180.0

    @pytest.mark.asyncio
    async def test_unavailable(self, jup):
        await _jattach(jup, _jresp(status=200, payload={}))
        assert await jup.get_sol_price_usd() is None
