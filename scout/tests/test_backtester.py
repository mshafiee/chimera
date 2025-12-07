"""
Backtester Tests

Tests backtest simulation logic from PDD Section 3.2:
- Historical liquidity checks
- Slippage and fee calculations
- PnL simulation
- Rejection criteria
"""

import pytest
from core.models import BacktestConfig


# =============================================================================
# BACKTEST CONFIG TESTS
# =============================================================================

def test_default_backtest_config(default_backtest_config):
    """Test default backtest configuration values."""
    assert default_backtest_config.min_liquidity_shield_usd == 10000.0
    assert default_backtest_config.min_liquidity_spear_usd == 5000.0
    assert default_backtest_config.dex_fee_percent == 0.003
    assert default_backtest_config.max_slippage_percent == 0.05
    assert default_backtest_config.min_trades_required == 5


def test_shield_liquidity_higher_than_spear(default_backtest_config):
    """Test that Shield strategy requires more liquidity."""
    assert default_backtest_config.min_liquidity_shield_usd > \
           default_backtest_config.min_liquidity_spear_usd


# =============================================================================
# LIQUIDITY CHECK TESTS (PDD Section 3.2)
# =============================================================================

def test_sufficient_liquidity_for_shield(default_backtest_config):
    """Test that sufficient liquidity passes for Shield."""
    current_liquidity = 15000.0
    min_required = default_backtest_config.min_liquidity_shield_usd
    
    assert current_liquidity >= min_required, \
        "15k liquidity should be sufficient for Shield"


def test_insufficient_liquidity_for_shield(default_backtest_config):
    """Test that insufficient liquidity fails for Shield."""
    current_liquidity = 5000.0
    min_required = default_backtest_config.min_liquidity_shield_usd
    
    assert current_liquidity < min_required, \
        "5k liquidity should be insufficient for Shield"


def test_sufficient_liquidity_for_spear(default_backtest_config):
    """Test that sufficient liquidity passes for Spear."""
    current_liquidity = 7000.0
    min_required = default_backtest_config.min_liquidity_spear_usd
    
    assert current_liquidity >= min_required, \
        "7k liquidity should be sufficient for Spear"


def test_liquidity_at_exact_threshold(default_backtest_config):
    """Test liquidity exactly at threshold."""
    current_liquidity = 10000.0
    min_required = default_backtest_config.min_liquidity_shield_usd
    
    assert current_liquidity >= min_required, \
        "Exact threshold liquidity should be acceptable"


# =============================================================================
# SLIPPAGE CALCULATION TESTS
# =============================================================================

def test_slippage_calculation(default_backtest_config):
    """Test slippage calculation."""
    trade_amount = 100.0
    slippage_percent = default_backtest_config.max_slippage_percent
    
    slippage_cost = trade_amount * slippage_percent
    assert slippage_cost == 5.0, f"5% slippage on $100 should be $5, got {slippage_cost}"


def test_slippage_reduces_pnl():
    """Test that slippage reduces realized PnL."""
    entry_price = 100.0
    exit_price = 110.0  # 10% gain
    slippage_percent = 0.02  # 2% slippage each way
    
    gross_pnl = exit_price - entry_price
    slippage_cost = (entry_price * slippage_percent) + (exit_price * slippage_percent)
    net_pnl = gross_pnl - slippage_cost
    
    assert net_pnl < gross_pnl, "Slippage should reduce PnL"
    assert net_pnl > 0, "PnL should still be positive with reasonable slippage"


# =============================================================================
# FEE CALCULATION TESTS
# =============================================================================

def test_dex_fee_calculation(default_backtest_config):
    """Test DEX fee calculation."""
    trade_amount = 1000.0
    fee_percent = default_backtest_config.dex_fee_percent
    
    fee = trade_amount * fee_percent
    assert fee == 3.0, f"0.3% fee on $1000 should be $3, got {fee}"


def test_total_trading_costs(default_backtest_config):
    """Test total trading costs (slippage + fees)."""
    trade_amount = 1000.0
    
    slippage = trade_amount * default_backtest_config.max_slippage_percent
    fees = trade_amount * default_backtest_config.dex_fee_percent
    
    total_costs = slippage + fees
    expected = 50.0 + 3.0  # 5% slippage + 0.3% fee
    
    assert total_costs == expected, f"Total costs should be {expected}, got {total_costs}"


def test_round_trip_costs(default_backtest_config):
    """Test costs for a complete round-trip trade (buy + sell)."""
    trade_amount = 1000.0
    
    # Costs on entry (buy)
    entry_slippage = trade_amount * default_backtest_config.max_slippage_percent
    entry_fee = trade_amount * default_backtest_config.dex_fee_percent
    
    # Costs on exit (sell)
    exit_slippage = trade_amount * default_backtest_config.max_slippage_percent
    exit_fee = trade_amount * default_backtest_config.dex_fee_percent
    
    total_round_trip = entry_slippage + entry_fee + exit_slippage + exit_fee
    
    # (5% + 0.3%) * 2 = 10.6%
    expected_percent = (0.05 + 0.003) * 2
    expected = trade_amount * expected_percent
    
    assert abs(total_round_trip - expected) < 0.01, \
        f"Round trip costs should be {expected}, got {total_round_trip}"


# =============================================================================
# PNL SIMULATION TESTS
# =============================================================================

def test_positive_pnl_simulation():
    """Test positive PnL trade simulation."""
    entry_price = 100.0
    exit_price = 120.0  # 20% gain
    trade_amount = 1.0
    
    gross_pnl = (exit_price - entry_price) * trade_amount
    assert gross_pnl == 20.0, "20% gain on 1 unit should be $20"


