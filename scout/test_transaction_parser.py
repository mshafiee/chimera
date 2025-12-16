#!/usr/bin/env python3
"""
Test script to manually fetch and inspect a SWAP transaction from Helius API.
This helps debug why parse_swap_transaction() is failing on mainnet.
"""

import os
import sys
import asyncio
import json
from pathlib import Path

# Add parent directory to path
sys.path.insert(0, str(Path(__file__).parent))

from core.helius_client import HeliusClient


async def fetch_and_inspect_transaction():
    """Fetch a sample SWAP transaction and inspect its structure."""
    
    # Get API key from environment
    api_key = os.getenv("HELIUS_API_KEY")
    if not api_key:
        print("ERROR: HELIUS_API_KEY environment variable not set")
        print("Set it with: export HELIUS_API_KEY=your_key_here")
        return
    
    print("=" * 80)
    print("Transaction Parser Debug Tool")
    print("=" * 80)
    print(f"API Key: {api_key[:8]}...{api_key[-4:]}")
    print()
    
    # Initialize Helius client
    client = HeliusClient(api_key=api_key)
    
    # Test wallet addresses (from discovery)
    test_wallets = [
        "srmqPvymPx9SrHEZqFDvC3KVszRAHd9X7ddZy4j6Yb3J",
        "8WwcNqdZwxWNjAhXLEDRVmk6xDxdvU1uqp3WCaZCG3x",
        "EmyzSTZrQfYJvKf3bLKj3RWdpHgXhVVuqbAELJtdZiX",
    ]
    
    for wallet in test_wallets:
        print(f"\n{'='*80}")
        print(f"Testing wallet: {wallet}")
        print(f"{'='*80}\n")
        
        # Fetch recent transactions
        print("Fetching recent SWAP transactions...")
        transactions = await client.get_wallet_transactions(
            wallet_address=wallet,
            days=30,
            limit=5  # Just get 5 for testing
        )
        
        if not transactions:
            print(f"  No transactions found for {wallet[:8]}...")
            continue
        
        print(f"  Found {len(transactions)} transactions")
        print()
        
        # Inspect first transaction
        tx = transactions[0]
        print("=" * 80)
        print("FIRST TRANSACTION - COMPLETE STRUCTURE")
        print("=" * 80)
        print(json.dumps(tx, indent=2, default=str))
        print()
        
        # Try to parse it
        print("=" * 80)
        print("ATTEMPTING TO PARSE")
        print("=" * 80)
        swap = client.parse_swap_transaction(tx, wallet_address=wallet)
        
        if swap:
            print("✓ Parse SUCCESSFUL!")
            print(json.dumps(swap, indent=2, default=str))
        else:
            print("✗ Parse FAILED!")
            print("\nDebugging info:")
            print(f"  Transaction type: {tx.get('type')}")
            print(f"  Signature: {tx.get('signature', '')[:16]}...")
            print(f"  Timestamp: {tx.get('timestamp')}")
            print(f"  Token transfers: {len(tx.get('tokenTransfers', []))}")
            print(f"  Native transfers: {len(tx.get('nativeTransfers', []))}")
            print(f"  Account data: {len(tx.get('accountData', []))}")
            
            # Inspect token transfers
            if tx.get('tokenTransfers'):
                print("\n  Token Transfers:")
                for i, transfer in enumerate(tx['tokenTransfers'][:3]):
                    print(f"    [{i}]")
                    print(f"      mint: {transfer.get('mint', 'N/A')[:16]}...")
                    print(f"      fromUserAccount: {transfer.get('fromUserAccount', 'N/A')[:16]}...")
                    print(f"      toUserAccount: {transfer.get('toUserAccount', 'N/A')[:16]}...")
                    print(f"      tokenAmount: {transfer.get('tokenAmount', 'N/A')}")
            
            # Inspect native transfers
            if tx.get('nativeTransfers'):
                print("\n  Native Transfers:")
                for i, transfer in enumerate(tx['nativeTransfers'][:3]):
                    print(f"    [{i}]")
                    print(f"      fromUserAccount: {transfer.get('fromUserAccount', 'N/A')[:16]}...")
                    print(f"      toUserAccount: {transfer.get('toUserAccount', 'N/A')[:16]}...")
                    print(f"      amount: {transfer.get('amount', 'N/A')}")
        
        print()
        
        # Test all transactions from this wallet
        print("=" * 80)
        print(f"TESTING ALL {len(transactions)} TRANSACTIONS")
        print("=" * 80)
        
        success_count = 0
        fail_count = 0
        
        for i, tx in enumerate(transactions):
            swap = client.parse_swap_transaction(tx, wallet_address=wallet)
            if swap:
                success_count += 1
                print(f"  [{i+1}] ✓ sig={tx.get('signature', '')[:8]}...")
            else:
                fail_count += 1
                print(f"  [{i+1}] ✗ sig={tx.get('signature', '')[:8]}...")
        
        print()
        print(f"Results: {success_count} successful, {fail_count} failed")
        print(f"Success rate: {success_count}/{len(transactions)} ({success_count*100/len(transactions):.1f}%)")
        
        # Only test first wallet for now
        break
    
    await client._close_session()
    print("\n" + "=" * 80)
    print("Test complete!")
    print("=" * 80)


if __name__ == "__main__":
    # Set API key if provided as argument
    if len(sys.argv) > 1:
        os.environ["HELIUS_API_KEY"] = sys.argv[1]
    
    asyncio.run(fetch_and_inspect_transaction())
