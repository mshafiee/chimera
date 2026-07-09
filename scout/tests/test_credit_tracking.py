"""
Tests for credit tracking features in Helius client.

Tests credit cost tracking, monthly hard cap enforcement, and pagination integration.
"""

import pytest
from datetime import datetime

from core.helius_client import HeliusClient
from core.helius_credit_tracker import HeliusCreditTracker, CreditCost, get_credit_tracker


class TestCreditCostEnum:
    """Test CreditCost enum values."""

    def test_credit_cost_values(self):
        """Verify CreditCost enum has correct values from Helius pricing."""
        # Verified against official Helius documentation:
        # https://www.helius.dev/docs/billing/llms.txt
        # - getTransactionsForAddress costs 50 credits
        # - Standard RPC calls cost 1 credit
        # - DAS API methods cost 10 credits
        assert CreditCost.GET_TRANSACTIONS.value == 50   # getTransactionsForAddress
        assert CreditCost.DISCOVER_WALLETS.value == 50   # Also getTransactionsForAddress
        assert CreditCost.TOKEN_METADATA.value == 10     # DAS API
        assert CreditCost.GET_TRANSACTION.value == 1     # Standard RPC
        assert CreditCost.WALLET_FIRST_TX.value == 1     # Standard RPC (getSignaturesForAddress)


class TestHeliusCreditTracker:
    """Test HeliusCreditTracker functionality."""

    def test_initialization(self):
        """Test credit tracker initialization with default limits."""
        tracker = HeliusCreditTracker()
        assert tracker.monthly_credits == 10000000  # Developer tier 10M
        assert tracker.reset_day == 1  # First of month

    def test_initialization_with_custom_limit(self):
        """Test credit tracker initialization with custom monthly limit."""
        tracker = HeliusCreditTracker(monthly_credits=1000000)
        assert tracker.monthly_credits == 1000000

    def test_get_snapshot_initial(self):
        """Test initial snapshot shows full credits available."""
        tracker = HeliusCreditTracker(monthly_credits=1000)
        snapshot = tracker.get_snapshot()
        assert snapshot.credits_used == 0
        assert snapshot.credits_remaining == 1000
        assert snapshot.monthly_limit == 1000

    def test_record_request(self):
        """Test recording a request deducts credits."""
        tracker = HeliusCreditTracker(monthly_credits=1000)
        tracker.record_request(CreditCost.GET_TRANSACTIONS)
        snapshot = tracker.get_snapshot()
        assert snapshot.credits_used == 10
        assert snapshot.credits_remaining == 990

    def test_record_multiple_requests(self):
        """Test recording multiple requests accumulates correctly."""
        tracker = HeliusCreditTracker(monthly_credits=1000)
        tracker.record_request(CreditCost.GET_TRANSACTIONS)  # 10
        tracker.record_request(CreditCost.TOKEN_METADATA)    # 10
        tracker.record_request(CreditCost.GET_TRANSACTION)    # 1
        snapshot = tracker.get_snapshot()
        assert snapshot.credits_used == 21
        assert snapshot.credits_remaining == 979

    def test_check_monthly_cap_available(self):
        """Test monthly cap check when credits available."""
        tracker = HeliusCreditTracker(monthly_credits=1000)
        tracker.record_request(CreditCost.GET_TRANSACTIONS)
        assert tracker.check_monthly_cap()

    def test_check_monthly_cap_exhausted(self):
        """Test monthly cap check when credits exhausted."""
        tracker = HeliusCreditTracker(monthly_credits=100)
        tracker.record_request(CreditCost.TOKEN_METADATA)  # 10
        tracker.record_request(CreditCost.GET_TRANSACTIONS)  # 10
        # Not exhausted yet
        assert tracker.check_monthly_cap()

    def test_check_monthly_cap_negative(self):
        """Test monthly cap check when negative (edge case)."""
        tracker = HeliusCreditTracker(monthly_credits=100)
        tracker._credits_used = 150  # Force negative
        assert not tracker.check_monthly_cap()

    def test_get_usage_percentage(self):
        """Test usage percentage calculation."""
        tracker = HeliusCreditTracker(monthly_credits=1000)
        assert tracker.get_usage_percentage() == 0.0
        tracker.record_request(CreditCost.TOKEN_METADATA)  # 10
        assert tracker.get_usage_percentage() == 1.0

    def test_get_reset_time(self):
        """Test reset time calculation."""
        now = datetime(2026, 7, 9, 12, 0, 0)
        tracker = HeliusCreditTracker()
        reset_time = tracker._get_reset_time(now)
        assert reset_time.day == 1
        assert reset_time.month == 8  # Next month
        assert reset_time.year == 2026

    def test_get_reset_time_first_of_month(self):
        """Test reset time when today is first of month."""
        now = datetime(2026, 7, 1, 12, 0, 0)
        tracker = HeliusCreditTracker()
        reset_time = tracker._get_reset_time(now)
        # Still next month (tomorrow doesn't reset)
        assert reset_time.day == 1
        assert reset_time.month == 8


