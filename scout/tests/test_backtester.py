"""Tests for backtesting simulator."""

from datetime import datetime, timedelta
from decimal import Decimal as _D
from core.backtester import BacktestSimulator, BacktestConfig
from core.liquidity import LiquidityProvider, LiquidityData
from core.models import HistoricalTrade, TradeAction


class MockLiquidityProvider(LiquidityProvider):
    """Mock liquidity provider for testing with predefined values."""
    
    def __init__(self, liquidity_map=None, historical_liquidity_map=None):
        """
        Initialize mock provider.
        
        Args:
            liquidity_map: Dict mapping token_address -> liquidity_usd
            historical_liquidity_map: Dict mapping (token_address, timestamp) -> liquidity_usd
        """
        super().__init__()
        self.liquidity_map = liquidity_map or {}
        self.historical_liquidity_map = historical_liquidity_map or {}
        self.sol_price_usd = 150.0
    
    def get_current_liquidity(self, token_address: str):
        """Override to return predefined liquidity."""
        if token_address in self.liquidity_map:
            liquidity = self.liquidity_map[token_address]
            return LiquidityData(
                token_address=token_address,
                liquidity_usd=liquidity,
                price_usd=0.001,  # Placeholder price
                volume_24h_usd=liquidity * 0.5,
                timestamp=datetime.utcnow(),
                source="mock",
            )
        return None
    
    def get_historical_liquidity(self, token_address: str, timestamp: datetime):
        """Override to return predefined historical liquidity."""
        key = (token_address, timestamp.date())
        if key in self.historical_liquidity_map:
            liquidity = self.historical_liquidity_map[key]
            return LiquidityData(
                token_address=token_address,
                liquidity_usd=liquidity,
                price_usd=0.001,
                volume_24h_usd=liquidity * 0.5,
                timestamp=timestamp,
                source="mock_historical",
            )
        # Fallback to current liquidity if historical not found
        return self.get_current_liquidity(token_address)
    
    def get_sol_price_usd(self) -> float:
        """Return mock SOL price."""
        return self.sol_price_usd

    def get_sol_price_usd_sync(self) -> float:
        """The simulator reads the sync wrapper; return the same mock price."""
        return self.sol_price_usd


def test_backtest_simulator_initialization():
    """Test simulator can be initialized."""
    liquidity = LiquidityProvider()
    config = BacktestConfig(
        min_liquidity_shield_usd=10000.0,
        min_liquidity_spear_usd=5000.0,
    )
    
    simulator = BacktestSimulator(liquidity, config)
    
    assert simulator is not None


def test_liquidity_check():
    """Test that trades below liquidity threshold are rejected."""
    # Create mock liquidity provider with predefined values
    liquidity_map = {
        "token_high_liquidity": 50000.0,  # Above Shield threshold
        "token_low_liquidity": 3000.0,   # Below both thresholds
        "token_medium_liquidity": 8000.0, # Between thresholds
    }
    
    mock_liquidity = MockLiquidityProvider(liquidity_map=liquidity_map)
    config = BacktestConfig(
        min_liquidity_shield_usd=10000.0,
        min_liquidity_spear_usd=5000.0,
    )
    simulator = BacktestSimulator(mock_liquidity, config)
    
    # Create test trades
    high_liq_trade = HistoricalTrade(
        token_address="token_high_liquidity",
        token_symbol="HIGH",
        action=TradeAction.BUY,
        amount_sol=0.5,
        price_at_trade=0.001,
        timestamp=datetime.utcnow(),
        tx_signature="tx1",
    )
    
    low_liq_trade = HistoricalTrade(
        token_address="token_low_liquidity",
        token_symbol="LOW",
        action=TradeAction.BUY,
        amount_sol=0.5,
        price_at_trade=0.001,
        timestamp=datetime.utcnow(),
        tx_signature="tx2",
    )
    
    # Test high liquidity trade (should pass)
    sim_trade_high, rejection = simulator._simulate_trade(
        high_liq_trade,
        min_liquidity=config.min_liquidity_shield_usd,
        sol_price=150.0,
    )
    assert not sim_trade_high.rejected, "High liquidity trade should not be rejected"
    assert sim_trade_high.liquidity_sufficient, "High liquidity should be sufficient"
    
    # Test low liquidity trade (should be rejected)
    sim_trade_low, rejection = simulator._simulate_trade(
        low_liq_trade,
        min_liquidity=config.min_liquidity_shield_usd,
        sol_price=150.0,
    )
    assert sim_trade_low.rejected, "Low liquidity trade should be rejected"
    assert not sim_trade_low.liquidity_sufficient, "Low liquidity should be insufficient"
    assert "liquidity" in rejection.lower() or "Insufficient" in rejection, "Rejection should mention liquidity"


