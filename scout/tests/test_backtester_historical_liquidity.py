"""Integration tests for backtester with historical liquidity.

S5 Fix (Survivorship Bias Prevention):
- Backtester now uses get_historical_liquidity (no fallback to current)
- Trades without historical liquidity data are REJECTED to prevent survivorship bias
- Previous fallback behavior allowed "mooned" tokens to pass incorrectly
"""

import sys
from pathlib import Path

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

from datetime import datetime, timedelta
from core.backtester import BacktestSimulator, BacktestConfig
from core.models import HistoricalTrade, TradeAction, LiquidityData
from core.liquidity import LiquidityProvider


class MockHistoricalLiquidityProvider(LiquidityProvider):
    """Mock liquidity provider with historical liquidity support.

    S5 Fix: No fallback to current liquidity - only returns historical data.
    """

    def __init__(self, historical_map=None):
        """
        Initialize with historical liquidity map.

        Args:
            historical_map: Dict mapping (token_address, date) -> liquidity_usd
        """
        super().__init__()
        self.historical_map = historical_map or {}
        self.calls_made = []

    def get_historical_liquidity(self, token_address: str, timestamp: datetime):
        """Return historical liquidity only - no fallback (S5 fix)."""
        self.calls_made.append((token_address, timestamp))

        date_key = timestamp.date()
        key = (token_address, date_key)

        if key in self.historical_map:
            liquidity = self.historical_map[key]
            return LiquidityData(
                token_address=token_address,
                liquidity_usd=liquidity,
                price_usd=0.001,
                volume_24h_usd=liquidity * 0.5,
                timestamp=timestamp,
                source="mock_historical",
            )

        return None

    def get_historical_liquidity_or_current(self, token_address: str, timestamp: datetime, strategy: str = "SHIELD"):
        """Delegate to historical lookup to preserve no-fallback test behavior."""
        return self.get_historical_liquidity(token_address, timestamp)


