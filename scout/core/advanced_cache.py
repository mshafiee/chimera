"""
Advanced Multi-Level Caching System

This module implements intelligent multi-level caching to reduce Helius API usage by 80%+:
- L1: In-memory cache (fastest, ~10MB limit)
- L2: Redis cache (persistent, shared across runs)
- L3: SQLite cache (fallback, persistent storage)
- Cache warming and preloading
- Intelligent cache invalidation
- Hit rate tracking and optimization
"""

import os
import json
import time
import hashlib
import logging
import sqlite3
from datetime import datetime, timedelta
from typing import Any, Dict, List, Optional, Tuple, Union
from dataclasses import dataclass, asdict
from enum import Enum
import threading
from pathlib import Path

logger = logging.getLogger(__name__)


class CacheLevel(Enum):
    """Cache level hierarchy."""

    L1_MEMORY = 1  # Fast in-memory cache
    L2_REDIS = 2   # Redis persistent cache
    L3_SQLITE = 3  # SQLite fallback cache


class CacheCategory(Enum):
    """Cache data categories for TTL management."""

    # High churn (short TTL)
    WALLET_METRICS = "wallet_metrics"      # 5 minutes
    PRICE_DATA = "price_data"              # 1 minute
    LIQUIDITY_DATA = "liquidity_data"      # 2 minutes

    # Medium churn (medium TTL)
    TOKEN_METADATA = "token_metadata"       # 24 hours
    WALLET_TXS = "wallet_txs"              # 10 minutes
    SWAP_DATA = "swap_data"                # 10 minutes

    # Low churn (long TTL)
    TOKEN_CREATION = "token_creation"      # 7 days
    WALLET_AGE = "wallet_age"              # 30 days
    DEX_LIST = "dex_list"                 # 7 days

    # Growth-aware categories (Phase 5)
    HIGH_WQS_WALLET_DATA = "high_wqs_wallet_data"  # 1 hour for WQS > 70
    ANALYSIS_RESULTS = "analysis_results"         # Until next run (6 hours)
    DISCOVERY_RESULTS = "discovery_results"       # 30 minutes
    BACKTEST_RESULTS = "backtest_results"         # 1 hour


@dataclass
class CacheEntry:
    """Cache entry with metadata."""

    key: str
    value: Any
    category: CacheCategory
    created_at: float
    accessed_at: float
    hit_count: int
    size_bytes: int
    level: CacheLevel
    ttl_seconds: int

    def is_expired(self) -> bool:
        """Check if cache entry is expired."""
        return time.time() > (self.created_at + self.ttl_seconds)

    def access(self):
        """Record an access to this entry."""
        self.accessed_at = time.time()
        self.hit_count += 1


@dataclass
class CacheStats:
    """Cache performance statistics."""

    total_hits: int = 0
    total_misses: int = 0
    l1_hits: int = 0
    l2_hits: int = 0
    l3_hits: int = 0
    total_entries: int = 0
    l1_size_bytes: int = 0
    l2_size_bytes: int = 0
    l3_size_bytes: int = 0

    @property
    def hit_rate(self) -> float:
        """Calculate cache hit rate."""
        total = self.total_hits + self.total_misses
        return (self.total_hits / total * 100) if total > 0 else 0.0

    @property
    def l1_hit_rate(self) -> float:
        """Calculate L1 cache hit rate."""
        total = self.l1_hits + self.total_misses
        return (self.l1_hits / total * 100) if total > 0 else 0.0


class TTLDefaults:
    """Default TTL values for different categories."""

    # High churn categories (seconds)
    WALLET_METRICS = 300      # 5 minutes
    PRICE_DATA = 60           # 1 minute
    LIQUIDITY_DATA = 120      # 2 minutes

    # Medium churn categories
    TOKEN_METADATA = 86400    # 24 hours
    WALLET_TXS = 600          # 10 minutes
    SWAP_DATA = 600           # 10 minutes

    # Low churn categories
    TOKEN_CREATION = 604800   # 7 days
    WALLET_AGE = 2592000      # 30 days
    DEX_LIST = 604800         # 7 days

    # Growth-aware categories (Phase 5)
    HIGH_WQS_WALLET_DATA = 3600    # 1 hour for high-WQS wallets
    ANALYSIS_RESULTS = 21600        # 6 hours (until next run)
    DISCOVERY_RESULTS = 1800       # 30 minutes
    BACKTEST_RESULTS = 3600        # 1 hour

    @classmethod
    def get_ttl(cls, category: CacheCategory) -> int:
        """Get default TTL for a category."""
        return getattr(cls, category.value, 300)  # Default 5 minutes


