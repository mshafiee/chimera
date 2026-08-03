"""
Tests for Helius 429 rate limit handling following Helius best practices.

Tests that the Python Helius client properly handles HTTP 429 responses:
- Retries retryable errors with exponential backoff
- Applies ±25% jitter on retries (capped at 30s)
- Exhausts retries and raises
- Connection pooling configuration
"""

import aiohttp
import pytest
from unittest.mock import AsyncMock, patch


class _FakeRequestInfo:
    """Minimal aiohttp RequestInfo stand-in (ClientResponseError str() needs it)."""
    real_url = "http://test.invalid"
    url = "http://test.invalid"
    method = "GET"


def _http_error(status: int, message: str) -> aiohttp.ClientResponseError:
    return aiohttp.ClientResponseError(
        request_info=_FakeRequestInfo(), history=None, status=status, message=message
    )


@pytest.mark.asyncio
async def test_retry_with_backoff_retries_retryable_errors():
    """A retryable 429 error is retried; the call eventually succeeds."""
    from core.helius_client import HeliusClient

    client = HeliusClient(api_key="test_key")

    attempts = {"n": 0}

    async def failing_factory():
        attempts["n"] += 1
        if attempts["n"] < 3:
            raise _http_error(429, "Rate limited")
        return "ok"

    with patch("core.helius_client.asyncio.sleep", new=AsyncMock()) as mock_sleep:
        result = await client._retry_with_backoff(failing_factory, max_retries=5)

    assert result == "ok"
    assert attempts["n"] == 3, "The failing calls must be retried"
    assert mock_sleep.await_count == 2, "Two retries -> two backoff sleeps"


@pytest.mark.asyncio
async def test_retry_with_backoff_deterministic_doubling():
    """Backoff doubles per attempt (1s, 2s) when jitter is neutralized."""
    from core.helius_client import HeliusClient

    client = HeliusClient(api_key="test_key")

    attempts = {"n": 0}

    async def failing_factory():
        attempts["n"] += 1
        if attempts["n"] < 3:
            raise _http_error(429, "Rate limited")
        return "ok"

    with patch("core.helius_client.asyncio.sleep", new=AsyncMock()) as mock_sleep, \
         patch("core.helius_client.random.uniform", return_value=0.0):
        await client._retry_with_backoff(failing_factory, max_retries=5)

    # attempt 0 -> 2**0 = 1s; attempt 1 -> 2**1 = 2s
    sleep_calls = [c.args[0] for c in mock_sleep.call_args_list]
    assert sleep_calls == [1.0, 2.0], f"Expected [1.0, 2.0], got {sleep_calls}"


@pytest.mark.asyncio
async def test_retry_with_backoff_exhausts_and_raises():
    """Persistent failures exhaust retries and re-raise."""
    from core.helius_client import HeliusClient

    client = HeliusClient(api_key="test_key")

    async def always_fails():
        raise _http_error(429, "Rate limited")

    with patch("core.helius_client.asyncio.sleep", new=AsyncMock()) as mock_sleep:
        with pytest.raises(aiohttp.ClientResponseError):
            await client._retry_with_backoff(always_fails, max_retries=5)

    # 5 attempts -> 4 backoff sleeps (final attempt raises without sleeping)
    assert mock_sleep.await_count == 4


@pytest.mark.asyncio
async def test_retry_with_backoff_non_retryable_fails_fast():
    """A non-retryable error (e.g. 400) fails immediately without retries."""
    from core.helius_client import HeliusClient

    client = HeliusClient(api_key="test_key")

    async def bad_request():
        raise _http_error(400, "Bad request")

    with patch("core.helius_client.asyncio.sleep", new=AsyncMock()) as mock_sleep:
        with pytest.raises(aiohttp.ClientResponseError):
            await client._retry_with_backoff(bad_request, max_retries=5)

    assert mock_sleep.await_count == 0, "Non-retryable errors must not back off"


@pytest.mark.asyncio
async def test_session_reuse():
    """Test that session is reused across requests."""
    from core.helius_client import HeliusClient

    client = HeliusClient(api_key="test_key")
    try:
        session1 = await client._get_session()
        session2 = await client._get_session()

        # Should be the same session instance
        assert session1 is session2
    finally:
        await client._close_session()


@pytest.mark.asyncio
async def test_connection_pooling_configuration():
    """Test that the session is a pooled aiohttp ClientSession."""
    from core.helius_client import HeliusClient

    client = HeliusClient(api_key="test_key")
    try:
        session = await client._get_session()

        assert session is not None
        assert isinstance(session, aiohttp.ClientSession)
        assert session.connector is not None
    finally:
        await client._close_session()
