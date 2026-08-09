"""
Coverage tests for core/backtester.py.

Covers the round-trip simulator branches not exercised by test_backtester.py:
time-decay weights, low-confidence liquidity exclusion, current-liquidity
gating in real mode, derived SOL prices, oversells, and walk-forward.
"""

from datetime import datetime, timedelta, timezone
from decimal import Decimal


from core.backtester import BacktestSimulator
from core.models import (
    BacktestConfig,
    HistoricalTrade,
    LiquidityData,
    TradeAction,
)


class FakeLiquidity:
    """Provider with controllable historical/current liquidity."""

    def __init__(self, historical=None, current=None, slippage=0.01,
                 vol_ratio=1.0, mode="simulated", source="real"):
        self.historical = historical or {}
        self.current = current or {}
        self.slippage = slippage
        self.vol_ratio = vol_ratio
        self.mode = mode
        self.source = source
        self.sol_price = 150.0

    def get_sol_price_usd_sync(self):
        return self.sol_price

    def get_historical_liquidity_or_current(self, token, ts):
        if token in self.historical:
            data = self.historical[token]
            return LiquidityData(
                token_address=token,
                liquidity_usd=Decimal(str(data["liq"])),
                price_usd=Decimal('0.001'),
                volume_24h_usd=Decimal(str(data.get("vol", data["liq"] * self.vol_ratio))),
                timestamp=ts,
                source=self.source,
            )
        return None

    def get_current_liquidity(self, token):
        if token in self.current:
            return LiquidityData(
                token_address=token,
                liquidity_usd=Decimal(str(self.current[token])),
                price_usd=Decimal('0.001'),
                volume_24h_usd=Decimal('1000'),
                timestamp=datetime.now(timezone.utc),
                source="current",
            )
        return None

    def estimate_slippage(self, *args, **kwargs):
        return self.slippage

    def classify_market_regime(self, start, end):
        return "BULL"


class _FakeTs:
    """Timestamp stand-in whose subtraction raises (decay error branch)."""

    tzinfo = None

    def __sub__(self, other):
        raise TypeError("not a real datetime")

    def __lt__(self, other):
        return False

    def replace(self, *args, **kwargs):
        return self

    def timestamp(self):
        return 1000000000.0


def _config(**kwargs):
    defaults = dict(
        min_liquidity_shield_usd=Decimal('10000'),
        min_liquidity_spear_usd=Decimal('5000'),
        dex_fee_percent=Decimal('0.003'),
        max_slippage_percent=Decimal('0.05'),
        min_trades_required=1,
        priority_fee_sol_per_trade=Decimal('0.00005'),
        jito_tip_sol_per_trade=Decimal('0.0001'),
        entry_delay_slippage_pct=Decimal('0.015'),
        exit_delay_slippage_pct=Decimal('0.010'),
        mev_penalty_pct=Decimal('0.002'),
    )
    defaults.update(kwargs)
    return BacktestConfig(**defaults)


def _trade(token="TOKEN1", action=TradeAction.BUY, amount='1', price_sol='1',
           token_amount='10', ts=None, pnl=None, ts_naive=False):
    if ts is None:
        ts = datetime.now(timezone.utc) - timedelta(days=1)
    return HistoricalTrade(
        token_address=token,
        token_symbol="TKN",
        action=action,
        amount_sol=Decimal(amount),
        price_at_trade=Decimal(price_sol),
        timestamp=ts,
        tx_signature="sig",
        token_amount=Decimal(token_amount) if token_amount is not None else None,
        price_sol=Decimal(price_sol) if price_sol is not None else None,
        price_usd=Decimal(price_sol) * Decimal('150') if price_sol is not None else None,
        pnl_sol=Decimal(pnl) if pnl is not None else None,
    )


def test_simulate_empty_trades():
    sim = BacktestSimulator(FakeLiquidity(), _config())
    result = sim.simulate_wallet("wallet1", [])
    assert result.passed is False
    assert result.failure_reason == "No trades to simulate"


