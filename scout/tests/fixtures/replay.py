"""
Replay helper for zero-credit benchmarking.

Patches HeliusClient._make_request to return recorded fixtures instead of
making real API calls. This enables benchmark runs without consuming credits.
"""

import json
import os
from pathlib import Path
from typing import Dict, Any, Optional, List
from unittest.mock import AsyncMock, patch
from datetime import datetime, timedelta

FIXTURE_DIR = Path(__file__).parent.parent / "fixtures" / "helius"


class FixtureReplayer:
    """Replays recorded Helius API responses from fixtures."""
    
    def __init__(self, fixture_dir: Optional[Path] = None):
        self.fixture_dir = fixture_dir or FIXTURE_DIR
        self.fixtures: Dict[str, Dict[str, Any]] = {}
        self._load_fixtures()
    
    def _load_fixtures(self):
        """Load all fixture files into memory."""
        if not self.fixture_dir.exists():
            raise FileNotFoundError(f"Fixture directory not found: {self.fixture_dir}")
        
        for fixture_file in self.fixture_dir.glob("*.json"):
            if fixture_file.name == "manifest.json":
                continue
            
            try:
                with open(fixture_file, 'r') as f:
                    fixture_data = json.load(f)
                    fixture_name = fixture_file.stem
                    self.fixtures[fixture_name] = fixture_data
            except Exception as e:
                print(f"WARNING: Failed to load fixture {fixture_file}: {e}")
    
    def get_wallet_transactions(
        self,
        wallet: str,
        days: int,
        limit: int,
        transaction_type: Optional[str] = None
    ) -> Optional[List[Dict[str, Any]]]:
        """
        Get wallet transactions from fixtures.
        
        Mimics the behavior of get_wallet_transactions but returns cached data.
        """
        # Find matching fixture phase
        for fixture_name, fixture_data in self.fixtures.items():
            if fixture_data.get("wallet") != wallet:
                continue
            
            # Try to match phase by days/limit
            for phase_name, phase_data in fixture_data.get("phases", {}).items():
                if phase_data.get("days") == days and phase_data.get("limit") == limit:
                    transactions = phase_data.get("transactions", [])
                    
                    # Apply client-side filtering by transaction_type if specified
                    if transaction_type:
                        filtered = [
                            tx for tx in transactions
                            if tx.get("type") == transaction_type
                        ]
                        return filtered
                    
                    return transactions
        
        return None
    
    def get_all_wallets(self) -> List[str]:
        """Get all wallet addresses from fixtures."""
        wallets = set()
        for fixture_data in self.fixtures.values():
            wallet = fixture_data.get("wallet")
            if wallet:
                wallets.add(wallet)
        return list(wallets)
    
    def get_fixture_for_wallet(self, wallet: str) -> Optional[Dict[str, Any]]:
        """Get complete fixture data for a wallet."""
        for fixture_name, fixture_data in self.fixtures.items():
            if fixture_data.get("wallet") == wallet:
                return fixture_data
        return None


def create_replay_patch(replayer: FixtureReplayer):
    """
    Create a mock patch for HeliusClient._make_request that uses fixtures.
    
    Usage:
        replayer = FixtureReplayer()
        with create_replay_patch(replayer):
            client = HeliusClient(api_key="dummy")
            txs = await client.get_wallet_transactions(wallet, days=30, limit=1000)
    """
    
    async def mock_make_request(self, endpoint: str, params: Dict[str, Any]) -> Optional[Any]:
        """Mock _make_request that returns fixture data."""
        
        # Parse endpoint to determine request type
        if "/transactions" in endpoint or "wallet" in endpoint.lower():
            wallet = params.get("wallet") or params.get("account")
            days = params.get("days", 30)
            limit = params.get("limit", 1000)
            transaction_type = params.get("type")
            
            if wallet:
                return replayer.get_wallet_transactions(
                    wallet, days, limit, transaction_type
                )
        
        # Return empty list for unknown endpoints
        return []
    
    return patch.object(
        'core.helius_client.HeliusClient',
        '_make_request',
        new=AsyncMock(side_effect=mock_make_request)
    )


def get_replayer() -> FixtureReplayer:
    """Get a fixture replayer instance."""
    return FixtureReplayer()


def get_test_wallets() -> List[str]:
    """Get wallet addresses from fixtures for testing."""
    replayer = get_replayer()
    return replayer.get_all_wallets()


if __name__ == "__main__":
    # Test the replayer
    print("Testing FixtureReplayer...")
    replayer = get_replayer()
    
    print(f"Loaded {len(replayer.fixtures)} fixtures")
    print(f"Test wallets: {replayer.get_all_wallets()}")
    
    # Test getting transactions
    for wallet in replayer.get_all_wallets():
        print(f"\nTesting wallet: {wallet}")
        fixture = replayer.get_fixture_for_wallet(wallet)
        if fixture:
            print(f"  Fixture name: {fixture.get('fixture_name')}")
            print(f"  Phases: {list(fixture.get('phases', {}).keys())}")
            
            # Test default phase
            txs = replayer.get_wallet_transactions(wallet, days=30, limit=1000)
            print(f"  Default phase transactions: {len(txs) if txs else 0}")