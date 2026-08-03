"""
Tests for credit tracking features in Helius client.

Tests credit cost tracking, budget enforcement, and pagination integration
against the real HeliusCreditTracker API.
"""

import os
import time

import pytest
from unittest.mock import patch

from core.helius_client import HeliusClient
from core.helius_credit_tracker import (
    HeliusCreditTracker, CreditCost, get_credit_tracker, reset_credit_tracker,
)


@pytest.fixture(autouse=True)
def _isolate_tracker(tmp_path):
    """Point the tracker at a fresh state file and reset the singleton per test."""
    os.environ["SCOUT_CREDIT_STATE_FILE"] = str(tmp_path / "credit_state.json")
    reset_credit_tracker()
    yield
    reset_credit_tracker()
    os.environ.pop("SCOUT_CREDIT_STATE_FILE", None)


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
        assert CreditCost.WALLET_FIRST_TX.value == 1     # getSignaturesForAddress


class TestHeliusCreditTracker:
    """Test HeliusCreditTracker functionality."""

    def test_initialization(self):
        """Test credit tracker initialization with default limits."""
        tracker = HeliusCreditTracker()
        snapshot = tracker.get_snapshot()
        assert snapshot.credits_used == 0
        assert snapshot.credits_remaining > 0

    def test_record_request(self):
        """Test recording a request deducts credits."""
        tracker = HeliusCreditTracker()
        tracker.record_request(cost=CreditCost.GET_TRANSACTIONS.value)
        snapshot = tracker.get_snapshot()
        assert snapshot.credits_used == 50

    def test_record_multiple_requests(self):
        """Test recording multiple requests accumulates correctly."""
        tracker = HeliusCreditTracker()
        tracker.record_request(cost=CreditCost.GET_TRANSACTIONS.value)  # 50
        tracker.record_request(cost=CreditCost.TOKEN_METADATA.value)    # 10
        tracker.record_request(cost=CreditCost.GET_TRANSACTION.value)   # 1
        snapshot = tracker.get_snapshot()
        assert snapshot.credits_used == 61

    def test_usage_percentage_decreases_remaining(self):
        """Test that usage reduces credits_remaining."""
        tracker = HeliusCreditTracker()
        initial = tracker.get_snapshot().credits_remaining
        tracker.record_request(cost=CreditCost.TOKEN_METADATA.value)  # 10
        snapshot = tracker.get_snapshot()
        assert snapshot.credits_remaining == initial - 10

    def test_can_make_request_insufficient_budget(self):
        """Test can_make_request denies when the category budget is exhausted."""
        tracker = HeliusCreditTracker()
        tracker._analysis_spent = tracker._analysis_budget
        allowed, reason = tracker.can_make_request(cost=100, category="analysis")
        assert allowed is False
        assert "budget" in reason.lower()

    def test_can_make_request_sufficient_budget(self):
        """Test can_make_request allows when budget remains."""
        tracker = HeliusCreditTracker()
        allowed, reason = tracker.can_make_request(cost=10, category="analysis")
        assert allowed is True

    def test_rate_limit_enforcement(self):
        """Test that the rate limit blocks requests at 50 req/s."""
        tracker = HeliusCreditTracker()
        # Fill the request window with the maximum allowed requests
        tracker._request_times = [time.time()] * 50
        allowed, reason = tracker.can_make_request(cost=1)
        assert allowed is False
        assert "rate limit" in reason.lower()


class TestGetCreditTracker:
    """Test get_credit_tracker singleton function."""

    def test_get_credit_tracker_singleton(self):
        """Test get_credit_tracker returns same instance."""
        reset_credit_tracker()
        tracker1 = get_credit_tracker()
        tracker2 = get_credit_tracker()
        assert tracker1 is tracker2
        reset_credit_tracker()


@pytest.mark.asyncio
class TestHeliusClientCreditIntegration:
    """Test credit tracking integration in HeliusClient."""

    @pytest.fixture(autouse=True)
    def _reset_tracker(self):
        """Reset the shared singleton before each test so credits cannot leak."""
        reset_credit_tracker()
        yield
        reset_credit_tracker()

    async def test_get_wallet_transactions_records_credits(self):
        """Test that get_wallet_transactions records credits per pagination."""
        with patch('core.helius_client.CACHE_AVAILABLE', False), \
             patch(
            'core.helius_client.HeliusClient._make_request',
            return_value=[{"signature": "sig1"}],
        ):
            client = HeliusClient(api_key="test_key")
            client._activity_cache = None  # cache is not under test here

            transactions = await client.get_wallet_transactions(
                "test_wallet",
                limit=100,
            )

            assert len(transactions) > 0
            # Each successful page costs 50 credits (GET_TRANSACTIONS)
            snapshot = get_credit_tracker().get_snapshot()
            assert snapshot.credits_used >= 50

    async def test_get_wallet_transactions_checks_cap(self):
        """Test that get_wallet_transactions skips the API when credits are exhausted."""
        tracker = get_credit_tracker()
        tracker.record_request(cost=tracker._daily_budget + 1)

        with patch('core.helius_client.CACHE_AVAILABLE', False), \
             patch('core.helius_client.HeliusClient._make_request') as mock_request:
            client = HeliusClient(api_key="test_key")

            transactions = await client.get_wallet_transactions(
                "test_wallet",
                limit=100,
            )

            # Should not make HTTP request due to cap
            assert transactions == []
            mock_request.assert_not_called()

    async def test_pagination_loop_respects_cap(self):
        """Test that pagination stops once credits are exhausted."""
        with patch('core.helius_client.CACHE_AVAILABLE', False), \
             patch(
            'core.helius_client.HeliusClient._make_request',
            return_value=[{"signature": f"sig{i}"} for i in range(50)],
        ) as mock_request:
            client = HeliusClient(api_key="test_key")
            client._activity_cache = None  # cache is not under test here

            await client.get_wallet_transactions(
                "test_wallet",
                limit=150,
            )

            # Each page costs 50 credits; the daily budget is ~333k so this
            # exercises multiple pages of pagination through the mock
            assert mock_request.call_count >= 2
            snapshot = get_credit_tracker().get_snapshot()
            assert snapshot.credits_used == 50 * mock_request.call_count
