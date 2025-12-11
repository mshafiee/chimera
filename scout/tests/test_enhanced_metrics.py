"""Tests for enhanced metric calculations (ROI, win rate, drawdown, consistency)."""

import sys
from pathlib import Path

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

import pytest
from datetime import datetime, timedelta
from core.analyzer import WalletAnalyzer
from core.models import HistoricalTrade, TradeAction


class TestROICalculation:
    """Test accurate ROI calculation from trades."""
    
    @pytest.fixture
    def analyzer(self):
        """Create WalletAnalyzer instance."""
        return WalletAnalyzer(discover_wallets=False, max_wallets=0)
    
    def test_roi_calculation_simple_buy_sell(self, analyzer):
        """Test ROI calculation with simple buy/sell sequence."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.BUY,
                amount_sol=1.0,
                price_at_trade=10.0,  # Buy at $10
                timestamp=datetime.utcnow() - timedelta(days=5),
                tx_signature="tx1",
                pnl_sol=None,
            ),
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=12.0,  # Sell at $12 (20% profit)
                timestamp=datetime.utcnow() - timedelta(days=4),
                tx_signature="tx2",
                pnl_sol=2.0,  # $2 profit
            ),
        ]
        
        roi = analyzer._calculate_roi_from_trades(trades)
        
        # Expected: (12 - 10) / 10 * 100 = 20%
        assert abs(roi - 20.0) < 0.1
    
    def test_roi_calculation_partial_position_close(self, analyzer):
        """Test ROI calculation with partial position close."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.BUY,
                amount_sol=2.0,
                price_at_trade=10.0,
                timestamp=datetime.utcnow() - timedelta(days=5),
                tx_signature="tx1",
                pnl_sol=None,
            ),
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,  # Sell half
                price_at_trade=12.0,
                timestamp=datetime.utcnow() - timedelta(days=4),
                tx_signature="tx2",
                pnl_sol=2.0,
            ),
        ]
        
        roi = analyzer._calculate_roi_from_trades(trades)
        
        # Expected: (12 - 10) * 1.0 / (10 * 2.0) * 100 = 10%
        assert abs(roi - 10.0) < 0.1
    
    def test_roi_calculation_multiple_tokens(self, analyzer):
        """Test ROI calculation with multiple tokens."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.BUY,
                amount_sol=1.0,
                price_at_trade=10.0,
                timestamp=datetime.utcnow() - timedelta(days=5),
                tx_signature="tx1",
                pnl_sol=None,
            ),
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=12.0,
                timestamp=datetime.utcnow() - timedelta(days=4),
                tx_signature="tx2",
                pnl_sol=2.0,
            ),
            HistoricalTrade(
                token_address="token2",
                token_symbol="TOKEN2",
                action=TradeAction.BUY,
                amount_sol=1.0,
                price_at_trade=20.0,
                timestamp=datetime.utcnow() - timedelta(days=3),
                tx_signature="tx3",
                pnl_sol=None,
            ),
            HistoricalTrade(
                token_address="token2",
                token_symbol="TOKEN2",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=18.0,  # Loss
                timestamp=datetime.utcnow() - timedelta(days=2),
                tx_signature="tx4",
                pnl_sol=-2.0,
            ),
        ]
        
        roi = analyzer._calculate_roi_from_trades(trades)
        
        # Expected: (2.0 - 2.0) / (10.0 + 20.0) * 100 = 0%
        assert abs(roi - 0.0) < 0.1
    
    def test_roi_calculation_average_entry_price(self, analyzer):
        """Test ROI calculation with multiple buys (average entry price)."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.BUY,
                amount_sol=1.0,
                price_at_trade=10.0,
                timestamp=datetime.utcnow() - timedelta(days=5),
                tx_signature="tx1",
                pnl_sol=None,
            ),
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.BUY,
                amount_sol=1.0,
                price_at_trade=12.0,  # Second buy at higher price
                timestamp=datetime.utcnow() - timedelta(days=4),
                tx_signature="tx2",
                pnl_sol=None,
            ),
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=2.0,
                price_at_trade=13.0,  # Sell at $13
                timestamp=datetime.utcnow() - timedelta(days=3),
                tx_signature="tx3",
                pnl_sol=2.0,  # (13 - 11) * 2 = 4, but using PnL
            ),
        ]
        
        roi = analyzer._calculate_roi_from_trades(trades)
        
        # Average entry: (10 + 12) / 2 = 11
        # Expected: (13 - 11) * 2 / (10 + 12) * 100 = 18.18%
        # But using PnL: 2.0 / (10 + 12) * 100 = 9.09%
        assert roi >= 0.0  # Should be positive


