"""
Tests for the quota re-probe circuit breaker (2026-08-11).

When Helius hits its daily credit limit ("max usage reached"), the client
opens a quota-exhaustion circuit breaker that pauses ALL requests until the
midnight-UTC reset. If the account is topped up mid-day (credits purchased
or plan upgraded), the probe re-tests the quota at a configurable cadence so
traffic resumes before midnight.
"""

import time

import pytest
from unittest.mock import AsyncMock, MagicMock, patch


def _make_client():
    from core.helius_client import HeliusClient

    return HeliusClient(api_key="test_key")


def _fake_response(status: int, body: str = "{}"):
    """aiohttp response stand-in usable with ``async with``."""
    resp = AsyncMock()
    resp.status = status
    resp.text = AsyncMock(return_value=body)
    resp.json = AsyncMock(return_value={})
    resp.headers = {}
    resp.raise_for_status = MagicMock(return_value=None)
    resp.__aenter__.return_value = resp
    resp.__aexit__.return_value = False
    return resp


def _fake_session(response):
    session = MagicMock()
    session.get.return_value = response
    return session


@pytest.mark.asyncio
async def test_probe_success_clears_quota_breaker():
    """A 200 probe clears the quota breaker and resets failure counts."""
    client = _make_client()
    client._quota_exhausted = True
    client._circuit_breaker_failures = 5
    client._circuit_breaker_reset_time = time.time() + 1000

    session = _fake_session(_fake_response(200))
    with patch.object(client, "_get_session", new=AsyncMock(return_value=session)):
        recovered = await client._probe_quota_once()

    assert recovered is True
    assert client._quota_exhausted is False
    assert client._circuit_breaker_failures == 0
    assert client._circuit_breaker_reset_time is None


@pytest.mark.asyncio
async def test_probe_429_keeps_breaker_open():
    """A quota-exhausted 429 keeps the breaker open and schedules no retry."""
    client = _make_client()
    client._quota_exhausted = True
    client._circuit_breaker_failures = 5

    session = _fake_session(
        _fake_response(429, "Quota exhausted: max usage reached")
    )
    with patch.object(client, "_get_session", new=AsyncMock(return_value=session)):
        recovered = await client._probe_quota_once()

    assert recovered is False
    assert client._quota_exhausted is True
    assert client._circuit_breaker_failures == 5


@pytest.mark.asyncio
async def test_probe_network_error_keeps_breaker_open():
    """A probe network error stays paused (fail-closed), no exception escapes."""
    client = _make_client()
    client._quota_exhausted = True

    session = MagicMock()
    session.get.side_effect = TimeoutError("boom")
    with patch.object(client, "_get_session", new=AsyncMock(return_value=session)):
        recovered = await client._probe_quota_once()

    assert recovered is False
    assert client._quota_exhausted is True


@pytest.mark.asyncio
async def test_make_request_probes_and_recovers_when_due():
    """When the probe is due, a blocked request probes; success lets it through."""
    client = _make_client()
    client._quota_exhausted = True
    client._circuit_breaker_failures = 5
    client._quota_last_probe_time = 0.0  # probe is due

    async def _probe_recovered():
        # Mirrors the real probe's side effect (see _probe_quota_once).
        client._quota_exhausted = False
        client._circuit_breaker_failures = 0
        client._circuit_breaker_reset_time = None
        return True

    with patch.object(client, "_probe_quota_once", new=_probe_recovered):
        session = _fake_session(_fake_response(200))
        with patch.object(client, "_get_session", new=AsyncMock(return_value=session)):
            result = await client._make_request("/account/So11111111111111111111111111111111111111112")

    assert result is not None
    assert client._quota_exhausted is False
    assert client._circuit_breaker_failures == 0


@pytest.mark.asyncio
async def test_make_request_stays_blocked_when_probe_still_exhausted():
    """A failed probe keeps the request blocked and the breaker open."""
    client = _make_client()
    client._quota_exhausted = True
    client._quota_last_probe_time = 0.0  # probe is due

    async def _probe_still_exhausted():
        client._quota_last_probe_time = time.time()
        return False

    with patch.object(client, "_probe_quota_once", new=_probe_still_exhausted):
        result = await client._make_request("/some/endpoint")

    assert result is None
    assert client._quota_exhausted is True
    # Probe timestamp was recorded so the next probe is throttled (no storm).
    assert client._quota_last_probe_time > 0


@pytest.mark.asyncio
async def test_make_request_throttles_probe_when_not_due():
    """Before the cadence elapses, blocked requests skip the probe entirely."""
    client = _make_client()
    client._quota_exhausted = True
    client._quota_last_probe_time = time.time()  # probe not due

    with patch.object(client, "_probe_quota_once", new=AsyncMock(return_value=True)) as probe:
        result = await client._make_request("/some/endpoint")

    assert result is None
    probe.assert_not_awaited()


@pytest.mark.asyncio
async def test_make_request_does_not_probe_for_regular_breaker():
    """The quota probe must never fire for the regular (non-quota) breaker."""
    client = _make_client()
    client._quota_exhausted = False
    client._circuit_breaker_failures = client._circuit_breaker_threshold
    client._circuit_breaker_reset_time = time.time() + 60

    with patch.object(client, "_probe_quota_once", new=AsyncMock(return_value=True)) as probe:
        result = await client._make_request("/some/endpoint")

    assert result is None
    probe.assert_not_awaited()


def test_is_quota_exhausted_reports_state():
    client = _make_client()
    assert client.is_quota_exhausted() is False
    client._quota_exhausted = True
    assert client.is_quota_exhausted() is True


def test_quota_probe_interval_config_default(monkeypatch):
    from config import ScoutConfig

    monkeypatch.delenv("SCOUT_QUOTA_PROBE_INTERVAL_SECONDS", raising=False)
    assert ScoutConfig.get_quota_probe_interval_seconds() == 600

    monkeypatch.setenv("SCOUT_QUOTA_PROBE_INTERVAL_SECONDS", "120")
    assert ScoutConfig.get_quota_probe_interval_seconds() == 120