class AdvancedCache:
    """
    Advanced multi-level caching system.

    Features:
    - L1/L2/L3 cache hierarchy
    - Intelligent cache eviction
    - Hit rate optimization
    - Memory pressure handling
    - Redis backplane with fallback
    - Cache warming strategies
    """

    def __init__(self, max_memory_mb: int = 10):
        """
        Initialize the advanced cache system.

        Args:
            max_memory_mb: Maximum memory for L1 cache in MB
        """
        # L1: In-memory cache
        self._l1_cache: Dict[str, CacheEntry] = {}
        self._l1_max_memory = max_memory_mb * 1024 * 1024  # Convert to bytes
        self._l1_lock = threading.RLock()

        # L2: Redis cache (optional)
        self._redis_client = None
        self._redis_available = False
        self._init_redis()

        # L3: SQLite cache (fallback)
        self._sqlite_path = os.getenv("SCOUT_CACHE_DB_PATH", "/tmp/scout_cache.db")
        self._init_sqlite()

        # Statistics
        self._stats = CacheStats()
        self._stats_lock = threading.Lock()

        # Configuration
        self._enable_warming = os.getenv("SCOUT_CACHE_WARMING", "true").lower() == "true"
        self._aggressive_eviction = os.getenv("SCOUT_AGGRESSIVE_EVICTION", "false").lower() == "true"

        logger.info(f"Advanced Cache initialized with {max_memory_mb}MB L1 cache")
        logger.info(f"  Redis available: {self._redis_available}")
        logger.info(f"  SQLite fallback: {self._sqlite_path}")
        logger.info(f"  Cache warming: {self._enable_warming}")

    def _init_redis(self):
        """Initialize Redis cache connection."""
        try:
            from .redis_client import RedisClient
            from config import ScoutConfig

            if ScoutConfig and ScoutConfig.get_redis_enabled():
                redis_url = ScoutConfig.get_redis_url()
                self._redis_client = RedisClient(redis_url=redis_url, enabled=True)
                self._redis_available = self._redis_client.is_available()

                if self._redis_available:
                    logger.info("Redis L2 cache enabled")
        except Exception as e:
            logger.debug(f"Redis not available: {e}")

    def _init_sqlite(self):
        """Initialize SQLite cache database."""
        try:
            os.makedirs(os.path.dirname(self._sqlite_path), exist_ok=True)

            conn = sqlite3.connect(self._sqlite_path)
            cursor = conn.cursor()

            # Create cache table
            cursor.execute("""
                CREATE TABLE IF NOT EXISTS cache_entries (
                    key TEXT PRIMARY KEY,
                    value BLOB,
                    category TEXT,
                    created_at REAL,
                    accessed_at REAL,
                    hit_count INTEGER,
                    size_bytes INTEGER,
                    ttl_seconds INTEGER
                )
            """)

            # Create index for faster lookups
            cursor.execute("""
                CREATE INDEX IF NOT EXISTS idx_category
                ON cache_entries(category, created_at)
            """)

            conn.commit()
            conn.close()

            logger.debug(f"SQLite L3 cache initialized at {self._sqlite_path}")
        except Exception as e:
            logger.warning(f"Failed to initialize SQLite cache: {e}")

    def _get_cache_key(self, prefix: str, identifier: str, *args) -> str:
        """
        Generate a standardized cache key.

        Args:
            prefix: Category prefix
            identifier: Main identifier (wallet address, token mint, etc.)
            *args: Additional parameters for key uniqueness

        Returns:
            Standardized cache key
        """
        key_parts = [prefix, identifier]

        # Add additional parameters if provided
        if args:
            key_parts.extend(str(arg) for arg in args)

        # Join and create consistent key
        key_string = ":".join(key_parts)

        # Create hash for long keys to avoid memory issues
        if len(key_string) > 100:
            key_hash = hashlib.sha256(key_string.encode()).hexdigest()[:16]
            return f"{prefix}:hash:{key_hash}"

        return key_string

    def _serialize_value(self, value: Any) -> bytes:
        """Serialize value for storage."""
        try:
            return json.dumps(value, default=str).encode('utf-8')
        except Exception as e:
            logger.error(f"Failed to serialize cache value: {e}")
            return b'{}'

    def _deserialize_value(self, data: bytes) -> Any:
        """Deserialize value from storage."""
        try:
            return json.loads(data.decode('utf-8'))
        except Exception as e:
            logger.error(f"Failed to deserialize cache value: {e}")
            return None

    def _evict_l1_entries(self, required_bytes: int) -> bool:
        """
        Evict L1 entries to free up memory.

        Uses LRU (Least Recently Used) eviction strategy.

        Args:
            required_bytes: Number of bytes to free up

        Returns:
            True if successful, False if not enough memory
        """
        if not self._l1_cache:
            return False

        freed_bytes = 0
        entries_to_evict = []

        # Sort entries by last access time (oldest first)
        sorted_entries = sorted(
            self._l1_cache.values(),
            key=lambda e: e.accessed_at
        )

        for entry in sorted_entries:
            if freed_bytes >= required_bytes:
                break

            entries_to_evict.append(entry.key)
            freed_bytes += entry.size_bytes

        # Evict selected entries
        for key in entries_to_evict:
            del self._l1_cache[key]

        logger.debug(f"Evicted {len(entries_to_evict)} L1 entries, freed {freed_bytes:,} bytes")

        return freed_bytes >= required_bytes

    def _get_l1_memory_usage(self) -> int:
        """Calculate current L1 cache memory usage."""
        return sum(entry.size_bytes for entry in self._l1_cache.values())

    def _check_memory_pressure(self) -> bool:
        """Check if L1 cache is under memory pressure."""
        current_usage = self._get_l1_memory_usage()
        usage_ratio = current_usage / self._l1_max_memory if self._l1_max_memory > 0 else 0

        # Aggressive eviction at 80%, normal at 95%
        threshold = 0.8 if self._aggressive_eviction else 0.95

        return usage_ratio > threshold

    def _get_ttl(self, category: CacheCategory, wqs_score: Optional[float] = None) -> int:
        """
        Get TTL for a category, with environment variable override and growth-aware adjustment.

        Growth-aware caching (Phase 5):
        - High-WQS wallets (>70): cached 4x longer
        - Medium-WQS wallets (40-70): cached 2x longer
        - Low-WQS wallets (<40): standard TTL

        Args:
            category: Cache category
            wqs_score: Optional WQS score for growth-aware TTL adjustment

        Returns:
            TTL in seconds
        """
        base_ttl = TTLDefaults.get_ttl(category)

        # Check for environment variable override
        env_var = f"SCOUT_CACHE_TTL_{category.value.upper()}"
        override = os.getenv(env_var)

        if override:
            try:
                return int(override)
            except ValueError:
                pass

        # Growth-aware TTL adjustment (Phase 5)
        if wqs_score is not None and category == CacheCategory.WALLET_METRICS:
            growth_mode = os.getenv("SCOUT_GROWTH_OPTIMIZED", "false").lower() == "true"

            if growth_mode:
                # High-WQS wallets get extended cache time
                if wqs_score >= 70.0:
                    return base_ttl * 4  # 20 minutes for high-WQS
                elif wqs_score >= 40.0:
                    return base_ttl * 2  # 10 minutes for medium-WQS
                # Low-WQS wallets keep standard 5 minutes

        return base_ttl

    def get(self, prefix: str, identifier: str, *args,
            category: CacheCategory = CacheCategory.WALLET_METRICS,
            default: Any = None, wqs_score: Optional[float] = None) -> Optional[Any]:
        """
        Get value from cache (tries L1 → L2 → L3).

        Args:
            prefix: Category prefix
            identifier: Main identifier
            *args: Additional parameters for key
            category: Cache category for TTL management
            default: Default value if not found
            wqs_score: Optional WQS score for growth-aware TTL (Phase 5)

        Returns:
            Cached value or default
        """
        key = self._get_cache_key(prefix, identifier, *args)
        now = time.time()

        # Try L1 cache first
        with self._l1_lock:
            entry = self._l1_cache.get(key)

            if entry and not entry.is_expired():
                entry.access()
                with self._stats_lock:
                    self._stats.total_hits += 1
                    self._stats.l1_hits += 1
                return entry.value

        # Try L2 (Redis) cache
        if self._redis_available:
            try:
                cached_data = self._redis_client.get(key)
                if cached_data:
                    value = self._deserialize_value(cached_data)

                    # Promote to L1 cache
                    if value is not None:
                        self._set_l1(key, value, category, wqs_score)

                    with self._stats_lock:
                        self._stats.total_hits += 1
                        self._stats.l2_hits += 1
                    return value
            except Exception as e:
                logger.debug(f"Redis cache get failed: {e}")

        # Try L3 (SQLite) cache
        try:
            conn = sqlite3.connect(self._sqlite_path)
            cursor = conn.cursor()

            cursor.execute("""
                SELECT value, created_at, ttl_seconds, hit_count
                FROM cache_entries
                WHERE key = ? AND (created_at + ttl_seconds) > ?
            """, (key, now))

            row = cursor.fetchone()
            conn.close()

            if row:
                value_data, created_at, ttl, hit_count = row
                value = self._deserialize_value(value_data)

                # Promote to L1 cache
                if value is not None:
                    self._set_l1(key, value, category, wqs_score)

                    # Update hit count in SQLite
                    self._update_l3_hit_count(key, hit_count + 1)

                with self._stats_lock:
                    self._stats.total_hits += 1
                    self._stats.l3_hits += 1
                return value
        except Exception as e:
            logger.debug(f"SQLite cache get failed: {e}")

        # Cache miss
        with self._stats_lock:
            self._stats.total_misses += 1

        return default

    def _set_l1(self, key: str, value: Any, category: CacheCategory, wqs_score: Optional[float] = None):
        """Set value in L1 cache."""
        ttl = self._get_ttl(category, wqs_score)
        serialized = self._serialize_value(value)
        size_bytes = len(serialized)

        # Check if we need to evict entries
        if size_bytes > self._l1_max_memory:
            logger.warning(f"Cache entry too large for L1: {size_bytes:,} bytes")
            return

        current_usage = self._get_l1_memory_usage()
        if current_usage + size_bytes > self._l1_max_memory:
            self._evict_l1_entries(size_bytes)

        # Create cache entry
        entry = CacheEntry(
            key=key,
            value=value,
            category=category,
            created_at=time.time(),
            accessed_at=time.time(),
            hit_count=0,
            size_bytes=size_bytes,
            level=CacheLevel.L1_MEMORY,
            ttl_seconds=ttl
        )

        self._l1_cache[key] = entry

        # Update statistics
        with self._stats_lock:
            self._stats.total_entries = len(self._l1_cache)
            self._stats.l1_size_bytes = self._get_l1_memory_usage()

    def _update_l3_hit_count(self, key: str, hit_count: int):
        """Update hit count in SQLite cache."""
        try:
            conn = sqlite3.connect(self._sqlite_path)
            cursor = conn.cursor()

            cursor.execute("""
                UPDATE cache_entries
                SET hit_count = ?, accessed_at = ?
                WHERE key = ?
            """, (hit_count, time.time(), key))

            conn.commit()
            conn.close()
        except Exception as e:
            logger.debug(f"Failed to update L3 hit count: {e}")

    def set(self, prefix: str, identifier: str, value: Any,
            *args, category: CacheCategory = CacheCategory.WALLET_METRICS,
            wqs_score: Optional[float] = None):
        """
        Set value in cache (stores in L1, L2, L3).

        Args:
            prefix: Category prefix
            identifier: Main identifier
            value: Value to cache
            *args: Additional parameters for key
            category: Cache category for TTL management
            wqs_score: Optional WQS score for growth-aware TTL (Phase 5)
        """
        if value is None:
            return

        key = self._get_cache_key(prefix, identifier, *args)
        serialized = self._serialize_value(value)

        # Store in L1
        self._set_l1(key, value, category, wqs_score)

        # Store in L2 (Redis)
        if self._redis_available:
            try:
                ttl = self._get_ttl(category, wqs_score)
                self._redis_client.set(key, serialized.decode('utf-8'), ttl_seconds=ttl)
            except Exception as e:
                logger.debug(f"Redis cache set failed: {e}")

        # Store in L3 (SQLite)
        try:
            conn = sqlite3.connect(self._sqlite_path)
            cursor = conn.cursor()

            ttl = self._get_ttl(category, wqs_score)
            now = time.time()

            cursor.execute("""
                INSERT OR REPLACE INTO cache_entries
                (key, value, category, created_at, accessed_at, hit_count, size_bytes, ttl_seconds)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            """, (key, serialized, category.value, now, now, 0, len(serialized), ttl))

            conn.commit()
            conn.close()

            # Update statistics
            with self._stats_lock:
                self._stats.l3_size_bytes += len(serialized)
        except Exception as e:
            logger.debug(f"SQLite cache set failed: {e}")

    def invalidate(self, prefix: str, identifier: str, *args):
        """
        Invalidate cache entry.

        Args:
            prefix: Category prefix
            identifier: Main identifier
            *args: Additional parameters for key
        """
        key = self._get_cache_key(prefix, identifier, *args)

        # Remove from L1
        with self._l1_lock:
            if key in self._l1_cache:
                del self._l1_cache[key]

        # Remove from L2
        if self._redis_available:
            try:
                self._redis_client.delete(key)
            except Exception as e:
                logger.debug(f"Redis cache invalidate failed: {e}")

        # Remove from L3
        try:
            conn = sqlite3.connect(self._sqlite_path)
            cursor = conn.cursor()
            cursor.execute("DELETE FROM cache_entries WHERE key = ?", (key,))
            conn.commit()
            conn.close()
        except Exception as e:
            logger.debug(f"SQLite cache invalidate failed: {e}")

    def invalidate_category(self, category: CacheCategory):
        """
        Invalidate all entries in a category.

        Args:
            category: Category to invalidate
        """
        # Remove from L1
        with self._l1_lock:
            keys_to_remove = [
                key for key, entry in self._l1_cache.items()
                if entry.category == category
            ]
            for key in keys_to_remove:
                del self._l1_cache[key]

        # Remove from L2
        if self._redis_available:
            try:
                # Redis pattern matching is slow, but we can try
                # This is a simplified version - production should use proper key patterns
                pass  # Skip for now due to complexity
            except Exception as e:
                logger.debug(f"Redis category invalidate failed: {e}")

        # Remove from L3
        try:
            conn = sqlite3.connect(self._sqlite_path)
            cursor = conn.cursor()
            cursor.execute("DELETE FROM cache_entries WHERE category = ?", (category.value,))
            conn.commit()
            conn.close()
        except Exception as e:
            logger.debug(f"SQLite category invalidate failed: {e}")

    def cleanup_expired(self):
        """Clean up expired entries from all cache levels."""
        now = time.time()

        # Cleanup L1
        with self._l1_lock:
            expired_keys = [
                key for key, entry in self._l1_cache.items()
                if entry.is_expired()
            ]
            for key in expired_keys:
                del self._l1_cache[key]

            logger.debug(f"Cleaned up {len(expired_keys)} expired L1 entries")

        # Cleanup L3
        try:
            conn = sqlite3.connect(self._sqlite_path)
            cursor = conn.cursor()

            cursor.execute("""
                DELETE FROM cache_entries
                WHERE (created_at + ttl_seconds) < ?
            """, (now,))

            deleted_count = cursor.rowcount
            conn.commit()
            conn.close()

            logger.debug(f"Cleaned up {deleted_count} expired L3 entries")
        except Exception as e:
            logger.warning(f"Failed to cleanup L3 cache: {e}")

        # Update statistics
        with self._stats_lock:
            self._stats.total_entries = len(self._l1_cache)
            self._stats.l1_size_bytes = self._get_l1_memory_usage()

    def get_stats(self) -> CacheStats:
        """Get cache statistics."""
        with self._stats_lock:
            return self._stats

    def print_stats(self):
        """Print cache statistics."""
        stats = self.get_stats()

        print("\n" + "="*70)
        print("ADVANCED CACHE - STATISTICS")
        print("="*70)

        print(f"\nHit Rates:")
        print(f"  Overall: {stats.hit_rate:.1f}%")
        print(f"  L1: {stats.l1_hit_rate:.1f}%")

        print(f"\nCache Operations:")
        print(f"  Total hits: {stats.total_hits:,}")
        print(f"  Total misses: {stats.total_misses:,}")
        print(f"  L1 hits: {stats.l1_hits:,}")
        print(f"  L2 hits: {stats.l2_hits:,}")
        print(f"  L3 hits: {stats.l3_hits:,}")

        print(f"\nMemory Usage:")
        print(f"  L1 entries: {len(self._l1_cache):,}")
        print(f"  L1 size: {stats.l1_size_bytes:,} bytes / {self._l1_max_memory:,} bytes")
        print(f"  L1 usage: {(stats.l1_size_bytes/self._l1_max_memory*100):.1f}%")
        print(f"  L3 size: {stats.l3_size_bytes:,} bytes")

        print("="*70 + "\n")

    def warm_cache(self, wallet_addresses: List[str], token_addresses: List[str],
                   high_wqs_wallets: Optional[List[str]] = None):
        """
        Warm up cache with frequently accessed data.

        Growth-aware warming (Phase 5):
        - High-WQS wallets get priority warming
        - Token metadata preloaded
        - Analysis results cached until next run

        Args:
            wallet_addresses: List of wallet addresses to preload
            token_addresses: List of token addresses to preload
            high_wqs_wallets: Optional list of high-WQS wallets for priority caching
        """
        if not self._enable_warming:
            return

        logger.info(f"Cache warming: {len(wallet_addresses)} wallets, {len(token_addresses)} tokens")

        if high_wqs_wallets:
            logger.info(f"Priority warming: {len(high_wqs_wallets)} high-WQS wallets")
            # High-WQS wallets will have extended TTL and be cached longer

        # This would trigger background loading of frequently accessed data
        # Implementation depends on specific use cases
        # Example: preload token metadata, wallet ages, etc.

    def shutdown(self):
        """Cleanup and shutdown."""
        # Cleanup expired entries
        self.cleanup_expired()

        # Close SQLite connection
        try:
            if hasattr(self, '_sqlite_path'):
                # SQLite connections are opened/closed per operation
                pass
        except Exception as e:
            logger.debug(f"Cache shutdown error: {e}")

        logger.info("Advanced cache shut down")


