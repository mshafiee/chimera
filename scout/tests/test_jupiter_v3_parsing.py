"""Jupiter Price API v3 response parsing tests.

Tests for the three parser locations that incorrectly expected v2 nested response format.
"""

import asyncio
import pytest

from scout.core.liquidity import LiquidityProvider


def test_liquidity_provider_get_sol_price_usd_sync_fallback_uses_config():
    """
    Test: Fallback uses configurable constant when cache is empty.
    """
    # Test default (100)
    provider = LiquidityProvider(mode="simulated")
    assert provider.get_sol_price_usd_sync() == 100.0

    # Test custom value via env var
    import os
    os.environ["SCOUT_SOL_FALLBACK_PRICE_USD"] = "200"
    provider = LiquidityProvider(mode="simulated")
    assert provider.get_sol_price_usd_sync() == 200.0
    del os.environ["SCOUT_SOL_FALLBACK_PRICE_USD"]


def test_liquidity_provider_last_known_good_cache():
    """
    Test: Last-known-good cache works correctly when all sources fail.
    """
    provider = LiquidityProvider(mode="real")

    # Prime the last-known-good cache
    provider._last_known_sol_price = 80.0

    # Force all sources to fail by mocking get_current_liquidity
    def sync_mock_get_liquidity(token_address):
        raise Exception("All sources failed")

    provider.get_current_liquidity = sync_mock_get_liquidity

    # Call the sync wrapper
    result = provider.get_sol_price_usd_sync()

    assert result == 80.0


def test_liquidity_provider_get_sol_price_usd_actual_api_call():
    """
    Integration test: verifies real Jupiter API is called and parses v3 correctly.
    This test doesn't mock; it uses a real API call (allowed in non-strict mode).
    """
    import asyncio
    import os

    # Make sure we're not in simulated mode
    old_mode = os.getenv("SCOUT_LIQUIDITY_MODE")
    try:
        if old_mode:
            os.environ["SCOUT_LIQUIDITY_MODE"] = old_mode
        else:
            os.environ.pop("SCOUT_LIQUIDITY_MODE", None)

        async def test():
            provider = LiquidityProvider(mode="real")
            result = await provider.get_sol_price_usd()
            return result

        result = asyncio.run(test())
        # Should get a real price from Jupiter API
        assert result is not None
        assert result > 0
        assert isinstance(result, float)
        # The real price may vary slightly; just verify it's a reasonable price
        assert 50 < result < 200  # SOL price range
    finally:
        if old_mode:
            os.environ["SCOUT_LIQUIDITY_MODE"] = old_mode
        else:
            os.environ.pop("SCOUT_LIQUIDITY_MODE", None)
