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
import os
from datetime import datetime, timedelta
from typing import Dict, Optional, Tuple
import random

from core.models import LiquidityData

# Import Birdeye client if available (lazy import to avoid circular imports)
BIRDEYE_AVAILABLE = False
BirdeyeClient = None

def _get_birdeye_client():
    """Lazy load BirdeyeClient to avoid circular imports."""
    global BIRDEYE_AVAILABLE, BirdeyeClient
    if BirdeyeClient is None:
        try:
            from core.birdeye_client import BirdeyeClient as _BirdeyeClient
            BirdeyeClient = _BirdeyeClient
            BIRDEYE_AVAILABLE = True
        except ImportError:
            BIRDEYE_AVAILABLE = False
    return BirdeyeClient


class LiquidityProvider:
    """
    Provides liquidity data for tokens.
    
    In production, this connects to:
    - Jupiter Price API (https://price.jup.ag)
    - Birdeye API (historical data)
    - Historical liquidity database
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
        db_path: Optional[str] = None,
    ):
        """
        Initialize the liquidity provider.
        
        Args:
            jupiter_api_url: Jupiter Price API URL
            birdeye_api_key: Birdeye API key for historical data
            cache_ttl_seconds: Cache TTL in seconds
            db_path: Path to SQLite database for historical liquidity storage
        """
        self.jupiter_api_url = jupiter_api_url
        self.birdeye_api_key = birdeye_api_key or os.getenv("BIRDEYE_API_KEY")
        self.cache_ttl = cache_ttl_seconds
        self.db_path = db_path or os.getenv("CHIMERA_DB_PATH", "data/chimera.db")
        
        # Initialize Birdeye client if API key is available
        self.birdeye_client = None
        if self.birdeye_api_key and BIRDEYE_AVAILABLE:
            self.birdeye_client = BirdeyeClient(self.birdeye_api_key)
        
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
        # Try database first (fastest)
        db_data = self._get_from_database(token_address, timestamp)
        if db_data:
            return db_data

        # Try Birdeye API if available
        if hasattr(self, 'birdeye_client') and self.birdeye_client:
            birdeye_data = self.birdeye_client.get_historical_liquidity(token_address, timestamp)
            if birdeye_data:
                # Store in database for future use
                self._store_in_database(birdeye_data)
                return birdeye_data

        # Fallback to simulation
        return self._simulate_historical_liquidity(token_address, timestamp)

    def _get_from_database(
        self, token_address: str, timestamp: datetime
    ) -> Optional[LiquidityData]:
        """Get historical liquidity from database."""
        if not hasattr(self, 'db_path') or not self.db_path:
            return None

        try:
            import sqlite3
            conn = sqlite3.connect(self.db_path)
            cursor = conn.cursor()

            # Query for data within 1 hour of requested timestamp
            time_start = timestamp - timedelta(hours=1)
            time_end = timestamp + timedelta(hours=1)

            cursor.execute(
                """
                SELECT liquidity_usd, price_usd, volume_24h_usd, timestamp, source
                FROM historical_liquidity
                WHERE token_address = ? AND timestamp BETWEEN ? AND ?
                ORDER BY ABS(julianday(timestamp) - julianday(?))
                LIMIT 1
                """,
                (token_address, time_start, time_end, timestamp),
            )

            row = cursor.fetchone()
            conn.close()

            if row:
                return LiquidityData(
                    token_address=token_address,
                    liquidity_usd=row[0],
                    price_usd=row[1],
                    volume_24h_usd=row[2],
                    timestamp=datetime.fromisoformat(row[3]) if isinstance(row[3], str) else row[3],
                    source=row[4] or "database",
                )
        except Exception as e:
            print(f"Failed to query historical liquidity from database: {e}")

        return None

    def _store_in_database(self, liquidity_data: LiquidityData) -> bool:
        """Store liquidity data in database."""
        if not hasattr(self, 'db_path') or not self.db_path:
            return False

        try:
            import sqlite3
            conn = sqlite3.connect(self.db_path)
            cursor = conn.cursor()

            cursor.execute(
                """
                INSERT OR REPLACE INTO historical_liquidity 
                (token_address, liquidity_usd, price_usd, volume_24h_usd, timestamp, source)
                VALUES (?, ?, ?, ?, ?, ?)
                """,
                (
                    liquidity_data.token_address,
                    liquidity_data.liquidity_usd,
                    liquidity_data.price_usd,
                    liquidity_data.volume_24h_usd,
                    liquidity_data.timestamp,
                    liquidity_data.source,
                ),
            )

            conn.commit()
            conn.close()
            return True
        except Exception as e:
            print(f"Failed to store liquidity data in database: {e}")
            return False
    
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
