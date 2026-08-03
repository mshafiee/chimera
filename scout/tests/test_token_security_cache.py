"""
Tests for token security caching in RugCheck client.

Tests TOKEN_SECURITY cache category, L1/L2 cache integration,
and cache invalidation.
"""

import pytest
from unittest.mock import patch, MagicMock, AsyncMock
from datetime import datetime, timedelta

from core.security_client import RugCheckClient
from core.advanced_cache import CacheCategory


class FakeResponse:
    """Minimal aiohttp response stand-in for RugCheck API calls."""

    def __init__(self, payload, status=200):
        self.status = status
        self._payload = payload

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return False

    async def json(self):
        return self._payload


class FakeSession:
    """Minimal aiohttp session stand-in (patch for _get_session)."""

    def __init__(self, payload, status=200):
        self._payload = payload
        self._status = status

    def get(self, url, headers=None, timeout=None):
        return FakeResponse(self._payload, self._status)


@pytest.mark.asyncio
class TestTokenSecurityCacheCategory:
    """Test TOKEN_SECURITY cache category in CacheCategory enum."""

    def test_token_security_category_exists(self):
        """Test that TOKEN_SECURITY category exists."""
        assert hasattr(CacheCategory, 'TOKEN_SECURITY')

    def test_token_security_category_value(self):
        """Test TOKEN_SECURITY category value."""
        assert CacheCategory.TOKEN_SECURITY.value == "token_security"

    def test_token_security_ttl_default(self):
        """Test default TTL for TOKEN_SECURITY category."""
        from core.advanced_cache import TTLDefaults
        
        assert hasattr(TTLDefaults, 'TOKEN_SECURITY')
        assert TTLDefaults.TOKEN_SECURITY == 7200  # 2 hours in seconds


@pytest.mark.asyncio
class TestRugCheckClientL1Cache:
    """Test L1 (in-memory) cache functionality."""

    async def test_l1_cache_hit(self):
        """Test that L1 cache returns cached results."""
        client = RugCheckClient()
        
        # Manually populate L1 cache
        token_mint = "test_token_mint"
        client._l1_cache[token_mint] = {
            "data": {
                "is_safe": True,
                "score": 0,
                "risks": [],
                "cached": False,
                "cache_level": "none"
            },
            "timestamp": datetime.now()
        }
        
        result = await client.get_token_risk(token_mint)
        
        assert result["cached"] is True
        assert result["cache_level"] == "L1"
        assert result["is_safe"] is True

    async def test_l1_cache_miss(self):
        """Test that L1 cache miss triggers API call."""
        client = RugCheckClient()
        
        # Mock API response
        mock_response = {
            "score": 0,
            "risks": []
        }
        
        with patch.object(client, '_get_session',
                           new=AsyncMock(return_value=FakeSession(mock_response))):
            result = await client.get_token_risk("test_token_mint")

            assert result["cached"] is False
            assert result["score"] == 0

    async def test_l1_cache_expiration(self):
        """Test that L1 cache entries expire after 2 hours."""
        client = RugCheckClient()
        
        # Populate L1 cache with old timestamp
        token_mint = "test_token_mint"
        client._l1_cache[token_mint] = {
            "data": {
                "is_safe": True,
                "score": 0,
                "risks": [],
                "cached": False,
                "cache_level": "none"
            },
            "timestamp": datetime.now() - timedelta(hours=3)  # 3 hours ago
        }
        
        # Mock API response for fresh call
        mock_response = {
            "score": 100,
            "risks": ["TestRisk"]
        }
        
        with patch.object(client, '_get_session',
                           new=AsyncMock(return_value=FakeSession(mock_response))):
            result = await client.get_token_risk(token_mint)

            # Should have called API due to expiration
            assert result["score"] == 100

    async def test_l1_cache_stores_results(self):
        """Test that L1 cache stores API results."""
        client = RugCheckClient()
        
        # Mock API response
        mock_response = {
            "score": 50,
            "risks": ["MutableMetadata"]
        }
        
        with patch.object(client, '_get_session',
                           new=AsyncMock(return_value=FakeSession(mock_response))):
            token_mint = "test_token_mint"
            await client.get_token_risk(token_mint)

            # Should be cached in L1
            assert token_mint in client._l1_cache
            assert client._l1_cache[token_mint]["data"]["score"] == 50

    async def test_l1_cache_clear(self):
        """Test clearing L1 cache."""
        client = RugCheckClient()
        
        # Populate L1 cache
        token_mint = "test_token_mint"
        client._l1_cache[token_mint] = {
            "data": {"is_safe": True},
            "timestamp": datetime.now()
        }
        
        assert token_mint in client._l1_cache
        
        # Clear cache
        client.clear_cache()
        
        assert token_mint not in client._l1_cache
        assert len(client._l1_cache) == 0