def test_slippage_estimation():
    """Test slippage calculation based on trade size vs liquidity."""
    # Create mock liquidity provider
    liquidity_map = {
        "token_small_pool": 10000.0,  # Small pool - high slippage expected
        "token_large_pool": 1000000.0,  # Large pool - low slippage expected
    }
    
    mock_liquidity = MockLiquidityProvider(liquidity_map=liquidity_map)
    config = BacktestConfig(
        min_liquidity_shield_usd=10000.0,
        min_liquidity_spear_usd=5000.0,
        max_slippage_percent=0.05,  # 5% max
    )
    simulator = BacktestSimulator(mock_liquidity, config)
    
    # Test small trade on large pool (low slippage)
    small_trade = HistoricalTrade(
        token_address="token_large_pool",
        token_symbol="LARGE",
        action=TradeAction.BUY,
        amount_sol=0.1,  # Small trade
        price_at_trade=0.001,
        timestamp=datetime.utcnow(),
        tx_signature="tx1",
    )
    
    sim_trade_small, _ = simulator._simulate_trade(
        small_trade,
        min_liquidity=config.min_liquidity_shield_usd,
        sol_price=150.0,
    )
    
    # Small trade on large pool should have low slippage
    assert sim_trade_small.estimated_slippage_percent < 0.01, "Small trade on large pool should have <1% slippage"
    
    # Test large trade on small pool (high slippage)
    large_trade = HistoricalTrade(
        token_address="token_small_pool",
        token_symbol="SMALL",
        action=TradeAction.BUY,
        amount_sol=10.0,  # Large trade
        price_at_trade=0.001,
        timestamp=datetime.utcnow(),
        tx_signature="tx2",
    )
    
    sim_trade_large, rejection = simulator._simulate_trade(
        large_trade,
        min_liquidity=config.min_liquidity_shield_usd,
        sol_price=150.0,
    )
    
    # Large trade on small pool should have high slippage or be rejected
    if sim_trade_large.rejected:
        assert "slippage" in rejection.lower() or "Slippage" in rejection, "Rejection should mention slippage"
    else:
        assert sim_trade_large.estimated_slippage_percent > sim_trade_small.estimated_slippage_percent, \
            "Large trade should have higher slippage than small trade"
    
    # Verify slippage increases with trade size
    medium_trade = HistoricalTrade(
        token_address="token_small_pool",
        token_symbol="SMALL",
        action=TradeAction.BUY,
        amount_sol=1.0,  # Medium trade
        price_at_trade=0.001,
        timestamp=datetime.utcnow(),
        tx_signature="tx3",
    )
    
    sim_trade_medium, _ = simulator._simulate_trade(
        medium_trade,
        min_liquidity=config.min_liquidity_shield_usd,
        sol_price=150.0,
    )
    
    if not sim_trade_medium.rejected and not sim_trade_large.rejected:
        assert sim_trade_small.estimated_slippage_percent < sim_trade_medium.estimated_slippage_percent < sim_trade_large.estimated_slippage_percent, \
            "Slippage should increase with trade size"


