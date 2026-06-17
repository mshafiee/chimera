"""
Activity-Based Caching for Intelligent Cache Invalidation

This module implements intelligent caching based on wallet activity patterns
to maximize cache hit rates and minimize API calls under Helius Developer Plan.

Activity Levels:
- Very High (>50 tx/day): 5-minute cache, aggressive invalidation
- High (10-50 tx/day): 15-minute cache
- Medium (1-10 tx/day): 1-hour cache
- Low (<1 tx/day): 6-hour cache
- Inactive: 24-hour cache, scheduled cleanup

Features:
- Dynamic TTL based on wallet activity level
- WQS-based cache extension for high-conviction wallets
- Activity tracking and prediction
- Scheduled cleanup of inactive entries
"""

import os
import time
import logging
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Tuple, Any, Set
from dataclasses import dataclass, field
from enum import Enum
import threading
import json
from pathlib import Path
from collections import defaultdict

logger = logging.getLogger(__name__)


class ActivityLevel(Enum):
    """Wallet activity levels based on transaction frequency."""
    VERY_HIGH = "very_high"  # > 50 tx/day
    HIGH = "high"            # 10-50 tx/day
    MEDIUM = "medium"        # 1-10 tx/day
    LOW = "low"              # < 1 tx/day
    INACTIVE = "inactive"    # No recent activity


@dataclass
class ActivityData:
    """Activity tracking data for a wallet."""
    wallet_address: str
    tx_count_24h: int
    tx_count_1h: int
    tx_count_5m: int
    last_activity_timestamp: float
    last_cache_update: float = field(default_factory=time.time)
    wqs_score: Optional[float] = None
    predicted_hit_rate: float = 0.5


@dataclass
class CacheEntry:
    """Cache entry with metadata."""
    key: str
    value: Any
    created_at: float
    last_accessed: float
    ttl_seconds: int
    activity_level: ActivityLevel
    access_count: int = 0
    size_bytes: int = 0

    def is_expired(self) -> bool:
        """Check if cache entry is expired."""
        return (time.time() - self.last_accessed) > self.ttl_seconds

    def time_until_expiry(self) -> float:
        """Get seconds until expiry."""
        return max(0, self.ttl_seconds - (time.time() - self.last_accessed))


@dataclass
class CacheConfig:
    """Configuration for activity-based cache."""

    # Activity thresholds (transactions per day)
    VERY_HIGH_MIN_TX: int = 50
    HIGH_MIN_TX: int = 10
    MEDIUM_MIN_TX: int = 1

    # Base TTL by activity level (seconds)
    VERY_HIGH_TTL: int = 300      # 5 minutes
    HIGH_TTL: int = 900           # 15 minutes
    MEDIUM_TTL: int = 3600         # 1 hour
    LOW_TTL: int = 21600           # 6 hours
    INACTIVE_TTL: int = 86400      # 24 hours

    # WQS-based TTL multipliers
    HIGH_WQS_THRESHOLD: float = 70.0
    HIGH_WQS_MULTIPLIER: float = 2.0

    # Activity refresh multipliers
    RECENT_ACTIVITY_MULTIPLIER: float = 1.5
    RECENT_ACTIVITY_THRESHOLD: int = 3600  # 1 hour

    # Cache limits
    MAX_ENTRIES: int = 10000
    MAX_MEMORY_MB: int = 100

    # Cleanup settings
    CLEANUP_INTERVAL_SECONDS: int = 3600  # 1 hour
    INACTIVE_HOURS_THRESHOLD: int = 24    # Clean after 24h inactive

    # Prediction settings
    PREDICTION_WINDOW_HOURS: int = 24
    MIN_PREDICTION_SAMPLES: int = 5

    # State persistence
    STATE_FILE: str = "activity_cache_state.json"


