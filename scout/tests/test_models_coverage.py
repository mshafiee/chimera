"""
Coverage tests for core/models.py.

Covers the remaining branches: HistoricalTrade action coercion, SimulatedResult
pnl_reduction_percent, BacktestConfig.get_min_liquidity and __post_init__
Decimal coercion.
"""

from datetime import datetime, timezone
from decimal import Decimal

from core.models import (
    BacktestConfig,
    HistoricalTrade,
    LiquidityData,
    SimulatedResult,
    TradeAction,
    TraderArchetype,
    ValidationResult,
    ValidationStatus,
    WalletRecord,
)


def _trade(action=TradeAction.BUY):
    return HistoricalTrade(
        token_address="TOKEN1",
        token_symbol="TKN",
        action=action,
        amount_sol=Decimal('1'),
        price_at_trade=Decimal('1'),
        timestamp=datetime.now(timezone.utc),
        tx_signature="sig1",
    )


def test_historical_trade_string_action_coerced():
    trade = _trade(action="sell")
    assert trade.action == TradeAction.SELL
    trade2 = _trade(action="buy")
    assert trade2.action == TradeAction.BUY


def test_pnl_reduction_percent_positive():
    result = SimulatedResult(
        wallet_address="w",
        total_trades=2,
        simulated_trades=2,
        rejected_trades=0,
        original_pnl_sol=Decimal('100'),
        simulated_pnl_sol=Decimal('70'),
        pnl_difference_sol=Decimal('30'),
        total_slippage_cost_sol=Decimal('0'),
        total_fee_cost_sol=Decimal('0'),
    )
    assert abs(result.pnl_reduction_percent - 30.0) < 1e-9


def test_pnl_reduction_percent_non_positive_original():
    result = SimulatedResult(
        wallet_address="w",
        total_trades=1,
        simulated_trades=1,
        rejected_trades=0,
        original_pnl_sol=Decimal('0'),
        simulated_pnl_sol=Decimal('-5'),
        pnl_difference_sol=Decimal('5'),
        total_slippage_cost_sol=Decimal('0'),
        total_fee_cost_sol=Decimal('0'),
    )
    assert result.pnl_reduction_percent == 0.0


def test_get_min_liquidity_strategies():
    config = BacktestConfig()
    assert config.get_min_liquidity("SHIELD") == Decimal('10000.0')
    assert config.get_min_liquidity("spear") == Decimal('5000.0')
    assert config.get_min_liquidity("UNKNOWN") == Decimal('10000.0')


def test_backtest_config_coerces_floats_to_decimal():
    config = BacktestConfig(
        min_liquidity_shield_usd=12345.67,
        min_liquidity_spear_usd=5000.5,
        dex_fee_percent=0.003,
        max_slippage_percent=0.05,
        priority_fee_sol_per_trade=0.00005,
        jito_tip_sol_per_trade=0.0001,
        entry_delay_slippage_pct=0.015,
        exit_delay_slippage_pct=0.010,
        mev_penalty_pct=0.002,
        shield_multiplier=1.0,
        spear_multiplier=1.5,
        simulate_at_size_sol=2.0,
    )
    assert isinstance(config.min_liquidity_shield_usd, Decimal)
    assert config.min_liquidity_shield_usd == Decimal('12345.67')
    assert isinstance(config.simulate_at_size_sol, Decimal)
    assert isinstance(config.dex_fee_percent, Decimal)


def test_backtest_config_keeps_decimal_inputs():
    config = BacktestConfig(
        min_liquidity_shield_usd=Decimal('999.5'),
        simulate_at_size_sol=None,
    )
    assert config.min_liquidity_shield_usd == Decimal('999.5')
    assert config.simulate_at_size_sol is None


def test_enums_and_dataclass_defaults():
    assert TradeAction.BUY.value == "BUY"
    assert TraderArchetype.WHALE.value == "WHALE"
    assert ValidationStatus.PASSED.value == "PASSED"
    record = WalletRecord(
        address="w", status="ACTIVE", wqs_score=80.0, roi_7d=1.0,
        roi_30d=2.0, trade_count_30d=10, win_rate=0.5,
        max_drawdown_30d=5.0, avg_trade_size_sol=Decimal('1'),
    )
    assert record.created_at
    assert record.notes is None


def test_validation_result_defaults():
    result = ValidationResult(wallet_address="w", status=ValidationStatus.PASSED)
    assert result.validated_at is not None
    assert result.recommended_status == "CANDIDATE"
    assert result.passed is False


def test_liquidity_data_defaults():
    data = LiquidityData(
        token_address="T",
        liquidity_usd=Decimal('1'),
        price_usd=Decimal('1'),
        volume_24h_usd=Decimal('1'),
        timestamp=datetime.now(timezone.utc),
    )
    assert data.source == "unknown"
    assert data.token_creation_timestamp is None
