#!/usr/bin/env python3
"""
Chimera Scout - Live API Health Check
Verifies connectivity to Helius, Birdeye, and DexScreener.
"""
import sys
import os
from pathlib import Path

# Add parent to path
sys.path.append(str(Path(__file__).parent.parent))

from config import ScoutConfig
from core.helius_client import HeliusClient
from core.birdeye_client import BirdeyeClient
from core.liquidity_sources.dexscreener_client import DexScreenerClient

def check_helius():
    print("\n[1/3] Checking Helius API...")
    key = ScoutConfig.get_helius_api_key()
    if not key:
        print("❌ HELIUS_API_KEY not found in env.")
        return False
    
    client = HeliusClient(api_key=key)
    # Try to fetch history for a known active wallet (e.g., a top trader or exchange)
    test_wallet = "5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1" 
    
    try:
        txs = client.get_wallet_transactions(test_wallet, limit=5)
        if txs and len(txs) > 0:
            print(f"✅ Helius connected. Fetched {len(txs)} txs.")
            return True
        else:
            print("⚠️  Helius connected but returned 0 transactions (might be rate limited or empty wallet).")
            return True
    except Exception as e:
        print(f"❌ Helius failed: {e}")
        return False

def check_birdeye():
    print("\n[2/3] Checking Birdeye API...")
    key = ScoutConfig.get_birdeye_api_key()
    if not key:
        print("⚠️  BIRDEYE_API_KEY not found. Historical liquidity will be limited.")
        return True # Not fatal
    
    client = BirdeyeClient(api_key=key)
    # Check Price of SOL
    sol_addr = "So11111111111111111111111111111111111111112"
    try:
        liq = client.get_current_liquidity(sol_addr)
        if liq:
            print(f"✅ Birdeye connected. SOL Price: ${liq.price_usd:.2f}")
            return True
        else:
            print("❌ Birdeye returned no data for SOL.")
            return False
    except Exception as e:
        print(f"❌ Birdeye failed: {e}")
        return False

def check_dexscreener():
    print("\n[3/3] Checking DexScreener (No key required)...")
    client = DexScreenerClient()
    sol_addr = "So11111111111111111111111111111111111111112"
    try:
        liq = client.get_current_liquidity(sol_addr)
        if liq:
            print(f"✅ DexScreener connected. SOL Liquidity found.")
            return True
        else:
            print("❌ DexScreener returned no data.")
            return False
    except Exception as e:
        print(f"❌ DexScreener failed: {e}")
        return False

if __name__ == "__main__":
    print("=== Scout Connectivity Check ===")
    h = check_helius()
    b = check_birdeye()
    d = check_dexscreener()
    
    if h and b and d:
        print("\n✅ All systems GO.")
        sys.exit(0)
    else:
        print("\n❌ Some systems failed checks.")
        sys.exit(1)
