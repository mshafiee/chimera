"""Jupiter API client for price and liquidity proxy data."""

import asyncio
import threading
import time
import os
from datetime import datetime
from typing import Optional
import aiohttp
from ..models import LiquidityData


class JupiterLiquidityClient:
    """Client for Jupiter Price API v3 to fetch price and liquidity estimates."""

    def __init__(self, api_url: str = "https://api.jup.ag/price", session: Optional[aiohttp.ClientSession] = None):
        """
        Initialize Jupiter client.

        Args:
            api_url: Jupiter Price API v3 URL (migrated from lite-api.jup.ag/price/v2)
            session: Optional aiohttp session (for connection pooling)
        """
        self.api_url = api_url
        self.rate_limit_delay = 0.3  # Seconds between requests
        self.last_request_time = 0.0
        # threading.Lock so the delay is enforced across event loops/threads
        self._rate_limit_lock = threading.Lock()
        self._session = session
        self._own_session = False
        # Get API key from environment for authenticated requests
        self.api_key = os.getenv("CHIMERA_JUPITER__API_KEY")

    async def _get_session(self) -> aiohttp.ClientSession:
        """Get or create aiohttp session (single pooled session).

        Sessions are bound to the event loop they were created on. When this
        client is used across loops (main loop + threads spawned by
        _run_async_coro/asyncio.run), reusing a session from a dead loop hangs
        forever. Create a fresh session when the running loop differs.
        """
        loop = asyncio.get_running_loop()
        if (
            self._session is None
            or getattr(self._session, "_loop", None) is not loop
        ):
            self._session = aiohttp.ClientSession()
            self._own_session = True
        return self._session

    async def _close_session(self):
        """Close session if we own it."""
        if self._own_session and self._session:
            try:
                await self._session.close()
            except Exception:
                pass
            self._session = None
            self._own_session = False

    async def _rate_limit(self):
        """Ensure we don't exceed rate limits.

        The next slot is reserved under the lock BEFORE sleeping so concurrent
        callers get distinct slots. The lock is released before the sleep:
        awaiting while holding a threading.Lock deadlocks the main event loop
        when another thread's coroutine blocks on the same lock.
        """
        with self._rate_limit_lock:
            current_time = time.time()
            time_since_last = current_time - self.last_request_time
            if time_since_last < self.rate_limit_delay:
                delay = self.rate_limit_delay - time_since_last
                # Reserve the slot now (inside the lock)
                self.last_request_time = time.time() + delay
            else:
                delay = 0
                self.last_request_time = time.time()
        if delay > 0:
            await asyncio.sleep(delay)

    async def get_current_liquidity(self, token_address: str) -> Optional[LiquidityData]:
        """
        Get current price and liquidity estimate for a token.

        Note: Jupiter Price API v3 provides improved price accuracy and reliability.
        Liquidity data is not directly available, so we use price as a proxy indicator.

        Args:
            token_address: Token mint address

        Returns:
            LiquidityData with price (liquidity_usd may be estimated/0)
        """
        await self._rate_limit()

        # Use v3 endpoint for improved accuracy and reliability
        url = f"{self.api_url}/v3"
        params = {"ids": token_address}

        headers = {}
        if self.api_key:
            headers["x-api-key"] = self.api_key

        try:
            session = await self._get_session()
            async with session.get(url, params=params, headers=headers, timeout=aiohttp.ClientTimeout(total=10)) as response:
                response.raise_for_status()
                data = await response.json() or {}

                # v3 response format: {"token_address": {"usdPrice": ..., ...}}
                price_data = data.get(token_address) or {}

                price = price_data.get("usdPrice") if isinstance(price_data, dict) else None
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
                    source="jupiter_v3",
                )

        except (aiohttp.ClientError, asyncio.TimeoutError) as e:
            import logging
            logger = logging.getLogger(__name__)
            logger.debug(f"Jupiter v3 API request failed: {e}")
        except (ValueError, KeyError, TypeError) as e:
            import logging
            logger = logging.getLogger(__name__)
            logger.debug(f"Jupiter v3 response parsing failed: {e}")

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

    async def close(self):
        """Public method to close session if we own it."""
        await self._close_session()

    async def __aenter__(self):
        """Async context manager entry."""
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Async context manager exit."""
        await self._close_session()