def _roundtrip_trades(liq=50000, token="TOKEN1", ts=None):
    ts = ts or (datetime.now(timezone.utc) - timedelta(days=1))
    buy = HistoricalTrade(
        token_address=token, token_symbol="TKN", action=TradeAction.BUY,
        amount_sol=Decimal('1'), price_at_trade=Decimal('1'), timestamp=ts,
        tx_signature="sig1", token_amount=Decimal('10'), price_sol=Decimal('1'),
        price_usd=Decimal('150'),
    )
    sell = HistoricalTrade(
        token_address=token, token_symbol="TKN", action=TradeAction.SELL,
        amount_sol=Decimal('1.5'), price_at_trade=Decimal('1.1'), timestamp=ts,
        tx_signature="sig2", token_amount=Decimal('10'), price_sol=Decimal('1.1'),
        price_usd=Decimal('165'), pnl_sol=Decimal('0.1'),
    )
    return [buy, sell]


def test_decay_weights_naive_timestamps():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    sim = BacktestSimulator(provider, _config(backtest_time_decay_enabled=True))
    trades = _roundtrip_trades()
    trades[0].timestamp = datetime.now(timezone.utc).replace(tzinfo=None) - timedelta(days=5)
    trades[1].timestamp = datetime.now(timezone.utc).replace(tzinfo=None) - timedelta(days=5)
    result = sim.simulate_wallet("wallet1", trades)
    assert result.simulated_trades == 2


def test_decay_weights_aware_timestamps():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    sim = BacktestSimulator(provider, _config(backtest_time_decay_enabled=True))
    result = sim.simulate_wallet("wallet1", _roundtrip_trades())
    assert result.simulated_trades == 2


def test_decay_weight_error_uses_one():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    sim = BacktestSimulator(provider, _config(backtest_time_decay_enabled=True))
    trades = _roundtrip_trades(ts=_FakeTs())
    result = sim.simulate_wallet("wallet1", trades)
    assert result.simulated_trades == 2


def test_no_closed_sells_fails():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    sim = BacktestSimulator(provider, _config())
    buy = _trade()
    result = sim.simulate_wallet("wallet1", [buy])
    assert result.passed is False
    assert "No closed SELL" in result.failure_reason


def test_low_confidence_all_sells_fail():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}}, source="fallback")
    sim = BacktestSimulator(provider, _config())
    result = sim.simulate_wallet("wallet1", _roundtrip_trades())
    assert result.passed is False
    assert "survivorship bias" in result.failure_reason


def test_low_confidence_ratio_fail():
    provider = FakeLiquidity(
        historical={"TOKEN1": {"liq": 50000}, "TOKEN2": {"liq": 50000}},
        source="fallback",
    )
    sim = BacktestSimulator(provider, _config())
    ts1 = datetime.now(timezone.utc) - timedelta(days=2)
    ts2 = datetime.now(timezone.utc) - timedelta(days=1)
    trades = _roundtrip_trades(token="TOKEN1", ts=ts1) + _roundtrip_trades(token="TOKEN2", ts=ts2)
    # Force a pre-existing failure reason too (negative simulated pnl)
    provider2 = FakeLiquidity(
        historical={"TOKEN1": {"liq": 50000}, "TOKEN2": {"liq": 50000}},
        source="real", slippage=0.5,
    )
    sim2 = BacktestSimulator(provider2, _config())
    result2 = sim2.simulate_wallet("wallet1", trades)
    assert result2.passed is False
    # Now with fallback source: 2 of 2 trades low confidence
    result = sim.simulate_wallet("wallet1", trades)
    assert result.passed is False
    assert "survivorship bias" in result.failure_reason


def test_current_liquidity_missing_rejects():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}}, current={}, mode="real")
    sim = BacktestSimulator(provider, _config(enforce_current_liquidity=True))
    result = sim.simulate_wallet("wallet1", _roundtrip_trades())
    assert result.rejected_trades >= 1
    assert any("current liquidity" in d for d in result.rejected_trade_details)