@pytest.mark.asyncio
class TestRugCheckClientL2Cache:
    """Test L2 (Redis) cache functionality."""

    async def test_l2_cache_hit(self):
        """Test that L2 cache returns cached results."""
        with patch('core.security_client.CACHE_AVAILABLE', True):
            with patch('core.security_client.CacheCategory', CacheCategory), \
                    patch('core.security_client.AdvancedCache') as MockCache:
                # Mock cache instance
                mock_cache_instance = MagicMock()
                mock_cache_instance.get.return_value = {
                    "is_safe": True,
                    "score": 0,
                    "risks": [],
                    "cached": True,
                    "cache_level": "L1"  # Will be overwritten
                }
                MockCache.return_value = mock_cache_instance
                
                client = RugCheckClient()
                
                result = await client.get_token_risk("test_token_mint")
                
                # Should have hit L2 cache
                assert mock_cache_instance.get.called
                assert result["cached"] is True

    async def test_l2_cache_miss_fallback_to_l1(self):
        """Test L2 cache miss falls back to L1."""
        with patch('core.security_client.CACHE_AVAILABLE', True):
            with patch('core.security_client.CacheCategory', CacheCategory), \
                    patch('core.security_client.AdvancedCache') as MockCache:
                # Mock cache instance
                mock_cache_instance = MagicMock()
                mock_cache_instance.get.return_value = None  # L2 miss
                MockCache.return_value = mock_cache_instance
                
                client = RugCheckClient()
                
                # Populate L1 cache
                token_mint = "test_token_mint"
                client._l1_cache[token_mint] = {
                    "data": {
                        "is_safe": True,
                        "score": 0,
                        "risks": [],
                        "cached": False,
                        "cache_level": "none"
                    },
                    "timestamp": datetime.now()
                }
                
                result = await client.get_token_risk(token_mint)
                
                # Should have checked L2, then hit L1
                assert mock_cache_instance.get.called
                assert result["cached"] is True
                assert result["cache_level"] == "L1"

    async def test_l2_cache_stores_results(self):
        """Test that L2 cache stores API results."""
        with patch('core.security_client.CACHE_AVAILABLE', True):
            with patch('core.security_client.CacheCategory', CacheCategory), \
                    patch('core.security_client.AdvancedCache') as MockCache:
                # Mock cache instance
                mock_cache_instance = MagicMock()
                mock_cache_instance.get.return_value = None  # L2 miss
                MockCache.return_value = mock_cache_instance
                
                client = RugCheckClient()
                
                # Mock API response
                mock_response = {
                    "score": 75,
                    "risks": ["HighConcentration"]
                }
                
                with patch.object(client, '_get_session',
                                   new=AsyncMock(return_value=FakeSession(mock_response))):
                    await client.get_token_risk("test_token_mint")

                    # Should have stored in L2 under the TOKEN_SECURITY prefix
                    assert mock_cache_instance.set.called
                    call_args = mock_cache_instance.set.call_args
                    assert call_args[0][0] == "token_security"
                    assert call_args[0][1] == "test_token_mint"

    async def test_l2_cache_uses_correct_category(self):
        """Test that L2 cache uses TOKEN_SECURITY category."""
        with patch('core.security_client.CACHE_AVAILABLE', True):
            with patch('core.security_client.CacheCategory', CacheCategory), \
                    patch('core.security_client.AdvancedCache') as MockCache:
                # Mock cache instance
                mock_cache_instance = MagicMock()
                mock_cache_instance.get.return_value = None
                MockCache.return_value = mock_cache_instance
                
                client = RugCheckClient()
                
                # Mock API response
                mock_response = {"score": 0, "risks": []}
                
                with patch.object(client, '_get_session',
                                   new=AsyncMock(return_value=FakeSession(mock_response))):
                    await client.get_token_risk("test_token_mint")

                    # Should use TOKEN_SECURITY category
                    assert mock_cache_instance.set.called
                    call_args = mock_cache_instance.set.call_args
                    assert call_args.kwargs.get('category') == CacheCategory.TOKEN_SECURITY

    async def test_l2_cache_error_handling(self):
        """Test that L2 cache errors don't break functionality."""
        with patch('core.security_client.CACHE_AVAILABLE', True):
            with patch('core.security_client.CacheCategory', CacheCategory), \
                    patch('core.security_client.AdvancedCache') as MockCache:
                # Mock cache instance that raises exception
                mock_cache_instance = MagicMock()
                mock_cache_instance.get.side_effect = Exception("Redis error")
                MockCache.return_value = mock_cache_instance
                
                client = RugCheckClient()
                
                # Mock API response
                mock_response = {"score": 0, "risks": []}
                
                with patch.object(client, '_get_session',
                                   new=AsyncMock(return_value=FakeSession(mock_response))):
                    # Should not raise exception
                    result = await client.get_token_risk("test_token_mint")

                    # Should fall back to API call
                    assert result["cached"] is False
                    assert result["score"] == 0


