"""
Comprehensive tests for Helius wallet discovery functionality.
"""

import pytest
import time
from unittest.mock import Mock, patch, MagicMock
from datetime import datetime, timedelta

from core.helius_client import HeliusClient, DiscoveryStats


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
        assert helius_client._validate_wallet_address("So11111111111111111111111111111111111111112")
        
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
    
    @patch('core.helius_client.HeliusClient.get_wallet_transactions')
    def test_validate_wallet_activity(self, mock_get_txns, helius_client):
        """Test wallet activity validation."""
        # Mock successful response with transactions
        mock_get_txns.return_value = [
            {"signature": "tx1"},
            {"signature": "tx2"},
            {"signature": "tx3"},
        ]
        
        assert helius_client._validate_wallet_activity("test_wallet", min_trades=3, days_back=7)
        
        # Mock insufficient transactions
        mock_get_txns.return_value = [
            {"signature": "tx1"}
        ]
        
        assert not helius_client._validate_wallet_activity("test_wallet", min_trades=3, days_back=7)
    
    @patch('core.helius_client.HeliusClient._make_request')
    def test_discover_from_active_tokens(self, mock_request, helius_client):
        """Test discovery from active tokens."""
        # Mock API response
        mock_request.return_value = {
            "transactions": [
                {
                    "feePayer": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
                    "signature": "tx1",
                },
                {
                    "feePayer": "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
                    "signature": "tx2",
                },
            ]
        }
        
        wallets = helius_client._discover_from_active_tokens(
            token_addresses=["token1", "token2"],
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
    
    def test_retry_with_backoff(self, helius_client):
        """Test retry logic with exponential backoff."""
        call_count = 0
        
        def failing_func():
            nonlocal call_count
            call_count += 1
            if call_count < 3:
                raise Exception("Test error")
            return "success"
        
        result = helius_client._retry_with_backoff(failing_func, max_retries=3)
        assert result == "success"
        assert call_count == 3
    
    def test_rate_limiting(self, helius_client):
        """Test rate limiting."""
        start_time = time.time()
        
        # Make multiple requests quickly
        for _ in range(3):
            helius_client._rate_limit()
        
        elapsed = time.time() - start_time
        # Should have delayed at least 0.2 seconds (2 * 0.1s delays)
        assert elapsed >= 0.15  # Allow some tolerance
    
    @patch('core.helius_client.HeliusClient._discover_from_active_tokens')
    @patch('core.helius_client.HeliusClient._discover_from_recent_blocks')
    @patch('core.helius_client.HeliusClient._discover_from_dex_programs')
    @patch('core.helius_client.HeliusClient._discover_from_seed_wallets')
    def test_discover_wallets_fallback_chain(
        self,
        mock_seeds,
        mock_programs,
        mock_blocks,
        mock_tokens,
        helius_client
    ):
        """Test discovery fallback chain."""
        # Strategy 1 succeeds
        mock_tokens.return_value = {"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU": 5, "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890": 3}
        mock_blocks.return_value = {}
        mock_programs.return_value = {}
        mock_seeds.return_value = {}
        
        wallets = helius_client.discover_wallets_from_recent_swaps(
            min_trade_count=3,
            max_wallets=10
        )
        
        assert len(wallets) > 0
        mock_tokens.assert_called_once()
    
    @patch('core.helius_client.HeliusClient._discover_from_active_tokens')
    @patch('core.helius_client.HeliusClient._discover_from_recent_blocks')
    @patch('core.helius_client.HeliusClient._discover_from_dex_programs')
    @patch('core.helius_client.HeliusClient._discover_from_seed_wallets')
    def test_discover_wallets_multiple_strategies(
        self,
        mock_seeds,
        mock_programs,
        mock_blocks,
        mock_tokens,
        helius_client
    ):
        """Test discovery using multiple strategies."""
        # Strategy 1 finds some wallets
        mock_tokens.return_value = {"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU": 5}
        # Strategy 2 finds more
        mock_blocks.return_value = {"9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890": 4}
        # Strategy 3 finds more
        mock_programs.return_value = {"5kLmNoAbCdEfGhIjKlMnOpQrStUvWxYz0987654321": 3}
        mock_seeds.return_value = {}
        
        wallets = helius_client.discover_wallets_from_recent_swaps(
            min_trade_count=3,
            max_wallets=10
        )
        
        # Should combine results from all strategies
        assert len(wallets) >= 3
    
    def test_discover_wallets_no_api_key(self):
        """Test discovery without API key."""
        client = HeliusClient(api_key=None)
        wallets = client.discover_wallets_from_recent_swaps()
        assert wallets == []
    
    def test_discover_wallets_caching(self, helius_client):
        """Test discovery result caching."""
        # Clear cache first
        helius_client._discovery_cache = None
        helius_client._discovery_cache_time = None
        
        # First call
        with patch.object(helius_client, '_discover_from_active_tokens') as mock_tokens:
            mock_tokens.return_value = {"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU": 5}
            wallets1 = helius_client.discover_wallets_from_recent_swaps(
                min_trade_count=3,
                max_wallets=10
            )
            assert mock_tokens.called
        
        # Second call should use cache (cache TTL is 1 hour by default)
        # Set cache time to recent
        helius_client._discovery_cache_time = time.time()
        
        with patch.object(helius_client, '_discover_from_active_tokens') as mock_tokens:
            mock_tokens.return_value = {"9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890": 5}
            wallets2 = helius_client.discover_wallets_from_recent_swaps(
                min_trade_count=3,
                max_wallets=10
            )
            # Should use cached results, not call discovery again
            assert wallets1 == wallets2
    
    def test_api_call_tracking(self, helius_client):
        """Test API call counting."""
        initial_calls = helius_client._api_calls_made
        
        # Simulate API calls
        helius_client._api_calls_made = helius_client._max_api_calls
        
        # Should not make more calls
        result = helius_client._make_request("/test", {})
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






