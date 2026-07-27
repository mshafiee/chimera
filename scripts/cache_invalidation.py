#!/usr/bin/env python3
"""
Monitor Parse Cache Invalidation Implementation

This script provides cache management functionality for both scout and operator
caches that need to be invalidated after parser improvements.
"""

import asyncio
import sys
import os
import argparse
from typing import Optional
from datetime import datetime

# Add scout to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'scout'))

from scout.core.caching import HeliusCachingWrapper
from scout.core.activity_cache import ActivityBasedCache
from scout.core.advanced_cache import AdvancedCache
from scout.core.liquidity import LiquidityProvider
from scout.core.feature_enrichment import FeatureEnricher

class CacheManager:
    """Manages cache invalidation for both scout and operator components"""
    
    def __init__(self, helius_api_key: Optional[str] = None):
        self.helius_api_key = helius_api_key or os.getenv('HELIUS_API_KEY')
        
    async def clear_scout_caches(self) -> dict:
        """Clear all scout in-memory caches"""
        results = {
            'timestamp': datetime.utcnow().isoformat(),
            'caches_cleared': [],
            'errors': []
        }
        
        try:
            # Clear Helius caching wrapper
            helius_cache = HeliusCachingWrapper(self.helius_api_key)
            cleared = helius_cache.clear_cache()
            results['caches_cleared'].append({
                'cache': 'HeliusCachingWrapper.parse_cache',
                'status': 'cleared',
                'details': cleared
            })
        except Exception as e:
            results['errors'].append({
                'cache': 'HeliusCachingWrapper',
                'error': str(e)
            })
        
        try:
            # Clear activity cache
            activity_cache = ActivityBasedCache()
            activity_cache.clear()
            results['caches_cleared'].append({
                'cache': 'ActivityBasedCache',
                'status': 'cleared'
            })
        except Exception as e:
            results['errors'].append({
                'cache': 'ActivityBasedCache',
                'error': str(e)
            })
        
        try:
            # Clear advanced cache
            adv_cache = AdvancedCache()
            adv_cache.reset_cache()
            results['caches_cleared'].append({
                'cache': 'AdvancedCache',
                'status': 'cleared'
            })
        except Exception as e:
            results['errors'].append({
                'cache': 'AdvancedCache',
                'error': str(e)
            })
        
        try:
            # Clear liquidity cache
            liq_cache = LiquidityProvider(self.helius_api_key)
            liq_cache.clear_cache()
            results['caches_cleared'].append({
                'cache': 'LiquidityProvider',
                'status': 'cleared'
            })
        except Exception as e:
            results['errors'].append({
                'cache': 'LiquidityProvider',
                'error': str(e)
            })
        
        try:
            # Clear feature enrichment cache
            feat_cache = FeatureEnricher()
            feat_cache.clear_cache()
            results['caches_cleared'].append({
                'cache': 'FeatureEnricher',
                'status': 'cleared'
            })
        except Exception as e:
            results['errors'].append({
                'cache': 'FeatureEnricher',
                'error': str(e)
            })
        
        return results
    
    async def clear_operator_cache_via_api(self, operator_url: str = 'http://localhost:8080', api_key: Optional[str] = None) -> dict:
        """Clear operator cache via admin API endpoint"""
        import aiohttp
        import json
        
        headers = {}
        if api_key:
            headers['Authorization'] = f'Bearer {api_key}'
        
        results = {
            'timestamp': datetime.utcnow().isoformat(),
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
    
    if args.clear_scout:
        print("Clearing scout caches...")
        scout_results = await manager.clear_scout_caches()
        print(f"✓ Cleared {len(scout_results['caches_cleared'])} caches")
        if scout_results['errors']:
            print(f"⚠ {len(scout_results['errors'])} errors occurred")
            for error in scout_results['errors']:
                print(f"  - {error['cache']}: {error['error']}")
    
    if args.clear_operator:
        print("Clearing operator cache via API...")
        operator_results = await manager.clear_operator_cache_via_api(args.operator_url, args.api_key)
        for attempt in operator_results['attempts']:
            status_symbol = '✓' if attempt['status'] == 'success' else '✗'
            print(f"{status_symbol} {attempt['endpoint']}: {attempt['status']}")
    
    print("\n✓ Cache invalidation complete")

if __name__ == '__main__':
    asyncio.run(main())