def test_historical_liquidity_validation():
    """Test that historical trades are validated against liquidity at time of trade."""
    # Create mock with historical liquidity data
    historical_liquidity_map = {
        ("token1", (datetime.utcnow() - timedelta(days=30)).date()): 20000.0,  # High liquidity 30 days ago
        ("token1", datetime.utcnow().date()): 5000.0,  # Low liquidity now
        ("token2", (datetime.utcnow() - timedelta(days=30)).date()): 3000.0,  # Low liquidity 30 days ago
        ("token2", datetime.utcnow().date()): 50000.0,  # High liquidity now
    }
    
    mock_liquidity = MockLiquidityProvider(historical_liquidity_map=historical_liquidity_map)
    config = BacktestConfig(
        min_liquidity_shield_usd=10000.0,
        min_liquidity_spear_usd=5000.0,
        enforce_current_liquidity=False,  # keep the test offline; historical gate is under test
    )
    simulator = BacktestSimulator(mock_liquidity, config)
    
    # Create historical trade that had sufficient liquidity at time of trade
    trade_with_historical_liq = HistoricalTrade(
        token_address="token1",
        token_symbol="TOKEN1",
        action=TradeAction.BUY,
        amount_sol=0.5,
        price_at_trade=0.001,
        timestamp=datetime.utcnow() - timedelta(days=30),
        tx_signature="tx1",
        liquidity_at_trade_usd=20000.0,
    )
    
    # Create historical trade that had insufficient liquidity at time of trade
    trade_without_historical_liq = HistoricalTrade(
        token_address="token2",
        token_symbol="TOKEN2",
        action=TradeAction.BUY,
        amount_sol=0.5,
        price_at_trade=0.001,
        timestamp=datetime.utcnow() - timedelta(days=30),
        tx_signature="tx2",
        liquidity_at_trade_usd=3000.0,
    )
    
    # Simulate trades
    sim_trade1, _ = simulator._simulate_trade(
        trade_with_historical_liq,
        min_liquidity=config.min_liquidity_shield_usd,
        sol_price=150.0,
    )
    
    sim_trade2, rejection2 = simulator._simulate_trade(
        trade_without_historical_liq,
        min_liquidity=config.min_liquidity_shield_usd,
        sol_price=150.0,
    )
    
    # Trade with sufficient historical liquidity should pass
    assert not sim_trade1.rejected, "Trade with sufficient historical liquidity should pass"
    assert sim_trade1.liquidity_sufficient

    # Trade without sufficient historical liquidity should be rejected
    assert sim_trade2.rejected, "Trade with insufficient historical liquidity should be rejected"
    assert "liquidity" in rejection2.lower() or "Insufficient" in rejection2, \
        "Rejection should mention liquidity"

    # A trade WITHOUT liquidity_at_trade_usd must consult the provider's
    # historical map (exercising the lookup path, not the attached field)
    trade_via_map = HistoricalTrade(
        token_address="token1",
        token_symbol="TOKEN1",
        action=TradeAction.BUY,
        amount_sol=0.5,
        price_at_trade=0.001,
        timestamp=datetime.utcnow() - timedelta(days=30),
        tx_signature="tx3",
        liquidity_at_trade_usd=None,  # forces the historical lookup
    )
    sim_trade_map, _ = simulator._simulate_trade(
        trade_via_map,
        min_liquidity=config.min_liquidity_shield_usd,
        sol_price=150.0,
    )
    # token1 had $20k historical liquidity 30 days ago (per historical_liquidity_map)
    assert not sim_trade_map.rejected, "Historical map lookup should provide sufficient liquidity"
    assert sim_trade_map.liquidity_sufficient
    
    # Test full wallet simulation with historical trades
    trades = [trade_with_historical_liq, trade_without_historical_liq]
    result = simulator.simulate_wallet("test_wallet", trades, strategy="SHIELD")
    
    # Should have rejected at least one trade due to liquidity
    assert result.rejected_trades > 0, "Should reject trades with insufficient historical liquidity"
    assert result.total_trades == 2, "Should process both trades"


# ── Financial-loss & missed-profit test suite ─────────────────────────────────


def _make_buy_trade(
    token_address: str,
    token_symbol: str,
    amount_sol: float,
    price: float,
    timestamp: datetime,
    tx: str,
    liquidity_usd: float = 50_000.0,
    pnl_sol: float = None,
) -> HistoricalTrade:
    return HistoricalTrade(
        token_address=token_address,
        token_symbol=token_symbol,
        action=TradeAction.BUY,
        amount_sol=amount_sol,
        price_at_trade=price,
        timestamp=timestamp,
        tx_signature=tx,
        liquidity_at_trade_usd=liquidity_usd,
        pnl_sol=pnl_sol,
    )


