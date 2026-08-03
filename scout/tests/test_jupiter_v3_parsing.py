"""Jupiter Price API v3 response parsing tests.

Tests for the three parser locations that incorrectly expected v2 nested response format.
"""

from core.liquidity import LiquidityProvider


def test_liquidity_provider_get_sol_price_usd_sync_fallback_uses_config(monkeypatch):
    """
    Test: Fallback uses configurable constant when cache is empty.
    """
    # Test default (100)
    provider = LiquidityProvider(mode="simulated")
    assert provider.get_sol_price_usd_sync() == 100.0

    # Test custom value via env var (monkeypatch restores it afterwards)
    monkeypatch.setenv("SCOUT_SOL_FALLBACK_PRICE_USD", "200")
    provider = LiquidityProvider(mode="simulated")
    assert provider.get_sol_price_usd_sync() == 200.0


def test_liquidity_provider_last_known_good_cache():
    """
    Test: Last-known-good cache works when the sync wrapper has a stale cache
    and the async sources are unavailable.
    """
    provider = LiquidityProvider(mode="real")

    # Prime the last-known-good cache
    provider._last_known_sol_price = 80.0

    # The sync wrapper only consults _sol_price_cache/_last_known_sol_price/
    # _sol_fallback_price, so the last-known-good value is returned directly.
    result = provider.get_sol_price_usd_sync()

    assert result == 80.0
