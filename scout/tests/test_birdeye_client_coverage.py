"""
Coverage tests for core.birdeye_client.BirdeyeClient.

Mocks aiohttp sessions/responses; covers all public methods and edge paths.
"""

import asyncio
import time
from datetime import datetime
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch

import aiohttp
import pytest

import core.birdeye_client as birdeye_mod
from core.birdeye_client import BirdeyeClient


class _FakeRequestInfo:
    real_url = "http://test.invalid"
    url = "http://test.invalid"
    method = "GET"


class _AioResp:
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


def _resp(status=200, payload=None, raise_exc=None):
    return _AioResp(status=status, payload=payload, raise_exc=raise_exc)


class _FakeSession:
    """Stub aiohttp session: get()/post() return queued FakeResponses."""

    def __init__(self, responses):
        self._responses = list(responses) if isinstance(responses, list) else [responses]
        self._idx = 0
        self.closed = False
        self._loop = None

    def _next(self):
        if self._idx < len(self._responses):
            r = self._responses[self._idx]
            self._idx += 1
            return r
        return _resp(status=404)

    def get(self, *a, **k):
        return self._next()

    async def close(self):
        self.closed = True


async def _attach(client, responses):
    """Attach a fake session to the client and bind it to the running loop."""
    fake = _FakeSession(responses)
    fake._loop = asyncio.get_running_loop()
    client._session = fake
    return fake


@pytest.fixture
def client():
    return BirdeyeClient(api_key="test_key")


class TestInit:
    def test_api_key_param(self):
        c = BirdeyeClient(api_key="abc")
        assert c.api_key == "abc"

    def test_api_key_env_fallback(self, monkeypatch):
        monkeypatch.setenv("BIRDEYE_API_KEY", "env_key")
        c = BirdeyeClient()
        assert c.api_key == "env_key"

    def test_session_passthrough(self):
        s = MagicMock()
        c = BirdeyeClient(api_key="k", session=s)
        assert c._session is s
        assert c._own_session is False


class TestSession:
    @pytest.mark.asyncio
    async def test_get_session_creates(self):
        c = BirdeyeClient(api_key="k")
        with patch.object(birdeye_mod.aiohttp, "ClientSession") as cls:
            cls.return_value = MagicMock(_loop=None)
            s = await c._get_session()
        assert c._own_session is True
        assert s is c._session

    @pytest.mark.asyncio
    async def test_get_session_reuses(self, client):
        fake = MagicMock(_loop=asyncio.get_running_loop())
        client._session = fake
        s = await client._get_session()
        assert s is fake

    @pytest.mark.asyncio
    async def test_get_session_loop_mismatch_creates_new(self, client):
        fake = MagicMock(_loop=object())
        client._session = fake
        with patch.object(birdeye_mod.aiohttp, "ClientSession") as cls:
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
        await client._close_session()
        assert client._session is None

    @pytest.mark.asyncio
    async def test_close_public(self, client):
        fake = _FakeSession([])
        fake._loop = asyncio.get_running_loop()
        client._session = fake
        client._own_session = True
        await client.close()
        assert fake.closed is True

    @pytest.mark.asyncio
    async def test_async_context_manager(self, client):
        fake = _FakeSession([])
        fake._loop = asyncio.get_running_loop()
        client._session = fake
        client._own_session = True
        async with client as c:
            assert c is client
        assert fake.closed is True


class TestRateLimit:
    @pytest.mark.asyncio
    async def test_rate_limit_no_delay(self, client):
        with patch("core.birdeye_client.asyncio.sleep", new=AsyncMock()) as slp:
            await client._rate_limit()
        slp.assert_not_awaited()

    @pytest.mark.asyncio
    async def test_rate_limit_with_delay(self, client):
        client.last_request_time = time.time()
        with patch("core.birdeye_client.asyncio.sleep", new=AsyncMock()) as slp:
            await client._rate_limit()
        assert slp.await_count == 1


class TestMakeRequest:
    @pytest.mark.asyncio
    async def test_no_api_key(self):
        c = BirdeyeClient(api_key="")
        assert await c._make_request("/x", {}) is None

    @pytest.mark.asyncio
    async def test_success(self, client):
        await _attach(client, _resp(status=200, payload={"ok": True}))
        data = await client._make_request("/defi/foo", {"a": 1})
        assert data == {"ok": True}

    @pytest.mark.asyncio
    async def test_client_error_returns_none(self, client):
        await _attach(client, _resp(
            status=500,
            raise_exc=aiohttp.ClientResponseError(
                request_info=_FakeRequestInfo(), history=None, status=500, message="boom",
            ),
        ))
        assert await client._make_request("/defi/foo", {}) is None

    @pytest.mark.asyncio
    async def test_http_error_raise_for_status(self, client):
        await _attach(client, _resp(
            status=404,
            raise_exc=aiohttp.ClientResponseError(
                request_info=_FakeRequestInfo(), history=None, status=404, message="nope",
            ),
        ))
        assert await client._make_request("/defi/foo", {}) is None