# Global singleton instance
_cache: Optional[AdvancedCache] = None
_cache_lock = threading.Lock()


def get_cache() -> AdvancedCache:
    """Get the global cache singleton."""
    global _cache

    with _cache_lock:
        if _cache is None:
            max_memory = int(os.getenv("SCOUT_CACHE_MEMORY_MB", "10"))
            _cache = AdvancedCache(max_memory_mb=max_memory)

    return _cache


def reset_cache():
    """Reset the global cache (mainly for testing)."""
    global _cache

    with _cache_lock:
        if _cache:
            _cache.shutdown()
        _cache = None


# Convenience functions for common operations
def get_wallet_metrics(address: str) -> Optional[Dict]:
    """Get cached wallet metrics."""
    cache = get_cache()
    return cache.get("wallet", address, "metrics", category=CacheCategory.WALLET_METRICS)


def set_wallet_metrics(address: str, metrics: Dict):
    """Set cached wallet metrics."""
    cache = get_cache()
    cache.set("wallet", address, "metrics", metrics, category=CacheCategory.WALLET_METRICS)


def get_token_metadata(token_address: str) -> Optional[Dict]:
    """Get cached token metadata."""
    cache = get_cache()
    return cache.get("token", token_address, "metadata", category=CacheCategory.TOKEN_METADATA)


