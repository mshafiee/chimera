#!/usr/bin/env python3
"""
Monitor Parse Cache Invalidation Implementation

This script provides cache management functionality for both scout and operator
caches that need to be invalidated after parser improvements.

NOTE: Scout caches are per-process in-memory structures. Instantiating classes
here and calling clear() only empties these fresh instances — it does NOT
affect the caches held by a running scout process. For a running deployment,
use the operator's admin API endpoint (--clear-operator) or restart the
scout service.
"""

import argparse
import os
import sys
from datetime import datetime, timezone

# Add the repo root (parent of scripts/) to the path so `import scout` resolves
sys.path.insert(0, os.path.dirname(os.path.dirname(__file__)))

from scout.core.advanced_cache import reset_cache as reset_advanced_cache  # noqa: E402


class CacheManager:
    """Manages cache invalidation for both scout and operator components"""

    def __init__(self, helius_api_key=None):
        self.helius_api_key = helius_api_key or os.getenv('HELIUS_API_KEY')

    def clear_scout_caches(self) -> dict:
        """Clear scout caches.

        Only the module-level AdvancedCache singleton can be reset from
        outside the running process; the per-instance caches (Helius wrapper,
        ActivityBasedCache, LiquidityProvider, FeatureEnricher) belong to the
        live scout process and cannot be invalidated from here.
        """
        results = {
            'timestamp': datetime.now(timezone.utc).isoformat(),
            'caches_cleared': [],
            'errors': []
        }

        # The only scout cache that can be reset process-externally is the
        # module-level AdvancedCache singleton.
        try:
            reset_advanced_cache()
            results['caches_cleared'].append({
                'cache': 'AdvancedCache (module singleton)',
                'status': 'cleared'
            })
        except Exception as e:
            results['errors'].append({
                'cache': 'AdvancedCache',
                'error': str(e)
            })

        # Per-instance caches cannot be reached from this process; note this
        # explicitly instead of pretending the running scout was invalidated.
        results['errors'].append({
            'cache': 'running scout process',
            'error': 'In-memory caches of a running scout cannot be cleared from a '
                     'separate process. Restart the scout service or use the operator '
                     'admin API to invalidate remotely.'
        })

        return results

    async def clear_operator_cache_via_api(self, operator_url: str = 'http://localhost:8080', api_key=None) -> dict:
        """Clear operator cache via admin API endpoint"""
        import aiohttp

        headers = {}
        if api_key:
            headers['Authorization'] = f'Bearer {api_key}'

        results = {
            'timestamp': datetime.now(timezone.utc).isoformat(),
            'operator_url': operator_url,
            'attempts': []
        }

        # Try to clear metadata cache (if endpoint exists)
        try:
            async with aiohttp.ClientSession() as session:
                async with session.post(
                    f'{operator_url}/api/v1/admin/cache/clear',
                    headers=headers,
                    timeout=aiohttp.ClientTimeout(total=10)
                ) as response:
                    if response.status == 200:
                        data = await response.json()
                        results['attempts'].append({
                            'endpoint': '/api/v1/admin/cache/clear',
                            'status': 'success',
                            'response': data
                        })
                    else:
                        text = await response.text()
                        results['attempts'].append({
                            'endpoint': '/api/v1/admin/cache/clear',
                            'status': f'failed ({response.status})',
                            'response': text
                        })
        except Exception as e:
            results['attempts'].append({
                'endpoint': '/api/v1/admin/cache/clear',
                'status': 'error',
                'error': str(e)
            })

        return results


async def main():
    parser = argparse.ArgumentParser(description='Manage cache invalidation for Chimera')
    parser.add_argument('--clear-scout', action='store_true', help='Clear all scout caches')
    parser.add_argument('--clear-operator', action='store_true', help='Clear operator cache via API')
    parser.add_argument('--operator-url', default='http://localhost:8080', help='Operator API URL')
    parser.add_argument('--api-key', help='Admin API key for operator')
    parser.add_argument('--helius-api-key', help='Helius API key')

    args = parser.parse_args()

    if not args.clear_scout and not args.clear_operator:
        parser.print_help()
        print("\nError: Must specify at least one action: --clear-scout or --clear-operator")
        sys.exit(1)

    manager = CacheManager(args.helius_api_key)
    had_failures = False

    if args.clear_scout:
        print("Clearing scout caches...")
        scout_results = manager.clear_scout_caches()
        print(f"✓ Cleared {len(scout_results['caches_cleared'])} caches")
        if scout_results['errors']:
            print(f"⚠ {len(scout_results['errors'])} error(s) occurred")
            for error in scout_results['errors']:
                print(f"  - {error['cache']}: {error['error']}")
            had_failures = True

    if args.clear_operator:
        print("Clearing operator cache via API...")
        operator_results = await manager.clear_operator_cache_via_api(args.operator_url, args.api_key)
        for attempt in operator_results['attempts']:
            status_symbol = '✓' if attempt['status'] == 'success' else '✗'
            print(f"{status_symbol} {attempt['endpoint']}: {attempt['status']}")
            if attempt['status'] != 'success':
                had_failures = True

    if had_failures:
        print("\n⚠ Cache invalidation completed with errors")
        sys.exit(1)

    print("\n✓ Cache invalidation complete")


if __name__ == '__main__':
    import asyncio
    asyncio.run(main())