@pytest.mark.asyncio
class TestRugCheckClientCacheIntegration:
    """Test L1/L2 cache integration."""

    async def test_cache_level_tracking(self):
        """Test that cache_level is correctly tracked."""
        client = RugCheckClient()
        
        # Test L1 cache
        token_mint = "test_token_mint"
        client._l1_cache[token_mint] = {
            "data": {
                "is_safe": True,
                "score": 0,
                "risks": [],
                "cached": False,
                "cache_level": "none"
            },
            "timestamp": datetime.now()
        }
        
        result = await client.get_token_risk(token_mint)
        assert result["cache_level"] == "L1"

    async def test_cache_precedence_l2_over_l1(self):
        """Test that L2 cache takes precedence over L1."""
        with patch('core.security_client.CACHE_AVAILABLE', True):
            with patch('core.security_client.CacheCategory', CacheCategory), \
                    patch('core.security_client.AdvancedCache') as MockCache:
                # Mock L2 cache
                mock_cache_instance = MagicMock()
                mock_cache_instance.get.return_value = {
                    "is_safe": False,
                    "score": 100,
                    "risks": ["Risk"],
                    "cached": True,
                    "cache_level": "L1"
                }
                MockCache.return_value = mock_cache_instance
                
                client = RugCheckClient()
                
                # Also populate L1 cache (different result)
                token_mint = "test_token_mint"
                client._l1_cache[token_mint] = {
                    "data": {
                        "is_safe": True,
                        "score": 0,
                        "risks": [],
                        "cached": False,
                        "cache_level": "none"
                    },
                    "timestamp": datetime.now()
                }
                
                result = await client.get_token_risk(token_mint)
                
                # Should return L2 result
                assert result["is_safe"] is False
                assert result["score"] == 100

    async def test_both_caches_updated_on_api_call(self):
        """Test that both L1 and L2 caches are updated on API call."""
        with patch('core.security_client.CACHE_AVAILABLE', True):
            with patch('core.security_client.CacheCategory', CacheCategory), \
                    patch('core.security_client.AdvancedCache') as MockCache:
                # Mock L2 cache
                mock_cache_instance = MagicMock()
                mock_cache_instance.get.return_value = None  # L2 miss
                MockCache.return_value = mock_cache_instance
                
                client = RugCheckClient()
                
                # Mock API response
                mock_response = {
                    "score": 25,
                    "risks": ["MutableMetadata"]
                }
                
                with patch.object(client, '_get_session',
                                   new=AsyncMock(return_value=FakeSession(mock_response))):
                    token_mint = "test_token_mint"
                    await client.get_token_risk(token_mint)

                    # Both caches should be updated
                    assert token_mint in client._l1_cache
                    assert client._l1_cache[token_mint]["data"]["score"] == 25
                    assert mock_cache_instance.set.called

    async def test_clear_all_caches(self):
        """Test clearing both L1 and L2 caches."""
        with patch('core.security_client.CACHE_AVAILABLE', True):
            with patch('core.security_client.CacheCategory', CacheCategory), \
                    patch('core.security_client.AdvancedCache') as MockCache:
                # Mock L2 cache
                mock_cache_instance = MagicMock()
                MockCache.return_value = mock_cache_instance
                
                client = RugCheckClient()
                
                # Populate caches
                token_mint = "test_token_mint"
                client._l1_cache[token_mint] = {
                    "data": {"is_safe": True},
                    "timestamp": datetime.now()
                }
                
                assert token_mint in client._l1_cache
                
                # Clear all caches
                await client.clear_all_caches()
                
                # L1 should be cleared
                assert token_mint not in client._l1_cache
                
                # L2 clear should be called
                assert mock_cache_instance.invalidate_category.called

    async def test_clear_all_caches_handles_errors(self):
        """Test clear_all_caches handles L2 errors gracefully."""
        with patch('core.security_client.CACHE_AVAILABLE', True):
            with patch('core.security_client.CacheCategory', CacheCategory), \
                    patch('core.security_client.AdvancedCache') as MockCache:
                # Mock L2 cache that raises exception
                mock_cache_instance = MagicMock()
                mock_cache_instance.invalidate_by_category.side_effect = Exception("Redis error")
                MockCache.return_value = mock_cache_instance
                
                client = RugCheckClient()

                # Populate L1 cache so the assertion is meaningful
                client._l1_cache["test_token_mint"] = {
                    "data": {"is_safe": True},
                    "timestamp": datetime.now()
                }

                # Should not raise exception
                await client.clear_all_caches()

                # L1 should still be cleared
                assert len(client._l1_cache) == 0