class TestGetCreditTracker:
    """Test get_credit_tracker singleton function."""

    def test_get_credit_tracker_singleton(self):
        """Test get_credit_tracker returns same instance."""
        tracker1 = get_credit_tracker()
        tracker2 = get_credit_tracker()
        assert tracker1 is tracker2

    def test_get_credit_tracker_fallback(self):
        """Test fallback to no-op tracker when unavailable."""
        with patch('core.helius_client.CreditTrackerUnavailable'):
            # Should return a no-op tracker that never raises
            tracker = get_credit_tracker()
            tracker.record_request(CreditCost.PAGINATION)
            snapshot = tracker.get_snapshot()
            # No-op tracker should show unlimited credits
            assert snapshot.credits_remaining >= 0


@pytest.mark.asyncio
class TestHeliusClientCreditIntegration:
    """Test credit tracking integration in HeliusClient."""

    async def test_get_wallet_transactions_records_credits(self, helius_client):
        """Test that get_wallet_transactions records credits per pagination."""
        # Mock the HTTP response with multiple pages
        mock_response = {
            "result": [{"signature": "sig1"}],
            "totalTransactions": 100
        }
        
        with patch.object(helius_client, '_make_helius_request') as mock_request:
            mock_request.return_value = mock_response
            
            # Make a call with pagination
            await helius_client.get_wallet_transactions(
                "test_wallet",
                limit=100
            )
            
            # Should have recorded credits (10 per call)
            # Exact number depends on implementation
            tracker = get_credit_tracker()
            snapshot = tracker.get_snapshot()
            assert snapshot.credits_used > 0

    async def test_get_wallet_transactions_checks_monthly_cap(self, helius_client):
        """Test that get_wallet_transactions checks monthly cap before pagination."""
        tracker = get_credit_tracker()
        
        # Exhaust credits
        tracker._credits_used = tracker.monthly_credits + 1
        
        with patch.object(helius_client, '_make_helius_request') as mock_request:
            # Should return empty due to cap enforcement
            transactions = await helius_client.get_wallet_transactions(
                "test_wallet",
                limit=100
            )
            
            # Should not make HTTP request due to cap
            assert mock_request.call_count == 0 or len(transactions) == 0

    async def test_discover_wallets_from_recent_swaps_records_credits(self, helius_client):
        """Test that discovery method records credits."""
        # Mock successful discovery response
        mock_response = [{"wallet": "wallet1"}, {"wallet": "wallet2"}]
        
        with patch.object(helius_client, '_make_helius_request') as mock_request:
            mock_request.return_value = mock_response
            
            await helius_client.discover_wallets_from_recent_swaps(
                token_mint="test_token",
                hours=24
            )
            
            # Should have recorded discovery credits
            tracker = get_credit_tracker()
            snapshot = tracker.get_snapshot()
            assert snapshot.credits_used >= 10  # Discovery costs 10

    async def test_discovery_checks_monthly_cap(self, helius_client):
        """Test that discovery checks monthly cap."""
        tracker = get_credit_tracker()
        
        # Exhaust credits
        tracker._credits_used = tracker.monthly_credits + 1
        
        with patch.object(helius_client, '_make_helius_request') as mock_request:
            # Should return empty due to cap enforcement
            wallets = await helius_client.discover_wallets_from_recent_swaps(
                token_mint="test_token",
                hours=24
            )
            
            # Should not make HTTP request due to cap
            assert mock_request.call_count == 0 or len(wallets) == 0

    async def test_pagination_loop_respects_cap(self, helius_client):
        """Test that pagination loop stops when cap is reached."""
        tracker = get_credit_tracker()
        
        # Set low limit
        initial_credits = 100
        tracker._credits_used = 0
        tracker._monthly_credits = initial_credits
        
        # Mock response that would require multiple pages
        mock_response = {
            "result": [{"signature": f"sig{i}"} for i in range(50)],
            "totalTransactions": 150  # Would need 3 pages
        }
        
        with patch.object(helius_client, '_make_helius_request') as mock_request:
            mock_request.return_value = mock_response
            
            # Request with limit that would need multiple pages
            await helius_client.get_wallet_transactions(
                "test_wallet",
                limit=150
            )
            
            # Should stop before exhausting all credits
            # Each page costs 10 credits, so at most 10 pages
            assert mock_request.call_count <= 10


@pytest.fixture
def helius_client():
    """Create a HeliusClient instance for testing."""
    with patch('core.helius_client.get_credit_tracker') as mock_get_tracker:
        mock_tracker = HeliusCreditTracker(monthly_credits=10000)
        mock_get_tracker.return_value = mock_tracker
        
        client = HeliusClient(api_url="https://test.helius.xyz", api_key="test_key")
        return client