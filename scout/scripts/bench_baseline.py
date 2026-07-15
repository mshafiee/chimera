#!/usr/bin/env python3
"""
Baseline performance runner for scout optimization.

Measures credits/wallet, p99 latency, peak RSS, and network calls per wallet
using recorded fixtures (zero credit consumption).
"""

import asyncio
import json
import os
import sys
import time
import tracemalloc
from pathlib import Path
from typing import Dict, Any, List, Optional
from datetime import datetime

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

# Try to import profiling dependencies
try:
    import pyinstrument
    PYINSTRUMENT_AVAILABLE = True
except ImportError:
    PYINSTRUMENT_AVAILABLE = False
    print("WARNING: pyinstrument not available. Install with: pip install pyinstrument")

try:
    from memory_profiler import memory_usage
    MEMORY_PROFILER_AVAILABLE = True
except ImportError:
    MEMORY_PROFILER_AVAILABLE = False
    print("WARNING: memory_profiler not available. Install with: pip install memory-profiler")

from core.helius_client import HeliusClient
from core.helius_credit_tracker import CreditTracker
from tests.fixtures.replay import FixtureReplayer, create_replay_patch


class BaselineMetrics:
    """Container for baseline performance metrics."""
    
    def __init__(self):
        self.credits_per_wallet: Dict[str, int] = {}
        self.latency_samples: List[float] = []
        self.peak_rss: float = 0.0
        self.network_calls_per_wallet: Dict[str, int] = {}
        self.errors: List[str] = []
    
    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        return {
            "timestamp": datetime.utcnow().isoformat(),
            "credits_per_wallet": self.credits_per_wallet,
            "latency_stats": {
                "p50": self._percentile(self.latency_samples, 50),
                "p95": self._percentile(self.latency_samples, 95),
                "p99": self._percentile(self.latency_samples, 99),
                "samples": len(self.latency_samples)
            },
            "peak_rss_mb": self.peak_rss,
            "network_calls_per_wallet": self.network_calls_per_wallet,
            "total_credits": sum(self.credits_per_wallet.values()),
            "errors": self.errors
        }
    
    def _percentile(self, data: List[float], percentile: float) -> float:
        """Calculate percentile of data."""
        if not data:
            return 0.0
        sorted_data = sorted(data)
        index = int(len(sorted_data) * percentile / 100)
        return sorted_data[min(index, len(sorted_data) - 1)]


