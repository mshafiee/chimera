"""
Liquidity Provider for Scout backtesting.

This module provides current and historical liquidity data for tokens,
used to validate whether historical trades can be replicated under
current market conditions.

Data sources (multi-source with deterministic ranking):
- Birdeye API: Best historical coverage, current liquidity
- DexScreener: Alternative liquidity source
- Jupiter API: Price data, liquidity proxy

Mode: real (default) or simulated (for testing/dev)
"""

import math
import os
import logging
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Tuple
import random
import requests

from .models import LiquidityData

# Import source clients
try:
    from .birdeye_client import BirdeyeClient
    BIRDEYE_AVAILABLE = True
except ImportError:
    BIRDEYE_AVAILABLE = False
    BirdeyeClient = None

try:
    from .liquidity_sources.dexscreener_client import DexScreenerClient
    DEXSCREENER_AVAILABLE = True
except ImportError:
    DEXSCREENER_AVAILABLE = False
    DexScreenerClient = None

try:
    from .liquidity_sources.jupiter_client import JupiterLiquidityClient
    JUPITER_AVAILABLE = True
except ImportError:
    JUPITER_AVAILABLE = False
    JupiterLiquidityClient = None

logger = logging.getLogger(__name__)


class LiquidityProvider:
    """
    Provides liquidity data for tokens using multi-source providers.
    
    Sources (priority order):
    1. Birdeye (best historical coverage)
    2. DexScreener (alternative liquidity)
    3. Jupiter (price + liquidity proxy)
    
    Usage:
        provider = LiquidityProvider()
        current = provider.get_current_liquidity("DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263")
        historical = provider.get_historical_liquidity(token, datetime(2024, 1, 1))
    """
    
    # Known tokens with typical liquidity ranges (for simulation mode only)
    KNOWN_TOKENS = {
        "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263": ("BONK", 5_000_000),
        "EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm": ("WIF", 2_000_000),
        "7GCihgDB8fe6KNjn2MYtkzZcRjQy3t9GHdC8uHYmW2hr": ("POPCAT", 500_000),
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v": ("USDC", 100_000_000),
        "So11111111111111111111111111111111111111112": ("SOL", 500_000_000),
    }
    
    def __init__(
        self,
        jupiter_api_url: str = "https://price.jup.ag/v6",
        birdeye_api_key: Optional[str] = None,
        dexscreener_api_key: Optional[str] = None,
        cache_ttl_seconds: int = 60,
        db_path: Optional[str] = None,
        mode: str = "real",
    ):
        """
        Initialize the liquidity provider.
        
        Args:
            jupiter_api_url: Jupiter Price API URL
            birdeye_api_key: Birdeye API key for historical data
            dexscreener_api_key: DexScreener API key (optional)
            cache_ttl_seconds: Cache TTL in seconds
            db_path: Path to SQLite database for historical liquidity storage
            mode: 'real' (default) or 'simulated' (for testing/dev)
        """
        self.mode = mode.lower() or os.getenv("SCOUT_LIQUIDITY_MODE", "real").lower()
        self.cache_ttl = cache_ttl_seconds or int(os.getenv("SCOUT_LIQUIDITY_CACHE_TTL_SECONDS", "60"))
        self.db_path = db_path or os.getenv("CHIMERA_DB_PATH", "data/chimera.db")
        
        # Initialize source clients (only in real mode)
        self.birdeye_client = None
        self.dexscreener_client = None
        self.jupiter_client = None
        
        if self.mode == "real":
            # Birdeye (priority 1)
            self.birdeye_api_key = birdeye_api_key or os.getenv("BIRDEYE_API_KEY")
            if self.birdeye_api_key and BIRDEYE_AVAILABLE and BirdeyeClient:
                self.birdeye_client = BirdeyeClient(self.birdeye_api_key)
            elif not self.birdeye_api_key:
                logger.warning("BIRDEYE_API_KEY not set - Birdeye source unavailable")
            
            # DexScreener (priority 2)
            if DEXSCREENER_AVAILABLE and DexScreenerClient:
                self.dexscreener_client = DexScreenerClient(dexscreener_api_key)
            
            # Jupiter (priority 3)
            if JUPITER_AVAILABLE and JupiterLiquidityClient:
                self.jupiter_client = JupiterLiquidityClient(jupiter_api_url)
            
            if not any([self.birdeye_client, self.dexscreener_client, self.jupiter_client]):
                logger.warning("No liquidity sources available - falling back to simulated mode")
                self.mode = "simulated"
        else:
            logger.info(f"LiquidityProvider running in {self.mode} mode")
        
        # In-memory cache
        self._cache: Dict[str, Tuple[LiquidityData, datetime]] = {}
        self._sol_price_cache: Optional[Tuple[float, datetime]] = None
    
    def get_current_liquidity(self, token_address: str) -> Optional[LiquidityData]:
        """
        Get current liquidity for a token using multi-source ranking.
        
        Args:
            token_address: Token mint address
            
        Returns:
            LiquidityData or None if not available
        """
        # Check cache first
        cached = self._get_from_cache(token_address)
        if cached:
            return cached
        
        # Simulated mode
        if self.mode == "simulated":
            liquidity_data = self._simulate_current_liquidity(token_address)
            if liquidity_data:
                self._add_to_cache(token_address, liquidity_data)
            return liquidity_data
        
        # Real mode: try sources in priority order
        candidates: List[LiquidityData] = []
        
        # 1. Birdeye
        if self.birdeye_client:
            try:
                birdeye_data = self.birdeye_client.get_current_liquidity(token_address)
                if birdeye_data and birdeye_data.liquidity_usd > 0:
                    candidates.append(birdeye_data)
            except Exception as e:
                logger.debug(f"Birdeye failed for {token_address[:8]}...: {e}")
        
        # 2. DexScreener
        if self.dexscreener_client:
            try:
                dexscreener_data = self.dexscreener_client.get_current_liquidity(token_address)
                if dexscreener_data and dexscreener_data.liquidity_usd > 0:
                    candidates.append(dexscreener_data)
            except Exception as e:
                logger.debug(f"DexScreener failed for {token_address[:8]}...: {e}")
        
        # 3. Jupiter (price only, liquidity_usd = 0)
        if self.jupiter_client:
            try:
                jupiter_data = self.jupiter_client.get_current_liquidity(token_address)
                if jupiter_data:
                    candidates.append(jupiter_data)
            except Exception as e:
                logger.debug(f"Jupiter failed for {token_address[:8]}...: {e}")
        
        # Deterministic ranking: pick best candidate
        liquidity_data = self._rank_liquidity_sources(candidates, token_address)
        
        if liquidity_data:
            self._add_to_cache(token_address, liquidity_data)
        
        return liquidity_data
    
    def _rank_liquidity_sources(
        self, candidates: List[LiquidityData], token_address: str
    ) -> Optional[LiquidityData]:
        """
        Deterministically rank liquidity sources and pick the best.
        
        Ranking criteria (in order):
        1. Highest liquidity_usd (if > 0)
        2. Newest timestamp
        3. Source priority (birdeye > dexscreener > jupiter)
        
        Args:
            candidates: List of LiquidityData from different sources
            token_address: Token address (for logging)
            
        Returns:
            Best LiquidityData or None
        """
        if not candidates:
            return None
        
        # Filter out candidates with no liquidity data (unless all are like that)
        has_liquidity = [c for c in candidates if c.liquidity_usd > 0]
        if has_liquidity:
            candidates = has_liquidity
        
        # Sort by: liquidity (desc), timestamp (desc), source priority
        source_priority = {"birdeye": 3, "dexscreener": 2, "jupiter": 1}
        
        def rank_key(c: LiquidityData) -> Tuple[float, float, int]:
            source_prio = source_priority.get(c.source.lower().split("_")[0], 0)
            return (
                c.liquidity_usd,  # Higher is better
                c.timestamp.timestamp() if isinstance(c.timestamp, datetime) else 0.0,  # Newer is better
                source_prio,  # Higher priority is better
            )
        
        best = max(candidates, key=rank_key)
        return best
    
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

        # Try Birdeye API if available (real mode)
        if self.mode == "real" and self.birdeye_client:
            try:
                birdeye_data = self.birdeye_client.get_historical_liquidity(token_address, timestamp)
                if birdeye_data:
                    # Check if within tolerance
                    time_diff = abs((birdeye_data.timestamp - timestamp).total_seconds() / 3600)
                    if time_diff <= tolerance_hours:
                        # Store in database for future use
                        self._store_in_database(birdeye_data)
                        return birdeye_data
            except Exception as e:
                logger.debug(f"Birdeye historical liquidity failed: {e}")

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
        
        # NEW CODE: Strict mode check
        if os.getenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "false").lower() == "true":
            logger.warning(f"Strict mode: Historical liquidity missing for {token_address}, rejecting.")
            return None
        
        # Fallback to current liquidity (only if explicitly allowed)
        # In real mode, we should avoid silent fallbacks unless necessary
        allow_fallback = os.getenv("SCOUT_LIQUIDITY_ALLOW_FALLBACK", "true").lower() == "true"
        
        if allow_fallback:
            current = self.get_current_liquidity(token_address)
            if current:
                # Use current liquidity as fallback but CAP it to avoid "Survivorship Bias"
                # If a token mooned (10k -> 10M), assuming 10M historical is dangerous.
                # If a token rugged (1M -> 1k), assuming 1k is strict/safe.
                # We cap at $50k to allow testing small caps but filter out mooners.
                safe_fallback_liquidity = min(current.liquidity_usd, 50000.0)
                
                logger.warning(
                    f"Historical liquidity not available for {token_address[:8]}... "
                    f"at {timestamp.isoformat()}. Using CAPPED current liquidity "
                    f"(${safe_fallback_liquidity:,.0f}) as fallback."
                )
                return LiquidityData(
                    token_address=current.token_address,
                    liquidity_usd=safe_fallback_liquidity,
                    price_usd=current.price_usd,
                    volume_24h_usd=current.volume_24h_usd,
                    timestamp=timestamp,  # Use historical timestamp
                    source=f"{current.source}_fallback_capped",
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

        # Try Jupiter client first (if available)
        if self.mode == "real" and self.jupiter_client:
            try:
                price = self.jupiter_client.get_sol_price_usd()
                if price and price > 0:
                    self._sol_price_cache = (price, datetime.utcnow())
                    return price
            except Exception as e:
                logger.debug(f"Jupiter SOL price failed: {e}")

        # Fallback: direct Jupiter API call
        try:
            url = "https://price.jup.ag/v6/price"
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
        except Exception as e:
            logger.debug(f"Direct Jupiter API call failed: {e}")

        # Fallback estimate (only if all else fails)
        logger.warning("Using fallback SOL price estimate: 150.0 USD")
        return 150.0
    
    def _simulate_current_liquidity(self, token_address: str) -> Optional[LiquidityData]:
        """
        Simulate current liquidity for testing (only used in simulated mode).
        
        Note: This uses randomness, so results are non-deterministic.
        Use real mode for production.
        """
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
