"""
Unit tests for Token-2022 extension safety checks.

Tests coverage for:
- Detection of risky extension types (TransferFee, ConfidentialTransfer, InterestBearing, TransferHook)
- Token-2022 allowlist functionality
- Proper handling of extension TLV format
- Integration with ScoutConfig.get_token_2022_allowlist()
"""

import pytest
from unittest.mock import MagicMock, patch, AsyncMock
import base64
import asyncio
import aiohttp
from scout.core.analyzer import WalletAnalyzer
from scout.config import ScoutConfig


class TestToken2022Extensions:
    """Test Token-2022 extension detection and allowlist functionality."""

    @pytest.fixture
    def analyzer(self):
        """Create a WalletAnalyzer instance for testing."""
        analyzer = WalletAnalyzer(rpc_url="http://localhost:8899")
        # Set API key to enable Token-2022 checks
        analyzer.helius_client.api_key = "test_api_key"
        return analyzer

    @pytest.fixture
    def token_2022_program(self):
        """Return Token-2022 program ID."""
        return "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"

    def _mock_token_2022_account(self, extension_type, extension_length, token_2022_program):
        """Helper to create mocked Token-2022 account data."""
        extension_data = bytearray(200)
        extension_data[165:167] = extension_type.to_bytes(2, 'little')
        extension_data[167:169] = extension_length.to_bytes(2, 'little')
        
        return {
            'result': {
                'value': {
                    'data': [base64.b64encode(bytes(extension_data)).decode(), 'base64'],
                    'owner': token_2022_program
                }
            }
        }

    def _mock_helius_session(self, account_response):
        """Helper to mock HeliusClient session with proper async context manager."""
        
        class MockResponse:
            def __init__(self, data):
                self.status = 200
                self._data = data
            
            async def json(self):
                return self._data
        
        class MockRequestContextManager:
            """Simulates aiohttp._RequestContextManager — sync return, async CM."""
            def __init__(self, response):
                self._response = response
            
            async def __aenter__(self):
                return self._response
            
            async def __aexit__(self, *args):
                pass
        
        class MockSession:
            """Simulates aiohttp.ClientSession."""
            def __init__(self, data):
                self._data = data
            
            def post(self, *args, **kwargs):
                # Regular method returning an async context manager (NOT async def)
                return MockRequestContextManager(MockResponse(self._data))
        
        return MockSession(account_response)

    @pytest.mark.asyncio
    async def test_token_2022_transfer_fee_rejection(self, analyzer, token_2022_program):
        """Test that Token-2022 tokens with TransferFeeConfig extension are rejected."""
        test_address = "TestToken1111111111111111111111111111111"
        
        account_response = self._mock_token_2022_account(1, 32, token_2022_program)  # TransferFeeConfig
        
        mock_session = self._mock_helius_session(account_response)
        with patch.object(analyzer.helius_client, '_get_session', new_callable=AsyncMock, return_value=mock_session):
            result = await analyzer._is_token_safe_uncached(test_address)
            assert result is False

    @pytest.mark.asyncio
    async def test_token_2022_confidential_transfer_rejection(self, analyzer, token_2022_program):
        """Test that Token-2022 tokens with ConfidentialTransfer extensions are rejected."""
        test_address = "TestToken2222222222222222222222222222222"
        
        account_response = self._mock_token_2022_account(5, 64, token_2022_program)  # ConfidentialTransferAccount
        
        mock_session = self._mock_helius_session(account_response)
        with patch.object(analyzer.helius_client, '_get_session', new_callable=AsyncMock, return_value=mock_session):
            result = await analyzer._is_token_safe_uncached(test_address)
            assert result is False

    @pytest.mark.asyncio
    async def test_token_2022_interest_bearing_rejection(self, analyzer, token_2022_program):
        """Test that Token-2022 tokens with InterestBearingConfig extension are rejected."""
        test_address = "TestToken3333333333333333333333333333333"
        
        account_response = self._mock_token_2022_account(10, 16, token_2022_program)  # InterestBearingConfig
        
        mock_session = self._mock_helius_session(account_response)
        with patch.object(analyzer.helius_client, '_get_session', new_callable=AsyncMock, return_value=mock_session):
            result = await analyzer._is_token_safe_uncached(test_address)
            assert result is False

    @pytest.mark.asyncio
    async def test_token_2022_transfer_hook_rejection(self, analyzer, token_2022_program):
        """Test that Token-2022 tokens with TransferHook extension are rejected."""
        test_address = "TestToken4444444444444444444444444444444"
        
        account_response = self._mock_token_2022_account(14, 32, token_2022_program)  # TransferHook
        
        mock_session = self._mock_helius_session(account_response)
        with patch.object(analyzer.helius_client, '_get_session', new_callable=AsyncMock, return_value=mock_session):
            result = await analyzer._is_token_safe_uncached(test_address)
            assert result is False

    @pytest.mark.asyncio
    async def test_token_2022_allowlist_bypass(self, analyzer, token_2022_program):
        """Test that tokens in allowlist are not rejected for risky extensions."""
        test_address = "AllowlistToken111111111111111111111111"
        
        # Set allowlist via environment variable
        with patch.dict('os.environ', {'SCOUT_TOKEN_2022_ALLOWLIST': test_address}):
            account_response = self._mock_token_2022_account(1, 32, token_2022_program)  # TransferFeeConfig
            
            mock_session = self._mock_helius_session(account_response)
            with patch.object(analyzer.helius_client, '_get_session', new_callable=AsyncMock, return_value=mock_session):
                # Should pass due to allowlist
                result = await analyzer._is_token_safe_uncached(test_address)
                assert result is True

    @pytest.mark.asyncio
    async def test_token_2022_multiple_risky_extensions(self, analyzer, token_2022_program):
        """Test detection when multiple risky extensions are present."""
        test_address = "TestToken5555555555555555555555555555555"
        
        # Mock account data with multiple extensions
        extension_data = bytearray(300)
        # Extension 1: TransferFeeConfig (type 1, length 32)
        extension_data[165:167] = (1).to_bytes(2, 'little')
        extension_data[167:169] = (32).to_bytes(2, 'little')
        # Extension 2: TokenMetadata (type 19, length 64) - safe extension
        extension_data[201:203] = (19).to_bytes(2, 'little')
        extension_data[203:205] = (64).to_bytes(2, 'little')
        
        account_response = {
            'result': {
                'value': {
                    'data': [base64.b64encode(bytes(extension_data)).decode(), 'base64'],
                    'owner': token_2022_program
                }
            }
        }
        
        mock_session = self._mock_helius_session(account_response)
        with patch.object(analyzer.helius_client, '_get_session', new_callable=AsyncMock, return_value=mock_session):
            # Should reject due to TransferFeeConfig
            result = await analyzer._is_token_safe_uncached(test_address)
            assert result is False

    @pytest.mark.asyncio
    async def test_token_2022_safe_extensions_accepted(self, analyzer, token_2022_program):
        """Test that Token-2022 tokens with only safe extensions are accepted."""
        test_address = "TestToken6666666666666666666666666666666"
        
        # Mock account data with only safe extensions
        extension_data = bytearray(300)
        # Extension 1: TokenMetadata (type 19)
        extension_data[165:167] = (19).to_bytes(2, 'little')
        extension_data[167:169] = (64).to_bytes(2, 'little')
        # Extension 2: MemoTransfer (type 8)
        extension_data[233:235] = (8).to_bytes(2, 'little')
        extension_data[235:237] = (8).to_bytes(2, 'little')
        # End marker (type 0)
        extension_data[245:247] = (0).to_bytes(2, 'little')
        extension_data[247:249] = (0).to_bytes(2, 'little')
        
        account_response = {
            'result': {
                'value': {
                    'data': [base64.b64encode(bytes(extension_data)).decode(), 'base64'],
                    'owner': token_2022_program
                }
            }
        }
        
        mock_session = self._mock_helius_session(account_response)
        with patch.object(analyzer.helius_client, '_get_session', new_callable=AsyncMock, return_value=mock_session):
            # Should pass - no risky extensions
            result = await analyzer._is_token_safe_uncached(test_address)
            assert result is True

    @pytest.mark.asyncio
    async def test_token_2022_extension_tlv_parsing(self, analyzer, token_2022_program):
        """Test proper TLV (Type-Length-Value) parsing of extension headers."""
        test_address = "TestToken7777777777777777777777777777777"
        
        extension_data = bytearray(400)
        # Test with various extension types and lengths
        offset = 165
        # Extension type 3: MintCloseAuthority (length 32) - safe
        extension_data[offset:offset+2] = (3).to_bytes(2, 'little')
        extension_data[offset+2:offset+4] = (32).to_bytes(2, 'little')
        offset += 36
        # Extension type 6: DefaultAccountState (length 8) - safe
        extension_data[offset:offset+2] = (6).to_bytes(2, 'little')
        extension_data[offset+2:offset+4] = (8).to_bytes(2, 'little')
        offset += 12
        # Extension type 2: TransferFeeAmount (length 32) - RISKY
        extension_data[offset:offset+2] = (2).to_bytes(2, 'little')
        extension_data[offset+2:offset+4] = (32).to_bytes(2, 'little')
        
        account_response = {
            'result': {
                'value': {
                    'data': [base64.b64encode(bytes(extension_data)).decode(), 'base64'],
                    'owner': token_2022_program
                }
            }
        }
        
        mock_session = self._mock_helius_session(account_response)
        with patch.object(analyzer.helius_client, '_get_session', new_callable=AsyncMock, return_value=mock_session):
            # Should reject due to TransferFeeAmount
            result = await analyzer._is_token_safe_uncached(test_address)
            assert result is False

    @pytest.mark.asyncio
    async def test_token_2022_sentinel_handling(self, analyzer, token_2022_program):
        """Test that extension scanning stops at type 0 sentinel."""
        test_address = "TestToken8888888888888888888888888888888"
        
        extension_data = bytearray(300)
        # Safe extension before sentinel
        extension_data[165:167] = (19).to_bytes(2, 'little')  # TokenMetadata
        extension_data[167:169] = (64).to_bytes(2, 'little')
        # Sentinel marker
        extension_data[233:235] = (0).to_bytes(2, 'little')
        extension_data[235:237] = (0).to_bytes(2, 'little')
        # Put a risky extension after sentinel (should be ignored)
        extension_data[241:243] = (1).to_bytes(2, 'little')  # TransferFeeConfig
        extension_data[243:245] = (32).to_bytes(2, 'little')
        
        account_response = {
            'result': {
                'value': {
                    'data': [base64.b64encode(bytes(extension_data)).decode(), 'base64'],
                    'owner': token_2022_program
                }
            }
        }
        
        mock_session = self._mock_helius_session(account_response)
        with patch.object(analyzer.helius_client, '_get_session', new_callable=AsyncMock, return_value=mock_session):
            # Should pass - risky extension after sentinel should be ignored
            result = await analyzer._is_token_safe_uncached(test_address)
            assert result is True

    def test_config_get_token_2022_allowlist_empty(self):
        """Test ScoutConfig.get_token_2022_allowlist() returns empty list when not configured."""
        with patch.dict('os.environ', {}, clear=True):
            allowlist = ScoutConfig.get_token_2022_allowlist()
            assert allowlist == []

    def test_config_get_token_2022_allowlist_single(self):
        """Test ScoutConfig.get_token_2022_allowlist() with single token."""
        test_token = "TestToken1111111111111111111111111111111"
        with patch.dict('os.environ', {'SCOUT_TOKEN_2022_ALLOWLIST': test_token}):
            allowlist = ScoutConfig.get_token_2022_allowlist()
            assert allowlist == [test_token]

    def test_config_get_token_2022_allowlist_multiple(self):
        """Test ScoutConfig.get_token_2022_allowlist() with multiple tokens."""
        tokens = "TokenA,TokenB,TokenC"
        expected = ["TokenA", "TokenB", "TokenC"]
        with patch.dict('os.environ', {'SCOUT_TOKEN_2022_ALLOWLIST': tokens}):
            allowlist = ScoutConfig.get_token_2022_allowlist()
            assert allowlist == expected

    def test_config_get_token_2022_allowlist_whitespace(self):
        """Test ScoutConfig.get_token_2022_allowlist() handles whitespace correctly."""
        tokens = " TokenA , TokenB , TokenC "
        expected = ["TokenA", "TokenB", "TokenC"]
        with patch.dict('os.environ', {'SCOUT_TOKEN_2022_ALLOWLIST': tokens}):
            allowlist = ScoutConfig.get_token_2022_allowlist()
            assert allowlist == expected

    def test_config_get_token_2022_allowlist_empty_items(self):
        """Test ScoutConfig.get_token_2022_allowlist() filters empty items."""
        tokens = "TokenA,,TokenC,"
        expected = ["TokenA", "TokenC"]
        with patch.dict('os.environ', {'SCOUT_TOKEN_2022_ALLOWLIST': tokens}):
            allowlist = ScoutConfig.get_token_2022_allowlist()
            assert allowlist == expected