def test_current_liquidity_too_low_rejects():
    provider = FakeLiquidity(
        historical={"TOKEN1": {"liq": 50000}}, current={"TOKEN1": 1000}, mode="real"
    )
    sim = BacktestSimulator(provider, _config(enforce_current_liquidity=True))
    result = sim.simulate_wallet("wallet1", _roundtrip_trades())
    assert result.rejected_trades >= 1
    assert any("Insufficient current liquidity" in d for d in result.rejected_trade_details)


def test_invalid_trade_size_rejects():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    sim = BacktestSimulator(provider, _config())
    trade = _trade(amount='0')
    result = sim.simulate_wallet("wallet1", [trade])
    assert result.rejected_trades == 1
    assert "Invalid trade size" in result.rejected_trade_details[0]


def test_simulate_at_size_sol_capped():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    config = _config(simulate_at_size_sol=Decimal('5'))  # bigger than 1 SOL trade
    sim = BacktestSimulator(provider, config)
    result = sim.simulate_wallet("wallet1", _roundtrip_trades())
    assert result.simulated_trades == 2


def test_simulate_at_size_sol_float_input():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    config = _config(simulate_at_size_sol=0.5)
    sim = BacktestSimulator(provider, config)
    result = sim.simulate_wallet("wallet1", _roundtrip_trades())
    assert result.simulated_trades == 2


def test_derived_sol_price_and_cache():
    ts = datetime.now(timezone.utc) - timedelta(days=1)
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    sim = BacktestSimulator(provider, _config())
    # Two buys in the same hour: second uses the hour cache
    buy1 = _trade(token="TOKEN1", ts=ts)
    buy2 = _trade(token="TOKEN1", ts=ts + timedelta(minutes=30))
    result = sim.simulate_wallet("wallet1", [buy1, buy2])
    assert result.simulated_trades == 2


def test_derived_sol_price_zero_falls_back():
    ts = datetime.now(timezone.utc) - timedelta(days=1)
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    sim = BacktestSimulator(provider, _config())
    # price_sol None -> no derivation path, uses current price
    buy = HistoricalTrade(
        token_address="TOKEN1", token_symbol="TKN", action=TradeAction.BUY,
        amount_sol=Decimal('1'), price_at_trade=Decimal('1'), timestamp=ts,
        tx_signature="s", token_amount=Decimal('10'), price_sol=None, price_usd=None,
    )
    result = sim.simulate_wallet("wallet1", [buy])
    assert result.total_trades == 1


def test_token_age_computed_with_real_creation():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    orig_get = provider.get_historical_liquidity_or_current

    def patched_get(token, ts):
        data = orig_get(token, ts)
        data.token_creation_timestamp = ts - timedelta(days=60)
        return data

    provider.get_historical_liquidity_or_current = patched_get
    sim = BacktestSimulator(provider, _config())
    result = sim.simulate_wallet("wallet1", _roundtrip_trades())
    assert result.simulated_trades == 2


def test_token_age_parse_error_tolerated():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    # token_creation_timestamp as a str -> total_seconds raises TypeError
    provider.historical["TOKEN1"] = {"liq": 50000, "vol": 50000}
    orig_get = provider.get_historical_liquidity_or_current

    def patched_get(token, ts):
        data = orig_get(token, ts)
        data.token_creation_timestamp = "not-a-datetime"
        return data

    provider.get_historical_liquidity_or_current = patched_get
    sim = BacktestSimulator(provider, _config())
    result = sim.simulate_wallet("wallet1", _roundtrip_trades())
    assert result.simulated_trades == 2