def _make_sell_trade(
    token_address: str,
    token_symbol: str,
    amount_sol: float,
    price: float,
    timestamp: datetime,
    tx: str,
    liquidity_usd: float = 50_000.0,
    pnl_sol: float = 0.0,
) -> HistoricalTrade:
    return HistoricalTrade(
        token_address=token_address,
        token_symbol=token_symbol,
        action=TradeAction.SELL,
        amount_sol=amount_sol,
        price_at_trade=price,
        timestamp=timestamp,
        tx_signature=tx,
        liquidity_at_trade_usd=liquidity_usd,
        pnl_sol=pnl_sol,
    )


def test_backtester_wallet_rejected_below_min_trades_despite_all_wins():
    """
    Test 80 (plan): A wallet with N-1 total events must be rejected if
    min_trades_required = N. The threshold applies to raw event count (BUYs + SELLs),
    blind to win rate.

    Risk: A wallet with 4 perfect round-trips (8 events) and min_trades_required=10
    is rejected — correct behavior. But this documents the quality-blind nature
    of the threshold: 10 minimum events regardless of outcome.
    """
    config = BacktestConfig(
        min_trades_required=10,
        min_liquidity_shield_usd=5_000.0,
        min_liquidity_spear_usd=2_500.0,
    )
    mock_liquidity = MockLiquidityProvider(
        liquidity_map={"token_few": 50_000.0},
    )
    simulator = BacktestSimulator(mock_liquidity, config)

    now = datetime.utcnow()
    # 3 BUY + 3 SELL = 6 total events < min_trades_required=10
    # Space them cleanly (BUYs first, SELLs after) to avoid position tracking issues
    trades = [
        _make_buy_trade("token_few", "FEW", 1.0, 1.0, now - timedelta(hours=72), "buy_0"),
        _make_buy_trade("token_few", "FEW", 1.0, 1.0, now - timedelta(hours=48), "buy_1"),
        _make_buy_trade("token_few", "FEW", 1.0, 1.0, now - timedelta(hours=24), "buy_2"),
        _make_sell_trade("token_few", "FEW", 1.0, 1.5, now - timedelta(hours=23), "sell_0", pnl_sol=0.5),
        _make_sell_trade("token_few", "FEW", 1.0, 1.5, now - timedelta(hours=22), "sell_1", pnl_sol=0.5),
        _make_sell_trade("token_few", "FEW", 1.0, 1.5, now - timedelta(hours=21), "sell_2", pnl_sol=0.5),
    ]

    result = simulator.simulate_wallet("test_wallet_count", trades, strategy="SHIELD")

    assert not result.passed, "Wallet with < min_trades_required must be rejected"
    assert result.failure_reason is not None
    assert "insufficient" in result.failure_reason.lower() or "trades" in result.failure_reason.lower(), (
        f"Failure reason must mention trade count: {result.failure_reason}"
    )


def test_backtester_second_sell_on_zero_position_returns_zero_pnl():
    """
    Test 78 (plan): Two SELL events for the same token after a single BUY.

    The second SELL has no remaining position to close. It must return zero PnL
    (not a negative number), and must NOT be silently treated as a new BUY.

    Risk: If the position tracker allows a "phantom sell" it creates a negative
    position balance, corrupting subsequent PnL calculations.
    """
    config = BacktestConfig(
        min_trades_required=1,
        min_liquidity_shield_usd=5_000.0,
        min_liquidity_spear_usd=2_500.0,
    )
    mock_liquidity = MockLiquidityProvider(
        liquidity_map={"token_double_sell": 50_000.0},
    )
    simulator = BacktestSimulator(mock_liquidity, config)

    now = datetime.utcnow()
    trades = [
        HistoricalTrade(token_address="token_double_sell", token_symbol="DS", action=TradeAction.BUY,
                        amount_sol=1.0, price_at_trade=1.0, timestamp=now - timedelta(hours=3),
                        tx_signature="buy1", token_amount=_D("1.0"), price_sol=_D("1.0")),
        HistoricalTrade(token_address="token_double_sell", token_symbol="DS", action=TradeAction.SELL,
                        amount_sol=1.0, price_at_trade=1.5, timestamp=now - timedelta(hours=2),
                        tx_signature="sell1", pnl_sol=_D("0.5"),
                        token_amount=_D("1.0"), price_sol=_D("1.5")),
        HistoricalTrade(token_address="token_double_sell", token_symbol="DS", action=TradeAction.SELL,
                        amount_sol=1.0, price_at_trade=1.5, timestamp=now - timedelta(hours=1),
                        tx_signature="sell2", pnl_sol=_D("0.3"),
                        token_amount=_D("1.0"), price_sol=_D("1.5")),
    ]

    result = simulator.simulate_wallet("wallet_double_sell", trades, strategy="SHIELD")

    assert result.total_trades == 3, "All 3 trades should be processed"
    # The second SELL has no remaining position: it must yield zero per-trade PnL
    sell_trades = [st for st in result.trades
                   if st.original_trade.tx_signature == "sell2"]
    assert sell_trades, "The second SELL must appear in the simulated trades"
    second_sell = sell_trades[0]
    assert second_sell.simulated_pnl_sol == 0, (
        f"Phantom sell must return zero PnL, got {second_sell.simulated_pnl_sol}"
    )