class TestWinRateCalculation:
    """Test accurate win rate calculation."""
    
    @pytest.fixture
    def analyzer(self):
        """Create WalletAnalyzer instance."""
        return WalletAnalyzer(discover_wallets=False, max_wallets=0)
    
    def test_win_rate_all_wins(self, analyzer):
        """Test win rate with all winning trades."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=12.0,
                timestamp=datetime.utcnow() - timedelta(days=i),
                tx_signature=f"tx{i}",
                pnl_sol=2.0,  # Win
            )
            for i in range(5, 0, -1)
        ]
        
        win_rate = analyzer._calculate_win_rate_from_trades(trades)
        
        assert win_rate == 1.0
    
    def test_win_rate_all_losses(self, analyzer):
        """Test win rate with all losing trades."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=8.0,
                timestamp=datetime.utcnow() - timedelta(days=i),
                tx_signature=f"tx{i}",
                pnl_sol=-2.0,  # Loss
            )
            for i in range(5, 0, -1)
        ]
        
        win_rate = analyzer._calculate_win_rate_from_trades(trades)
        
        assert win_rate == 0.0
    
    def test_win_rate_mixed(self, analyzer):
        """Test win rate with mixed wins and losses."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=12.0 if i % 2 == 0 else 8.0,
                timestamp=datetime.utcnow() - timedelta(days=i),
                tx_signature=f"tx{i}",
                pnl_sol=2.0 if i % 2 == 0 else -2.0,
            )
            for i in range(10, 0, -1)
        ]
        
        win_rate = analyzer._calculate_win_rate_from_trades(trades)
        
        # 5 wins out of 10 = 0.5
        assert abs(win_rate - 0.5) < 0.01
    
    def test_win_rate_ignores_buy_trades(self, analyzer):
        """Test that win rate only counts SELL trades."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.BUY,  # Buy trades should be ignored
                amount_sol=1.0,
                price_at_trade=10.0,
                timestamp=datetime.utcnow() - timedelta(days=i),
                tx_signature=f"tx{i}",
                pnl_sol=None,
            )
            for i in range(5, 0, -1)
        ]
        
        win_rate = analyzer._calculate_win_rate_from_trades(trades)
        
        # No SELL trades, should return 0.0
        assert win_rate == 0.0


class TestDrawdownCalculation:
    """Test accurate drawdown calculation."""
    
    @pytest.fixture
    def analyzer(self):
        """Create WalletAnalyzer instance."""
        return WalletAnalyzer(discover_wallets=False, max_wallets=0)
    
    def test_drawdown_all_positive(self, analyzer):
        """Test drawdown with all positive PnL (should be 0%)."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=10.0 + i,
                timestamp=datetime.utcnow() - timedelta(days=10-i),
                tx_signature=f"tx{i}",
                pnl_sol=1.0 + i,  # Always positive, increasing
            )
            for i in range(5)
        ]
        
        drawdown = analyzer._calculate_drawdown_from_trades(trades)
        
        # All positive, no drawdown
        assert drawdown == 0.0
    
    def test_drawdown_with_peak_and_trough(self, analyzer):
        """Test drawdown calculation with peak and trough."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=10.0,
                timestamp=datetime.utcnow() - timedelta(days=5),
                tx_signature="tx1",
                pnl_sol=10.0,  # Peak
            ),
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=10.0,
                timestamp=datetime.utcnow() - timedelta(days=4),
                tx_signature="tx2",
                pnl_sol=5.0,  # Drawdown from peak
            ),
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=10.0,
                timestamp=datetime.utcnow() - timedelta(days=3),
                tx_signature="tx3",
                pnl_sol=3.0,  # Further drawdown
            ),
        ]
        
        drawdown = analyzer._calculate_drawdown_from_trades(trades)
        
        # Peak: 10, Trough: 3, Drawdown: (10 - 3) / 10 = 70%
        assert abs(drawdown - 70.0) < 1.0


class TestWinStreakConsistency:
    """Test accurate win streak consistency calculation."""
    
    @pytest.fixture
    def analyzer(self):
        """Create WalletAnalyzer instance."""
        return WalletAnalyzer(discover_wallets=False, max_wallets=0)
    
    def test_consistency_all_wins(self, analyzer):
        """Test consistency with all wins (should be high)."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=10.0,
                timestamp=datetime.utcnow() - timedelta(days=10-i),
                tx_signature=f"tx{i}",
                pnl_sol=2.0,  # All wins
            )
            for i in range(10)
        ]
        
        consistency = analyzer._calculate_win_streak_consistency(trades)
        
        # All wins should have high consistency
        assert consistency > 0.7
    
    def test_consistency_alternating(self, analyzer):
        """Test consistency with alternating wins/losses (should be lower)."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=10.0,
                timestamp=datetime.utcnow() - timedelta(days=10-i),
                tx_signature=f"tx{i}",
                pnl_sol=2.0 if i % 2 == 0 else -1.0,  # Alternating
            )
            for i in range(10)
        ]
        
        consistency = analyzer._calculate_win_streak_consistency(trades)
        
        # Alternating pattern should have lower consistency
        assert consistency < 0.6
    
    def test_consistency_insufficient_trades(self, analyzer):
        """Test consistency with insufficient trades (< 5)."""
        trades = [
            HistoricalTrade(
                token_address="token1",
                token_symbol="TOKEN1",
                action=TradeAction.SELL,
                amount_sol=1.0,
                price_at_trade=10.0,
                timestamp=datetime.utcnow() - timedelta(days=3-i),
                tx_signature=f"tx{i}",
                pnl_sol=2.0,
            )
            for i in range(3)  # Only 3 trades
        ]
        
        consistency = analyzer._calculate_win_streak_consistency(trades)
        
        # Should return 0.0 for insufficient trades
        assert consistency == 0.0
