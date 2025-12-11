"""Integration tests for backtester with historical liquidity."""

import sys
from pathlib import Path

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

import pytest
from datetime import datetime, timedelta
from core.backtester import BacktestSimulator, BacktestConfig
from core.models import HistoricalTrade, TradeAction, LiquidityData
from core.liquidity import LiquidityProvider


class MockHistoricalLiquidityProvider(LiquidityProvider):
    """Mock liquidity provider with historical liquidity support."""
    
    def __init__(self, historical_map=None):
        """
        Initialize with historical liquidity map.
        
        Args:
            historical_map: Dict mapping (token_address, date) -> liquidity_usd
        """
        super().__init__()
        self.historical_map = historical_map or {}
        self.calls_made = []
    
    def get_historical_liquidity_or_current(self, token_address: str, timestamp: datetime):
        """Override to return historical liquidity."""
        self.calls_made.append((token_address, timestamp))
        
        # Check historical map
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
        
        # Fallback to current
        return LiquidityData(
            token_address=token_address,
            liquidity_usd=100000.0,
            price_usd=0.001,
            volume_24h_usd=50000.0,
            timestamp=timestamp,
            source="mock_fallback",
        )


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
    
    def test_backtester_fallback_to_current_liquidity(self):
        """Test that backtester falls back to current liquidity when historical unavailable."""
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        trade_timestamp = datetime.utcnow() - timedelta(days=30)
        
        # No historical liquidity in map
        provider = MockHistoricalLiquidityProvider()
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
        result, _ = simulator._simulate_trade(trade, 10000.0, 150.0)
        
        # Verify fallback was used
        assert result.current_liquidity_usd == 100000.0  # Fallback value
        assert "fallback" in result.current_liquidity_usd or result.liquidity_sufficient is True
    
    def test_backtester_simulates_wallet_with_historical_liquidity(self):
        """Test full wallet simulation with historical liquidity."""
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        
        # Create historical liquidity for different dates
        historical_map = {
            (token, (datetime.utcnow() - timedelta(days=5)).date()): 50000.0,
            (token, (datetime.utcnow() - timedelta(days=4)).date()): 55000.0,
            (token, (datetime.utcnow() - timedelta(days=3)).date()): 60000.0,
        }
        
        provider = MockHistoricalLiquidityProvider(historical_map)
        config = BacktestConfig(
            min_liquidity_shield_usd=10000.0,
            min_liquidity_spear_usd=5000.0,
            dex_fee_percent=0.003,
            max_slippage_percent=0.05,
            min_trades_required=3,
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
            for i in range(6)
        ]
        
        # Simulate wallet
        result = simulator.simulate_wallet("test_wallet", trades, strategy="SHIELD")
        
        # Verify all trades were processed
        assert result.total_trades == 6
        assert result.simulated_trades > 0
        
        # Verify historical liquidity was used for each trade
        assert len(provider.calls_made) >= 6
    
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


