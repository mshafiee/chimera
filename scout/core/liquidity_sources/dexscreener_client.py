"""DexScreener API client for liquidity and price data."""

import os
import time
from datetime import datetime
from typing import Optional
import requests
from ..models import LiquidityData


class DexScreenerClient:
    """Client for DexScreener API to fetch liquidity and price data."""

    def __init__(self, api_key: Optional[str] = None):
        """
        Initialize DexScreener client.

        Args:
            api_key: DexScreener API key (optional, public API doesn't require key)
        """
        self.api_key = api_key or os.getenv("DEXSCREENER_API_KEY", "")
        self.base_url = "https://api.dexscreener.com/latest/dex"
        self.rate_limit_delay = 0.5  # Seconds between requests
        self.last_request_time = 0.0

    def _rate_limit(self):
        """Ensure we don't exceed rate limits."""
        current_time = time.time()
        time_since_last = current_time - self.last_request_time
        if time_since_last < self.rate_limit_delay:
            time.sleep(self.rate_limit_delay - time_since_last)
        self.last_request_time = time.time()

    def get_current_liquidity(self, token_address: str) -> Optional[LiquidityData]:
        """
        Get current liquidity for a token.

        Args:
            token_address: Token mint address (Solana)

        Returns:
            LiquidityData or None if not available
        """
        self._rate_limit()

        # DexScreener uses token address directly
        url = f"{self.base_url}/tokens/{token_address}"

        try:
            response = requests.get(url, timeout=10)
            response.raise_for_status()
            data = response.json()

            if not data or "pairs" not in data or not data["pairs"]:
                return None

            # Get the pair with highest liquidity
            pairs = data["pairs"]
            best_pair = max(
                pairs,
                key=lambda p: float(p.get("liquidity", {}).get("usd", 0) or 0),
            )

            liquidity_usd = float(best_pair.get("liquidity", {}).get("usd", 0) or 0)
            price_usd = float(best_pair.get("priceUsd", 0) or 0)
            volume_24h_usd = float(best_pair.get("volume", {}).get("h24", 0) or 0)

            if liquidity_usd > 0:
                return LiquidityData(
                    token_address=token_address,
                    liquidity_usd=liquidity_usd,
                    price_usd=price_usd,
                    volume_24h_usd=volume_24h_usd,
                    timestamp=datetime.utcnow(),
                    source="dexscreener",
                )

        except requests.exceptions.RequestException as e:
            import logging
            logger = logging.getLogger(__name__)
            logger.debug(f"DexScreener API request failed: {e}")
        except (ValueError, KeyError, TypeError) as e:
            import logging
            logger = logging.getLogger(__name__)
            logger.debug(f"DexScreener response parsing failed: {e}")

        return None
