"""
Tests for arbitrage wallet detection (Plan 2).
"""

import pytest
from datetime import datetime, timedelta
from decimal import Decimal

from core.models import TraderArchetype, TradeAction, HistoricalTrade
from core.wqs import WalletMetrics
from core.analyzer import WalletAnalyzer


class TestArbitrageDetection:
    """Tests for arbitrage wallet detection and exclusion."""

    @pytest.fixture
    def analyzer(self):
        """Create a WalletAnalyzer instance for testing."""
        return WalletAnalyzer(helius_api_key="test_key")

    def test_detect_round_trip_ratio_high(self, analyzer):
        """Test round-trip detection with high ratio (arbitrage)."""
        # Create mock transactions with round-trip behavior
        # Transaction 1: Token A both bought and sold (round-trip)
        tx1 = {
            "type": "SWAP",
            "tokenTransfers": [
                {
                    "mint": "TokenA1111111111111111111111111111111",
                    "fromUserAccount": "test_wallet_address",
                    "toUserAccount": "other_wallet",
                    "tokenAmount": "1000"
                },
                {
                    "mint": "TokenA1111111111111111111111111111111",
                    "fromUserAccount": "other_wallet",
                    "toUserAccount": "test_wallet_address",
                    "tokenAmount": "1000"  # Equal amounts for simple test
                },
                {
                    "mint": "So11111111111111111111111111111111111111112",
                    "fromUserAccount": "test_wallet_address",
                    "toUserAccount": "dex_router",
                    "tokenAmount": "10000000"  # ~10 SOL spent
                },
                {
                    "mint": "So11111111111111111111111111111111111111112",
                    "fromUserAccount": "dex_router",
                    "toUserAccount": "test_wallet_address",
                    "tokenAmount": "10050000"  # ~10.05 SOL received
                },
            ]
        }

        # Transaction 2: Another round-trip
        tx2 = {
            "type": "SWAP",
            "tokenTransfers": [
                {
                    "mint": "TokenB2222222222222222222222222222222",
                    "fromUserAccount": "test_wallet_address",
                    "toUserAccount": "other_wallet",
                    "tokenAmount": "500"
                },
                {
                    "mint": "TokenB2222222222222222222222222222222",
                    "fromUserAccount": "other_wallet",
                    "toUserAccount": "test_wallet_address",
                    "tokenAmount": "500"
                },
                {
                    "mint": "So11111111111111111111111111111111111111112",
                    "fromUserAccount": "test_wallet_address",
                    "toUserAccount": "dex_router",
                    "tokenAmount": "5000000"
                },
                {
                    "mint": "So11111111111111111111111111111111111111112",
                    "fromUserAccount": "dex_router",
                    "toUserAccount": "test_wallet_address",
                    "tokenAmount": "5020000"
                },
            ]
        }

        # Transaction 3: Normal directional trade (not a round-trip)
        tx3 = {
            "type": "SWAP",
            "tokenTransfers": [
                {
                    "mint": "So11111111111111111111111111111111111111112",
                    "fromUserAccount": "test_wallet_address",
                    "toUserAccount": "dex_router",
                    "tokenAmount": "20000000"
                },
                {
                    "mint": "TokenC3333333333333333333333333333333",
                    "fromUserAccount": "dex_router",
                    "toUserAccount": "test_wallet_address",
                    "tokenAmount": "2000"
                },
            ]
        }

        # Production only flags round-trips for wallets with >= 10 transactions,
        # so pad to 10 (6 round-trips + 4 normal swaps => ratio 0.6)
        transactions = [tx1, tx2] * 3 + [tx3] * 4

        ratio = analyzer._detect_round_trip_ratio_from_transactions(
            transactions, "test_wallet_address"
        )

        # 6 round-trips out of 10 swaps = 60%
        assert ratio >= 0.6, f"Expected ratio >= 0.6, got {ratio}"

    def test_detect_round_trip_ratio_low(self, analyzer):
        """Test round-trip detection with low ratio (normal trader)."""
        # Create mock normal directional trades
        tx1 = {
            "type": "SWAP",
            "tokenTransfers": [
                {
                    "mint": "So11111111111111111111111111111111111111112",
                    "fromUserAccount": "test_wallet_address",
                    "toUserAccount": "dex_router",
                    "tokenAmount": "20000000"
                },
                {
                    "mint": "TokenA1111111111111111111111111111111",
                    "fromUserAccount": "dex_router",
                    "toUserAccount": "test_wallet_address",
                    "tokenAmount": "1000"
                },
            ]
        }

        tx2 = {
            "type": "SWAP",
            "tokenTransfers": [
                {
                    "mint": "TokenA1111111111111111111111111111111",
                    "fromUserAccount": "test_wallet_address",
                    "toUserAccount": "dex_router",
                    "tokenAmount": "500"
                },
                {
                    "mint": "So11111111111111111111111111111111111111112",
                    "fromUserAccount": "dex_router",
                    "toUserAccount": "test_wallet_address",
                    "tokenAmount": "10000000"
                },
            ]
        }

        tx3 = {
            "type": "SWAP",
            "tokenTransfers": [
                {
                    "mint": "So11111111111111111111111111111111111111112",
                    "fromUserAccount": "test_wallet_address",
                    "toUserAccount": "dex_router",
                    "tokenAmount": "30000000"
                },
                {
                    "mint": "TokenB2222222222222222222222222222222",
                    "fromUserAccount": "dex_router",
                    "toUserAccount": "test_wallet_address",
                    "tokenAmount": "2000"
                },
            ]
        }

        # Production only flags round-trips for wallets with >= 10 transactions
        transactions = [tx1, tx2, tx3] * 3 + [tx1]
        ratio = analyzer._detect_round_trip_ratio_from_transactions(
            transactions, "test_wallet_address"
        )

        # Should detect 0 round-trips out of 10 swaps = 0%
        assert ratio == 0.0

    def test_archetype_arbitrage(self, analyzer):
        """Test ARBITRAGE archetype classification."""
        # Create metrics with high round-trip ratio
        metrics = WalletMetrics(
            address="test_wallet",
            roi_7d=50.0,
            roi_30d=100.0,
            trade_count_30d=20,
            win_rate=0.70,
            avg_trade_size_sol=Decimal("10.0"),
            avg_entry_delay_seconds=5.0,
            is_fresh_wallet=False,
            round_trip_ratio=0.75,  # 75% round-trips -> ARBITRAGE
        )

        # Create some dummy trades
        trades = [
            HistoricalTrade(
                token_address="TokenA",
                token_symbol="TOKA",
                action=TradeAction.BUY,
                amount_sol=Decimal("10.0"),
                price_at_trade=Decimal("0.01"),
                timestamp=datetime.now(),
                tx_signature="sig1",
            ),
            HistoricalTrade(
                token_address="TokenB",
                token_symbol="TOKB",
                action=TradeAction.SELL,
                amount_sol=Decimal("15.0"),
                price_at_trade=Decimal("0.015"),
                timestamp=datetime.now() + timedelta(hours=1),
                tx_signature="sig2",
            ),
        ]

        archetype = analyzer.determine_archetype(metrics, trades)
        assert archetype == TraderArchetype.ARBITRAGE

    def test_archetype_not_arbitrage(self, analyzer):
        """Test that non-arbitrage wallets are not classified as ARBITRAGE."""
        # Create metrics with low round-trip ratio
        metrics = WalletMetrics(
            address="test_wallet",
            roi_7d=50.0,
            roi_30d=100.0,
            trade_count_30d=20,
            win_rate=0.70,
            avg_trade_size_sol=Decimal("10.0"),
            avg_entry_delay_seconds=5.0,
            is_fresh_wallet=False,
            round_trip_ratio=0.10,  # Only 10% round-trips -> not ARBITRAGE
        )

        # Create some dummy trades
        trades = [
            HistoricalTrade(
                token_address="TokenA",
                token_symbol="TOKA",
                action=TradeAction.BUY,
                amount_sol=Decimal("10.0"),
                price_at_trade=Decimal("0.01"),
                timestamp=datetime.now(),
                tx_signature="sig1",
            ),
        ]

        archetype = analyzer.determine_archetype(metrics, trades)
        assert archetype != TraderArchetype.ARBITRAGE
        # Should classify as SNIPER (low entry delay)
        assert archetype == TraderArchetype.SNIPER

    def test_wqs_short_circuit_arbitrage(self):
        """Test that WQS short-circuits ARBITRAGE wallets."""
        from core.wqs import calculate_wqs

        metrics = WalletMetrics(
            address="arb_wallet",
            roi_7d=1000.0,  # Even with amazing ROI
            roi_30d=2000.0,
            trade_count_30d=100,
            win_rate=0.90,
            max_drawdown_30d=1.0,
            avg_trade_size_sol=Decimal("50.0"),
            profit_factor=5.0,
            archetype="ARBITRAGE",  # Bot behavior
        )

        wqs = calculate_wqs(metrics)
        assert wqs == 0.0

    def test_wqs_short_circuit_arbitrage_with_confidence(self):
        """Test that WQS with confidence short-circuits ARBITRAGE wallets."""
        from core.wqs import calculate_wqs_with_confidence

        metrics = WalletMetrics(
            address="arb_wallet",
            roi_7d=1000.0,
            roi_30d=2000.0,
            trade_count_30d=100,
            win_rate=0.90,
            max_drawdown_30d=1.0,
            avg_trade_size_sol=Decimal("50.0"),
            profit_factor=5.0,
            archetype="ARBITRAGE",
        )

        result = calculate_wqs_with_confidence(metrics)
        assert result.score == 0.0
        assert result.confidence == 0.0
        assert result.adjusted_score == 0.0

    def test_round_trip_insufficient_trades(self, analyzer):
        """Test that round-trip detection requires >= 3 transactions.

        (The production caller additionally requires 10+ transactions before
        round_trip_ratio is set at all — see analyzer.py around the caller.)
        """
        # Create only 2 transactions (below the method's 3-transaction minimum),
        # but make tx1 a GENUINE round-trip so a regression in the minimum-trade
        # gate would be caught (ratio would become 0.5 instead of 0.0)
        tx1 = {
            "type": "SWAP",
            "tokenTransfers": [
                {
                    "mint": "TokenA1111111111111111111111111111111",
                    "fromUserAccount": "test_wallet_address",
                    "toUserAccount": "other_wallet",
                    "tokenAmount": "1000"
                },
                {
                    "mint": "TokenA1111111111111111111111111111111",
                    "fromUserAccount": "other_wallet",
                    "toUserAccount": "test_wallet_address",
                    "tokenAmount": "1000"  # Equal amounts = genuine round-trip
                },
            ]
        }

        tx2 = {
            "type": "SWAP",
            "tokenTransfers": [
                {
                    "mint": "So11111111111111111111111111111111111111112",
                    "fromUserAccount": "test_wallet_address",
                    "toUserAccount": "dex_router",
                    "tokenAmount": "20000000"
                },
                {
                    "mint": "TokenB2222222222222222222222222222222",
                    "fromUserAccount": "dex_router",
                    "toUserAccount": "test_wallet_address",
                    "tokenAmount": "2000"
                },
            ]
        }

        transactions = [tx1, tx2]
        ratio = analyzer._detect_round_trip_ratio_from_transactions(
            transactions, "test_wallet_address"
        )

        # Should return 0.0 due to insufficient transactions
        assert ratio == 0.0


if __name__ == "__main__":
    pytest.main([__file__, "-v"])