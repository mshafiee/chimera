"""
Caching wrapper for Helius API calls using ActivityBasedCache.

This module provides intelligent caching for wallet transaction data based on
wallet activity patterns, reducing redundant API calls to Helius.
"""

import logging
import time
from typing import List, Dict, Any, Optional
from .activity_cache import ActivityBasedCache, ActivityLevel, CacheConfig

logger = logging.getLogger(__name__)


class HeliusCachingWrapper:
    """
    Caching wrapper for Helius client with activity-based cache invalidation.

    Strategy:
    - Cache wallet transactions with dynamic TTL based on activity level
    - Track wallet activity (tx count, WQS) for cache optimization
    - Automatic cleanup of inactive wallet caches
    - Thread-safe operations with statistics tracking
    """

    def __init__(self, cache_config: Optional[CacheConfig] = None):
        """Initialize the caching wrapper."""
        self.cache = ActivityBasedCache(cache_config)
        self._wallet_activity: Dict[str, Dict[str, Any]] = {}
        self._last_activity_update: Dict[str, float] = {}

        logger.info("HeliusCachingWrapper initialized")

    def get_cached_transactions(
        self,
        wallet_address: str,
        days: int = 30,
        limit: int = 100
    ) -> Optional[List[Dict[str, Any]]]:
        """
        Get cached transactions for a wallet if available.

        Args:
            wallet_address: Wallet address
            days: Number of days of transactions
            limit: Maximum transactions

        Returns:
            Cached transaction list or None if not found/expired
        """
        # Generate cache key
        cache_key = f"txs:{wallet_address}:{days}:{limit}"

        # Try cache
        cached = self.cache.get(cache_key)
        if cached is not None:
            logger.debug(f"Cache HIT for {wallet_address[:8]}...: {len(cached)} transactions")
            return cached

        logger.debug(f"Cache MISS for {wallet_address[:8]}...")
        return None

    def cache_transactions(
        self,
        wallet_address: str,
        transactions: List[Dict[str, Any]],
        days: int = 30,
        limit: int = 100,
        wqs_score: Optional[float] = None
    ) -> bool:
        """
        Cache transaction data for a wallet.

        Args:
            wallet_address: Wallet address
            transactions: Transaction list to cache
            days: Number of days (for cache key)
            limit: Maximum transactions (for cache key)
            wqs_score: Optional WQS score for cache extension

        Returns:
            True if cached successfully
        """
        if not transactions:
            return False

        # Calculate activity metrics
        tx_count = len(transactions)
        tx_count_24h = self._estimate_24h_tx_count(transactions, days)

        # Update wallet activity tracking
        self.cache.update_wallet_activity(wallet_address, tx_count_24h, wqs_score)

        # Generate cache key
        cache_key = f"txs:{wallet_address}:{days}:{limit}"

        # Cache with activity-based TTL
        success = self.cache.set(cache_key, transactions, wallet=wallet_address)

        if success:
            logger.info(f"Cached {tx_count} transactions for {wallet_address[:8]}... (activity: {tx_count_24h} tx/24h)")

        return success

    def _estimate_24h_tx_count(self, transactions: List[Dict[str, Any]], days: int) -> int:
        """Estimate transactions per 24h based on current data."""
        if not transactions or days <= 0:
            return 0

        # Calculate average daily rate
        avg_daily = len(transactions) / max(1, days)

        # Estimate last 24h count from recent transactions
        now = time.time()
        recent_24h_count = sum(1 for tx in transactions if tx.get('timestamp', 0) > (now - 86400))

        # Use actual recent count if available, otherwise estimate from average
        return recent_24h_count if recent_24h_count > 0 else int(avg_daily)

    def invalidate_wallet(self, wallet_address: str) -> int:
        """
        Invalidate all cached data for a wallet.

        Args:
            wallet_address: Wallet address to invalidate

        Returns:
            Number of entries invalidated
        """
        # Invalidate from activity cache
        count = self.cache.invalidate_wallet(wallet_address)

        # Clear activity tracking
        if wallet_address in self._wallet_activity:
            del self._wallet_activity[wallet_address]
        if wallet_address in self._last_activity_update:
            del self._last_activity_update[wallet_address]

        logger.debug(f"Invalidated {count} cache entries for {wallet_address[:8]}...")
        return count

    def get_cache_stats(self) -> Dict[str, Any]:
        """Get comprehensive cache statistics."""
        stats = self.cache.get_cache_stats()
        stats['wallets_tracked'] = len(self._wallet_activity)
        stats['activity_distribution'] = self.cache.get_activity_distribution()
        return stats

    def cleanup_inactive_wallets(self, hours_threshold: int = 24) -> int:
        """
        Clean up cache entries for inactive wallets.

        Args:
            hours_threshold: Hours of inactivity before cleanup

        Returns:
            Number of entries cleaned up
        """
        count = self.cache.invalidate_inactive_wallets(hours_threshold)

        # Clean up local activity tracking
        cutoff_time = time.time() - (hours_threshold * 3600)
        inactive_wallets = [
            wallet for wallet, last_update in self._last_activity_update.items()
            if last_update < cutoff_time
        ]

        for wallet in inactive_wallets:
            if wallet in self._wallet_activity:
                del self._wallet_activity[wallet]
            if wallet in self._last_activity_update:
                del self._last_activity_update[wallet]

        logger.info(f"Cleaned up {count} entries for {len(inactive_wallets)} inactive wallets")
        return count

    def get_wallet_activity_level(self, wallet_address: str) -> ActivityLevel:
        """
        Get the current activity level for a wallet.

        Args:
            wallet_address: Wallet address

        Returns:
            Activity level enum
        """
        # Check if we have activity data
        if wallet_address not in self._wallet_activity:
            return ActivityLevel.INACTIVE

        # Get transaction counts
        activity_data = self._wallet_activity[wallet_address]
        tx_count_24h = activity_data.get('tx_count_24h', 0)

        # Determine activity level
        if tx_count_24h > 50:
            return ActivityLevel.VERY_HIGH
        elif tx_count_24h > 10:
            return ActivityLevel.HIGH
        elif tx_count_24h > 1:
            return ActivityLevel.MEDIUM
        elif tx_count_24h > 0:
            return ActivityLevel.LOW
        else:
            return ActivityLevel.INACTIVE

    def get_cache_hit_rate(self) -> float:
        """Get current cache hit rate."""
        stats = self.cache.get_cache_stats()
        return stats.get('hit_rate', 0.0)

    def reset_statistics(self) -> None:
        """Reset cache statistics."""
        self.cache.reset_statistics()
        logger.info("Cache statistics reset")

    def clear_cache(self) -> None:
        """Clear all cached data."""
        self.cache.clear()
        self._wallet_activity.clear()
        self._last_activity_update.clear()
        logger.info("Cache cleared")