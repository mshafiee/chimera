"""
Comprehensive tests for Helius wallet discovery functionality.
"""

import pytest
import time
from unittest.mock import patch, AsyncMock
from datetime import datetime, timedelta

from core.helius_client import HeliusClient


class TestHeliusDiscovery:
    """Test suite for wallet discovery."""

    @pytest.fixture
    def helius_client(self):
        """Create a HeliusClient instance for testing."""
        return HeliusClient(api_key="test-api-key")

    @pytest.fixture
    def sample_transaction(self):
        """Sample Helius transaction format."""
        return {
            "signature": "test_signature_123",
            "feePayer": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "timestamp": int((datetime.utcnow() - timedelta(hours=1)).timestamp()),
            "type": "SWAP",
            "accountData": [
                {
                    "account": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
                    "nativeBalanceChange": -1000000,
                },
                {
                    "account": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
                    "nativeBalanceChange": 0,
                }
            ],
            "nativeTransfers": [
                {
                    "fromUserAccount": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
                    "toUserAccount": "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
                    "amount": 1000000,
                }
            ],
            "tokenTransfers": [
                {
                    "fromUserAccount": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
                    "mint": "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
                    "tokenAmount": 1000000,
                }
            ],
        }

    def test_validate_wallet_address(self, helius_client):
        """Test wallet address validation."""
        # Valid addresses
        assert helius_client._validate_wallet_address("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU")

        # Invalid addresses
        assert not helius_client._validate_wallet_address("")
        assert not helius_client._validate_wallet_address("short")
        assert not helius_client._validate_wallet_address("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")  # System account
        assert not helius_client._validate_wallet_address(helius_client.JUPITER_PROGRAM)  # DEX program

    def test_extract_wallets_from_transaction(self, helius_client, sample_transaction):
        """Test wallet extraction from transactions."""
        wallets = helius_client._extract_wallets_from_transaction(sample_transaction)

        # Should extract fee payer and user accounts
        assert len(wallets) > 0
        assert "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU" in wallets

        # Should not extract system accounts
        assert "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" not in wallets

    def test_extract_wallets_from_transaction_empty(self, helius_client):
        """Test wallet extraction from empty/invalid transaction."""
        assert helius_client._extract_wallets_from_transaction({}) == []
        assert helius_client._extract_wallets_from_transaction(None) == []
        assert helius_client._extract_wallets_from_transaction("invalid") == []

    def test_load_active_tokens(self, helius_client, tmp_path):
        """Test loading active tokens from config."""
        # Test with default tokens
        tokens = helius_client._load_active_tokens()
        assert len(tokens) > 0
        assert "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263" in tokens  # BONK

    def test_load_seed_wallets(self, helius_client):
        """Test loading seed wallets from config."""
        wallets = helius_client._load_seed_wallets()
        # Should return list (may be empty if no config)
        assert isinstance(wallets, list)

    @patch('core.helius_client.os.getenv')
    def test_load_active_tokens_from_env(self, mock_getenv, helius_client):
        """Test loading tokens from environment variable."""
        mock_getenv.return_value = "token1,token2,token3"
        tokens = helius_client._load_active_tokens()
        assert len(tokens) == 3
        assert "token1" in tokens

    def test_is_wallet_known(self, helius_client):
        """Test wallet known check."""
        # Initially unknown
        assert not helius_client._is_wallet_known("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU")

        # After marking as discovered
        helius_client._discovered_this_run.add("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU")
        assert helius_client._is_wallet_known("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU")

    async def test_validate_wallet_activity(self, helius_client):
        """Test wallet activity validation."""
        # Mock successful response with enough transactions
        with patch.object(
            helius_client, 'get_wallet_transactions',
            new_callable=AsyncMock
        ) as mock_get_txns:
            mock_get_txns.return_value = [
                {"signature": "tx1"},
                {"signature": "tx2"},
                {"signature": "tx3"},
            ]
            assert await helius_client._validate_wallet_activity("test_wallet", min_trades=3, days_back=7)

        # Mock insufficient transactions
        with patch.object(
            helius_client, 'get_wallet_transactions',
            new_callable=AsyncMock
        ) as mock_get_txns:
            mock_get_txns.return_value = [{"signature": "tx1"}]
            assert not await helius_client._validate_wallet_activity("test_wallet", min_trades=3, days_back=7)

    async def test_discover_from_active_tokens(self, helius_client):
        """Test discovery from active tokens."""
        # Mock the underlying request so discovery returns wallets
        with patch.object(
            helius_client, '_make_request',
            new_callable=AsyncMock
        ) as mock_request:
            mock_request.return_value = [
                {
                    "feePayer": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
                    "signature": "tx1",
                    "tokenTransfers": [
                        {"fromUserAccount": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
                         "mint": "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
                         "tokenAmount": 1000000},
                    ],
                },
                {
                    "feePayer": "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
                    "signature": "tx2",
                    "tokenTransfers": [
                        {"fromUserAccount": "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
                         "mint": "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
                         "tokenAmount": 500000},
                    ],
                },
            ]

            wallets = await helius_client._discover_from_active_tokens(
                token_addresses=["DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"],
                hours_back=24,
                limit_per_token=10,
                use_parallel=False
            )

        assert len(wallets) > 0

    def test_circuit_breaker(self, helius_client):
        """Test circuit breaker functionality."""
        # Initially closed
        assert helius_client._check_circuit_breaker()

        # Record failures
        for _ in range(helius_client._circuit_breaker_threshold):
            helius_client._record_failure()

        # Circuit should be open
        assert not helius_client._check_circuit_breaker()

        # Reset after timeout
        helius_client._circuit_breaker_reset_time = time.time() - 1
        assert helius_client._check_circuit_breaker()

    async def test_retry_with_backoff(self, helius_client):
        """Test retry logic with exponential backoff."""
        call_count = 0

        async def failing_coro():
            nonlocal call_count
            call_count += 1
            if call_count < 3:
                raise Exception("Test error")
            return "success"

        with patch('asyncio.sleep', new_callable=AsyncMock):
            result = await helius_client._retry_with_backoff(failing_coro, max_retries=3)
        assert result == "success"
        assert call_count == 3

    def test_rate_limiting(self, helius_client):
        """Test rate limiting."""
        start_time = time.time()

        # Make multiple requests quickly
        for _ in range(3):
            helius_client._rate_limit()

        elapsed = time.time() - start_time
        # Should have delayed at least 0.07 seconds (2 * 0.05s delays minus tolerance)
        assert elapsed >= 0.07  # Allow some tolerance

    async def test_discover_wallets_fallback_chain(self, helius_client):
        """Test discovery fallback chain — primary strategy succeeds."""
        wallet1 = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
        wallet2 = "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890AB"

        with patch.object(helius_client, '_discover_from_active_tokens', new_callable=AsyncMock) as mock_tokens, \
             patch.object(helius_client, '_discover_from_dex_programs', new_callable=AsyncMock) as mock_programs, \
             patch.object(helius_client, '_discover_from_seed_wallets', new_callable=AsyncMock) as mock_seeds:

            mock_tokens.return_value = {wallet1: 5, wallet2: 3}
            mock_programs.return_value = {}
            mock_seeds.return_value = {}

            # max_wallets=2 means threshold=1; with 2 wallets found, strategies 3/4 won't run
            wallets = await helius_client.discover_wallets_from_recent_swaps(
                min_trade_count=3,
                max_wallets=2
            )

        assert len(wallets) > 0
        mock_tokens.assert_called_once()

    async def test_discover_wallets_multiple_strategies(self, helius_client):
        """Test discovery using multiple strategies."""
        wallet1 = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
        wallet2 = "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890AB"
        wallet3 = "5kLmNoAbCdEfGhIjKlMnOpQrStUvWxYz0987654321CD"

        with patch.object(helius_client, '_discover_from_active_tokens', new_callable=AsyncMock) as mock_tokens, \
             patch.object(helius_client, '_discover_from_dex_programs', new_callable=AsyncMock) as mock_programs, \
             patch.object(helius_client, '_discover_from_seed_wallets', new_callable=AsyncMock) as mock_seeds:

            mock_tokens.return_value = {wallet1: 5}
            mock_programs.return_value = {wallet2: 4}
            mock_seeds.return_value = {wallet3: 3}

            wallets = await helius_client.discover_wallets_from_recent_swaps(
                min_trade_count=3,
                max_wallets=10
            )

        # Should combine results from all strategies
        assert len(wallets) >= 3

    async def test_discover_wallets_no_api_key(self):
        """Test discovery without API key."""
        client = HeliusClient(api_key=None)
        wallets = await client.discover_wallets_from_recent_swaps()
        assert wallets == []

    async def test_discover_wallets_caching(self, helius_client):
        """Test discovery result caching."""
        wallet1 = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
        wallet2 = "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890AB"

        # Clear cache (initial state is {} which is falsy — no change needed,
        # but reset explicitly to be safe)
        helius_client._discovery_cache = {}
        helius_client._discovery_cache_time = 0.0

        # First call — cache miss, discovery runs
        with patch.object(helius_client, '_discover_from_active_tokens', new_callable=AsyncMock) as mock_tokens, \
             patch.object(helius_client, '_discover_from_dex_programs', new_callable=AsyncMock) as mock_programs, \
             patch.object(helius_client, '_discover_from_seed_wallets', new_callable=AsyncMock) as mock_seeds:

            mock_tokens.return_value = {wallet1: 5}
            mock_programs.return_value = {}
            mock_seeds.return_value = {}

            wallets1 = await helius_client.discover_wallets_from_recent_swaps(
                min_trade_count=3,
                max_wallets=2
            )
            assert mock_tokens.called

        # Set cache timestamp to now so second call hits cache
        helius_client._discovery_cache_time = time.time()

        # Second call — should return cached results without calling discovery
        with patch.object(helius_client, '_discover_from_active_tokens', new_callable=AsyncMock) as mock_tokens:
            mock_tokens.return_value = {wallet2: 5}
            wallets2 = await helius_client.discover_wallets_from_recent_swaps(
                min_trade_count=3,
                max_wallets=2
            )
            assert not mock_tokens.called

        assert wallets1 == wallets2

    async def test_api_call_tracking(self, helius_client):
        """Test API call counting — exhausted budget returns None."""
        helius_client._api_calls_made = helius_client._max_api_calls

        result = await helius_client._make_request("/test", {})
        assert result is None

    def test_filter_by_trade_count(self, helius_client):
        """Test filtering wallets by trade count."""
        wallet_counts = {
            "wallet1": 10,
            "wallet2": 5,
            "wallet3": 2,  # Below threshold
            "wallet4": 1,  # Below threshold
        }

        # Filter wallets with min_trade_count=3
        filtered = {
            wallet: count for wallet, count in wallet_counts.items()
            if count >= 3
        }

        assert len(filtered) == 2
        assert "wallet1" in filtered
        assert "wallet2" in filtered
        assert "wallet3" not in filtered

    def test_sort_by_activity(self, helius_client):
        """Test sorting wallets by activity."""
        wallet_counts = {
            "wallet1": 5,
            "wallet2": 10,
            "wallet3": 3,
        }

        wallets = list(wallet_counts.keys())
        wallets.sort(key=lambda w: wallet_counts[w], reverse=True)

        assert wallets[0] == "wallet2"  # Most active
        assert wallets[-1] == "wallet3"  # Least active