class TestBacktesterHistoricalLiquidity:
    """Test backtester integration with historical liquidity."""

    def test_backtester_uses_historical_liquidity(self):
        """Test that backtester uses historical liquidity at trade timestamp."""
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        trade_timestamp = datetime.utcnow() - timedelta(days=5)

        # Create historical liquidity map
        historical_map = {
            (token, trade_timestamp.date()): 50000.0,  # Historical liquidity
        }

        provider = MockHistoricalLiquidityProvider(historical_map)
        config = BacktestConfig(
            min_liquidity_shield_usd=10000.0,
            min_liquidity_spear_usd=5000.0,
            dex_fee_percent=0.003,
            max_slippage_percent=0.05,
            enforce_current_liquidity=False,  # test is offline; current-liq check not under test
        )
        simulator = BacktestSimulator(provider, config)

        # Create trade
        trade = HistoricalTrade(
            token_address=token,
            token_symbol="BONK",
            action=TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.000012,
            timestamp=trade_timestamp,
            tx_signature="tx1",
            pnl_sol=None,
        )

        # Simulate trade
        result, _ = simulator._simulate_trade(trade, 10000.0, 150.0)

        # Verify historical liquidity was used
        assert len(provider.calls_made) > 0
        call_token, call_timestamp = provider.calls_made[0]
        assert call_token == token
        assert abs((call_timestamp - trade_timestamp).total_seconds()) < 3600

        # Verify result uses historical liquidity
        assert result.current_liquidity_usd == 50000.0
        assert result.liquidity_sufficient is True

    def test_backtester_rejects_when_no_historical_liquidity(self):
        """S5 Fix: Test that backtester REJECTS trades when historical liquidity unavailable.

        This prevents survivorship bias - tokens that mooned AFTER the trade period
        should not pass backtesting just because they have high current liquidity.
        """
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        trade_timestamp = datetime.utcnow() - timedelta(days=30)

        # No historical liquidity in map - returns None
        provider = MockHistoricalLiquidityProvider()
        config = BacktestConfig(
            min_liquidity_shield_usd=10000.0,
            min_liquidity_spear_usd=5000.0,
            dex_fee_percent=0.003,
            max_slippage_percent=0.05,
            enforce_current_liquidity=False,
        )
        simulator = BacktestSimulator(provider, config)

        # Create trade
        trade = HistoricalTrade(
            token_address=token,
            token_symbol="BONK",
            action=TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.000012,
            timestamp=trade_timestamp,
            tx_signature="tx1",
            pnl_sol=None,
        )

        # Simulate trade
        result, rejection_reason = simulator._simulate_trade(trade, 10000.0, 150.0)

        # S5 FIX: Trade should be REJECTED when no historical liquidity available
        assert result.rejected is True, (
            "Trade must be rejected when historical liquidity unavailable "
            "(prevents survivorship bias from mooned tokens)"
        )
        assert "liquidity" in rejection_reason.lower() or "fetch" in rejection_reason.lower()

    def test_backtester_simulates_wallet_with_historical_liquidity(self):
        """Test full wallet simulation with historical liquidity - rejects trades without data."""
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"

        # Create historical liquidity for only some dates
        historical_map = {
            (token, (datetime.utcnow() - timedelta(days=5)).date()): 50000.0,
            (token, (datetime.utcnow() - timedelta(days=3)).date()): 60000.0,
            # Day 4 has NO historical data - should be rejected
        }

        provider = MockHistoricalLiquidityProvider(historical_map)
        config = BacktestConfig(
            min_liquidity_shield_usd=10000.0,
            min_liquidity_spear_usd=5000.0,
            dex_fee_percent=0.003,
            max_slippage_percent=0.05,
            min_trades_required=2,  # Lower threshold since some trades will be rejected
            enforce_current_liquidity=False,
        )
        simulator = BacktestSimulator(provider, config)

        # Create trades at different timestamps
        trades = [
            HistoricalTrade(
                token_address=token,
                token_symbol="BONK",
                action=TradeAction.BUY if i % 2 == 0 else TradeAction.SELL,
                amount_sol=0.5,
                price_at_trade=0.000012,
                timestamp=datetime.utcnow() - timedelta(days=5-i),
                tx_signature=f"tx{i}",
                pnl_sol=0.05 if i % 2 == 1 else None,
            )
            for i in range(5)
        ]

        # Simulate wallet
        result = simulator.simulate_wallet("test_wallet", trades, strategy="SHIELD")

        # Verify trades were processed (some rejected due to no historical data)
        assert result.total_trades == 5
        assert result.rejected_trades > 0, (
            "At least one trade should be rejected due to missing historical liquidity"
        )

        # Verify historical liquidity was checked for each trade
        assert len(provider.calls_made) >= 5

    def test_backtester_rejects_low_historical_liquidity(self):
        """Test that backtester rejects trades with insufficient historical liquidity."""
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        trade_timestamp = datetime.utcnow() - timedelta(days=5)

        # Historical liquidity below threshold
        historical_map = {
            (token, trade_timestamp.date()): 5000.0,  # Below 10000 threshold
        }

        provider = MockHistoricalLiquidityProvider(historical_map)
        config = BacktestConfig(
            min_liquidity_shield_usd=10000.0,
            min_liquidity_spear_usd=5000.0,
            dex_fee_percent=0.003,
            max_slippage_percent=0.05,
        )
        simulator = BacktestSimulator(provider, config)

        # Create trade
        trade = HistoricalTrade(
            token_address=token,
            token_symbol="BONK",
            action=TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.000012,
            timestamp=trade_timestamp,
            tx_signature="tx1",
            pnl_sol=None,
        )

        # Simulate trade
        result, rejection_reason = simulator._simulate_trade(trade, 10000.0, 150.0)

        # Verify trade was rejected
        assert result.rejected is True
        assert "liquidity" in rejection_reason.lower() or "Insufficient" in rejection_reason