class TestGetHistoricalPrice:
    async def _call(self, client, payload):
        await _attach(client, _resp(status=200, payload=payload))
        ts = datetime(2026, 1, 1)
        return await client.get_historical_price("TOKEN", ts)

    @pytest.mark.asyncio
    async def test_no_data(self, client):
        assert await self._call(client, None) is None

    @pytest.mark.asyncio
    async def test_no_data_key(self, client):
        assert await self._call(client, {"other": 1}) is None

    @pytest.mark.asyncio
    async def test_items_dict(self, client):
        payload = {"data": {"items": {"123": {"value": 1.5}}}}
        assert await self._call(client, payload) == 1.5

    @pytest.mark.asyncio
    async def test_items_dict_price_key(self, client):
        payload = {"data": {"items": {"123": {"price": 2.5}}}}
        assert await self._call(client, payload) == 2.5

    @pytest.mark.asyncio
    async def test_items_dict_empty_values(self, client):
        payload = {"data": {"items": {"123": "nope"}}}
        assert await self._call(client, payload) is None

    @pytest.mark.asyncio
    async def test_flat_dict_value(self, client):
        payload = {"data": {"value": 9.9}}
        assert await self._call(client, payload) == 9.9

    @pytest.mark.asyncio
    async def test_flat_dict_price(self, client):
        payload = {"data": {"price": 8.8}}
        assert await self._call(client, payload) == 8.8

    @pytest.mark.asyncio
    async def test_list_data(self, client):
        payload = {"data": [{"value": 3.3}]}
        assert await self._call(client, payload) == 3.3

    @pytest.mark.asyncio
    async def test_empty_list(self, client):
        assert await self._call(client, {"data": []}) is None


class TestGetHistoricalLiquidity:
    @pytest.mark.asyncio
    async def test_no_price(self, client):
        await _attach(client, _resp(status=200, payload={}))
        ts = datetime(2026, 1, 1)
        assert await client.get_historical_liquidity("TOKEN", ts) is None

    @pytest.mark.asyncio
    async def test_no_current_liquidity(self, client):
        ts = datetime(2026, 1, 1)
        await _attach(client, [
            _resp(status=200, payload={"data": {"value": 1.0}}),  # price
            _resp(status=200, payload={"data": {"liquidity": 0}}),  # overview
        ])
        assert await client.get_historical_liquidity("TOKEN", ts) is None

    @pytest.mark.asyncio
    async def test_proxy_uses_current_liquidity(self, client):
        ts = datetime(2026, 1, 1)
        await _attach(client, [
            _resp(status=200, payload={"data": {"value": 1.0}}),
            _resp(status=200, payload={"data": {"liquidity": 5000, "price": 1.0, "volume24hUSD": 200}}),
        ])
        liq = await client.get_historical_liquidity("TOKEN", ts)
        assert liq is not None
        assert liq.token_address == "TOKEN"
        assert liq.liquidity_usd == 5000
        assert liq.price_usd == 1.0
        assert liq.volume_24h_usd == 200
        assert liq.source == "birdeye_historical_proxy"
        assert liq.timestamp == ts


class TestGetCurrentLiquidity:
    @pytest.mark.asyncio
    async def test_no_data(self, client):
        await _attach(client, _resp(status=200, payload={}))
        assert await client.get_current_liquidity("TOKEN") is None

    @pytest.mark.asyncio
    async def test_zero_liquidity(self, client):
        await _attach(client, _resp(status=200, payload={"data": {"liquidity": 0}}))
        assert await client.get_current_liquidity("TOKEN") is None

    @pytest.mark.asyncio
    async def test_success(self, client):
        await _attach(client, _resp(
            status=200,
            payload={"data": {"liquidity": 1000, "price": 2.0, "volume24hUSD": 50}},
        ))
        liq = await client.get_current_liquidity("TOKEN")
        assert liq is not None
        assert liq.liquidity_usd == 1000
        assert liq.price_usd == 2.0
        assert liq.source == "birdeye"


class TestGetTokenCreationInfo:
    @pytest.mark.asyncio
    async def test_no_api_key(self):
        c = BirdeyeClient(api_key="")
        assert await c.get_token_creation_info("TOKEN") is None

    @pytest.mark.asyncio
    async def test_429_returns_none(self, client):
        await _attach(client, _resp(status=429))
        assert await client.get_token_creation_info("TOKEN") is None

    @pytest.mark.asyncio
    async def test_success(self, client):
        await _attach(client, _resp(status=200, payload={"data": {"ts": 123}}))
        assert await client.get_token_creation_info("TOKEN") == {"ts": 123}

    @pytest.mark.asyncio
    async def test_no_data_key(self, client):
        await _attach(client, _resp(status=200, payload={"nope": 1}))
        assert await client.get_token_creation_info("TOKEN") is None

    @pytest.mark.asyncio
    async def test_http_error(self, client):
        await _attach(client, _resp(
            status=500,
            raise_exc=aiohttp.ClientResponseError(
                request_info=_FakeRequestInfo(), history=None, status=500, message="x",
            ),
        ))
        assert await client.get_token_creation_info("TOKEN") is None

    @pytest.mark.asyncio
    async def test_generic_exception(self, client):
        fake = _FakeSession([_resp(status=200, payload={})])
        fake.get = MagicMock(side_effect=Exception("boom"))
        client._session = fake
        fake._loop = asyncio.get_running_loop()
        assert await client.get_token_creation_info("TOKEN") is None


class TestGetTokenMetadata:
    @pytest.mark.asyncio
    async def test_no_data(self, client):
        await _attach(client, _resp(status=200, payload={}))
        assert await client.get_token_metadata("TOKEN") is None

    @pytest.mark.asyncio
    async def test_partial_meta(self, client):
        await _attach(client, _resp(
            status=200,
            payload={"data": {"symbol": "BONK", "name": None, "decimals": 5}},
        ))
        meta = await client.get_token_metadata("TOKEN")
        assert meta == {"symbol": "BONK", "decimals": 5}

    @pytest.mark.asyncio
    async def test_empty_meta(self, client):
        await _attach(client, _resp(status=200, payload={"data": {}}))
        assert await client.get_token_metadata("TOKEN") is None
