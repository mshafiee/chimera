#!/usr/bin/env python3
"""
Capture real Helius API responses as fixtures for zero-credit benchmarking.

This script should be run ONCE with a real API key to capture representative
wallet responses. After capture, all subsequent benchmark runs use replay.

WARNING: This script consumes real Helius credits. Run only when necessary.
"""

import asyncio
import json
import os
import sys
from pathlib import Path
from typing import Dict, Any, List
from datetime import datetime, timedelta

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

from core.helius_client import HeliusClient
from core.helius_credit_tracker import CreditTracker

FIXTURE_DIR = Path(__file__).parent.parent / "tests" / "fixtures" / "helius"
FIXTURE_DIR.mkdir(parents=True, exist_ok=True)

# Representative wallet mix for fixtures
# High-activity wallet with many SWAPs
HIGH_ACTIVITY_WALLET = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
# Low-activity wallet
LOW_ACTIVITY_WALLET = "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890AB"
# Wallet that might have non-SWAP transaction types
NON_SWAP_WALLET = "5kLmNoAbCdEfGhIjKlMnOpQrStUvWxYz0987654321CD"

WALLET_FIXTURES = [
    {
        "wallet": HIGH_ACTIVITY_WALLET,
        "name": "high_activity_wallet",
        "description": "High-activity wallet with many SWAP transactions"
    },
    {
        "wallet": LOW_ACTIVITY_WALLET,
        "name": "low_activity_wallet",
        "description": "Low-activity wallet with few transactions"
    },
    {
        "wallet": NON_SWAP_WALLET,
        "name": "non_swap_wallet",
        "description": "Wallet with potential non-SWAP transaction types"
    }
]


async def capture_wallet_transactions(
    client: HeliusClient,
    wallet: str,
    fixture_name: str,
    days: int = 30,
    limit: int = 1000
) -> Dict[str, Any]:
    """
    Capture wallet transactions and save as fixture.
    
    Returns fixture metadata including credit consumption.
    """
    print(f"Capturing transactions for {fixture_name} ({wallet})...")
    
    # Track credit consumption
    credit_tracker = CreditTracker()
    initial_credits = credit_tracker.get_remaining_credits()
    
    # Capture transactions with different parameters for cache key testing
    fixtures = {}
    
    # Capture with default parameters (discovery phase typical)
    print(f"  Capturing with days={days}, limit={limit}...")
    txs = await client.get_wallet_transactions(wallet, days=days, limit=limit)
    fixtures["default"] = {
        "wallet": wallet,
        "days": days,
        "limit": limit,
        "transactions": txs,
        "count": len(txs) if txs else 0
    }
    
    # Capture with analysis phase parameters (different days/limit for cache key testing)
    print(f"  Capturing with days=7, limit=500...")
    txs_analysis = await client.get_wallet_transactions(wallet, days=7, limit=500)
    fixtures["analysis"] = {
        "wallet": wallet,
        "days": 7,
        "limit": 500,
        "transactions": txs_analysis,
        "count": len(txs_analysis) if txs_analysis else 0
    }
    
    # Capture with validation phase parameters
    print(f"  Capturing with days=1, limit=100...")
    txs_validation = await client.get_wallet_transactions(wallet, days=1, limit=100)
    fixtures["validation"] = {
        "wallet": wallet,
        "days": 1,
        "limit": 100,
        "transactions": txs_validation,
        "count": len(txs_validation) if txs_validation else 0
    }
    
    # Calculate credit consumption
    final_credits = credit_tracker.get_remaining_credits()
    credits_consumed = initial_credits - final_credits
    
    fixture_metadata = {
        "fixture_name": fixture_name,
        "wallet": wallet,
        "captured_at": datetime.utcnow().isoformat(),
        "credits_consumed": credits_consumed,
        "phases": fixtures
    }
    
    # Save fixture file
    fixture_file = FIXTURE_DIR / f"{fixture_name}.json"
    with open(fixture_file, 'w') as f:
        json.dump(fixture_metadata, f, indent=2, default=str)
    
    print(f"  ✓ Saved to {fixture_file}")
    print(f"  Credits consumed: {credits_consumed}")
    
    return fixture_metadata


async def main():
    """Main capture function."""
    print("=" * 60)
    print("Helius Fixture Capture Script")
    print("=" * 60)
    print()
    print("WARNING: This script consumes real Helius credits!")
    print("Fixtures are for benchmark reproducibility only.")
    print()
    
    # Check for API key
    api_key = os.getenv("HELIUS_API_KEY")
    if not api_key:
        print("ERROR: HELIUS_API_KEY environment variable not set")
        print("Export it with: export HELIUS_API_KEY=your_key_here")
        sys.exit(1)
    
    print(f"Using API key: {api_key[:10]}...")
    print()
    
    # Create Helius client
    client = HeliusClient(api_key=api_key)
    
    # Capture all fixtures
    all_metadata = []
    for wallet_fixture in WALLET_FIXTURES:
        try:
            metadata = await capture_wallet_transactions(
                client,
                wallet_fixture["wallet"],
                wallet_fixture["name"]
            )
            all_metadata.append(metadata)
            print()
        except Exception as e:
            print(f"ERROR capturing {wallet_fixture['name']}: {e}")
            import traceback
            traceback.print_exc()
            print()
    
    # Save manifest
    manifest = {
        "captured_at": datetime.utcnow().isoformat(),
        "total_fixtures": len(all_metadata),
        "total_credits_consumed": sum(m["credits_consumed"] for m in all_metadata),
        "fixtures": all_metadata
    }
    
    manifest_file = FIXTURE_DIR / "manifest.json"
    with open(manifest_file, 'w') as f:
        json.dump(manifest, f, indent=2, default=str)
    
    print("=" * 60)
    print("Capture Complete!")
    print("=" * 60)
    print(f"Total fixtures: {len(all_metadata)}")
    print(f"Total credits consumed: {manifest['total_credits_consumed']}")
    print(f"Manifest saved to: {manifest_file}")
    print()
    print("You can now run benchmarks with zero credit consumption using:")
    print("  python -m scout.scripts.bench_baseline")


if __name__ == "__main__":
    asyncio.run(main())