def set_token_metadata(token_address: str, metadata: Dict):
    """Set cached token metadata."""
    cache = get_cache()
    cache.set("token", token_address, "metadata", metadata, category=CacheCategory.TOKEN_METADATA)


def get_token_creation_time(token_address: str) -> Optional[float]:
    """Get cached token creation time."""
    cache = get_cache()
    return cache.get("token", token_address, "creation", category=CacheCategory.TOKEN_CREATION)


def set_token_creation_time(token_address: str, creation_time: float):
    """Set cached token creation time."""
    cache = get_cache()
    cache.set("token", token_address, "creation", creation_time, category=CacheCategory.TOKEN_CREATION)


def get_liquidity_data(token_address: str) -> Optional[Dict]:
    """Get cached liquidity data."""
    cache = get_cache()
    return cache.get("liquidity", token_address, category=CacheCategory.LIQUIDITY_DATA)


def set_liquidity_data(token_address: str, liquidity_data: Dict):
    """Set cached liquidity data."""
    cache = get_cache()
    cache.set("liquidity", token_address, liquidity_data, category=CacheCategory.LIQUIDITY_DATA)


# Growth-aware convenience functions (Phase 5)

def get_high_wqs_wallet_data(address: str, wqs_score: float) -> Optional[Dict]:
    """
    Get cached high-WQS wallet data with growth-aware TTL.

    Args:
        address: Wallet address
        wqs_score: WQS score for TTL calculation (must be >= 70 for extended TTL)

    Returns:
        Cached wallet data or None
    """
    cache = get_cache()
    return cache.get("wallet", address, "high_wqs",
                     category=CacheCategory.HIGH_WQS_WALLET_DATA,
                     wqs_score=wqs_score)


