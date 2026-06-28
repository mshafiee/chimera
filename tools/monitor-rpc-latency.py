#!/usr/bin/env python3
"""
RPC Latency Monitoring Tool

Continuously monitors RPC latency and provides real-time alerts
when approaching or exceeding the 50ms budget.

Usage:
    python3 tools/monitor-rpc-latency.py [--url BASE_URL] [--warning MS] [--critical MS]

Environment Variables:
    CHIMERA_BASE_URL - Base URL for Chimera API (default: http://localhost:3000)
    RPC_LATENCY_WARNING_MS - Warning threshold (default: 40)
    RPC_LATENCY_CRITICAL_MS - Critical threshold (default: 50)
"""

import argparse
import asyncio
import os
import sys
from datetime import datetime
from typing import Optional

import aiohttp


# Configuration defaults
DEFAULT_BASE_URL = os.getenv("CHIMERA_BASE_URL", "http://localhost:3000")
DEFAULT_WARNING_MS = float(os.getenv("RPC_LATENCY_WARNING_MS", "40"))
DEFAULT_CRITICAL_MS = float(os.getenv("RPC_LATENCY_CRITICAL_MS", "50"))
CHECK_INTERVAL_SECONDS = 30


# ANSI color codes for terminal output
class Colors:
    GREEN = "\033[92m"      # ✅ Good
    YELLOW = "\033[93m"     # ⚠️  Warning
    RED = "\033[91m"        # 🔴 Critical
    RESET = "\033[0m"       # Reset


def get_status_symbol(latency_ms: float, warning_ms: float, critical_ms: float) -> str:
    """Get the status emoji and color for a given latency."""
    if latency_ms < warning_ms:
        return f"{Colors.GREEN}✅{Colors.RESET}"
    elif latency_ms < critical_ms:
        return f"{Colors.YELLOW}⚠️ {Colors.RESET}"
    else:
        return f"{Colors.RED}🔴{Colors.RESET}"


def get_color_for_latency(latency_ms: float, warning_ms: float, critical_ms: float) -> str:
    """Get the color code for latency value display."""
    if latency_ms < warning_ms:
        return Colors.GREEN
    elif latency_ms < critical_ms:
        return Colors.YELLOW
    else:
        return Colors.RED


async def check_rpc_latency(session: aiohttp.ClientSession, base_url: str) -> Optional[dict]:
    """
    Check RPC latency by calling the health endpoint and measuring response time.

    Returns:
        dict with keys: latency_ms, rpc_healthy, rpc_primary, latency_history
        or None if the check fails
    """
    try:
        start = datetime.now()
        async with session.get(
            f"{base_url}/api/v1/health",
            timeout=aiohttp.ClientTimeout(total=10)
        ) as response:
            if response.status != 200:
                print(f"ERROR: Health endpoint returned status {response.status}")
                return None

            data = await response.json()
            latency_ms = (datetime.now() - start).total_seconds() * 1000

            # Extract RPC health information
            rpc_health = data.get("rpc", {})
            rpc_healthy = rpc_health.get("healthy", True)
            rpc_latency = rpc_health.get("latency_ms", None)

            return {
                "latency_ms": latency_ms,
                "rpc_healthy": rpc_healthy,
                "rpc_latency_ms": rpc_latency,
                "queue_depth": data.get("queue_depth", 0),
                "circuit_breaker": data.get("circuit_breaker", {}).get("state", "UNKNOWN"),
                "fallback_duration": data.get("fallback_duration_secs", None),
            }

    except asyncio.TimeoutError:
        print(f"ERROR: Request timed out after 10 seconds")
        return None
    except aiohttp.ClientError as e:
        print(f"ERROR: Failed to connect to {base_url}: {e}")
        return None
    except Exception as e:
        print(f"ERROR: Unexpected error: {e}")
        return None


async def get_rpc_metrics(session: aiohttp.ClientSession, base_url: str) -> Optional[dict]:
    """
    Get detailed RPC metrics from the metrics endpoint.
    """
    try:
        async with session.get(
            f"{base_url}/metrics",
            timeout=aiohttp.ClientTimeout(total=5)
        ) as response:
            if response.status != 200:
                return None

            metrics_text = await response.text()

            # Parse Prometheus metrics for RPC latency histogram
            metrics = {
                "p50": None,
                "p95": None,
                "p99": None,
                "health": None,
            }

            for line in metrics_text.split('\n'):
                if line.startswith('chimera_rpc_latency_ms_bucket'):
                    # Parse histogram buckets
                    if 'le="5"' in line:
                        parts = line.split(' ')
                        if len(parts) >= 2:
                            metrics["p50"] = float(parts[-1])
                    elif 'le="50"' in line:
                        parts = line.split(' ')
                        if len(parts) >= 2:
                            metrics["p95"] = float(parts[-1])
                    elif 'le="100"' in line:
                        parts = line.split(' ')
                        if len(parts) >= 2:
                            metrics["p99"] = float(parts[-1])
                elif line.startswith('chimera_rpc_health'):
                    parts = line.split(' ')
                    if len(parts) >= 2:
                        metrics["health"] = float(parts[-1])

            return metrics

    except Exception as e:
        print(f"ERROR: Failed to get metrics: {e}")
        return None


