"""
Liquidity Provider for Scout backtesting.

This module provides current and historical liquidity data for tokens,
used to validate whether historical trades can be replicated under
current market conditions.

Data sources:
- Jupiter API: Current liquidity and price data
- Birdeye API: Historical liquidity snapshots
- DexScreener: Alternative liquidity source

Current implementation: Stub with realistic estimates for testing.
In production, connect to actual APIs.
"""

import math
from dataclasses import dataclass
from datetime import datetime, timedelta
from typing import Dict, Optional, Tuple
import random


@dataclass
class LiquidityData:
    """Liquidity data for a token at a specific point in time."""
    token_address: str
    liquidity_usd: float
    price_usd: float
    volume_24h_usd: float
    timestamp: datetime
    source: str  # 'jupiter', 'birdeye', 'dexscreener', 'simulated'


class LiquidityProvider:
    """
    Provides liquidity data for tokens.
    
    In production, this connects to:
    - Jupiter Price API (https://price.jup.ag)
    - Birdeye API for historical data
    - DexScreener as fallback
    
    Usage:
        provider = LiquidityProvider()
        current = provider.get_current_liquidity("DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263")
        historical = provider.get_historical_liquidity(token, datetime(2024, 1, 1))
    """
    
    # Known tokens with typical liquidity ranges (for simulation)
    KNOWN_TOKENS = {
        "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263": ("BONK", 5_000_000),   # BONK - high liquidity
        "EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm": ("WIF", 2_000_000),    # WIF
        "7GCihgDB8fe6KNjn2MYtkzZcRjQy3t9GHdC8uHYmW2hr": ("POPCAT", 500_000),   # POPCAT
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v": ("USDC", 100_000_000), # USDC - stablecoin
        "So11111111111111111111111111111111111111112": ("SOL", 500_000_000),    # Wrapped SOL
    }
    
    def __init__(
        self,
        jupiter_api_url: str = "https://price.jup.ag/v6",
        birdeye_api_key: Optional[str] = None,
        cache_ttl_seconds: int = 60,
    ):
        """
        Initialize the liquidity provider.
        
        Args:
            jupiter_api_url: Jupiter Price API URL
            birdeye_api_key: Birdeye API key for historical data
            cache_ttl_seconds: Cache TTL in seconds
        """
        self.jupiter_api_url = jupiter_api_url
        self.birdeye_api_key = birdeye_api_key
        self.cache_ttl = cache_ttl_seconds
        
        # In-memory cache
        self._cache: Dict[str, Tuple[LiquidityData, datetime]] = {}
    
    def get_current_liquidity(self, token_address: str) -> Optional[LiquidityData]:
        """
        Get current liquidity for a token.
        
        Args:
            token_address: Token mint address
            
        Returns:
            LiquidityData or None if not available
        """
        # Check cache first
        cached = self._get_from_cache(token_address)
        if cached:
            return cached
        
        # In production: Query Jupiter API
        # response = requests.get(f"{self.jupiter_api_url}/price?ids={token_address}")
        # data = response.json()
        
        # Simulate liquidity data
        liquidity_data = self._simulate_current_liquidity(token_address)
        
        if liquidity_data:
            self._add_to_cache(token_address, liquidity_data)
        
        return liquidity_data
    
    def get_historical_liquidity(
        self,
        token_address: str,
        timestamp: datetime,
    ) -> Optional[LiquidityData]:
        """
        Get historical liquidity for a token at a specific time.
        
        Args:
            token_address: Token mint address
            timestamp: Historical timestamp
            
        Returns:
            LiquidityData or None if not available
        """
        # In production: Query Birdeye API or historical database
        # response = requests.get(
        #     f"https://public-api.birdeye.so/defi/history_price",
        #     params={"address": token_address, "time_from": timestamp.timestamp()},
        #     headers={"X-API-KEY": self.birdeye_api_key}
        # )
        
        # Simulate historical liquidity
        return self._simulate_historical_liquidity(token_address, timestamp)
    
    def estimate_slippage(
        self,
        token_address: str,
        amount_sol: float,
        liquidity_usd: float,
        sol_price_usd: float = 150.0,
    ) -> float:
        """
        Estimate slippage for a trade based on trade size vs liquidity.
        
        Uses a square root model: slippage increases with sqrt of trade size
        relative to liquidity.
        
        Args:
            token_address: Token mint address
            amount_sol: Trade size in SOL
            liquidity_usd: Pool liquidity in USD
            sol_price_usd: SOL price in USD
            
        Returns:
            Estimated slippage as a decimal (0.01 = 1%)
        """
        if liquidity_usd <= 0:
            return 1.0  # 100% slippage (trade would fail)
        
        trade_value_usd = amount_sol * sol_price_usd
        
        # Square root model for slippage estimation
        # Slippage = k * sqrt(trade_value / liquidity)
        # where k is a constant based on typical AMM behavior
        k = 0.1  # Calibrated for typical Solana DEX behavior
        
        slippage = k * math.sqrt(trade_value_usd / liquidity_usd)
        
        # Cap at 100%
        return min(slippage, 1.0)
    
    def get_sol_price_usd(self) -> float:
        """
        Get current SOL price in USD.
        
        Returns:
            SOL price in USD
        """
        # In production: Query price API
        # For now, return a reasonable estimate
        return 150.0
    
    def _simulate_current_liquidity(self, token_address: str) -> Optional[LiquidityData]:
        """Simulate current liquidity for testing."""
        # Check if it's a known token
        if token_address in self.KNOWN_TOKENS:
            symbol, base_liquidity = self.KNOWN_TOKENS[token_address]
            # Add some randomness (±20%)
            liquidity = base_liquidity * (0.8 + random.random() * 0.4)
        else:
            # Unknown token: random liquidity between $1k and $500k
            symbol = "UNKNOWN"
            liquidity = random.uniform(1000, 500000)
        
        # Simulate price (not critical for liquidity checks)
        price = random.uniform(0.0000001, 100.0)
        
        return LiquidityData(
            token_address=token_address,
            liquidity_usd=liquidity,
            price_usd=price,
            volume_24h_usd=liquidity * random.uniform(0.1, 2.0),
            timestamp=datetime.utcnow(),
            source="simulated",
        )
    
    def _simulate_historical_liquidity(
        self,
        token_address: str,
        timestamp: datetime,
    ) -> Optional[LiquidityData]:
        """Simulate historical liquidity for testing."""
        # Get current liquidity as baseline
        current = self._simulate_current_liquidity(token_address)
        if not current:
            return None
        
        # Historical liquidity tends to be lower for newer tokens
        days_ago = (datetime.utcnow() - timestamp).days
        
        # Apply a decay factor (older = potentially less liquidity)
        # But also some randomness
        if days_ago > 0:
            decay_factor = max(0.3, 1.0 - (days_ago * 0.02))  # 2% per day, min 30%
            decay_factor *= (0.7 + random.random() * 0.6)  # ±30% randomness
        else:
            decay_factor = 1.0
        
        return LiquidityData(
            token_address=token_address,
            liquidity_usd=current.liquidity_usd * decay_factor,
            price_usd=current.price_usd * (0.5 + random.random()),  # Random historical price
            volume_24h_usd=current.volume_24h_usd * decay_factor,
            timestamp=timestamp,
            source="simulated_historical",
        )
    
    def _get_from_cache(self, token_address: str) -> Optional[LiquidityData]:
        """Get data from cache if not expired."""
        if token_address not in self._cache:
            return None
        
        data, cached_at = self._cache[token_address]
        age = (datetime.utcnow() - cached_at).total_seconds()
        
        if age > self.cache_ttl:
            del self._cache[token_address]
            return None
        
        return data
    
    def _add_to_cache(self, token_address: str, data: LiquidityData) -> None:
        """Add data to cache."""
        self._cache[token_address] = (data, datetime.utcnow())
    
    def clear_cache(self) -> None:
        """Clear the liquidity cache."""
        self._cache.clear()


# Example usage
if __name__ == "__main__":
    provider = LiquidityProvider()
    
    # Test with known token
    bonk_address = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
    
    current = provider.get_current_liquidity(bonk_address)
    if current:
        print(f"BONK Current Liquidity: ${current.liquidity_usd:,.0f}")
    
    historical = provider.get_historical_liquidity(
        bonk_address,
        datetime.utcnow() - timedelta(days=30)
    )
    if historical:
        print(f"BONK Historical Liquidity (30d ago): ${historical.liquidity_usd:,.0f}")
    
    # Test slippage estimation
    slippage = provider.estimate_slippage(
        bonk_address,
        amount_sol=1.0,
        liquidity_usd=100000,
    )
    print(f"Estimated slippage for 1 SOL trade: {slippage*100:.2f}%")