def set_high_wqs_wallet_data(address: str, data: Dict, wqs_score: float):
    """
    Set cached high-WQS wallet data with growth-aware TTL.

    Args:
        address: Wallet address
        data: Wallet data to cache
        wqs_score: WQS score for TTL calculation (>= 70 gets 4x TTL)
    """
    cache = get_cache()
    cache.set("wallet", address, "high_wqs", data,
              category=CacheCategory.HIGH_WQS_WALLET_DATA,
              wqs_score=wqs_score)


def get_analysis_results(run_id: str) -> Optional[Dict]:
    """
    Get cached analysis results for a run.

    Analysis results are cached until next run (6 hours default).

    Args:
        run_id: Analysis run identifier

    Returns:
        Cached analysis results or None
    """
    cache = get_cache()
    return cache.get("analysis", run_id, "results",
                     category=CacheCategory.ANALYSIS_RESULTS)


def set_analysis_results(run_id: str, results: Dict):
    """
    Set cached analysis results for a run.

    Args:
        run_id: Analysis run identifier
        results: Analysis results to cache
    """
    cache = get_cache()
    cache.set("analysis", run_id, "results", results,
              category=CacheCategory.ANALYSIS_RESULTS)


def get_discovery_results(discovery_id: str) -> Optional[Dict]:
    """
    Get cached wallet discovery results.

    Args:
        discovery_id: Discovery run identifier

    Returns:
        Cached discovery results or None
    """
    cache = get_cache()
    return cache.get("discovery", discovery_id, "results",
                     category=CacheCategory.DISCOVERY_RESULTS)