def test_all_sells_low_conf_no_prior_failure():
    """all_sells_low_conf fires with failure_reason still None: a real-source
    SELL (no trade-attached liquidity) is counted as closed, while a separate
    fallback-source trade raises the low-confidence count."""
    ts1 = datetime.now(timezone.utc) - timedelta(days=2)
    ts2 = datetime.now(timezone.utc) - timedelta(days=1)
    provider = FakeLiquidity(
        historical={"TOKEN1": {"liq": 50000}, "TOKEN2": {"liq": 50000}},
        source="real",
    )
    orig_get = provider.get_historical_liquidity_or_current

    def patched_get(token, ts):
        data = orig_get(token, ts)
        if token == "TOKEN2":
            data.source = "fallback"
        return data

    provider.get_historical_liquidity_or_current = patched_get
    sim = BacktestSimulator(provider, _config())
    trades = _roundtrip_trades(token="TOKEN1", ts=ts1)
    trades.append(_trade(token="TOKEN2", ts=ts2))
    result = sim.simulate_wallet("wallet1", trades)
    assert result.passed is False
    assert result.failure_reason == (
        "All SELL trades used low-confidence liquidity (survivorship bias)"
    )


def test_low_confidence_ratio_branch():
    """Not all sells low-confidence, but the ratio exceeds the threshold."""
    ts1 = datetime.now(timezone.utc) - timedelta(days=2)
    ts2 = datetime.now(timezone.utc) - timedelta(days=1)
    provider = FakeLiquidity(
        historical={"TOKEN1": {"liq": 50000}, "TOKEN2": {"liq": 50000}},
        source="real",
    )
    orig_get = provider.get_historical_liquidity_or_current

    def patched_get(token, ts):
        data = orig_get(token, ts)
        if token == "TOKEN1":
            data.source = "fallback"
        return data

    provider.get_historical_liquidity_or_current = patched_get
    sim = BacktestSimulator(provider, _config())
    trades = _roundtrip_trades(token="TOKEN1", ts=ts1) + _roundtrip_trades(token="TOKEN2", ts=ts2)
    # TOKEN2's trades carry trade-attached liquidity, so not ALL sells are
    # low-confidence — the ratio branch (2/4 = 50% > 15%) must fire instead.
    for t in trades:
        if t.token_address == "TOKEN2":
            t.liquidity_at_trade_usd = Decimal('50000')
    result = sim.simulate_wallet("wallet1", trades)
    assert result.passed is False
    assert "survivorship bias risk" in result.failure_reason


def test_low_confidence_all_sells_fail_with_existing_reason():
    # 3 low-liquidity tokens are rejected (60% rejection -> prior failure),
    # the accepted roundtrip uses fallback liquidity -> all-sells bias appends
    ts = datetime.now(timezone.utc) - timedelta(days=1)
    provider = FakeLiquidity(
        historical={f"LOW{i}": {"liq": 100} for i in range(3)}
        | {"OK": {"liq": 50000}},
        source="fallback",
    )
    sim = BacktestSimulator(provider, _config())
    trades = [_trade(token=f"LOW{i}", ts=ts) for i in range(3)]
    trades += _roundtrip_trades(token="OK", ts=ts)
    result = sim.simulate_wallet("wallet1", trades)
    assert result.passed is False
    assert "Too many trades rejected" in result.failure_reason
    assert "survivorship bias" in result.failure_reason


def test_low_confidence_ratio_with_existing_reason():
    ts1 = datetime.now(timezone.utc) - timedelta(days=2)
    ts2 = datetime.now(timezone.utc) - timedelta(days=1)
    provider = FakeLiquidity(
        historical={"TOKEN1": {"liq": 50000}, "TOKEN2": {"liq": 50000}},
        source="real",
    )
    orig_get = provider.get_historical_liquidity_or_current

    def patched_get(token, ts):
        data = orig_get(token, ts)
        if token == "TOKEN1":
            data.source = "fallback"
        return data

    provider.get_historical_liquidity_or_current = patched_get
    sim = BacktestSimulator(provider, _config())
    trades = _roundtrip_trades(token="TOKEN1", ts=ts1) + _roundtrip_trades(token="TOKEN2", ts=ts2)
    for t in trades:
        if t.token_address == "TOKEN2":
            t.liquidity_at_trade_usd = Decimal('50000')
    # Negative PnL failure fires first; the bias message must be appended
    for t in trades:
        t.amount_sol = Decimal('0.5')
    result = sim.simulate_wallet("wallet1", trades)
    assert result.passed is False
    assert "Negative simulated realized PnL" in result.failure_reason
    assert "survivorship bias risk" in result.failure_reason