def test_backtester_mooned_token_inflates_backtest_when_no_historical_liquidity():
    """
    Test 81 (plan): A token that mooned AFTER the backtest period shows high current
    liquidity. When historical liquidity data is unavailable, the backtester falls back
    to current liquidity, making the trade appear valid.

    This is the survivorship bias problem: trades that should be rejected (historical
    pool was $500 — too small) pass because the fallback uses today's $5M pool.

    The mock returns current ($5M) when historical lookup fails (no date in map).
    Result: trade passes $5k liquidity threshold despite being historical illiquid.
    """
    config = BacktestConfig(
        min_trades_required=1,
        min_liquidity_shield_usd=5_000.0,
        min_liquidity_spear_usd=2_500.0,
    )

    # Token mooned: current liquidity = $5M, but historical (at trade time) was $500
    # MockLiquidityProvider falls back to current when historical key not in map
    mock_liquidity = MockLiquidityProvider(
        liquidity_map={"token_mooned": 5_000_000.0},  # Current = post-moon
        historical_liquidity_map={},  # No historical data → fallback to current
    )
    simulator = BacktestSimulator(mock_liquidity, config)

    now = datetime.utcnow()
    trade = HistoricalTrade(
        token_address="token_mooned",
        token_symbol="MOON",
        action=TradeAction.BUY,
        amount_sol=1.0,
        price_at_trade=0.0001,
        timestamp=now - timedelta(days=30),
        tx_signature="buy_moon",
        liquidity_at_trade_usd=None,  # No historical liquidity attached
        pnl_sol=None,
        token_amount=_D("0.01"),
        price_sol=_D("0.0001"),
    )
    sell = HistoricalTrade(
        token_address="token_mooned",
        token_symbol="MOON",
        action=TradeAction.SELL,
        amount_sol=1.0,
        price_at_trade=100.0,  # Mooned
        timestamp=now - timedelta(days=1),
        tx_signature="sell_moon",
        liquidity_at_trade_usd=None,
        pnl_sol=50.0,  # Huge profit from moon
        token_amount=_D("0.01"),  # 1 SOL at $100/token -> 0.01 tokens
        price_sol=_D("100.0"),
    )

    result = simulator.simulate_wallet("wallet_moon", [trade, sell], strategy="SHIELD")

    # The mooned-token scenario must NOT inflate the backtest into a pass:
    # with no historical liquidity the trade is rejected (or the wallet fails),
    # so the wallet must not be reported as profitable.
    assert not result.passed, (
        "Mooned-token wallet must not pass the backtest (survivorship bias guard)"
    )
    assert result.failure_reason is not None