@pytest.mark.asyncio
class TestRugCheckClientCacheKeys:
    """Test cache key generation and format."""

    async def test_l2_cache_key_format(self):
        """Test that L2 cache uses correct key format."""
        with patch('core.security_client.CACHE_AVAILABLE', True):
            with patch('core.security_client.CacheCategory', CacheCategory), \
                    patch('core.security_client.AdvancedCache') as MockCache:
                # Mock L2 cache
                mock_cache_instance = MagicMock()
                mock_cache_instance.get.return_value = None
                MockCache.return_value = mock_cache_instance
                
                client = RugCheckClient()
                
                # Mock API response
                mock_response = {"score": 0, "risks": []}
                
                with patch.object(client, '_get_session',
                                   new=AsyncMock(return_value=FakeSession(mock_response))):
                    token_mint = "test_token_mint"
                    await client.get_token_risk(token_mint)

                    # Check cache key format (prefix + identifier)
                    assert mock_cache_instance.set.called
                    call_args = mock_cache_instance.set.call_args
                    prefix = call_args[0][0]
                    identifier = call_args[0][1]
                    assert prefix == "token_security"
                    assert identifier == token_mint

    async def test_different_tokens_different_keys(self):
        """Test that different tokens use different cache keys."""
        with patch('core.security_client.CACHE_AVAILABLE', True):
            with patch('core.security_client.CacheCategory', CacheCategory), \
                    patch('core.security_client.AdvancedCache') as MockCache:
                # Mock L2 cache
                mock_cache_instance = MagicMock()
                mock_cache_instance.get.return_value = None
                MockCache.return_value = mock_cache_instance
                
                client = RugCheckClient()
                
                # Mock API response
                mock_response = {"score": 0, "risks": []}
                
                with patch.object(client, '_get_session',
                                   new=AsyncMock(return_value=FakeSession(mock_response))):
                    await client.get_token_risk("token1")
                    await client.get_token_risk("token2")

                    # Should have made 2 cache set calls with different identifiers
                    assert mock_cache_instance.set.call_count == 2
                    identifiers = [call.args[1] for call in mock_cache_instance.set.call_args_list]
                    assert len(set(identifiers)) == 2

    async def test_cache_key_isolation(self):
        """Test that cache keys are properly isolated between tokens."""
        client = RugCheckClient()
        
        # Populate L1 cache for different tokens
        token1 = "token1_mint"
        token2 = "token2_mint"
        
        client._l1_cache[token1] = {
            "data": {"is_safe": True, "score": 0},
            "timestamp": datetime.now()
        }
        
        client._l1_cache[token2] = {
            "data": {"is_safe": False, "score": 100},
            "timestamp": datetime.now()
        }
        
        result1 = await client.get_token_risk(token1)
        result2 = await client.get_token_risk(token2)
        
        assert result1["is_safe"] is True
        assert result2["is_safe"] is False