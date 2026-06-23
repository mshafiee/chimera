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

import json
import math
import os
import logging
import time
import threading
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Tuple
import random

from .utils import utcnow

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

try:
    from .redis_client import RedisClient, REDIS_AVAILABLE
except ImportError:
    REDIS_AVAILABLE = False
    RedisClient = None

try:
    from config import ScoutConfig
    CONFIG_AVAILABLE = True
except ImportError:
    CONFIG_AVAILABLE = False
    ScoutConfig = None

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
        jupiter_api_url: str = "https://lite-api.jup.ag/price",
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
            
            # Check if we should fail hard in production instead of silent fallback
            strict_mode = os.getenv("SCOUT_LIQUIDITY_STRICT_MODE", "false").lower() == "true"
            if not any([self.birdeye_client, self.dexscreener_client, self.jupiter_client]):
                if strict_mode:
                    raise RuntimeError(
                        "STRICT_MODE: No liquidity sources available in production. "
                        "At least one of BIRDEYE_API_KEY, DexScreener, or Jupiter must be configured. "
                        "Set SCOUT_LIQUIDITY_STRICT_MODE=false to allow fallback to simulated mode."
                    )
                logger.warning("No liquidity sources available - falling back to simulated mode")
                self.mode = "simulated"
        else:
            logger.info(f"LiquidityProvider running in {self.mode} mode")
        
        # In-memory cache (fallback)
        self._cache: Dict[str, Tuple[LiquidityData, datetime]] = {}
        self._sol_price_cache: Optional[Tuple[float, datetime]] = None

        # Historical SOL price window for market regime classification
        self._sol_price_history: List[Tuple[datetime, float]] = []

        # Rate limiting for external API calls
        self._rate_limit_lock = threading.Lock()
        self._last_request_time = 0.0
        self._rate_limit_delay = float(os.getenv("SCOUT_LIQUIDITY_RATE_LIMIT_MS", "100")) / 1000.0  # Default 100ms

        # Redis client (if enabled)
        self.redis_client = None
        if REDIS_AVAILABLE and RedisClient:
            try:
                if CONFIG_AVAILABLE and ScoutConfig:
                    redis_enabled = ScoutConfig.get_redis_enabled()
                    redis_url = ScoutConfig.get_redis_url()
                else:
                    redis_enabled = os.getenv("REDIS_ENABLED", "false").lower() == "true"
                    redis_url = os.getenv("REDIS_URL", "redis://localhost:6379")
                
                if redis_enabled:
                    self.redis_client = RedisClient(redis_url=redis_url, enabled=True)
                    if self.redis_client.is_available():
                        logger.info("Redis cache enabled for liquidity data")
                    else:
                        logger.warning("Redis enabled but unavailable, using fallback cache")
                        self.redis_client = None
            except Exception as e:
                logger.warning(f"Failed to initialize Redis client: {e}, using fallback cache")

    def _rate_limit(self):
        """
        Rate limiting for external API calls (thread-safe, synchronous).

        Ensures we don't exceed rate limits for external APIs by enforcing
        a minimum delay between requests.
        """
        with self._rate_limit_lock:
            current_time = time.time()
            time_since_last = current_time - self._last_request_time
            if time_since_last < self._rate_limit_delay:
                time.sleep(self._rate_limit_delay - time_since_last)
            self._last_request_time = time.time()

    async def _rate_limit_async(self):
        """
        Async rate limiting for external API calls (thread-safe, asynchronous).

        Ensures we don't exceed rate limits for external APIs by enforcing
        a minimum delay between requests. Uses asyncio.sleep instead of time.sleep.
        """
        with self._rate_limit_lock:
            current_time = time.time()
            time_since_last = current_time - self._last_request_time
            if time_since_last < self._rate_limit_delay:
                delay = self._rate_limit_delay - time_since_last
                # Release lock during sleep to allow other threads to check
            else:
                delay = 0
        if delay > 0:
            import asyncio
            await asyncio.sleep(delay)
            # Update last request time after sleep
            with self._rate_limit_lock:
                self._last_request_time = time.time()

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
                self._rate_limit()
                birdeye_data = self.birdeye_client.get_current_liquidity(token_address)
                if birdeye_data and birdeye_data.liquidity_usd > 0:
                    candidates.append(birdeye_data)
            except Exception as e:
                logger.debug(f"Birdeye failed for {token_address[:8]}...: {e}")
        
        # 2. DexScreener
        if self.dexscreener_client:
            try:
                self._rate_limit()
                dexscreener_data = self.dexscreener_client.get_current_liquidity(token_address)
                if dexscreener_data and dexscreener_data.liquidity_usd > 0:
                    candidates.append(dexscreener_data)
            except Exception as e:
                logger.debug(f"DexScreener failed for {token_address[:8]}...: {e}")

        # 3. Jupiter (price only, liquidity_usd = 0)
        if self.jupiter_client:
            try:
                self._rate_limit()
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

        # Strict mode check (production recommended)
        # If strict mode is ON, and we finish checking all sources and find nothing,
        # we return None (which is default behavior).
        # The key strict check is in get_historical_liquidity_or_current to prevent fallback.
        # But for optimization, if strict and BIRDEYE not available, we can fail early.
        try:
            from config import ScoutConfig
            strict_mode = ScoutConfig.get_strict_historical_liquidity() if ScoutConfig else False
        except ImportError:
            strict_mode = os.getenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "true").lower() == "true"
        
        if strict_mode:
            if not (self.mode == "real" and self.birdeye_client):
                return None

        # Try Birdeye API if available (real mode)
        if self.mode == "real" and self.birdeye_client:
            try:
                self._rate_limit()
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
        strategy: str = "SHIELD",
    ) -> Optional[LiquidityData]:
        """
        Get historical liquidity with intelligent multi-tier fallback.

        Fallback strategy (in order):
        1. Historical liquidity (ideal)
        2. Grace period current with 30% haircut (for recent trades)
        3. Current with confidence-based penalty (for older trades)
        4. Simulated (last resort, only if allowed)

        Args:
            token_address: Token mint address
            timestamp: Historical timestamp
            strategy: Trading strategy ('SHIELD' or 'SPEAR') for different thresholds

        Returns:
            LiquidityData with confidence score, or None if all sources fail
        """
        # Try to get historical liquidity first (highest confidence)
        historical = self.get_historical_liquidity(token_address, timestamp)
        if historical:
            # Add confidence score to historical data
            historical.source = f"{historical.source}_confidence_1.0"
            return historical

        # Calculate trade age and token age for confidence scoring
        now = utcnow()
        if timestamp.tzinfo:
            trade_age = now - timestamp
        else:
            trade_age = now.replace(tzinfo=None) - timestamp

        # Get token creation time if available
        token_age_days = None
        try:
            from .analyzer import WalletAnalyzer
            token_creation = WalletAnalyzer.get_token_creation_time(token_address)
            if token_creation:
                token_age = now - token_creation
                token_age_days = token_age.days
        except Exception:
            pass

        # Get grace period configuration
        try:
            from config import ScoutConfig
            _grace_days = ScoutConfig.get_historical_liquidity_grace_period_days()
        except ImportError:
            _grace_days = int(os.getenv("SCOUT_HISTORICAL_LIQUIDITY_GRACE_PERIOD_DAYS", "14"))

        # Tier 2: Grace period fallback (for recent trades)
        if trade_age.days < _grace_days:
            current = self.get_current_liquidity(token_address)
            if current:
                # Calculate confidence based on recency
                confidence = 1.0 - (trade_age.days / _grace_days) * 0.3  # 0.7-1.0
                haircut_liquidity = current.liquidity_usd * 0.7
                logger.info(
                    f"Grace period fallback: Using current liquidity for {token_address[:8]}... "
                    f"with 30%% haircut (${haircut_liquidity:,.0f}, confidence: {confidence:.2f})"
                )
                return LiquidityData(
                    token_address=current.token_address,
                    liquidity_usd=haircut_liquidity,
                    price_usd=current.price_usd,
                    volume_24h_usd=current.volume_24h_usd,
                    timestamp=timestamp,
                    source=f"{current.source}_grace_period_haircut_conf_{confidence:.2f}",
                )

        # Tier 3: Confidence-weighted current fallback (for older trades)
        # Check strict mode first
        try:
            from config import ScoutConfig
            strict_mode = ScoutConfig.get_strict_historical_liquidity() if ScoutConfig else False
        except ImportError:
            strict_mode = os.getenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "true").lower() == "true"

        # Check if flexible mode is enabled
        flexible_mode = os.getenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "").lower() == "flexible"

        if strict_mode and not flexible_mode:
            logger.warning(f"Strict mode: Historical liquidity missing for {token_address}, rejecting.")
            return None

        # For flexible mode or when fallback is allowed, use current with penalty
        allow_fallback = os.getenv("SCOUT_LIQUIDITY_ALLOW_FALLBACK", "true").lower() == "true" or flexible_mode

        if allow_fallback:
            current = self.get_current_liquidity(token_address)
            if current:
                # Calculate confidence score based on multiple factors
                confidence_factors = []

                # Factor 1: Trade age (older trades = lower confidence)
                age_confidence = max(0.3, 1.0 - (trade_age.days / 90.0))  # Decays over 90 days
                confidence_factors.append(age_confidence)

                # Factor 2: Token age (newer tokens = more uncertain)
                if token_age_days is not None:
                    if token_age_days < 7:
                        token_confidence = 0.5  # Very new tokens are uncertain
                    elif token_age_days < 30:
                        token_confidence = 0.7  # Recent tokens
                    else:
                        token_confidence = 0.9  # Established tokens
                    confidence_factors.append(token_confidence)

                # Factor 3: Strategy-specific requirements
                if strategy == "SHIELD":
                    strategy_confidence = 0.8  # Shield is more conservative
                else:  # SPEAR
                    strategy_confidence = 0.6  # Spear accepts more risk
                confidence_factors.append(strategy_confidence)

                # Calculate overall confidence
                overall_confidence = min(confidence_factors) if confidence_factors else 0.5

                # Apply confidence-based penalty to liquidity
                # Lower confidence = more conservative haircut
                confidence_haircut = 0.3 + (1.0 - overall_confidence) * 0.4  # 30-70% haircut
                confidence_penalty_liquidity = current.liquidity_usd * (1.0 - confidence_haircut)

                # Cap to prevent survivorship bias
                max_fallback = 10000.0 if strategy == "SHIELD" else 5000.0
                safe_fallback_liquidity = min(confidence_penalty_liquidity, max_fallback)

                logger.info(
                    f"Confidence-weighted fallback: {token_address[:8]}... "
                    f"confidence={overall_confidence:.2f}, haircut={confidence_haircut*100:.0f}%, "
                    f"result=${safe_fallback_liquidity:,.0f}"
                )

                return LiquidityData(
                    token_address=current.token_address,
                    liquidity_usd=safe_fallback_liquidity,
                    price_usd=current.price_usd,
                    volume_24h_usd=current.volume_24h_usd,
                    timestamp=timestamp,
                    source=f"{current.source}_confidence_weighted_conf_{overall_confidence:.2f}",
                )

        # Tier 4: Last resort - simulated mode (only if enabled)
        simulated_mode = os.getenv("SCOUT_LIQUIDITY_MODE", "").lower() == "simulated"
        if simulated_mode:
            logger.warning(f"Simulated mode: Using simulated liquidity for {token_address[:8]}...")
            return self._get_simulated_liquidity(token_address, timestamp)

        # All sources failed
        logger.warning(f"All liquidity sources failed for {token_address[:8]}...")
        return None

    def _get_database_connection(self):
        """
        Helper to get a configured database connection with WAL mode.

        Supports both SQLite (development) and PostgreSQL (production) via
        SCOUT_DB_BACKEND environment variable.

        WAL (Write-Ahead Logging) mode allows concurrent reads while writes
        are in progress, preventing database locks when Rust Operator reads
        while Python Scout writes.
        """
        from .db import get_connection, execute_query

        conn = get_connection(self.db_path)  # 10s timeout for busy retries
        # WAL mode is enabled by default in get_connection for SQLite
        return conn

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
            conn = self._get_database_connection()
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
            conn = self._get_database_connection()
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
            conn = self._get_database_connection()
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
        volume_24h_usd: float = 0.0,
        token_age_days: float = 365.0,
    ) -> float:
        """
        Estimate slippage for a trade based on trade size vs liquidity.

        Uses a square root model: slippage increases with sqrt of trade size
        relative to liquidity. Enhanced with:
        - Volume/turnover volatility adjustment
        - Order-book depth proxy (bid/ask spread from volume/liquidity ratio)
        - Token age factor (newer tokens have wider spreads)

        Args:
            token_address: Token mint address
            amount_sol: Trade size in SOL
            liquidity_usd: Pool liquidity in USD
            sol_price_usd: SOL price in USD
            volume_24h_usd: 24h Volume in USD
            token_age_days: Age of the token in days (default 365 = very mature)

        Returns:
            Estimated slippage as a decimal (0.01 = 1%)
        """
        if liquidity_usd <= 0:
            return 1.0  # 100% slippage (trade would fail)

        trade_value_usd = amount_sol * sol_price_usd

        # Base Slippage (AMM Constant Product Approximation)
        base_slippage = 0.1 * math.sqrt(trade_value_usd / liquidity_usd)

        # Turnover factor: single multiplier from volume/liquidity ratio (saturating).
        # Combines what was previously split into volatility + depth multipliers
        # to avoid double-counting correlated inputs.
        turnover_factor = 1.0
        if liquidity_usd > 0 and volume_24h_usd > 0:
            turnover_ratio = volume_24h_usd / liquidity_usd
            if turnover_ratio > 20.0:
                turnover_factor = 3.0
            elif turnover_ratio > 10.0:
                turnover_factor = 2.0
            elif turnover_ratio > 3.0:
                turnover_factor = 1.5
            elif turnover_ratio > 1.0:
                turnover_factor = 1.2

        # Phase 5c: Token age factor — additive term, not multiplicative,
        # to avoid blowup when combined with high-turnover tokens.
        age_additive = 0.0
        if token_age_days < 365:
            if token_age_days < 1:
                age_additive = 0.03   # Up to +3% additional slippage for brand-new tokens
            elif token_age_days < 7:
                age_additive = 0.02   # Up to +2% for <1 week
            elif token_age_days < 30:
                age_additive = 0.01   # Up to +1% for <1 month
            elif token_age_days < 90:
                age_additive = 0.005  # Up to +0.5% for <3 months

        final_slippage = base_slippage * turnover_factor + age_additive

        # Remove hardcoded 0.05% floor - use pure market-based calculation
        # Small trades should have lower slippage, not a guaranteed minimum
        trade_size_component = min(0.005, trade_value_usd / 20000.0)
        return min(final_slippage + trade_size_component, 1.0)
    
    async def get_sol_price_usd(self) -> float:
        """
        Get current SOL price in USD.
        
        Returns:
            SOL price in USD
        """
        # Cache for short period
        if self._sol_price_cache:
            price, cached_at = self._sol_price_cache
            if (utcnow() - cached_at).total_seconds() < 60:
                return price

        # Try Jupiter client first (if available)
        if self.mode == "real" and self.jupiter_client:
            try:
                await self._rate_limit_async()
                price = await self.jupiter_client.get_sol_price_usd()
                if price and price > 0:
                    self._sol_price_cache = (price, utcnow())
                    return price
            except Exception as e:
                logger.debug(f"Jupiter SOL price failed: {e}")

        # Fallback: direct Jupiter API call (async)
        try:
            import aiohttp
            url = "https://lite-api.jup.ag/price/v2"
            async with aiohttp.ClientSession() as session:
                async with session.get(url, params={"ids": "So11111111111111111111111111111111111111112"}, timeout=aiohttp.ClientTimeout(total=10)) as resp:
                    resp.raise_for_status()
                    data = await resp.json() or {}
            price = (
                data.get("data", {})
                .get("So11111111111111111111111111111111111111112", {})
                .get("price")
            )
            if price is not None:
                price_f = float(price)
                if price_f > 0:
                    self._sol_price_cache = (price_f, utcnow())
                    return price_f
        except Exception as e:
            logger.debug(f"Direct Jupiter API call failed: {e}")

        # Fallback estimate (only if all else fails)
        logger.warning("Using fallback SOL price estimate: 150.0 USD")
        return 150.0

    def get_sol_price_usd_sync(self) -> float:
        """Synchronous wrapper for get_sol_price_usd using the in-memory cache.

        Used by the backtester which runs in a non-async context. Returns the
        most recently cached price, or a conservative fallback if no fresh
        cache entry exists (the async updater will correct it on the next run).
        """
        if self._sol_price_cache:
            price, cached_at = self._sol_price_cache
            if (utcnow() - cached_at).total_seconds() < 300:
                return price
        return 150.0  # Conservative fallback; corrected on next async refresh

    def cache_historical_sol_price(self, ts: datetime, price: float):
        """Record a historical SOL price observation for market regime classification.

        Called by the analyzer during trade processing to build a time-series
        window used by classify_market_regime(). Keeps the last 200 entries.
        """
        self._sol_price_history.append((ts, price))
        if len(self._sol_price_history) > 200:
            self._sol_price_history = self._sol_price_history[-200:]

    def classify_market_regime(
        self,
        start_ts: datetime,
        end_ts: datetime,
    ) -> Optional[str]:
        """
        Classify the SOL/USD market regime between two timestamps.

        Uses the historical price cache first (populated by cache_historical_sol_price).
        Falls back to a heuristic based on current price if insufficient history.

        Returns: "BULL", "BEAR", "SIDEWAYS", or None
        """
        span_days = (end_ts - start_ts).days
        if span_days < 7:
            return None

        # Try historical cache first
        prices_in_window = [
            p for t, p in self._sol_price_history
            if start_ts <= t <= end_ts
        ]
        if len(prices_in_window) >= 2:
            start_price = prices_in_window[0]
            end_price = prices_in_window[-1]
            change_pct = ((end_price - start_price) / start_price) * 100
        else:
            # Fallback heuristic
            current_price = self.get_sol_price_usd_sync()
            change_pct = (current_price - 150.0) / 150.0 * 100 if span_days > 30 else 0.0

        if change_pct > 20:
            return "BULL"
        elif change_pct < -20:
            return "BEAR"
        return "SIDEWAYS"

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
            liquidity = random.uniform(1000, 500000)
        
        # Simulate price (not critical for liquidity checks)
        price = random.uniform(0.0000001, 100.0)
        
        return LiquidityData(
            token_address=token_address,
            liquidity_usd=liquidity,
            price_usd=price,
            volume_24h_usd=liquidity * random.uniform(0.1, 2.0),
            timestamp=utcnow(),
            source="simulated",
        )
    
    def _get_from_cache(self, token_address: str) -> Optional[LiquidityData]:
        """
        Get liquidity data from cache (Redis or in-memory).
        
        Args:
            token_address: Token address
            
        Returns:
            Cached LiquidityData or None
        """
        # Try Redis first if available
        if self.redis_client and self.redis_client.is_available():
            try:
                cache_key = f"liquidity:{token_address}"
                cached_json = self.redis_client.get(cache_key)
                if cached_json:
                    data_dict = json.loads(cached_json)
                    # Reconstruct LiquidityData from dict
                    return LiquidityData(
                        token_address=data_dict["token_address"],
                        liquidity_usd=data_dict["liquidity_usd"],
                        price_usd=data_dict["price_usd"],
                        volume_24h_usd=data_dict.get("volume_24h_usd", 0.0),
                        timestamp=datetime.fromisoformat(data_dict["timestamp"]),
                        source=data_dict.get("source", "cache"),
                    )
            except Exception as e:
                logger.debug(f"Redis cache get failed for {token_address[:8]}...: {e}")
        
        # Fallback to in-memory cache
        if token_address in self._cache:
            data, cached_time = self._cache[token_address]
            age = (utcnow() - cached_time).total_seconds()
            if age < self.cache_ttl:
                return data
            else:
                # Expired, remove from cache
                del self._cache[token_address]
        
        return None
    
    def _add_to_cache(self, token_address: str, data: LiquidityData) -> None:
        """
        Add liquidity data to cache (Redis or in-memory).
        
        Args:
            token_address: Token address
            data: LiquidityData to cache
        """
        # Try Redis first if available
        if self.redis_client and self.redis_client.is_available():
            try:
                cache_key = f"liquidity:{token_address}"
                data_dict = {
                    "token_address": data.token_address,
                    "liquidity_usd": data.liquidity_usd,
                    "price_usd": data.price_usd,
                    "volume_24h_usd": data.volume_24h_usd,
                    "timestamp": data.timestamp.isoformat(),
                    "source": data.source,
                }
                self.redis_client.set(cache_key, json.dumps(data_dict), ttl_seconds=self.cache_ttl)
                return
            except Exception as e:
                logger.debug(f"Redis cache set failed for {token_address[:8]}...: {e}")

        # Fallback to in-memory cache
        self._cache[token_address] = (data, utcnow())
    
    def clear_cache(self) -> None:
        """Clear the liquidity cache (Redis and in-memory)."""
        if self.redis_client and self.redis_client.is_available():
            try:
                self.redis_client.clear()
            except Exception as e:
                logger.debug(f"Redis cache clear failed: {e}")
        self._cache.clear()

    async def close(self) -> None:
        """Close all underlying client sessions."""
        if self.birdeye_client:
            try:
                await self.birdeye_client.close()
            except Exception as e:
                logger.debug(f"Failed to close Birdeye client: {e}")
        if self.jupiter_client:
            try:
                await self.jupiter_client.close()
            except Exception as e:
                logger.debug(f"Failed to close Jupiter client: {e}")


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
        utcnow() - timedelta(days=30)
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
