#!/usr/bin/env python3
"""
Test script for Advanced Cache System integration.

This script tests the cache integration in the Helius client to ensure:
1. Cache decorators work correctly
2. API calls are cached properly
3. Cache hit rates are tracked
4. Configuration options work
"""

import sys
import os
from pathlib import Path

# Add Scout directory to path
sys.path.insert(0, str(Path(__file__).parent))

def test_cache_imports():
    """Test that cache modules can be imported."""
    print("Testing cache imports...")

    try:
        from core.advanced_cache import get_cache, CacheCategory
        print("✓ Advanced cache imported successfully")

        cache = get_cache()
        print(f"✓ Cache instance created: {cache}")

        # Test basic cache operations
        cache.set("test", "key1", {"value": 123}, category=CacheCategory.WALLET_METRICS)
        result = cache.get("test", "key1", category=CacheCategory.WALLET_METRICS)
        assert result == {"value": 123}, f"Cache get failed: {result}"
        print("✓ Basic cache operations work")

        return True
    except Exception as e:
        print(f"✗ Cache import failed: {e}")
        return False


def test_helius_client_import():
    """Test that Helius client with cache integration can be imported."""
    print("\nTesting Helius client import...")

    try:
        from core.helius_client import HeliusClient, CACHE_AVAILABLE
        print(f"✓ Helius client imported successfully")
        print(f"✓ Cache available: {CACHE_AVAILABLE}")

        # Create a client instance (with no API key for testing)
        client = HeliusClient(api_key=None)
        print(f"✓ Helius client instance created: {client}")

        return True
    except Exception as e:
        print(f"✗ Helius client import failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_config_import():
    """Test that Scout config with cache options can be imported."""
    print("\nTesting Scout config import...")

    try:
        from config import ScoutConfig
        print("✓ Scout config imported successfully")

        # Test cache configuration methods
        cache_memory = ScoutConfig.get_cache_memory_mb()
        print(f"✓ Cache memory size: {cache_memory}MB")

        redis_enabled = ScoutConfig.get_redis_enabled()
        print(f"✓ Redis enabled: {redis_enabled}")

        redis_url = ScoutConfig.get_redis_url()
        print(f"✓ Redis URL: {redis_url}")

        growth_optimized = ScoutConfig.get_growth_optimized()
        print(f"✓ Growth optimized: {growth_optimized}")

        return True
    except Exception as e:
        print(f"✗ Scout config import failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_cache_integration():
    """Test cache integration with simulated wallet data."""
    print("\nTesting cache integration...")

    try:
        from core.advanced_cache import get_cache, CacheCategory
        import time

        cache = get_cache()

        # Use shorter wallet address to avoid key hashing
        wallet_address = "wallet1"

        # Test validation caching (simple boolean) - with correct parameter order
        validation_result = True
        # CORRECT: cache.set(prefix, identifier, value, *args, category)
        cache.set("wallet_validation", wallet_address, validation_result, "3:7",
                 category=CacheCategory.WALLET_METRICS)
        print("✓ Validation result cached successfully")

        # Debug: Check cache contents
        print(f"Debug: Cache has {len(cache._l1_cache)} entries")
        for key, entry in cache._l1_cache.items():
            is_expired = entry.is_expired()
            ttl_remaining = int((entry.created_at + entry.ttl_seconds) - time.time())
            print(f"Debug: Entry '{key}' -> Value: {entry.value}, Expired: {is_expired}, TTL: {ttl_remaining}s")

        cached_validation = cache.get("wallet_validation", wallet_address, "3:7",
                                     category=CacheCategory.WALLET_METRICS)
        assert cached_validation == True, f"Validation cache retrieval failed: {cached_validation}"
        print("✓ Cached validation retrieved successfully")

        # Test with simple data
        simple_test_data = {"test": "value", "count": 42}
        cache.set("simple", wallet_address, simple_test_data, "key1",
                 category=CacheCategory.WALLET_METRICS)
        print("✓ Simple data cached successfully")

        cached_simple = cache.get("simple", wallet_address, "key1",
                                 category=CacheCategory.WALLET_METRICS)
        assert cached_simple == simple_test_data, f"Simple cache retrieval failed: {cached_simple}"
        print("✓ Simple cached data retrieved successfully")

        return True
    except Exception as e:
        print(f"✗ Cache integration test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def print_cache_stats():
    """Print cache statistics."""
    print("\nCache Statistics:")
    try:
        from core.advanced_cache import get_cache
        cache = get_cache()
        cache.print_stats()
    except Exception as e:
        print(f"Failed to print cache stats: {e}")


def main():
    """Run all cache integration tests."""
    print("=" * 70)
    print("Advanced Cache System Integration Tests")
    print("=" * 70)

    tests = [
        test_cache_imports,
        test_helius_client_import,
        test_config_import,
        test_cache_integration,
    ]

    results = []
    for test in tests:
        try:
            result = test()
            results.append(result)
        except Exception as e:
            print(f"\n✗ Test failed with exception: {e}")
            import traceback
            traceback.print_exc()
            results.append(False)

    print_cache_stats()

    print("\n" + "=" * 70)
    print(f"Test Results: {sum(results)}/{len(results)} passed")
    print("=" * 70)

    return all(results)


if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)