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
import requests

from .models import LiquidityData

# Import Birdeye client if available (lazy import to avoid circular imports)
BIRDEYE_AVAILABLE = False
BirdeyeClient = None

def _get_birdeye_client():
    """Lazy load BirdeyeClient to avoid circular imports."""
    global BIRDEYE_AVAILABLE, BirdeyeClient
    if BirdeyeClient is None:
        try:
            from .birdeye_client import BirdeyeClient as _BirdeyeClient
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
        if self.birdeye_api_key:
            client_cls = _get_birdeye_client()
            if client_cls is not None and BIRDEYE_AVAILABLE:
                self.birdeye_client = client_cls(self.birdeye_api_key)
        
        # In-memory cache
        self._cache: Dict[str, Tuple[LiquidityData, datetime]] = {}
        self._sol_price_cache: Optional[Tuple[float, datetime]] = None
    
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
        tolerance_hours: int = 6,
    ) -> Optional[LiquidityData]:
        """
        Get historical liquidity for a token at a specific timestamp.
        
        Queries the historical_liquidity table for the closest snapshot
        to the requested timestamp. Returns data only if within tolerance.
        
        Args:
            token_address: Token mint address
            timestamp: Historical timestamp
            tolerance_hours: Maximum time difference to accept (default 6 hours)
            
        Returns:
            LiquidityData or None if not available within tolerance
        """
        # Try database first (fastest)
        db_data = self._get_from_database(token_address, timestamp, tolerance_hours)
        if db_data:
            return db_data

        # Try Birdeye API if available
        if hasattr(self, 'birdeye_client') and self.birdeye_client:
            birdeye_data = self.birdeye_client.get_historical_liquidity(token_address, timestamp)
            if birdeye_data:
                # Check if within tolerance
                time_diff = abs((birdeye_data.timestamp - timestamp).total_seconds() / 3600)
                if time_diff <= tolerance_hours:
                    # Store in database for future use
                    self._store_in_database(birdeye_data)
                    return birdeye_data

        # Don't fallback to simulation - return None if no historical data
        return None
    
    def get_historical_liquidity_or_current(
        self,
        token_address: str,
        timestamp: datetime,
    ) -> Optional[LiquidityData]:
        """
        Get historical liquidity, falling back to current if unavailable.
        
        This is the primary method for backtesting - it ensures we always
        have liquidity data, even if historical data is missing.
        
        Args:
            token_address: Token mint address
            timestamp: Historical timestamp
            
        Returns:
            LiquidityData (historical if available, otherwise current)
        """
        # Try to get historical liquidity first
        historical = self.get_historical_liquidity(token_address, timestamp)
        if historical:
            return historical
        
        # Fallback to current liquidity
        current = self.get_current_liquidity(token_address)
        if current:
            # Create historical data point from current
            # Log fallback for monitoring
            import logging
            logger = logging.getLogger(__name__)
            logger.warning(
                f"Historical liquidity not available for {token_address[:8]}... "
                f"at {timestamp.isoformat()}, using current liquidity as fallback"
            )
            return LiquidityData(
                token_address=current.token_address,
                liquidity_usd=current.liquidity_usd,
                price_usd=current.price_usd,
                volume_24h_usd=current.volume_24h_usd,
                timestamp=timestamp,  # Use historical timestamp
                source=f"{current.source}_fallback",
            )
        return None

    def _get_from_database(
        self, 
        token_address: str, 
        timestamp: datetime,
        tolerance_hours: int = 6,
    ) -> Optional[LiquidityData]:
        """
        Get historical liquidity from database.
        
        Args:
            token_address: Token mint address
            timestamp: Historical timestamp
            tolerance_hours: Maximum time difference to accept
            
        Returns:
            LiquidityData or None if not found within tolerance
        """
        if not hasattr(self, 'db_path') or not self.db_path:
            return None

        try:
            import sqlite3
            conn = sqlite3.connect(self.db_path)
            cursor = conn.cursor()

            # Query for data within tolerance of requested timestamp
            time_start = timestamp - timedelta(hours=tolerance_hours)
            time_end = timestamp + timedelta(hours=tolerance_hours)

            cursor.execute(
                """
                SELECT liquidity_usd, price_usd, volume_24h_usd, timestamp, source
                FROM historical_liquidity
                WHERE token_address = ? AND timestamp BETWEEN ? AND ?
                ORDER BY ABS(julianday(timestamp) - julianday(?))
                LIMIT 1
                """,
                (token_address, time_start.isoformat(), time_end.isoformat(), timestamp.isoformat()),
            )

            row = cursor.fetchone()
            conn.close()

            if row:
                # Parse timestamp
                if isinstance(row[3], str):
                    row_timestamp = datetime.fromisoformat(row[3].replace('Z', '+00:00'))
                else:
                    row_timestamp = row[3]
                
                # Verify it's within tolerance
                time_diff = abs((row_timestamp - timestamp).total_seconds() / 3600)
                if time_diff <= tolerance_hours:
                    return LiquidityData(
                        token_address=token_address,
                        liquidity_usd=row[0],
                        price_usd=row[1],
                        volume_24h_usd=row[2],
                        timestamp=row_timestamp,
                        source=row[4] or "database",
                    )
        except Exception as e:
            import logging
            logger = logging.getLogger(__name__)
            logger.error(f"Failed to query historical liquidity from database: {e}")

        return None

    def _store_in_database(self, liquidity_data: LiquidityData) -> bool:
        """
        Store liquidity data in database.
        
        Uses INSERT OR REPLACE to handle duplicate timestamps gracefully.
        """
        if not hasattr(self, 'db_path') or not self.db_path:
            return False

        try:
            import sqlite3
            conn = sqlite3.connect(self.db_path)
            cursor = conn.cursor()

            # Ensure table exists
            cursor.execute("""
                CREATE TABLE IF NOT EXISTS historical_liquidity (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    token_address TEXT NOT NULL,
                    liquidity_usd REAL NOT NULL,
                    price_usd REAL,
                    volume_24h_usd REAL,
                    timestamp TIMESTAMP NOT NULL,
                    source TEXT,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    UNIQUE(token_address, timestamp)
                )
            """)
            
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
                    liquidity_data.timestamp.isoformat() if isinstance(liquidity_data.timestamp, datetime) else liquidity_data.timestamp,
                    liquidity_data.source,
                ),
            )

            conn.commit()
            conn.close()
            return True
        except Exception as e:
            import logging
            logger = logging.getLogger(__name__)
            logger.error(f"Failed to store liquidity data in database: {e}")
            return False
    
    def store_liquidity_batch(self, liquidity_data_list: list[LiquidityData]) -> int:
        """
        Store multiple liquidity snapshots in a single transaction.
        
        Args:
            liquidity_data_list: List of LiquidityData objects to store
            
        Returns:
            Number of successfully stored records
        """
        if not liquidity_data_list:
            return 0
            
        if not hasattr(self, 'db_path') or not self.db_path:
            return 0

        try:
            import sqlite3
            conn = sqlite3.connect(self.db_path)
            cursor = conn.cursor()

            # Ensure table exists
            cursor.execute("""
                CREATE TABLE IF NOT EXISTS historical_liquidity (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    token_address TEXT NOT NULL,
                    liquidity_usd REAL NOT NULL,
                    price_usd REAL,
                    volume_24h_usd REAL,
                    timestamp TIMESTAMP NOT NULL,
                    source TEXT,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    UNIQUE(token_address, timestamp)
                )
            """)
            
            stored_count = 0
            for liquidity_data in liquidity_data_list:
                try:
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
                            liquidity_data.timestamp.isoformat() if isinstance(liquidity_data.timestamp, datetime) else liquidity_data.timestamp,
                            liquidity_data.source,
                        ),
                    )
                    stored_count += 1
                except Exception as e:
                    import logging
                    logger = logging.getLogger(__name__)
                    logger.warning(f"Failed to store liquidity data for {liquidity_data.token_address[:8]}...: {e}")
                    continue

            conn.commit()
            conn.close()
            return stored_count
        except Exception as e:
            import logging
            logger = logging.getLogger(__name__)
            logger.error(f"Failed to store liquidity data batch: {e}")
            return 0
    
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
        # Cache for short period
        if self._sol_price_cache:
            price, cached_at = self._sol_price_cache
            if (datetime.utcnow() - cached_at).total_seconds() < 60:
                return price

        # Best-effort: Jupiter price API (no key required)
        try:
            url = f"{self.jupiter_api_url}/price"
            resp = requests.get(url, params={"ids": "So11111111111111111111111111111111111111112"}, timeout=10)
            resp.raise_for_status()
            data = resp.json() or {}
            price = (
                data.get("data", {})
                .get("So11111111111111111111111111111111111111112", {})
                .get("price")
            )
            if price is not None:
                price_f = float(price)
                if price_f > 0:
                    self._sol_price_cache = (price_f, datetime.utcnow())
                    return price_f
        except Exception:
            pass

        # Fallback estimate
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
