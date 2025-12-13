"""Birdeye API client for historical liquidity and price data."""

import os
import time
from datetime import datetime
from typing import Optional, Dict, Any
import requests
from .models import LiquidityData


class BirdeyeClient:
    """Client for Birdeye API to fetch historical liquidity and price data."""

    def __init__(self, api_key: Optional[str] = None):
        """
        Initialize Birdeye client.

        Args:
            api_key: Birdeye API key (from BIRDEYE_API_KEY env var if not provided)
        """
        self.api_key = api_key or os.getenv("BIRDEYE_API_KEY", "")
        self.base_url = "https://public-api.birdeye.so"
        self.rate_limit_delay = 1.0  # Seconds between requests to avoid rate limits
        self.last_request_time = 0.0

    def _rate_limit(self):
        """Ensure we don't exceed rate limits."""
        current_time = time.time()
        time_since_last = current_time - self.last_request_time
        if time_since_last < self.rate_limit_delay:
            time.sleep(self.rate_limit_delay - time_since_last)
        self.last_request_time = time.time()

    def _make_request(self, endpoint: str, params: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """
        Make a request to Birdeye API.

        Args:
            endpoint: API endpoint path
            params: Query parameters

        Returns:
            JSON response or None if request failed
        """
        if not self.api_key:
            return None

        self._rate_limit()

        url = f"{self.base_url}{endpoint}"
        headers = {"X-API-KEY": self.api_key}

        try:
            response = requests.get(url, params=params, headers=headers, timeout=10)
            response.raise_for_status()
            return response.json()
        except requests.exceptions.RequestException as e:
            print(f"Birdeye API request failed: {e}")
            return None

    def get_historical_price(
        self, token_address: str, timestamp: datetime
    ) -> Optional[float]:
        """
        Get historical price for a token at a specific timestamp.

        Args:
            token_address: Token mint address
            timestamp: Historical timestamp

        Returns:
            Price in USD or None if not available
        """
        endpoint = "/defi/history_price"
        params = {
            "address": token_address,
            "time_from": int(timestamp.timestamp()),
            "time_to": int(timestamp.timestamp()) + 3600,  # 1 hour window
        }

        data = self._make_request(endpoint, params)
        if not data or "data" not in data:
            return None

        # Extract price from response
        # Birdeye response format may vary, adjust as needed
        price_data = data.get("data", {})
        if isinstance(price_data, list) and len(price_data) > 0:
            return price_data[0].get("value")
        elif isinstance(price_data, dict):
            return price_data.get("value")

        return None

    def get_historical_liquidity(
        self, token_address: str, timestamp: datetime
    ) -> Optional[LiquidityData]:
        """
        Get historical liquidity for a token at a specific timestamp.

        Args:
            token_address: Token mint address
            timestamp: Historical timestamp

        Returns:
            LiquidityData or None if not available
        """
        # Birdeye may not have direct liquidity endpoint
        # We can derive from price and volume data, or use current liquidity
        # For now, try to get price and estimate liquidity

        price = self.get_historical_price(token_address, timestamp)
        if price is None:
            return None

        # Get current liquidity as fallback (Birdeye may not have historical liquidity)
        # In production, you might maintain your own historical liquidity database
        current_liq = self.get_current_liquidity(token_address)
        if current_liq:
            # Use current liquidity as approximation (not ideal, but better than nothing)
            return LiquidityData(
                token_address=token_address,
                liquidity_usd=current_liq.liquidity_usd,
                price_usd=price,
                volume_24h_usd=current_liq.volume_24h_usd if current_liq else 0.0,
                timestamp=timestamp,
                source="birdeye_historical",
            )

        return None

    def get_current_liquidity(self, token_address: str) -> Optional[LiquidityData]:
        """
        Get current liquidity for a token.

        Args:
            token_address: Token mint address

        Returns:
            LiquidityData or None if not available
        """
        # Try to get token overview which may include liquidity
        endpoint = "/defi/token_overview"
        params = {"address": token_address}

        data = self._make_request(endpoint, params)
        if not data or "data" not in data:
            return None

        overview = data.get("data", {})
        liquidity = overview.get("liquidity", 0.0)
        price = overview.get("price", 0.0)
        volume_24h = overview.get("volume24hUSD", 0.0)

        if liquidity > 0:
            return LiquidityData(
                token_address=token_address,
                liquidity_usd=liquidity,
                price_usd=price,
                volume_24h_usd=volume_24h,
                timestamp=datetime.utcnow(),
                source="birdeye",
            )

        return None

    def get_token_creation_info(self, token_address: str) -> Optional[Dict[str, Any]]:
        """
        Get token creation info (including timestamp).
        
        Args:
            token_address: Token mint address
            
        Returns:
            Dict containing creation info or None
        """
        endpoint = "/defi/token_creation_info"
        params = {"address": token_address}
        
        try:
            url = f"{self.base_url}/defi/token_creation_info"
            headers = {
                "X-API-KEY": self.api_key,
                "x-chain": "solana"
            }
            params = {"address": token_address}
            
            response = requests.get(url, headers=headers, params=params, timeout=10)
            
            if response.status_code == 429:
                return None
                
            response.raise_for_status()
            data = response.json()
            
            if data and "data" in data:
                return data["data"]
            return None
        except Exception as e:
            return None

    def get_token_metadata(self, token_address: str) -> Optional[Dict[str, Any]]:
        """
        Best-effort token metadata from Birdeye.

        Returns a dict that may include: symbol, name, decimals.
        """
        endpoint = "/defi/token_overview"
        params = {"address": token_address}
        data = self._make_request(endpoint, params)
        if not data or "data" not in data:
            return None
        overview = data.get("data", {}) or {}
        meta: Dict[str, Any] = {}
        for k in ("symbol", "name", "decimals"):
            if k in overview and overview[k] is not None:
                meta[k] = overview[k]
        return meta or None
