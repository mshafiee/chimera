#!/usr/bin/env python3
"""
Production Signal Capture Tool

Captures production webhook signal patterns for use in load testing.
Run this tool in read-only mode on production for 7 days to capture realistic signal patterns.

Usage:
    python tools/capture-production-signals.py --duration 7d --output tests/load/fixtures/production_signals.json

Environment Variables:
    CHIMERA_BASE_URL - Base URL for Chimera API (default: http://localhost:3000)
    CHIMERA_API_KEY - API key for authentication (if required)
"""

import argparse
import asyncio
import aiohttp
import json
import os
import sys
from datetime import datetime, timedelta
from typing import Dict, List, Any
from collections import defaultdict


DEFAULT_BASE_URL = os.getenv("CHIMERA_BASE_URL", "http://localhost:3000")
DEFAULT_DURATION_HOURS = 24
DEFAULT_OUTPUT_FILE = "tests/load/fixtures/production_signals.json"


class SignalCapture:
    """Captures and analyzes production signal patterns."""

    def __init__(self, base_url: str, api_key: str = None):
        self.base_url = base_url.rstrip('/')
        self.api_key = api_key
        self.signals = []
        self.stats = defaultdict(lambda: defaultdict(int))

    def _get_headers(self) -> Dict[str, str]:
        """Get request headers."""
        headers = {
            'Content-Type': 'application/json',
        }
        if self.api_key:
            headers['Authorization'] = f'Bearer {self.api_key}'
        return headers

    async def capture_signals(self, duration_hours: int, interval_seconds: int = 60):
        """
        Capture signals over a duration.

        Args:
            duration_hours: How long to capture signals (hours)
            interval_seconds: How often to poll for new signals (seconds)
        """
        print(f"\n=== Production Signal Capture ===")
        print(f"Duration: {duration_hours} hours")
        print(f"Poll interval: {interval_seconds} seconds")
        print(f"Output: Will analyze signal patterns\n")

        end_time = datetime.now() + timedelta(hours=duration_hours)
        poll_count = 0

        try:
            async with aiohttp.ClientSession() as session:
                while datetime.now() < end_time:
                    poll_count += 1
                    print(f"[{datetime.now().strftime('%H:%M:%S')}] Poll #{poll_count}")

                    # Fetch recent trades/signals
                    await self._fetch_recent_signals(session)

                    # Wait before next poll
                    await asyncio.sleep(interval_seconds)

        except KeyboardInterrupt:
            print("\n\nCapture interrupted by user")

    async def _fetch_recent_signals(self, session: aiohttp.ClientSession):
        """Fetch recent signals from the API."""
        try:
            # Get recent trades (last 100)
            async with session.get(
                f"{self.base_url}/api/v1/trades?limit=100",
                headers=self._get_headers()
            ) as response:
                if response.status == 200:
                    trades = await response.json()

                    for trade in trades:
                        self._analyze_signal(trade)

        except aiohttp.ClientError as e:
            print(f"  Error fetching signals: {e}")

    def _analyze_signal(self, trade: Dict[str, Any]):
        """Analyze a signal and update statistics."""
        # Extract key fields
        strategy = trade.get('strategy', 'UNKNOWN')
        action = trade.get('action', 'UNKNOWN')
        amount_sol = trade.get('amount_sol', 0)
        token = trade.get('token', 'UNKNOWN')

        # Update statistics
        self.stats['strategy'][strategy] += 1
        self.stats['action'][action] += 1
        self.stats['token'][token] += 1

        # Track amount distribution
        if amount_sol < 0.05:
            self.stats['amount_range']['<0.05'] += 1
        elif amount_sol < 0.1:
            self.stats['amount_range']['0.05-0.1'] += 1
        elif amount_sol < 0.2:
            self.stats['amount_range']['0.1-0.2'] += 1
        else:
            self.stats['amount_range']['>0.2'] += 1

        # Store signal for later analysis
        self.signals.append({
            'strategy': strategy,
            'action': action,
            'amount_sol': float(amount_sol) if amount_sol else 0,
            'token': token,
            'timestamp': trade.get('created_at'),
        })

    def print_statistics(self):
        """Print captured signal statistics."""
        print(f"\n=== Signal Statistics ===")
        print(f"Total signals captured: {len(self.signals)}")

        print(f"\nStrategy Distribution:")
        for strategy, count in sorted(self.stats['strategy'].items(), key=lambda x: -x[1]):
            pct = (count / len(self.signals) * 100) if self.signals else 0
            print(f"  {strategy}: {count} ({pct:.1f}%)")

        print(f"\nAction Distribution:")
        for action, count in sorted(self.stats['action'].items(), key=lambda x: -x[1]):
            pct = (count / len(self.signals) * 100) if self.signals else 0
            print(f"  {action}: {count} ({pct:.1f}%)")

        print(f"\nTop Tokens:")
        token_counts = sorted(self.stats['token'].items(), key=lambda x: -x[1])[:10]
        for token, count in token_counts:
            pct = (count / len(self.signals) * 100) if self.signals else 0
            print(f"  {token}: {count} ({pct:.1f}%)")

        print(f"\nAmount Distribution:")
        for range_key, count in self.stats['amount_range'].items():
            pct = (count / len(self.signals) * 100) if self.signals else 0
            print(f"  {range_key}: {count} ({pct:.1f}%)")

    def save_patterns(self, output_file: str):
        """Save signal patterns to file for load testing."""
        output = {
            'capture_time': datetime.now().isoformat(),
            'total_signals': len(self.signals),
            'statistics': {
                'strategy_distribution': dict(self.stats['strategy']),
                'action_distribution': dict(self.stats['action']),
                'token_distribution': dict(self.stats['token']),
                'amount_distribution': dict(self.stats['amount_range']),
            },
            'signals': self.signals[-1000:],  # Last 1000 signals for reference
        }

        # Create directory if needed
        os.makedirs(os.path.dirname(output_file), exist_ok=True)

        with open(output_file, 'w') as f:
            json.dump(output, f, indent=2)

        print(f"\n=== Output Saved ===")
        print(f"Patterns saved to: {output_file}")

        # Print load test configuration suggestions
        self._print_load_test_suggestions(output)

    def _print_load_test_suggestions(self, output_file: str):
        """Print suggested load test configurations."""
        if not self.signals:
            return

        # Calculate suggestions based on captured patterns
        total_signals = len(self.signals)
        signals_per_hour = total_signals / 24 if total_signals > 0 else 0

        print(f"\n=== Load Test Configuration Suggestions ===")
        print(f"Based on {total_signals} signals captured:")
        print(f"  Average signals/hour: {signals_per_hour:.1f}")
        print(f"  Average signals/sec: {signals_per_hour / 3600:.4f}")

        # Find most common strategy
        top_strategy = max(self.stats['strategy'].items(), key=lambda x: x[1])[0]
        print(f"  Most common strategy: {top_strategy}")

        # Find buy/sell ratio
        buy_count = self.stats['action'].get('BUY', 0)
        sell_count = self.stats['action'].get('SELL', 0)
        total = buy_count + sell_count
        buy_ratio = (buy_count / total * 100) if total > 0 else 0
        print(f"  Buy/Sell ratio: {buy_ratio:.1f}% BUY")

        print(f"\nSuggested k6 command:")
        print(f"  k6 run tests/load/production_wallet_simulation.js \\")
        print(f"    --env PRODUCTION_SIGNALS_FILE={output_file} \\")
        print(f"    --env WEBHOOK_URL={self.base_url}/api/v1/webhook")


