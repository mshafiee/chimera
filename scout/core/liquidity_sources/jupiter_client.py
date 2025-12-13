"""Jupiter API client for price and liquidity proxy data."""

import os
import asyncio
import time
from datetime import datetime
from typing import Optional
import aiohttp
from ..models import LiquidityData


class JupiterLiquidityClient:
    """Client for Jupiter Price API to fetch price and liquidity estimates."""

    def __init__(self, api_url: str = "https://price.jup.ag/v6", session: Optional[aiohttp.ClientSession] = None):
        """
        Initialize Jupiter client.

        Args:
            api_url: Jupiter Price API URL
            session: Optional aiohttp session (for connection pooling)
        """
        self.api_url = api_url
        self.rate_limit_delay = 0.3  # Seconds between requests
        self.last_request_time = 0.0
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

    async def _rate_limit(self):
        """Ensure we don't exceed rate limits."""
        current_time = time.time()
        time_since_last = current_time - self.last_request_time
        if time_since_last < self.rate_limit_delay:
            await asyncio.sleep(self.rate_limit_delay - time_since_last)
        self.last_request_time = time.time()

    async def get_current_liquidity(self, token_address: str) -> Optional[LiquidityData]:
        """
        Get current price and liquidity estimate for a token.

        Note: Jupiter Price API doesn't directly provide liquidity,
        but we can use price data as a proxy indicator.

        Args:
            token_address: Token mint address

        Returns:
            LiquidityData with price (liquidity_usd may be estimated/0)
        """
        await self._rate_limit()

        url = f"{self.api_url}/price"
        params = {"ids": token_address}

        try:
            session = await self._get_session()
            async with session.get(url, params=params, timeout=aiohttp.ClientTimeout(total=10)) as response:
                response.raise_for_status()
                data = await response.json() or {}

                price_data = (
                    data.get("data", {})
                    .get(token_address, {})
                )

                price = price_data.get("price")
                if price is None:
                    return None

                price_f = float(price)
                if price_f <= 0:
                    return None

                # Jupiter doesn't provide liquidity directly, so we return
                # price-only data (liquidity_usd = 0 indicates estimate unavailable)
                return LiquidityData(
                    token_address=token_address,
                    liquidity_usd=0.0,  # Not available from Jupiter
                    price_usd=price_f,
                    volume_24h_usd=0.0,  # Not available from Jupiter
                    timestamp=datetime.utcnow(),
                    source="jupiter",
                )

        except aiohttp.ClientError as e:
            import logging
            logger = logging.getLogger(__name__)
            logger.debug(f"Jupiter API request failed: {e}")
        except (ValueError, KeyError, TypeError) as e:
            import logging
            logger = logging.getLogger(__name__)
            logger.debug(f"Jupiter response parsing failed: {e}")

        return None

    async def get_sol_price_usd(self) -> Optional[float]:
        """
        Get current SOL price in USD.

        Returns:
            SOL price in USD or None if unavailable
        """
        sol_mint = "So11111111111111111111111111111111111111112"
        liq_data = await self.get_current_liquidity(sol_mint)
        if liq_data and liq_data.price_usd > 0:
            return liq_data.price_usd
        return None
    
    async def __aenter__(self):
        """Async context manager entry."""
        return self
    
    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Async context manager exit."""
        await self._close_session()