def test_medium_turnover_multiplier():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}}, vol_ratio=5)
    sim = BacktestSimulator(provider, _config())
    result = sim.simulate_wallet("wallet1", _roundtrip_trades())
    assert result.simulated_trades == 2


def test_high_turnover_multiplier():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}}, vol_ratio=50)
    sim = BacktestSimulator(provider, _config())
    result = sim.simulate_wallet("wallet1", _roundtrip_trades())
    assert result.simulated_trades == 2


def test_buy_qty_estimated_from_price():
    ts = datetime.now(timezone.utc) - timedelta(days=1)
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    sim = BacktestSimulator(provider, _config())
    buy = _trade(token_amount=None, ts=ts)
    sell = _trade(action=TradeAction.SELL, token_amount=None, ts=ts,
                  amount='1', price_sol='1.1', pnl='0.1')
    result = sim.simulate_wallet("wallet1", [buy, sell])
    assert result.simulated_trades == 2


def test_sell_without_position_rejects():
    ts = datetime.now(timezone.utc) - timedelta(days=1)
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    sim = BacktestSimulator(provider, _config())
    sell = _trade(action=TradeAction.SELL, ts=ts)
    result = sim.simulate_wallet("wallet1", [sell])
    assert result.rejected_trades == 1
    assert "SELL without tracked position" in result.rejected_trade_details[0]


def test_oversell_warning():
    ts = datetime.now(timezone.utc) - timedelta(days=1)
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    sim = BacktestSimulator(provider, _config())
    buy = _trade(token_amount='5', ts=ts)
    sell = _trade(action=TradeAction.SELL, token_amount='10', ts=ts, pnl='0.1')
    result = sim.simulate_wallet("wallet1", [buy, sell])
    # Oversell prorates; both trades accepted (BUY + prorated SELL)
    assert result.rejected_trades == 0
    assert result.simulated_trades == 2


def test_walk_forward_empty():
    sim = BacktestSimulator(FakeLiquidity(), _config())
    result = sim.run_walk_forward("wallet1", [])
    assert result.passed is False
    assert result.failure_reason == "No trades to simulate"


def test_walk_forward_success():
    provider = FakeLiquidity(
        historical={f"TOKEN{i}": {"liq": 50000} for i in range(6)}
    )
    sim = BacktestSimulator(provider, _config())
    trades = []
    for i in range(6):
        day = datetime.now(timezone.utc) - timedelta(days=10 - i)
        trades.extend(_roundtrip_trades(ts=day, token=f"TOKEN{i}"))
    result = sim.run_walk_forward("wallet1", trades, holdout_fraction=0.3,
                                  min_test_trades=1)
    assert result.total_trades == 12


def test_walk_forward_insufficient_test_trades():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    sim = BacktestSimulator(provider, _config())
    trades = _roundtrip_trades()
    result = sim.run_walk_forward("wallet1", trades, holdout_fraction=0.1,
                                  min_test_trades=5)
    assert result.passed is False
    assert "Insufficient test data" in result.failure_reason


def test_legacy_simulate_trade():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 50000}})
    sim = BacktestSimulator(provider, _config())
    trade = _trade(token_amount='10')
    sim_trade, reason = sim._simulate_trade(trade, 10000.0, 150.0)
    assert reason is None
    assert sim_trade.rejected is False


def test_rejected_trade_tracks_original_pnl():
    provider = FakeLiquidity(historical={"TOKEN1": {"liq": 100}})
    sim = BacktestSimulator(provider, _config(min_liquidity_shield_usd=Decimal('10000')))
    ts = datetime.now(timezone.utc) - timedelta(days=1)
    sell = _trade(action=TradeAction.SELL, ts=ts, pnl='0.5')
    result = sim.simulate_wallet("wallet1", [sell])
    assert result.rejected_trades == 1
    assert result.original_pnl_sol == Decimal('0.5')