def test_backtester_slippage_underestimate_large_trade_small_pool():
    """
    Test 85 (plan): For a 10 SOL trade in a $6k pool at $150/SOL ($1500 trade),
    the square-root slippage model gives ~3.5% estimated slippage.
    Real on-chain slippage for such a concentrated impact would be 5-15%.

    This test documents the known underestimate so that if the model is improved
    (e.g., to use a more realistic AMM impact model), the threshold can be updated.
    """
    config = BacktestConfig(
        min_trades_required=1,
        min_liquidity_shield_usd=5_000.0,
        min_liquidity_spear_usd=2_500.0,
        max_slippage_percent=0.05,  # 5% realistic threshold
    )
    mock_liquidity = MockLiquidityProvider(
        liquidity_map={"token_small_pool": 6_000.0},
    )
    simulator = BacktestSimulator(mock_liquidity, config)

    trade = HistoricalTrade(
        token_address="token_small_pool",
        token_symbol="SMALL",
        action=TradeAction.BUY,
        amount_sol=10.0,  # 10 SOL = $1500 at $150/SOL
        price_at_trade=0.001,
        timestamp=datetime.utcnow() - timedelta(hours=1),
        tx_signature="buy_large",
        liquidity_at_trade_usd=6_000.0,
        pnl_sol=None,
    )

    sim_trade, rejection = simulator._simulate_trade(
        trade,
        min_liquidity=config.min_liquidity_shield_usd,
        sol_price=150.0,
    )

    # A 10 SOL ($1500) trade against a $6k pool has ~25-33% pool impact, far
    # above the 5% cap, so the trade must be rejected for excessive slippage.
    assert sim_trade.rejected, (
        f"Trade must be rejected for excessive slippage. "
        f"Estimated: {float(sim_trade.estimated_slippage_percent) * 100:.1f}%"
    )
    assert "slippage" in (rejection or "").lower(), f"Rejection must mention slippage: {rejection}"


# ─── P5: Prove backtester computes positive PnL for known profitable trades ───

def test_backtester_simulated_pnl_positive_for_known_profitable_trades():
    """Prove: backtester correctly computes positive PnL for 30% gain trades.

    15 BUY/SELL pairs: buy 2.0 SOL / sell 2.6 SOL (+30% gross gain each).
    After slippage + DEX fees + priority fees, simulated PnL must remain positive.
    Proves the backtester's round-trip cashflow model gets the math right.
    """
    from decimal import Decimal

    liquidity_map = {f"profit_token_{k}": 500_000.0 for k in range(15)}
    mock_provider = MockLiquidityProvider(liquidity_map=liquidity_map)
    config = BacktestConfig(
        min_liquidity_shield_usd=10_000.0,
        min_trades_required=5,
    )
    simulator = BacktestSimulator(mock_provider, config)

    trades = []
    base_time = datetime.utcnow() - timedelta(days=30)
    for k in range(15):
        token = f"profit_token_{k}"
        trades.append(HistoricalTrade(
            token_address=token,
            token_symbol=f"PROF{k}",
            action=TradeAction.BUY,
            amount_sol=Decimal("2.0"),
            price_at_trade=Decimal("100.0"),
            timestamp=base_time + timedelta(days=2 * k),
            tx_signature=f"buy_profit_{k}",
            token_amount=_ROUND_TRIP_TOKEN_QTY,
        ))
        trades.append(HistoricalTrade(
            token_address=token,
            token_symbol=f"PROF{k}",
            action=TradeAction.SELL,
            amount_sol=Decimal("2.6"),      # +30% gross gain
            price_at_trade=Decimal("130.0"),
            timestamp=base_time + timedelta(days=2 * k, hours=4),
            tx_signature=f"sell_profit_{k}",
            token_amount=_ROUND_TRIP_TOKEN_QTY,
            pnl_sol=Decimal("0.59"),
        ))

    result = simulator.simulate_wallet("wallet_profitable", trades, strategy="SHIELD")

    assert result.simulated_pnl_sol > 0, (
        f"30% gain trades must produce positive simulated PnL. "
        f"Got: {result.simulated_pnl_sol} SOL. "
        f"Rejections: {result.rejected_trades}/{result.total_trades} — "
        f"{result.rejected_trade_details}"
    )
    assert result.passed, (
        f"Profitable trades must pass backtest. Failure: {result.failure_reason}"
    )
    assert result.rejected_trades == 0, (
        f"No trades should be rejected with $500k liquidity. "
        f"Rejected: {result.rejected_trade_details}"
    )


# ─── P6: Prove backtester detects and rejects loss-making trades ──────────────