def print_header(warning_ms: float, critical_ms: float):
    """Print the monitoring header."""
    print(f"\n{'='*70}")
    print(f"RPC Latency Monitor")
    print(f"{'='*70}")
    print(f"WARNING Threshold: {warning_ms}ms")
    print(f"CRITICAL Threshold: {critical_ms}ms")
    print(f"Check Interval: {CHECK_INTERVAL_SECONDS} seconds")
    print(f"Press Ctrl+C to stop")
    print(f"{'='*70}\n")


def print_status_row(check_num: int, latency_ms: float, warning_ms: float, critical_ms: float,
                    metrics: dict, check_data: dict):
    """Print a single status row with color coding."""
    timestamp = datetime.now().strftime("%H:%M:%S")
    status = get_status_symbol(latency_ms, warning_ms, critical_ms)
    latency_color = get_color_for_latency(latency_ms, warning_ms, critical_ms)

    # Build status line
    status_line = (
        f"[{timestamp}] #{check_num:3d} | {status} | "
        f"Latency: {latency_color}{latency_ms:.1f}ms{Colors.RESET}"
    )

    # Add additional info if available
    if check_data:
        if check_data.get("rpc_latency_ms"):
            status_line += f" | RPC: {check_data['rpc_latency_ms']:.1f}ms"
        if check_data.get("queue_depth") is not None:
            qd = check_data["queue_depth"]
            qd_color = Colors.GREEN if qd < 800 else Colors.YELLOW if qd < 900 else Colors.RED
            status_line += f" | Queue: {qd_color}{qd}{Colors.RESET}"
        if check_data.get("circuit_breaker"):
            cb = check_data["circuit_breaker"]
            cb_color = Colors.GREEN if cb == "ACTIVE" else Colors.RED
            status_line += f" | CB: {cb_color}{cb}{Colors.RESET}"
        if check_data.get("fallback_duration"):
            fd = check_data["fallback_duration"]
            status_line += f" | Fallback: {fd}s"

    # Add histogram metrics if available
    if metrics:
        if metrics.get("p95") is not None:
            status_line += f" | p95: {metrics['p95']:.1f}ms"
        if metrics.get("health") is not None:
            health_emoji = "✅" if metrics["health"] == 1 else "❌"
            status_line += f" | Health: {health_emoji}"

    print(status_line)


async def main():
    """Main monitoring loop."""
    parser = argparse.ArgumentParser(
        description="Monitor Chimera RPC latency in real-time"
    )
    parser.add_argument(
        "--url",
        default=DEFAULT_BASE_URL,
        help=f"Base URL for Chimera API (default: {DEFAULT_BASE_URL})"
    )
    parser.add_argument(
        "--warning",
        type=float,
        default=DEFAULT_WARNING_MS,
        help=f"Warning threshold in ms (default: {DEFAULT_WARNING_MS})"
    )
    parser.add_argument(
        "--critical",
        type=float,
        default=DEFAULT_CRITICAL_MS,
        help=f"Critical threshold in ms (default: {DEFAULT_CRITICAL_MS})"
    )
    parser.add_argument(
        "--interval",
        type=int,
        default=CHECK_INTERVAL_SECONDS,
        help=f"Check interval in seconds (default: {CHECK_INTERVAL_SECONDS})"
    )
    parser.add_argument(
        "--metrics",
        action="store_true",
        help="Include Prometheus histogram metrics in output"
    )

    args = parser.parse_args()

    print_header(args.warning, args.critical)

    check_count = 0
    consecutive_critical = 0
    max_consecutive_critical = 0

    try:
        async with aiohttp.ClientSession() as session:
            while True:
                check_count += 1

                # Perform latency check
                check_data = await check_rpc_latency(session, args.url)

                if check_data is None:
                    print(f"[{datetime.now().strftime('%H:%M:%S')}] ERROR: Failed to get health status")
                    await asyncio.sleep(args.interval)
                    continue

                latency_ms = check_data["latency_ms"]

                # Get histogram metrics if requested
                metrics = None
                if args.metrics:
                    metrics = await get_rpc_metrics(session, args.url)

                # Print status
                print_status_row(
                    check_count, latency_ms, args.warning, args.critical, metrics, check_data
                )

                # Track consecutive critical alerts
                if latency_ms >= args.critical:
                    consecutive_critical += 1
                    max_consecutive_critical = max(max_consecutive_critical, consecutive_critical)

                    if consecutive_critical == 3:
                        print(f"\n{Colors.RED}{'='*70}{Colors.RESET}")
                        print(f"{Colors.RED}CRITICAL: RPC latency has exceeded {args.critical}ms for {consecutive_critical} consecutive checks{Colors.RESET}")
                        print(f"{Colors.RED}Trading may be degraded. Consider investigating.{Colors.RESET}")
                        print(f"{Colors.RED}{'='*70}{Colors.RESET}\n")
                else:
                    consecutive_critical = 0

                # Wait before next check
                await asyncio.sleep(args.interval)

    except KeyboardInterrupt:
        print(f"\n\n{'='*70}")
        print("Monitoring stopped by user")
        print(f"Total checks performed: {check_count}")
        print(f"Max consecutive critical alerts: {max_consecutive_critical}")
        print(f"{'='*70}\n")
        sys.exit(0)


if __name__ == "__main__":
    asyncio.run(main())
