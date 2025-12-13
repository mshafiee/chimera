"""
Redis Client with SQLite Fallback

Provides Redis-backed caching with automatic fallback to SQLite
if Redis is unavailable. This ensures graceful degradation.
"""

import os
import json
import logging
from typing import Optional, Dict, Any
from datetime import datetime, timedelta

logger = logging.getLogger(__name__)

# Try to import redis, but don't fail if not available
try:
    import redis
    REDIS_AVAILABLE = True
except ImportError:
    REDIS_AVAILABLE = False
    redis = None

# Import config if available
try:
    from config import ScoutConfig
    CONFIG_AVAILABLE = True
except ImportError:
    CONFIG_AVAILABLE = False
    ScoutConfig = None


class RedisClient:
    """
    Redis client wrapper with SQLite fallback.
    
    If Redis is unavailable or disabled, falls back to in-memory cache
    (SQLite fallback can be added later if needed).
    """
    
    def __init__(self, redis_url: Optional[str] = None, enabled: Optional[bool] = None):
        """
        Initialize Redis client.
        
        Args:
            redis_url: Redis connection URL (defaults to config)
            enabled: Whether Redis is enabled (defaults to config)
        """
        # Get from config if not provided
        if enabled is None and CONFIG_AVAILABLE:
            enabled = ScoutConfig.get_redis_enabled()
        self.enabled = enabled or False
        
        if redis_url is None and CONFIG_AVAILABLE:
            redis_url = ScoutConfig.get_redis_url()
        self.redis_url = redis_url or "redis://localhost:6379"
        
        self.redis_client = None
        self._fallback_cache: Dict[str, tuple] = {}  # key -> (value, expiry_time)
        
        if self.enabled and REDIS_AVAILABLE:
            try:
                # Parse Redis URL
                if self.redis_url.startswith("redis://"):
                    # Simple parsing - in production, use redis.from_url()
                    self.redis_client = redis.Redis.from_url(
                        self.redis_url,
                        decode_responses=True,
                        socket_connect_timeout=2,
                        socket_timeout=2,
                    )
                    # Test connection
                    self.redis_client.ping()
                    logger.info("Redis client initialized successfully")
                else:
                    logger.warning(f"Invalid Redis URL format: {self.redis_url}")
                    self.enabled = False
            except Exception as e:
                logger.warning(f"Failed to connect to Redis: {e}. Using fallback cache.")
                self.enabled = False
                self.redis_client = None
        else:
            if not REDIS_AVAILABLE:
                logger.debug("Redis library not available, using fallback cache")
            else:
                logger.debug("Redis disabled, using fallback cache")
    
    def get(self, key: str) -> Optional[str]:
        """
        Get value from cache.
        
        Args:
            key: Cache key
            
        Returns:
            Cached value or None if not found/expired
        """
        if self.enabled and self.redis_client:
            try:
                return self.redis_client.get(key)
            except Exception as e:
                logger.debug(f"Redis get failed for key {key}: {e}, using fallback")
                # Fall through to fallback
        
        # Fallback: in-memory cache
        if key in self._fallback_cache:
            value, expiry = self._fallback_cache[key]
            if expiry is None or datetime.utcnow() < expiry:
                return value
            else:
                # Expired
                del self._fallback_cache[key]
        
        return None
    
    def set(self, key: str, value: str, ttl_seconds: Optional[int] = None):
        """
        Set value in cache.
        
        Args:
            key: Cache key
            value: Value to cache
            ttl_seconds: Time to live in seconds (None = no expiry)
        """
        if self.enabled and self.redis_client:
            try:
                if ttl_seconds:
                    self.redis_client.setex(key, ttl_seconds, value)
                else:
                    self.redis_client.set(key, value)
                return
            except Exception as e:
                logger.debug(f"Redis set failed for key {key}: {e}, using fallback")
                # Fall through to fallback
        
        # Fallback: in-memory cache
        expiry = None
        if ttl_seconds:
            expiry = datetime.utcnow() + timedelta(seconds=ttl_seconds)
        self._fallback_cache[key] = (value, expiry)
        
        # Cleanup expired entries periodically (simple implementation)
        if len(self._fallback_cache) > 1000:
            now = datetime.utcnow()
            expired_keys = [
                k for k, (_, exp) in self._fallback_cache.items()
                if exp is not None and now >= exp
            ]
            for k in expired_keys:
                del self._fallback_cache[k]
    
    def delete(self, key: str):
        """Delete key from cache."""
        if self.enabled and self.redis_client:
            try:
                self.redis_client.delete(key)
                return
            except Exception as e:
                logger.debug(f"Redis delete failed for key {key}: {e}")
        
        # Fallback
        self._fallback_cache.pop(key, None)
    
    def clear(self):
        """Clear all cached values."""
        if self.enabled and self.redis_client:
            try:
                self.redis_client.flushdb()
                return
            except Exception as e:
                logger.debug(f"Redis clear failed: {e}")
        
        # Fallback
        self._fallback_cache.clear()
    
    def is_available(self) -> bool:
        """Check if Redis is available and working."""
        if not self.enabled or not self.redis_client:
            return False
        try:
            self.redis_client.ping()
            return True
        except Exception:
            return False
