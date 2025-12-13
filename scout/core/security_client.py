"""
Security Client - Token Risk Assessment

Integrates with RugCheck.xyz API to assess token security risks before
including them in wallet analysis or copy trading.
"""

import logging
import os
import aiohttp
from typing import Dict, Optional, List

# Import config module if available
try:
    from config import ScoutConfig
    CONFIG_AVAILABLE = True
except ImportError:
    CONFIG_AVAILABLE = False
    ScoutConfig = None

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
        
        self._cache: Dict[str, Dict] = {}  # Cache token risk assessments
        self._session = session
        self._own_session = False
    
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
        # Check cache first
        if token_mint in self._cache:
            cached_result = self._cache[token_mint].copy()
            cached_result["cached"] = True
            return cached_result
        
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
                        "cached": False
                    }
                    
                    # Cache result
                    self._cache[token_mint] = result
                    return result
                    
                elif response.status == 404:
                    # Token not found in RugCheck - assume safe but log
                    logger.debug(f"Token {token_mint} not found in RugCheck")
                    result = {
                        "is_safe": True,  # Fail open for unknown tokens
                        "score": 0,
                        "risks": [],
                        "cached": False
                    }
                    self._cache[token_mint] = result
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
                            "cached": False
                        }
                    else:
                        # Fail open: allow token if API fails
                        return {
                            "is_safe": True,
                            "score": 0,
                            "risks": [],
                            "cached": False
                        }
                    
        except aiohttp.ClientError as e:
            logger.error(f"RugCheck API request failed for {token_mint}: {e}")
            # Handle based on fail_mode
            if self.fail_mode == "closed":
                return {
                    "is_safe": False,
                    "score": 9999,
                    "risks": ["RugCheck API error"],
                    "cached": False
                }
            else:
                return {
                    "is_safe": True,
                    "score": 0,
                    "risks": [],
                    "cached": False
                }
        except Exception as e:
            logger.error(f"Unexpected error in RugCheck check for {token_mint}: {e}")
            if self.fail_mode == "closed":
                return {
                    "is_safe": False,
                    "score": 9999,
                    "risks": ["RugCheck check error"],
                    "cached": False
                }
            else:
                return {
                    "is_safe": True,
                    "score": 0,
                    "risks": [],
                    "cached": False
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
        """Clear the risk assessment cache."""
        self._cache.clear()