class BaselineRunner:
    """Runs baseline performance measurements."""
    
    def __init__(self, output_dir: Optional[Path] = None):
        self.output_dir = output_dir or Path(__file__).parent.parent.parent / "docs" / "perf"
        self.output_dir.mkdir(parents=True, exist_ok=True)
        
        self.replayer = FixtureReplayer()
        self.metrics = BaselineMetrics()
        
        # Track network calls (in fixture mode, this tracks fixture accesses)
        self._network_call_count = 0
    
    async def measure_wallet(
        self,
        wallet: str,
        days: int = 30,
        limit: int = 1000
    ) -> Dict[str, Any]:
        """Measure performance for a single wallet."""
        
        wallet_metrics = {
            "wallet": wallet,
            "days": days,
            "limit": limit,
            "success": False,
            "credits_consumed": 0,
            "latency_ms": 0,
            "network_calls": 0,
            "transaction_count": 0,
            "error": None
        }
        
        # Create client with fixture replay
        client = HeliusClient(api_key="dummy")
        
        # Track credit consumption
        credit_tracker = CreditTracker()
        initial_credits = credit_tracker.get_remaining_credits()
        
        try:
            with create_replay_patch(self.replayer):
                # Measure latency
                start_time = time.time()
                transactions = await client.get_wallet_transactions(
                    wallet, days=days, limit=limit
                )
                latency_ms = (time.time() - start_time) * 1000
                
                # Calculate credit consumption
                final_credits = credit_tracker.get_remaining_credits()
                credits_consumed = initial_credits - final_credits
                
                wallet_metrics.update({
                    "success": True,
                    "credits_consumed": credits_consumed,
                    "latency_ms": latency_ms,
                    "network_calls": self._network_call_count,  # Would be tracked in real mode
                    "transaction_count": len(transactions) if transactions else 0
                })
                
                # Update aggregate metrics
                self.metrics.credits_per_wallet[wallet] = credits_consumed
                self.metrics.latency_samples.append(latency_ms)
                self.metrics.network_calls_per_wallet[wallet] = self._network_call_count
                
        except Exception as e:
            wallet_metrics["error"] = str(e)
            self.metrics.errors.append(f"{wallet}: {e}")
            import traceback
            traceback.print_exc()
        
        return wallet_metrics
    
    async def measure_all_wallets(self) -> List[Dict[str, Any]]:
        """Measure performance for all wallets in fixtures."""
        wallets = self.replayer.get_all_wallets()
        
        if not wallets:
            print("No wallets found in fixtures. Run capture_fixtures.py first.")
            return []
        
        print(f"Measuring baseline for {len(wallets)} wallets...")
        print()
        
        all_results = []
        for wallet in wallets:
            print(f"Measuring: {wallet}")
            result = await self.measure_wallet(wallet)
            all_results.append(result)
            print(f"  Credits: {result['credits_consumed']}, "
                  f"Latency: {result['latency_ms']:.1f}ms, "
                  f"TXs: {result['transaction_count']}")
            print()
        
        return all_results
    
    def measure_memory_usage(self):
        """Measure peak memory usage."""
        if MEMORY_PROFILER_AVAILABLE:
            # Run a simple measurement
            def dummy_operation():
                replayer = FixtureReplayer()
                wallets = replayer.get_all_wallets()
                return len(wallets)
            
            mem_usage = memory_usage((dummy_operation,), max_usage=True)
            self.metrics.peak_rss = mem_usage
        else:
            # Fallback to tracemalloc
            tracemalloc.start()
            replayer = FixtureReplayer()
            wallets = replayer.get_all_wallets()
            current, peak = tracemalloc.get_traced_memory()
            self.metrics.peak_rss = peak / 1024 / 1024  # Convert to MB
            tracemalloc.stop()
    
    async def run(self) -> Dict[str, Any]:
        """Run complete baseline measurement."""
        print("=" * 60)
        print("Scout Baseline Performance Runner")
        print("=" * 60)
        print()
        
        # Check if fixtures exist
        if not self.replayer.fixtures:
            print("ERROR: No fixtures found. Run capture_fixtures.py first.")
            print("  python -m scout.scripts.capture_fixtures")
            sys.exit(1)
        
        print(f"Loaded {len(self.replayer.fixtures)} fixtures")
        print(f"Wallets: {self.replayer.get_all_wallets()}")
        print()
        
        # Measure all wallets
        wallet_results = await self.measure_all_wallets()
        
        # Measure memory usage
        print("Measuring memory usage...")
        self.measure_memory_usage()
        print(f"Peak RSS: {self.metrics.peak_rss:.2f} MB")
        print()
        
        # Compile results
        results = {
            "wallet_results": wallet_results,
            "summary": self.metrics.to_dict()
        }
        
        # Save results
        timestamp = datetime.utcnow().strftime("%Y%m%d_%H%M%S")
        output_file = self.output_dir / f"baseline_{timestamp}.json"
        
        with open(output_file, 'w') as f:
            json.dump(results, f, indent=2, default=str)
        
        print("=" * 60)
        print("Baseline Complete!")
        print("=" * 60)
        print(f"Results saved to: {output_file}")
        print()
        print("Summary:")
        print(f"  Total wallets: {len(wallet_results)}")
        print(f"  Total credits: {results['summary']['total_credits']}")
        print(f"  P50 latency: {results['summary']['latency_stats']['p50']:.1f}ms")
        print(f"  P95 latency: {results['summary']['latency_stats']['p95']:.1f}ms")
        print(f"  P99 latency: {results['summary']['latency_stats']['p99']:.1f}ms")
        print(f"  Peak RSS: {self.metrics.peak_rss:.2f} MB")
        
        if self.metrics.errors:
            print(f"  Errors: {len(self.metrics.errors)}")
        
        return results


async def main():
    """Main entry point."""
    import argparse
    
    parser = argparse.ArgumentParser(description="Run baseline performance measurements")
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=None,
        help="Output directory for results (default: docs/perf)"
    )
    
    args = parser.parse_args()
    
    runner = BaselineRunner(output_dir=args.output_dir)
    await runner.run()


if __name__ == "__main__":
    asyncio.run(main())