class ActivityBasedCache:
    """
    Activity-based caching with intelligent invalidation.

    Strategy:
    - Track wallet activity levels
    - Set dynamic TTL based on activity
    - Extend cache for high-WQS wallets
    - Schedule cleanup of inactive entries

    Features:
    - Dynamic TTL calculation
    - Activity tracking and prediction
    - High-WQS cache extension
    - Scheduled cleanup
    """

    def __init__(self, config: Optional[CacheConfig] = None):
        """Initialize the activity-based cache."""
        self._config = config or CacheConfig()
        self._lock = threading.RLock()  # Reentrant for nested calls

        # Cache storage
        self._cache: Dict[str, CacheEntry] = {}

        # Activity tracking
        self._activity_data: Dict[str, ActivityData] = {}

        # Statistics
        self._stats = {
            'hits': 0,
            'misses': 0,
            'evictions': 0,
            'expirations': 0,
        }

        # Memory tracking
        self._total_memory_bytes = 0

        # Last cleanup time
        self._last_cleanup = time.time()

        logger.info("ActivityBasedCache initialized")

    def should_cache_wallet(self, wallet: str, activity_level: ActivityLevel) -> bool:
        """
        Determine if a wallet should be cached based on activity.

        Args:
            wallet: Wallet address
            activity_level: Current activity level

        Returns:
            True if wallet should be cached
        """
        with self._lock:
            # Always cache active wallets
            if activity_level in [ActivityLevel.VERY_HIGH, ActivityLevel.HIGH]:
                return True

            # Cache medium activity if we have space
            if activity_level == ActivityLevel.MEDIUM:
                return len(self._cache) < (self._config.MAX_ENTRIES * 0.8)

            # Low activity only if high WQS
            if activity_level == ActivityLevel.LOW:
                if wallet in self._activity_data:
                    wqs = self._activity_data[wallet].wqs_score
                    if wqs and wqs >= self._config.HIGH_WQS_THRESHOLD:
                        return True

            # Don't cache inactive wallets
            return False

    def get_cache_ttl(self, wallet: str, last_activity: float) -> int:
        """
        Get appropriate cache TTL for a wallet.

        Args:
            wallet: Wallet address
            last_activity: Timestamp of last activity

        Returns:
            TTL in seconds
        """
        with self._lock:
            # Get activity level
            activity_level = self._get_activity_level(wallet)

            # Get base TTL
            base_ttl = self._get_base_ttl(activity_level)

            # Apply WQS multiplier if applicable
            ttl = base_ttl
            if wallet in self._activity_data:
                wqs = self._activity_data[wallet].wqs_score
                if wqs and wqs >= self._config.HIGH_WQS_THRESHOLD:
                    ttl = int(base_ttl * self._config.HIGH_WQS_MULTIPLIER)

            # Apply recent activity multiplier
            time_since_activity = time.time() - last_activity
            if time_since_activity < self._config.RECENT_ACTIVITY_THRESHOLD:
                ttl = int(ttl * self._config.RECENT_ACTIVITY_MULTIPLIER)

            return ttl

    def _get_activity_level(self, wallet: str) -> ActivityLevel:
        """Get activity level for a wallet."""
        if wallet not in self._activity_data:
            return ActivityLevel.INACTIVE

        activity = self._activity_data[wallet]

        # Check based on 24h transaction count
        if activity.tx_count_24h > self._config.VERY_HIGH_MIN_TX:
            return ActivityLevel.VERY_HIGH
        elif activity.tx_count_24h > self._config.HIGH_MIN_TX:
            return ActivityLevel.HIGH
        elif activity.tx_count_24h > self._config.MEDIUM_MIN_TX:
            return ActivityLevel.MEDIUM
        elif activity.tx_count_24h > 0:
            return ActivityLevel.LOW
        else:
            return ActivityLevel.INACTIVE

    def _get_base_ttl(self, activity_level: ActivityLevel) -> int:
        """Get base TTL for an activity level."""
        ttl_map = {
            ActivityLevel.VERY_HIGH: self._config.VERY_HIGH_TTL,
            ActivityLevel.HIGH: self._config.HIGH_TTL,
            ActivityLevel.MEDIUM: self._config.MEDIUM_TTL,
            ActivityLevel.LOW: self._config.LOW_TTL,
            ActivityLevel.INACTIVE: self._config.INACTIVE_TTL,
        }
        return ttl_map.get(activity_level, self._config.MEDIUM_TTL)

    def get(self, key: str) -> Optional[Any]:
        """
        Get value from cache.

        Args:
            key: Cache key

        Returns:
            Cached value or None if not found/expired
        """
        with self._lock:
            # Check if cleanup is needed
            self._check_cleanup()

            if key not in self._cache:
                self._stats['misses'] += 1
                return None

            entry = self._cache[key]

            # Check if expired
            if entry.is_expired():
                del self._cache[key]
                self._stats['expirations'] += 1
                self._stats['misses'] += 1
                return None

            # Update access stats
            entry.last_accessed = time.time()
            entry.access_count += 1
            self._stats['hits'] += 1

            return entry.value

    def set(self, key: str, value: Any, wallet: Optional[str] = None) -> bool:
        """
        Set value in cache.

        Args:
            key: Cache key
            value: Value to cache
            wallet: Associated wallet address (for activity-based TTL)

        Returns:
            True if successfully cached
        """
        with self._lock:
            # Check if cleanup is needed
            self._check_cleanup()

            # Estimate size
            size = self._estimate_size(value)

            # Check memory limits
            if self._total_memory_bytes + size > (self._config.MAX_MEMORY_MB * 1024 * 1024):
                self._evict_lru()

            # Check entry count limits
            if len(self._cache) >= self._config.MAX_ENTRIES:
                self._evict_lru()

            # Determine activity level and TTL
            if wallet:
                activity_level = self._get_activity_level(wallet)
                ttl = self.get_cache_ttl(wallet, time.time())
            else:
                activity_level = ActivityLevel.MEDIUM
                ttl = self._config.MEDIUM_TTL

            # Create cache entry
            entry = CacheEntry(
                key=key,
                value=value,
                created_at=time.time(),
                last_accessed=time.time(),
                ttl_seconds=ttl,
                activity_level=activity_level,
                size_bytes=size,
            )

            # Add to cache
            self._cache[key] = entry
            self._total_memory_bytes += size

            return True

    def _estimate_size(self, value: Any) -> int:
        """Estimate memory size of a value."""
        try:
            return len(str(value).encode('utf-8'))
        except Exception:
            return 100  # Default estimate

    def _evict_lru(self) -> None:
        """Evict least recently used entry."""
        if not self._cache:
            return

        # Find LRU entry
        lru_key = min(self._cache.keys(), key=lambda k: self._cache[k].last_accessed)
        entry = self._cache[lru_key]

        del self._cache[lru_key]
        self._total_memory_bytes -= entry.size_bytes
        self._stats['evictions'] += 1

        logger.debug(f"Evicted LRU entry: {lru_key}")

    def invalidate(self, key: str) -> bool:
        """
        Invalidate a cache entry.

        Args:
            key: Cache key to invalidate

        Returns:
            True if entry was found and invalidated
        """
        with self._lock:
            if key in self._cache:
                entry = self._cache[key]
                del self._cache[key]
                self._total_memory_bytes -= entry.size_bytes
                return True
            return False

    def invalidate_wallet(self, wallet: str) -> int:
        """
        Invalidate all cache entries for a wallet.

        Args:
            wallet: Wallet address

        Returns:
            Number of entries invalidated
        """
        with self._lock:
            # Find all keys for this wallet
            wallet_keys = [k for k in self._cache.keys() if wallet in k]

            count = 0
            for key in wallet_keys:
                if self.invalidate(key):
                    count += 1

            logger.debug(f"Invalidated {count} entries for wallet {wallet[:8]}...")
            return count

    def invalidate_inactive_wallets(self, hours_threshold: int = 24) -> int:
        """
        Invalidate cache entries for inactive wallets.

        Args:
            hours_threshold: Hours of inactivity to trigger invalidation

        Returns:
            Number of entries invalidated
        """
        with self._lock:
            cutoff_time = time.time() - (hours_threshold * 3600)
            count = 0

            # Find inactive wallets
            inactive_wallets = []
            for wallet, activity in self._activity_data.items():
                if activity.last_activity_timestamp < cutoff_time:
                    inactive_wallets.append(wallet)

            # Invalidate their cache entries
            for wallet in inactive_wallets:
                count += self.invalidate_wallet(wallet)

            # Clean up activity data
            for wallet in inactive_wallets:
                del self._activity_data[wallet]

            logger.info(f"Invalidated {count} entries for {len(inactive_wallets)} inactive wallets")
            return count

    def update_wallet_activity(
        self, wallet: str, tx_count_24h: int, wqs: Optional[float] = None
    ) -> None:
        """
        Update activity tracking for a wallet.

        Args:
            wallet: Wallet address
            tx_count_24h: Transactions in last 24 hours
            wqs: Optional WQS score for cache extension
        """
        with self._lock:
            now = time.time()

            if wallet not in self._activity_data:
                self._activity_data[wallet] = ActivityData(
                    wallet_address=wallet,
                    tx_count_24h=tx_count_24h,
                    tx_count_1h=0,
                    tx_count_5m=0,
                    last_activity_timestamp=now,
                    wqs_score=wqs,
                )
            else:
                self._activity_data[wallet].tx_count_24h = tx_count_24h
                self._activity_data[wallet].last_activity_timestamp = now
                if wqs is not None:
                    self._activity_data[wallet].wqs_score = wqs

            logger.debug(f"Updated activity for {wallet[:8]}...: {tx_count_24h} tx/24h")

    def predict_cache_hit_rate(self, pattern: ActivityLevel) -> float:
        """
        Predict cache hit rate for an activity pattern.

        Args:
            pattern: Activity level to predict for

        Returns:
            Predicted hit rate (0-1)
        """
        # Base hit rates by activity level
        base_rates = {
            ActivityLevel.VERY_HIGH: 0.95,
            ActivityLevel.HIGH: 0.85,
            ActivityLevel.MEDIUM: 0.70,
            ActivityLevel.LOW: 0.50,
            ActivityLevel.INACTIVE: 0.20,
        }

        return base_rates.get(pattern, 0.5)

    def get_cache_stats(self) -> Dict[str, Any]:
        """Get cache statistics."""
        with self._lock:
            total_requests = self._stats['hits'] + self._stats['misses']
            hit_rate = self._stats['hits'] / max(1, total_requests)

            return {
                'entries': len(self._cache),
                'memory_bytes': self._total_memory_bytes,
                'memory_mb': self._total_memory_bytes / (1024 * 1024),
                'hits': self._stats['hits'],
                'misses': self._stats['misses'],
                'evictions': self._stats['evictions'],
                'expirations': self._stats['expirations'],
                'hit_rate': hit_rate,
                'wallets_tracked': len(self._activity_data),
            }

    def get_activity_distribution(self) -> Dict[str, int]:
        """Get distribution of wallets by activity level."""
        with self._lock:
            distribution = {level.value: 0 for level in ActivityLevel}

            for activity in self._activity_data.values():
                level = self._get_activity_level(activity.wallet_address)
                distribution[level.value] += 1

            return distribution

    def _check_cleanup(self) -> None:
        """Check and perform cleanup if needed."""
        now = time.time()
        if now - self._last_cleanup < self._config.CLEANUP_INTERVAL_SECONDS:
            return

        self._last_cleanup = now
        self.invalidate_inactive_wallets(self._config.INACTIVE_HOURS_THRESHOLD)

        # Also clean expired entries
        expired_keys = [k for k, v in self._cache.items() if v.is_expired()]
        for key in expired_keys:
            entry = self._cache[key]
            del self._cache[key]
            self._total_memory_bytes -= entry.size_bytes
            self._stats['expirations'] += 1

        logger.debug(f"Cleanup: removed {len(expired_keys)} expired entries")

    def clear(self) -> None:
        """Clear all cache entries."""
        with self._lock:
            self._cache.clear()
            self._total_memory_bytes = 0
            logger.info("Cache cleared")

    def reset_statistics(self) -> None:
        """Reset cache statistics."""
        with self._lock:
            self._stats = {
                'hits': 0,
                'misses': 0,
                'evictions': 0,
                'expirations': 0,
            }
            logger.info("Statistics reset")