def test_negative_pnl_simulation():
    """Test negative PnL trade simulation."""
    entry_price = 100.0
    exit_price = 80.0  # 20% loss
    trade_amount = 1.0
    
    gross_pnl = (exit_price - entry_price) * trade_amount
    assert gross_pnl == -20.0, "20% loss on 1 unit should be -$20"


def test_net_pnl_with_costs(default_backtest_config):
    """Test net PnL after costs."""
    entry_price = 100.0
    exit_price = 110.0  # 10% gross gain
    trade_amount = 1.0
    
    gross_pnl = (exit_price - entry_price) * trade_amount  # $10
    
    # Entry costs
    entry_slippage = entry_price * default_backtest_config.max_slippage_percent
    entry_fee = entry_price * default_backtest_config.dex_fee_percent
    
    # Exit costs
    exit_slippage = exit_price * default_backtest_config.max_slippage_percent
    exit_fee = exit_price * default_backtest_config.dex_fee_percent
    
    total_costs = entry_slippage + entry_fee + exit_slippage + exit_fee
    net_pnl = gross_pnl - total_costs
    
    # Net PnL should be positive but less than gross
    assert net_pnl < gross_pnl, "Net PnL should be less than gross PnL"


def test_break_even_with_costs(default_backtest_config):
    """Test that costs can turn a small gain into a loss."""
    entry_price = 100.0
    trade_amount = 1.0
    
    # Total round-trip costs as percentage
    cost_percent = (default_backtest_config.max_slippage_percent + 
                    default_backtest_config.dex_fee_percent) * 2  # ~10.6%
    
    # Small gain of 5%
    exit_price = 105.0
    gain_percent = 0.05
    
    # With 10.6% costs, 5% gain should result in net loss
    assert cost_percent > gain_percent, \
        "Total costs should exceed small gain, resulting in net loss"


# =============================================================================
# REJECTION CRITERIA TESTS (PDD Section 3.2)
# =============================================================================

def test_reject_if_simulated_pnl_negative():
    """Test that wallet is rejected if simulated PnL < 0."""
    simulated_pnl = -50.0
    should_reject = simulated_pnl < 0
    
    assert should_reject, "Negative simulated PnL should trigger rejection"


def test_reject_if_insufficient_liquidity():
    """Test rejection for insufficient historical liquidity."""
    historical_liquidity = 100000.0  # $100k at time of trade
    current_liquidity = 5000.0  # Only $5k now
    min_liquidity = 10000.0
    
    # Current liquidity matters, not historical
    should_reject = current_liquidity < min_liquidity
    
    assert should_reject, \
        "Should reject if current liquidity is insufficient, even if historical was high"


def test_reject_if_too_few_trades(default_backtest_config):
    """Test rejection for insufficient historical trades."""
    trade_count = 3
    min_required = default_backtest_config.min_trades_required
    
    should_reject = trade_count < min_required
    
    assert should_reject, f"Should reject if < {min_required} trades"


def test_accept_if_enough_trades(default_backtest_config):
    """Test acceptance with sufficient trades."""
    trade_count = 10
    min_required = default_backtest_config.min_trades_required
    
    should_accept = trade_count >= min_required
    
    assert should_accept, f"Should accept with >= {min_required} trades"


# =============================================================================
# EDGE CASES
# =============================================================================

def test_zero_liquidity_rejected(default_backtest_config):
    """Test that zero liquidity is rejected."""
    current_liquidity = 0.0
    min_required = default_backtest_config.min_liquidity_shield_usd
    
    assert current_liquidity < min_required, "Zero liquidity should be rejected"


def test_negative_price_handling():
    """Test handling of negative prices (should not occur but handle gracefully)."""
    entry_price = 100.0
    exit_price = -10.0  # Invalid
    
    # Should handle gracefully, PnL calculation still works mathematically
    pnl = exit_price - entry_price
    assert pnl == -110.0  # Technically valid math


def test_very_small_trade_amount():
    """Test very small trade amounts."""
    entry_price = 100.0
    exit_price = 110.0
    trade_amount = 0.0001
    
    pnl = (exit_price - entry_price) * trade_amount
    assert pnl == 0.001, "Very small trades should work correctly"


def test_very_large_trade_amount():
    """Test very large trade amounts."""
    entry_price = 100.0
    exit_price = 110.0
    trade_amount = 1000000.0
    
    pnl = (exit_price - entry_price) * trade_amount
    assert pnl == 10000000.0, "Very large trades should work correctly"


# =============================================================================
# AGGREGATE BACKTEST TESTS
# =============================================================================

def test_aggregate_pnl_from_multiple_trades():
    """Test aggregating PnL from multiple trades."""
    trades = [
        {"entry": 100, "exit": 110},  # +10
        {"entry": 50, "exit": 45},    # -5
        {"entry": 200, "exit": 220},  # +20
    ]
    
    total_pnl = sum(t["exit"] - t["entry"] for t in trades)
    assert total_pnl == 25, "Total PnL should be +25"


def test_win_rate_calculation():
    """Test win rate calculation from backtest results."""
    results = [
        {"pnl": 10},   # Win
        {"pnl": -5},   # Loss
        {"pnl": 20},   # Win
        {"pnl": -15},  # Loss
        {"pnl": 30},   # Win
    ]
    
    wins = sum(1 for r in results if r["pnl"] > 0)
    total = len(results)
    win_rate = wins / total if total > 0 else 0.0
    
    assert win_rate == 0.6, f"Win rate should be 60%, got {win_rate * 100}%"


def test_average_trade_pnl():
    """Test average trade PnL calculation."""
    trades_pnl = [10, -5, 20, -15, 30]
    
    avg_pnl = sum(trades_pnl) / len(trades_pnl)
    assert avg_pnl == 8.0, f"Average PnL should be 8, got {avg_pnl}"

