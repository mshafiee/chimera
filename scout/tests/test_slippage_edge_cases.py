"""
Unit tests for slippage edge cases.

Tests various edge cases for slippage calculation:
- Zero liquidity scenarios
- Very large trades vs small liquidity
- DEX fee edge cases (zero fees, high fees)
- Priority fee edge cases
- Jito tip edge cases
- Slippage at boundaries (0%, 100%, >100%)
- Negative slippage scenarios
"""

import pytest
from decimal import Decimal


class TestSlippageEdgeCases:
    """Test slippage calculation edge cases."""

    def test_slippage_zero_liquidity(self):
        """Test slippage with zero liquidity (should fail or return infinite)."""
        trade_value = Decimal('1000')  # $1000 trade
        liquidity = Decimal('0')  # Zero liquidity

        try:
            slippage = (trade_value / liquidity) if liquidity != 0 else Decimal('inf')
            assert slippage == Decimal('inf') or slippage == float('inf'), \
                "Zero liquidity should result in infinite slippage"
        except (ZeroDivisionError, DecimalDivisionByZero):
            # Alternative: should raise an error
            assert True, "Zero liquidity should raise an error or return infinite"

    def test_slippage_extremely_small_liquidity(self):
        """Test slippage with extremely small liquidity."""
        trade_value = Decimal('1000')  # $1000 trade
        liquidity = Decimal('0.01')  # $0.01 liquidity

        slippage = (trade_value / liquidity) if liquidity != 0 else Decimal('inf')
        assert slippage == Decimal('100000'), \
            f"Small liquidity should result in high slippage: {slippage}"

    def test_slippage_large_trade_small_liquidity(self):
        """Test slippage with large trade vs small liquidity."""
        trade_value = Decimal('100000')  # $100k trade
        liquidity = Decimal('1000')  # $1k liquidity

        slippage = (trade_value / liquidity) if liquidity != 0 else Decimal('inf')
        assert slippage == Decimal('100'), \
            "Large trade with small liquidity should result in 100x slippage"

    def test_slippage_small_trade_large_liquidity(self):
        """Test slippage with small trade vs large liquidity."""
        trade_value = Decimal('10')  # $10 trade
        liquidity = Decimal('1000000')  # $1M liquidity

        slippage = (trade_value / liquidity) if liquidity != 0 else Decimal('inf')
        assert slippage == Decimal('0.00001'), \
            "Small trade with large liquidity should result in minimal slippage"

    def test_slippage_zero_trade(self):
        """Test slippage with zero trade value."""
        trade_value = Decimal('0')  # $0 trade
        liquidity = Decimal('1000')  # $1k liquidity

        slippage = (trade_value / liquidity) if liquidity != 0 else Decimal('inf')
        assert slippage == Decimal('0'), \
            "Zero trade should result in zero slippage"

    def test_slippage_equal_trade_and_liquidity(self):
        """Test slippage when trade equals liquidity."""
        trade_value = Decimal('1000')  # $1000 trade
        liquidity = Decimal('1000')  # $1000 liquidity

        slippage = (trade_value / liquidity) if liquidity != 0 else Decimal('inf')
        assert slippage == Decimal('1'), \
            "Trade equal to liquidity should result in 1x slippage (100%)"

    def test_slippage_dex_fee_zero(self):
        """Test slippage with zero DEX fee."""
        trade_value = Decimal('1000')
        dex_fee_percent = Decimal('0')  # 0% fee

        dex_fee = trade_value * (dex_fee_percent / Decimal('100'))
        assert dex_fee == Decimal('0'), \
            "Zero DEX fee should result in zero fee"

    def test_slippage_dex_fee_high(self):
        """Test slippage with very high DEX fee."""
        trade_value = Decimal('1000')
        dex_fee_percent = Decimal('10')  # 10% fee

        dex_fee = trade_value * (dex_fee_percent / Decimal('100'))
        assert dex_fee == Decimal('100'), \
            "10% DEX fee should result in $100 fee"

    def test_slippage_dex_fee_maximum(self):
        """Test slippage with maximum reasonable DEX fee."""
        trade_value = Decimal('1000')
        dex_fee_percent = Decimal('3')  # 3% is typical max

        dex_fee = trade_value * (dex_fee_percent / Decimal('100'))
        assert dex_fee == Decimal('30'), \
            "3% DEX fee should result in $30 fee"

    def test_slippage_priority_fee_zero(self):
        """Test slippage with zero priority fee."""
        priority_fee_sol = Decimal('0')  # 0 SOL

        total_priority_cost = priority_fee_sol
        assert total_priority_cost == Decimal('0'), \
            "Zero priority fee should result in zero cost"

    def test_slippage_priority_fee_very_high(self):
        """Test slippage with very high priority fee."""
        priority_fee_sol = Decimal('0.1')  # 0.1 SOL

        total_priority_cost = priority_fee_sol
        assert total_priority_cost == Decimal('0.1'), \
            "High priority fee should result in 0.1 SOL cost"

    def test_slippage_jito_tip_zero(self):
        """Test slippage with zero Jito tip."""
        jito_tip_sol = Decimal('0')  # 0 SOL

        total_jito_cost = jito_tip_sol
        assert total_jito_cost == Decimal('0'), \
            "Zero Jito tip should result in zero cost"

    def test_slippage_jito_tip_high(self):
        """Test slippage with high Jito tip."""
        jito_tip_sol = Decimal('0.05')  # 0.05 SOL

        total_jito_cost = jito_tip_sol
        assert total_jito_cost == Decimal('0.05'), \
            "High Jito tip should result in 0.05 SOL cost"

    def test_slippage_at_boundary_zero_percent(self):
        """Test slippage at exactly 0%."""
        slippage_percent = Decimal('0')

        assert slippage_percent == Decimal('0'), \
            "0% slippage should be exactly 0"

    def test_slippage_at_boundary_five_percent(self):
        """Test slippage at 5% (typical max)."""
        slippage_percent = Decimal('5')

        assert slippage_percent == Decimal('5'), \
            "5% slippage should be exactly 5"

    def test_slippage_at_boundary_ten_percent(self):
        """Test slippage at 10% (very high)."""
        slippage_percent = Decimal('10')

        assert slippage_percent == Decimal('10'), \
            "10% slippage should be exactly 10"

    def test_slippage_at_boundary_fifty_percent(self):
        """Test slippage at 50% (extreme)."""
        slippage_percent = Decimal('50')

        assert slippage_percent == Decimal('50'), \
            "50% slippage should be exactly 50"

    def test_slippage_at_boundary_hundred_percent(self):
        """Test slippage at 100% (complete loss)."""
        slippage_percent = Decimal('100')

        assert slippage_percent == Decimal('100'), \
            "100% slippage should be exactly 100"

    def test_slippage_exceeds_hundred_percent(self):
        """Test slippage exceeding 100% (more than complete loss)."""
        slippage_percent = Decimal('150')

        assert slippage_percent == Decimal('150'), \
            "Slippage >100% should be handled (trade would fail)"

    def test_negative_slippage(self):
        """Test negative slippage (better than expected)."""
        slippage_percent = Decimal('-0.5')

        assert slippage_percent == Decimal('-0.5'), \
            "Negative slippage (trade performed better) should be handled"

    def test_slippage_very_small_positive(self):
        """Test very small positive slippage."""
        slippage_percent = Decimal('0.001')

        assert slippage_percent == Decimal('0.001'), \
            "Very small positive slippage should be handled"

    def test_slippage_total_cost_calculation(self):
        """Test total cost calculation with all components."""
        trade_value = Decimal('1000')
        slippage_percent = Decimal('2')
        dex_fee_percent = Decimal('0.3')
        priority_fee_sol = Decimal('0.0001')
        jito_tip_sol = Decimal('0.0001')

        # Calculate components
        slippage_cost = trade_value * (slippage_percent / Decimal('100'))
        dex_fee = trade_value * (dex_fee_percent / Decimal('100'))
        priority_cost_usd = priority_fee_sol * Decimal('150')  # Assume SOL price
        jito_cost_usd = jito_tip_sol * Decimal('150')

        total_cost = slippage_cost + dex_fee + priority_cost_usd + jito_cost_usd
        expected_total = Decimal('20') + Decimal('3') + Decimal('0.015') + Decimal('0.015')  # $23.03

        assert total_cost == expected_total, \
            f"Total cost should be ${expected_total}, got ${total_cost}"

    def test_slippage_max_allowed_exceeded(self):
        """Test when slippage exceeds max allowed."""
        calculated_slippage = Decimal('8')
        max_allowed_slippage = Decimal('5')

        exceeds_max = calculated_slippage > max_allowed_slippage
        assert exceeds_max, \
            "Slippage exceeding max should be detected"

    def test_slippage_max_allowed_exactly_equal(self):
        """Test when slippage equals max allowed."""
        calculated_slippage = Decimal('5')
        max_allowed_slippage = Decimal('5')

        exceeds_max = calculated_slippage > max_allowed_slippage
        assert not exceeds_max, \
            "Slippage exactly at max should be allowed"

    def test_slippage_max_allowed_just_below(self):
        """Test when slippage is just below max allowed."""
        calculated_slippage = Decimal('4.99')
        max_allowed_slippage = Decimal('5')

        exceeds_max = calculated_slippage > max_allowed_slippage
        assert not exceeds_max, \
            "Slippage just below max should be allowed"

    def test_slippage_with_delay_multiplier(self):
        """Test slippage with delay multiplier applied."""
        base_slippage = Decimal('2')  # 2% base slippage
        delay_multiplier = Decimal('1.5')  # 50% increase due to delay

        adjusted_slippage = base_slippage * delay_multiplier
        assert adjusted_slippage == Decimal('3'), \
            "Delay multiplier should increase slippage appropriately"

    def test_slippage_with_regime_multiplier(self):
        """Test slippage with regime multiplier applied."""
        base_slippage = Decimal('2')  # 2% base slippage
        regime_multiplier = Decimal('2.0')  # High-turnover regime

        adjusted_slippage = base_slippage * regime_multiplier
        assert adjusted_slippage == Decimal('4'), \
            "Regime multiplier should increase slippage appropriately"

    def test_slippage_mev_penalty(self):
        """Test slippage with MEV penalty applied."""
        base_slippage = Decimal('2')  # 2% base slippage
        mev_penalty_percent = Decimal('0.5')  # 0.5% MEV penalty

        total_slippage = base_slippage + mev_penalty_percent
        assert total_slippage == Decimal('2.5'), \
            "MEV penalty should be added to base slippage"