def test_backtester_simulated_pnl_negative_for_known_losing_trades():
    """Prove: backtester correctly identifies loss-making wallets and fails them.

    15 BUY/SELL pairs: buy 2.0 SOL / sell 1.5 SOL (-25% loss each).
    After costs, simulated PnL must be negative → backtest fails → wallet rejected.
    Proves the system does not promote wallets who consistently lose money.
    """
    from decimal import Decimal

    liquidity_map = {f"loss_token_{k}": 500_000.0 for k in range(15)}
    mock_provider = MockLiquidityProvider(liquidity_map=liquidity_map)
    config = BacktestConfig(
        min_liquidity_shield_usd=10_000.0,
        min_trades_required=5,
    )
    simulator = BacktestSimulator(mock_provider, config)

    trades = []
    base_time = datetime.utcnow() - timedelta(days=30)
    for k in range(15):
        token = f"loss_token_{k}"
        trades.append(HistoricalTrade(
            token_address=token,
            token_symbol=f"LOSS{k}",
            action=TradeAction.BUY,
            amount_sol=Decimal("2.0"),
            price_at_trade=Decimal("100.0"),
            timestamp=base_time + timedelta(days=2 * k),
            tx_signature=f"buy_loss_{k}",
            token_amount=_ROUND_TRIP_TOKEN_QTY,
        ))
        trades.append(HistoricalTrade(
            token_address=token,
            token_symbol=f"LOSS{k}",
            action=TradeAction.SELL,
            amount_sol=Decimal("1.5"),      # -25% loss
            price_at_trade=Decimal("75.0"),
            timestamp=base_time + timedelta(days=2 * k, hours=4),
            tx_signature=f"sell_loss_{k}",
            token_amount=_ROUND_TRIP_TOKEN_QTY,
            pnl_sol=Decimal("-0.51"),
        ))

    result = simulator.simulate_wallet("wallet_losing", trades, strategy="SHIELD")

    assert result.simulated_pnl_sol < 0, (
        f"25% loss trades must produce negative simulated PnL. "
        f"Got: {result.simulated_pnl_sol} SOL"
    )
    assert not result.passed, (
        f"Loss-making wallet must fail backtest. "
        f"Simulated PnL: {result.simulated_pnl_sol} SOL, passed: {result.passed}"
    )


# ── Category W: Walk-forward validation ──────────────────────────────────────

# SOL price = $150. BUY 2.0 SOL at $100/token → qty = 2.0/(100/150) = 3.0 tokens.
_ROUND_TRIP_TOKEN_QTY = _D("3.0")


def _make_profitable_round_trip(token: str, idx: int, ts: datetime) -> list:
    """BUY 2 SOL at $100, SELL proceeds=2.6 SOL at $130 (+30%).
    amount_sol on SELL = proceeds in SOL (entry × price_ratio = 2.0 × 1.3 = 2.6).
    token_amount=3.0 tokens so the round-trip position ledger works correctly."""
    buy_ts = ts
    sell_ts = datetime(ts.year, ts.month, ts.day, ts.hour, ts.minute + 1, 0)
    return [
        HistoricalTrade(token_address=token, token_symbol=f"TOK{idx}", action=TradeAction.BUY,
                        amount_sol=2.0, price_at_trade=100.0, timestamp=buy_ts,
                        tx_signature=f"buy{idx}", liquidity_at_trade_usd=500_000.0,
                        token_amount=_ROUND_TRIP_TOKEN_QTY),
        HistoricalTrade(token_address=token, token_symbol=f"TOK{idx}", action=TradeAction.SELL,
                        amount_sol=2.6, price_at_trade=130.0, timestamp=sell_ts,
                        tx_signature=f"sell{idx}", liquidity_at_trade_usd=500_000.0,
                        pnl_sol=0.6, token_amount=_ROUND_TRIP_TOKEN_QTY),
    ]


