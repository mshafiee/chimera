"""
Security Client - Token Risk Assessment

Integrates with RugCheck.xyz API to assess token security risks before
including them in wallet analysis or copy trading.
"""

import logging
import aiohttp
from typing import Dict, Optional
from datetime import datetime, timedelta

# Import config module if available
try:
    from config import ScoutConfig
    CONFIG_AVAILABLE = True
except ImportError:
    CONFIG_AVAILABLE = False
    ScoutConfig = None

# Import cache system
try:
    from advanced_cache import AdvancedCache, CacheCategory
    CACHE_AVAILABLE = True
except ImportError:
    CACHE_AVAILABLE = False
    AdvancedCache = None
    CacheCategory = None

logger = logging.getLogger(__name__)


class RugCheckClient:
    """
    Client for RugCheck.xyz API to assess token security.
    
    RugCheck provides comprehensive token risk analysis including:
    - Mutable metadata checks
    - Top holder concentration
    - Freeze authority
    - Supply bundling
    - And other security flags
    """
    
    def __init__(self, api_key: Optional[str] = None, fail_mode: Optional[str] = None, session: Optional[aiohttp.ClientSession] = None):
        """
        Initialize RugCheck client.
        
        Args:
            api_key: Optional API key (RugCheck may have public API). If None, uses config.
            fail_mode: "open" (allow if API fails) or "closed" (reject if API fails). If None, uses config.
            session: Optional aiohttp session (for connection pooling)
        """
        self.base_url = "https://api.rugcheck.xyz/v1"
        
        # Get from config if not provided
        if api_key is None and CONFIG_AVAILABLE:
            api_key = ScoutConfig.get_rugcheck_api_key()
        self.api_key = api_key
        
        if fail_mode is None and CONFIG_AVAILABLE:
            fail_mode = ScoutConfig.get_rugcheck_fail_mode()
        self.fail_mode = fail_mode or "closed"
        
        self._l1_cache: Dict[str, Dict] = {}  # In-memory L1 cache with timestamps
        self._session = session
        self._own_session = False
        
        # Initialize advanced cache if available
        self._cache: Optional[AdvancedCache] = None
        if CACHE_AVAILABLE:
            self._cache = AdvancedCache()
    
    async def _get_session(self) -> aiohttp.ClientSession:
        """Get or create aiohttp session."""
        if self._session is None:
            self._session = aiohttp.ClientSession()
            self._own_session = True
        return self._session

    async def _close_session(self):
        """Close session if we own it."""
        if self._own_session and self._session:
            await self._session.close()
            self._session = None
            self._own_session = False

    async def close(self):
        """Close session if we own it (public method)."""
        await self._close_session()

    async def get_token_risk(self, token_mint: str) -> Dict:
        """
        Get risk assessment for a token from RugCheck.
        
        Args:
            token_mint: Token mint address
            
        Returns:
            Dict with keys:
                - is_safe: bool (True if token is safe to trade)
                - score: int (RugCheck risk score, higher = more risky)
                - risks: List[str] (List of risk flags found)
                - cached: bool (True if result was from cache)
        """
        # Check L2 (Redis) cache first if available
        if CACHE_AVAILABLE and self._cache:
            try:
                cache_key = f"token_security:{token_mint}"
                cached_data = self._cache.get(cache_key, category=CacheCategory.TOKEN_SECURITY)
                if cached_data is not None:
                    cached_result = cached_data.copy()
                    cached_result["cached"] = True
                    cached_result["cache_level"] = "L2"
                    logger.debug(f"Token security cache L2 hit for {token_mint[:8]}...")
                    return cached_result
            except Exception as e:
                logger.warning(f"L2 cache check failed for {token_mint[:8]}...: {e}")
        
        # Check L1 (in-memory) cache next
        if token_mint in self._l1_cache:
            cached_entry = self._l1_cache[token_mint]
            # Check if L1 entry is still valid (2 hours)
            if datetime.now() - cached_entry["timestamp"] < timedelta(hours=2):
                cached_result = cached_entry["data"].copy()
                cached_result["cached"] = True
                cached_result["cache_level"] = "L1"
                logger.debug(f"Token security cache L1 hit for {token_mint[:8]}...")
                return cached_result
            else:
                # Expired entry, remove it
                del self._l1_cache[token_mint]
        
        # Cache miss - fetch from API
        try:
            url = f"{self.base_url}/tokens/{token_mint}/report"
            headers = {}
            if self.api_key:
                headers["Authorization"] = f"Bearer {self.api_key}"
            
            session = await self._get_session()
            async with session.get(url, headers=headers, timeout=aiohttp.ClientTimeout(total=10)) as response:
                if response.status == 200:
                    data = await response.json()
                    score = data.get('score', 0)
                    risks = data.get('risks', [])
                    
                    # Critical Failures
                    # RugCheck score: Lower is better, but exact threshold may vary
                    # Using 2000 as threshold based on plan specification
                    is_danger = score > 2000
                    
                    # Check specific flags
                    risk_names = [r.get('name', '') for r in risks if isinstance(r, dict)]
                    has_mutable_metadata = any('MutableMetadata' in name or 'mutable' in name.lower() for name in risk_names)
                    
                    # Check top holder concentration
                    top_holders_concentration = 0
                    for r in risks:
                        if isinstance(r, dict):
                            if 'TopHoldersPercentage' in r.get('name', '') or 'top' in r.get('name', '').lower():
                                top_holders_concentration = r.get('value', 0)
                                break
                    
                    # High concentration (>80%) is risky
                    high_concentration = top_holders_concentration > 80
                    
                    # Determine if safe
                    is_safe = not (is_danger or has_mutable_metadata or high_concentration)
                    
                    result = {
                        "is_safe": is_safe,
                        "score": score,
                        "risks": risk_names,
                        "cached": False,
                        "cache_level": "none"
                    }
                    
                    # Cache in L1 (in-memory)
                    self._l1_cache[token_mint] = {
                        "data": result.copy(),
                        "timestamp": datetime.now()
                    }
                    
                    # Cache in L2 (Redis) if available
                    if CACHE_AVAILABLE and self._cache:
                        try:
                            cache_key = f"token_security:{token_mint}"
                            self._cache.set(cache_key, result, category=CacheCategory.TOKEN_SECURITY)
                        except Exception as e:
                            logger.warning(f"Failed to cache token security in L2 for {token_mint[:8]}...: {e}")
                    
                    return result
                    
                elif response.status == 404:
                    # Token not found in RugCheck - assume safe but log
                    logger.debug(f"Token {token_mint} not found in RugCheck")
                    result = {
                        "is_safe": True,  # Fail open for unknown tokens
                        "score": 0,
                        "risks": [],
                        "cached": False,
                        "cache_level": "none"
                    }
                    # Cache in L1
                    self._l1_cache[token_mint] = {
                        "data": result.copy(),
                        "timestamp": datetime.now()
                    }
                    # Cache in L2 if available
                    if CACHE_AVAILABLE and self._cache:
                        try:
                            cache_key = f"token_security:{token_mint}"
                            self._cache.set(cache_key, result, category=CacheCategory.TOKEN_SECURITY)
                        except Exception:
                            pass
                    return result
                else:
                    logger.warning(f"RugCheck API returned status {response.status} for token {token_mint}")
                    # Handle based on fail_mode
                    if self.fail_mode == "closed":
                        # Fail closed: reject token if API fails
                        return {
                            "is_safe": False,
                            "score": 9999,  # High score indicates unknown risk
                            "risks": ["RugCheck API unavailable"],
                            "cached": False,
                            "cache_level": "none"
                        }
                    else:
                        # Fail open: allow token if API fails
                        return {
                            "is_safe": True,
                            "score": 0,
                            "risks": [],
                            "cached": False,
                            "cache_level": "none"
                        }
                    
        except aiohttp.ClientError as e:
            logger.error(f"RugCheck API request failed for {token_mint}: {e}")
            # Handle based on fail_mode
            if self.fail_mode == "closed":
                return {
                    "is_safe": False,
                    "score": 9999,
                    "risks": ["RugCheck API error"],
                    "cached": False,
                    "cache_level": "none"
                }
            else:
                return {
                    "is_safe": True,
                    "score": 0,
                    "risks": [],
                    "cached": False,
                    "cache_level": "none"
                }
        except Exception as e:
            logger.error(f"Unexpected error in RugCheck check for {token_mint}: {e}")
            if self.fail_mode == "closed":
                return {
                    "is_safe": False,
                    "score": 9999,
                    "risks": ["RugCheck check error"],
                    "cached": False,
                    "cache_level": "none"
                }
            else:
                return {
                    "is_safe": True,
                    "score": 0,
                    "risks": [],
                    "cached": False,
                    "cache_level": "none"
                }
    
    async def is_token_safe(self, token_mint: str) -> bool:
        """
        Convenience method to check if token is safe.
        
        Args:
            token_mint: Token mint address
            
        Returns:
            True if token is safe to trade, False otherwise
        """
        risk = await self.get_token_risk(token_mint)
        return risk.get("is_safe", False)
    
    def clear_cache(self):
        """Clear the L1 risk assessment cache."""
        self._l1_cache.clear()
    
    async def clear_all_caches(self):
        """Clear both L1 and L2 caches."""
        self._l1_cache.clear()
        if CACHE_AVAILABLE and self._cache:
            try:
                # Clear L2 cache by category
                await self._cache.invalidate_by_category(CacheCategory.TOKEN_SECURITY)
                logger.info("Cleared L2 Redis cache for TOKEN_SECURITY category")
            except Exception as e:
                logger.warning(f"Failed to clear L2 cache: {e}")