async def analyze_existing_data(base_url: str, days: int = 7):
    """
    Analyze existing production data without real-time capture.

    Args:
        base_url: Chimera API base URL
        days: Number of days to analyze
    """
    print(f"\n=== Analyzing Production Data (Last {days} Days) ===")

    capture = SignalCapture(base_url)

    async with aiohttp.ClientSession() as session:
        # Fetch trades from the last N days
        for day in range(days):
            date = datetime.now() - timedelta(days=day)
            date_str = date.strftime('%Y-%m-%d')

            print(f"Fetching data for {date_str}...")

            try:
                async with session.get(
                    f"{base_url}/api/v1/trades?date={date_str}&limit=1000",
                    headers=capture._get_headers()
                ) as response:
                    if response.status == 200:
                        trades = await response.json()
                        print(f"  Found {len(trades)} trades")

                        for trade in trades:
                            capture._analyze_signal(trade)

            except aiohttp.ClientError as e:
                print(f"  Error: {e}")

    capture.print_statistics()

    # Save patterns
    output_file = f"tests/load/fixtures/production_signals_{datetime.now().strftime('%Y%m%d')}.json"
    capture.save_patterns(output_file)


def main():
    parser = argparse.ArgumentParser(
        description="Capture production signal patterns for load testing"
    )
    parser.add_argument(
        '--url',
        default=DEFAULT_BASE_URL,
        help=f"Chimera API base URL (default: {DEFAULT_BASE_URL})"
    )
    parser.add_argument(
        '--api-key',
        help="API key for authentication (if required)"
    )
    parser.add_argument(
        '--duration',
        type=int,
        default=DEFAULT_DURATION_HOURS,
        help=f"Capture duration in hours (default: {DEFAULT_DURATION_HOURS})"
    )
    parser.add_argument(
        '--interval',
        type=int,
        default=60,
        help="Poll interval in seconds (default: 60)"
    )
    parser.add_argument(
        '--output',
        default=DEFAULT_OUTPUT_FILE,
        help=f"Output file path (default: {DEFAULT_OUTPUT_FILE})"
    )
    parser.add_argument(
        '--analyze',
        type=int,
        metavar='DAYS',
        help="Analyze existing data from last N days (no real-time capture)"
    )

    args = parser.parse_args()

    capture = SignalCapture(args.url, args.api_key)

    if args.analyze:
        # Analyze existing data
        asyncio.run(analyze_existing_data(args.url, args.analyze))
    else:
        # Real-time capture
        try:
            asyncio.run(capture.capture_signals(args.duration, args.interval))
            capture.print_statistics()
            capture.save_patterns(args.output)
        except KeyboardInterrupt:
            print("\nCapture interrupted")
            capture.print_statistics()
            capture.save_patterns(args.output)


if __name__ == "__main__":
    main()