def _make_losing_round_trip(token: str, idx: int, ts: datetime) -> list:
    """BUY 2 SOL at $100, SELL proceeds=1.6 SOL at $80 (-20%).
    amount_sol on SELL = proceeds in SOL (entry × price_ratio = 2.0 × 0.8 = 1.6)."""
    buy_ts = ts
    sell_ts = datetime(ts.year, ts.month, ts.day, ts.hour, ts.minute + 1, 0)
    return [
        HistoricalTrade(token_address=token, token_symbol=f"TOK{idx}", action=TradeAction.BUY,
                        amount_sol=2.0, price_at_trade=100.0, timestamp=buy_ts,
                        tx_signature=f"bloss{idx}", liquidity_at_trade_usd=500_000.0,
                        token_amount=_ROUND_TRIP_TOKEN_QTY),
        HistoricalTrade(token_address=token, token_symbol=f"TOK{idx}", action=TradeAction.SELL,
                        amount_sol=1.6, price_at_trade=80.0, timestamp=sell_ts,
                        tx_signature=f"sloss{idx}", liquidity_at_trade_usd=500_000.0,
                        pnl_sol=-0.4, token_amount=_ROUND_TRIP_TOKEN_QTY),
    ]


def _make_walk_forward_simulator():
    liquidity = MockLiquidityProvider(
        liquidity_map={f"token{i}": 500_000.0 for i in range(20)},
    )
    config = BacktestConfig(
        min_liquidity_shield_usd=10_000.0,
        min_trades_required=4,
    )
    return BacktestSimulator(liquidity, config)


def test_walk_forward_70_30_split_both_profitable():
    """W1: 30 profitable trades → train (21) and OOS (9) both pass → walk-forward passes."""
    simulator = _make_walk_forward_simulator()
    trades = []
    for i in range(15):
        ts = datetime(2024, 1, i + 1, 12, 0, 0)
        trades.extend(_make_profitable_round_trip(f"token{i}", i, ts))

    result = simulator.run_walk_forward("wallet_A", trades, strategy="SHIELD")
    assert result.passed, (
        f"30 profitable trades → walk-forward should pass. "
        f"Failure: {result.failure_reason}"
    )


def test_walk_forward_rejects_wallet_that_deteriorates_out_of_sample():
    """W2: Good in-sample, bad OOS → FAILED_WALK_FORWARD. Proves no data leakage."""
    simulator = _make_walk_forward_simulator()
    # First 14 trades (7 round-trips) = profitable in-sample
    # Last 10 trades (5 round-trips) = losing OOS
    trades = []
    for i in range(7):
        ts = datetime(2024, 1, i + 1, 12, 0, 0)
        trades.extend(_make_profitable_round_trip(f"token{i}", i, ts))
    for i in range(5):
        ts = datetime(2024, 1, 8 + i, 12, 0, 0)
        trades.extend(_make_losing_round_trip(f"token{7 + i}", 7 + i, ts))

    result = simulator.run_walk_forward("wallet_B", trades, strategy="SHIELD")
    assert not result.passed, "OOS losses must cause walk-forward rejection"
    assert result.failure_reason and "WALK_FORWARD" in result.failure_reason, (
        f"Failure reason must mention WALK_FORWARD, got: {result.failure_reason}"
    )


def test_walk_forward_minimum_test_set_size():
    """W3: Only 10 total trades → OOS set has 3 trades < min_test_trades=5 → fails."""
    simulator = _make_walk_forward_simulator()
    trades = []
    for i in range(5):
        ts = datetime(2024, 1, i + 1, 12, 0, 0)
        trades.extend(_make_profitable_round_trip(f"token{i}", i, ts))

    result = simulator.run_walk_forward("wallet_C", trades, strategy="SHIELD", min_test_trades=5)
    assert not result.passed, "Insufficient OOS data must fail walk-forward"
    assert result.failure_reason and "Insufficient test data" in result.failure_reason, (
        f"Must report insufficient test data, got: {result.failure_reason}"
    )


def test_walk_forward_train_failure_short_circuits_oos():
    """W4: Bad in-sample performance → FAILED_WALK_FORWARD_IN_SAMPLE before OOS runs."""
    simulator = _make_walk_forward_simulator()
    # All losing trades — in-sample will fail
    trades = []
    for i in range(12):
        ts = datetime(2024, 1, i + 1, 12, 0, 0)
        trades.extend(_make_losing_round_trip(f"token{i}", i, ts))

    result = simulator.run_walk_forward("wallet_D", trades, strategy="SHIELD")
    assert not result.passed, "All-losing wallet must fail walk-forward"
    assert result.failure_reason and "IN_SAMPLE" in result.failure_reason, (
        f"In-sample failure must be reported, got: {result.failure_reason}"
    )
