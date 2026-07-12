"""DexScreener API client for liquidity and price data."""

import os
import re
import time
from datetime import datetime
from typing import Optional
from urllib.parse import quote
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

    def _validate_solana_address(self, address: str) -> bool:
        """Validate Solana address format (base58, 32-44 chars)."""
        if not address:
            return False
        # Base58 pattern: 32-44 chars, alphanumeric except 0OIl
        return bool(re.match(r'^[1-9A-HJ-NP-Za-km-z]{32,44}$', address))

    def _safe_url_encode(self, address: str) -> str:
        """Safely encode Solana address for URL."""
        return quote(address, safe='')

    def get_current_liquidity(self, token_address: str) -> Optional[LiquidityData]:
        """
        Get current liquidity for a token.

        Args:
            token_address: Token mint address (Solana)

        Returns:
            LiquidityData or None if not available
        """
        self._rate_limit()

        if not self._validate_solana_address(token_address):
            import logging
            logger = logging.getLogger(__name__)
            logger.warning(f"Invalid Solana address format: {token_address}")
            return None

        safe_address = self._safe_url_encode(token_address)
        url = f"{self.base_url}/tokens/{safe_address}"

        try:
            response = requests.get(url, timeout=10)
            response.raise_for_status()
            data = response.json()

            if not data or "pairs" not in data or not data["pairs"]:
                return None

            pairs = data["pairs"]
            if not isinstance(pairs, list):
                raise ValueError(f"Expected list of pairs, got {type(pairs).__name__}")

            best_pair = max(
                pairs,
                key=lambda p: float(p.get("liquidity", {}).get("usd", 0) or 0),
            )

            liquidity_src = best_pair.get("liquidity", {})
            price_src = best_pair.get("priceUsd")
            volume_src = best_pair.get("volume", {})

            if not isinstance(liquidity_src, dict):
                raise ValueError(f"Expected dict for liquidity, got {type(liquidity_src).__name__}")
            if not isinstance(price_src, (int, float, str)):
                raise ValueError(f"Unexpected priceUsd type: {type(price_src).__name__}")
            if not isinstance(volume_src, dict):
                raise ValueError(f"Expected dict for volume, got {type(volume_src).__name__}")

            liquidity_usd = float(liquidity_src.get("usd", 0) or 0)
            price_usd = float(price_src)
            volume_24h_usd = float(volume_src.get("h24", 0) or 0)

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