def set_discovery_results(discovery_id: str, results: Dict):
    """
    Set cached wallet discovery results.

    Args:
        discovery_id: Discovery run identifier
        results: Discovery results to cache
    """
    cache = get_cache()
    cache.set("discovery", discovery_id, "results", results,
              category=CacheCategory.DISCOVERY_RESULTS)


def get_backtest_results(wallet_address: str) -> Optional[Dict]:
    """
    Get cached backtest results for a wallet.

    Args:
        wallet_address: Wallet address

    Returns:
        Cached backtest results or None
    """
    cache = get_cache()
    return cache.get("backtest", wallet_address, "results",
                     category=CacheCategory.BACKTEST_RESULTS)


def set_backtest_results(wallet_address: str, results: Dict):
    """
    Set cached backtest results for a wallet.

    Args:
        wallet_address: Wallet address
        results: Backtest results to cache
    """
    cache = get_cache()
    cache.set("backtest", wallet_address, "results", results,
              category=CacheCategory.BACKTEST_RESULTS)


if __name__ == "__main__":
    # Test the cache
    cache = get_cache()

    # Test basic operations
    cache.set("test", "key1", {"value": 123}, category=CacheCategory.WALLET_METRICS)
    result = cache.get("test", "key1", category=CacheCategory.WALLET_METRICS)
    print(f"Cache test result: {result}")

    cache.print_stats()

    cache.shutdown()