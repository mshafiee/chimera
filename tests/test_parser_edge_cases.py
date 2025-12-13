import pytest
from datetime import datetime
from scout.core.analyzer import WalletAnalyzer
from scout.core.models import TradeAction
from scout.core.wqs import WalletMetrics

class TestTransactionParser:
    @pytest.fixture
    def analyzer(self):
        # We initialize with discover_wallets=False to avoid network calls
        return WalletAnalyzer(discover_wallets=False)

    def test_parse_token_to_token_swap_stub(self, analyzer):
        """
        Test parsing a swap between two SPL tokens.
        Note: The current implementation in analyzer._parse_swap_to_trade relies on Helius
        providing 'sol_amount' or 'usd_amount' natively. This test checks if we handle
        raw structures gracefully even if some enrichment is mocked.
        """
        wallet = "Wallet123"
        # Mock Helius swap structure matching what parse_swap_transaction outputs or returns
        # Actually _parse_swap_to_trade expects the output of parse_swap_transaction.
        # Let's mock the input to _parse_swap_to_trade which is a dictionary.
        
        swap_data = {
            "signature": "sig1",
            "timestamp": 1234567890,
            "direction": "BUY",
            "token_mint": "WIF_MINT",
            "token_out": "WIF_MINT", # If direction is BUY, we bought this
            "token_amount": 50,
            "sol_amount": None, # Non-SOL swap
            "usd_amount": 100.0, # Known USD value
            "price_usd": 2.0,
            "token_symbol": "WIF"
        }
        
        # Mock liquidity provider sol price for conversion
        analyzer.liquidity_provider.get_sol_price_usd = lambda: 100.0 # $100 per SOL
        
        trade = analyzer._parse_swap_to_trade(swap_data, wallet)
        
        assert trade is not None
        assert trade.tx_signature == "sig1"
        assert trade.action == TradeAction.BUY
        assert trade.token_address == "WIF_MINT"
        # 100 USD / 100 USD/SOL = 1 SOL equivalent
        assert trade.amount_sol == 1.0 
        assert trade.price_usd == 2.0

    def test_parse_complex_routing_stub(self, analyzer):
        """
        Test extracting trade from a swap dict that might have missing SOL data
        but present USD data (simulating complex route resolution).
        """
        wallet = "Wallet123"
        swap_data = {
            "signature": "sig2",
            "timestamp": 1234567890,
            "direction": "SELL",
            "token_mint": "BONK_MINT",
            "token_amount": 1000,
            "sol_amount": 0.5, # We got SOL out
            "usd_amount": 50.0,
            "token_symbol": "BONK"
        }
        
        trade = analyzer._parse_swap_to_trade(swap_data, wallet)
        
        assert trade is not None
        assert trade.action == TradeAction.SELL
        assert trade.amount_sol == 0.5
        assert trade.token_amount == 1000
