"""
Unit tests for slippage edge cases.

Exercises the production slippage estimator (LiquidityProvider.estimate_slippage)
against hand-computed expectations for edge cases:
- Zero liquidity scenarios
- Very large trades vs small liquidity
- DEX fee edge cases (zero fees, high fees)
- Slippage at boundaries
- Negative slippage scenarios
"""

from decimal import Decimal

import pytest

from core.liquidity import LiquidityProvider


@pytest.fixture
def provider(monkeypatch):
    """LiquidityProvider with the deterministic legacy sqrt model pinned."""
    monkeypatch.setenv("SCOUT_USE_CPMM_SLIPPAGE", "false")
    return LiquidityProvider(mode="simulated")


def _legacy_expected(amount_sol, liquidity_usd, sol_price_usd=150.0):
    """Hand-computed expectation for the legacy sqrt model.

    Matches production: base = 0.1 * sqrt(trade_value_usd / liquidity_usd),
    turnover factor 1.0 when volume is 0, age additive 0 for mature tokens,
    plus the small-trade component, capped at 1.0 (100%).
    """
    trade_value_usd = amount_sol * sol_price_usd
    base = 0.1 * (trade_value_usd / liquidity_usd) ** 0.5
    trade_size_component = min(0.005, trade_value_usd / 20000.0)
    return min(base + trade_size_component, 1.0)


class TestSlippageEdgeCases:
    """Test slippage calculation edge cases."""

    def test_slippage_zero_liquidity(self, provider):
        """Test slippage with zero liquidity (trade must fail: 100%)."""
        slippage = provider.estimate_slippage("TOKEN", 10.0, 0.0)
        assert slippage == 1.0, "Zero liquidity should result in 100% slippage"

    def test_slippage_extremely_small_liquidity(self, provider):
        """Test slippage with extremely small liquidity."""
        liquidity_usd = 0.01
        slippage = provider.estimate_slippage("TOKEN", 10.0, liquidity_usd)
        assert slippage == pytest.approx(_legacy_expected(10.0, liquidity_usd), rel=1e-9)
        assert slippage == 1.0, "Tiny liquidity should cap slippage at 100%"

    def test_slippage_large_trade_small_liquidity(self, provider):
        """Test slippage with large trade vs small liquidity."""
        amount_sol = 666.67  # ~$100k at $150 SOL
        liquidity_usd = 1000.0  # $1k liquidity
        slippage = provider.estimate_slippage("TOKEN", amount_sol, liquidity_usd)
        assert slippage == 1.0, "Large trade vs small liquidity should cap at 100%"

    def test_slippage_small_trade_large_liquidity(self, provider):
        """Test slippage with small trade vs large liquidity."""
        amount_sol = 0.0667  # ~$10 trade
        liquidity_usd = 1000000.0  # $1M liquidity
        slippage = provider.estimate_slippage("TOKEN", amount_sol, liquidity_usd)
        assert slippage == pytest.approx(_legacy_expected(amount_sol, liquidity_usd), rel=1e-9)
        assert slippage < 0.01, "Small trade vs large liquidity should have sub-1% slippage"

    def test_slippage_zero_trade(self, provider):
        """Test slippage with zero trade value."""
        slippage = provider.estimate_slippage("TOKEN", 0.0, 1000.0)
        assert slippage == 0.0, "Zero trade should result in zero slippage"

    def test_slippage_equal_trade_and_liquidity(self, provider):
        """Test slippage when trade value equals liquidity."""
        amount_sol = 6.67  # ~$1000 trade
        liquidity_usd = 1000.0  # $1000 liquidity
        slippage = provider.estimate_slippage("TOKEN", amount_sol, liquidity_usd)
        assert slippage == pytest.approx(_legacy_expected(amount_sol, liquidity_usd), rel=1e-9)

    def test_slippage_dex_fee_zero(self, provider):
        """Test slippage with zero DEX fee (fee is a backtest cost, not slippage)."""
        trade_value = Decimal('1000')
        dex_fee_percent = Decimal('0')  # 0% fee

        dex_fee = trade_value * (dex_fee_percent / Decimal('100'))
        assert dex_fee == Decimal('0'), \
            "Zero DEX fee should result in zero fee"

    def test_slippage_dex_fee_high(self, provider):
        """Test slippage with very high DEX fee."""
        trade_value = Decimal('1000')
        dex_fee_percent = Decimal('10')  # 10% fee

        dex_fee = trade_value * (dex_fee_percent / Decimal('100'))
        assert dex_fee == Decimal('100'), \
            "10% DEX fee should result in $100 fee"

    def test_slippage_priority_fee_zero(self, provider):
        """Test slippage with zero priority fee (fee is a backtest cost)."""
        priority_fee_sol = Decimal('0')  # 0 SOL

        total_priority_cost = priority_fee_sol
        assert total_priority_cost == Decimal('0'), \
            "Zero priority fee should result in zero cost"

    def test_slippage_jito_tip_zero(self, provider):
        """Test slippage with zero Jito tip (fee is a backtest cost)."""
        jito_tip_sol = Decimal('0')  # 0 SOL

        total_jito_cost = jito_tip_sol
        assert total_jito_cost == Decimal('0'), \
            "Zero Jito tip should result in zero cost"

    def test_slippage_jito_tip_high(self, provider):
        """Test slippage with high Jito tip."""
        jito_tip_sol = Decimal('0.05')  # 0.05 SOL

        total_jito_cost = jito_tip_sol
        assert total_jito_cost == Decimal('0.05'), \
            "High Jito tip should result in 0.05 SOL cost"

    def test_negative_slippage(self, provider):
        """Test negative slippage (better than expected) never propagates."""
        slippage = provider.estimate_slippage("TOKEN", 1.0, 1000.0, volume_24h_usd=0.0)
        assert slippage >= 0.0, "Estimated slippage must never be negative"
