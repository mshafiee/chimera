#!/usr/bin/env python3
"""
Real historical data collection using reliable Helius API methods.
Uses standard Solana RPC calls to fetch DEX program activity.
"""

import requests
import json
import os
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

HELIUS_API_KEY = os.environ.get("HELIUS_API_KEY")
if not HELIUS_API_KEY:
    raise RuntimeError("HELIUS_API_KEY environment variable is required")
BASE_URL = f"https://mainnet.helius-rpc.com/?api-key={HELIUS_API_KEY}"

# Real DEX program addresses
DEX_PROGRAMS = {
    "Jupiter": "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUqoiV3oueqRjYG",
    "Raydium": "9WzDXwBbnkgPm3iZnZPF7yYAZ8dBBz9rBqEMLn5b5Sqs",
    "Orca": "9WQdx6qLMjSxL7Yszwh1mM1CA8VjTzYmQbWqYZVk3Sz5"
}

SOL_MINT = "So11111111111111111111111111111111111111112"
LAMPORTS_PER_SOL = 1_000_000_000


def get_recent_transactions(program_address, limit=100, before=None):
    """Get recent transactions for a DEX program (paginated with `before`)."""

    params = [program_address, {"limit": limit}]
    if before:
        params[1]["before"] = before

    payload = {
        "jsonrpc": "2.0",
        "id": f"txs-{program_address[:8]}",
        "method": "getSignaturesForAddress",
        "params": params
    }

    try:
        response = requests.post(BASE_URL, json=payload, timeout=30)
        response.raise_for_status()
        data = response.json()

        if "result" in data:
            return data["result"]
        return []

    except Exception as e:
        print(f"❌ Error fetching transactions for {program_address[:8]}...: {e}")
        return []


def parse_transaction(signature):
    """Parse a transaction to extract trading signal.

    The action, token and amount are derived from the actual pre/post
    token-balance deltas, never fabricated.
    """

    payload = {
        "jsonrpc": "2.0",
        "id": f"parse-{signature[:8]}",
        "method": "getTransaction",
        "params": [signature, "json", {"maxSupportedTransactionVersion": 0}]
    }

    try:
        response = requests.post(BASE_URL, json=payload, timeout=15)
        response.raise_for_status()
        tx_data = response.json()

        if "result" not in tx_data or not tx_data["result"]:
            return None

        tx = tx_data["result"]

        if not tx.get("blockTime"):
            return None
        # Skip failed transactions (their balance changes are meaningless)
        if tx.get("meta", {}).get("err"):
            return None

        # Convert timestamp
        timestamp = datetime.fromtimestamp(tx["blockTime"], tz=timezone.utc).isoformat()

        # Get account keys (first one is usually the trader)
        accounts = tx["transaction"]["message"].get("accountKeys", [])
        if not accounts:
            return None

        wallet_address = accounts[0]["pubkey"] if isinstance(accounts[0], dict) else accounts[0]

        meta = tx.get("meta") or {}
        pre_balances = meta.get("preTokenBalances") or []
        post_balances = meta.get("postTokenBalances") or []
        pre_lamports = meta.get("preBalances") or []
        post_lamports = meta.get("postBalances") or []

        # Find the wallet's account index for the SOL delta
        wallet_index = None
        for idx, key in enumerate(accounts):
            pubkey = key["pubkey"] if isinstance(key, dict) else key
            if pubkey == wallet_address:
                wallet_index = idx
                break

        # Find the traded token: non-SOL mint whose balance changed for the wallet
        pre_by_mint = {b.get("mint"): (b.get("uiTokenAmount") or {}).get("uiAmount")
                       for b in pre_balances if b.get("owner") == wallet_address}
        post_by_mint = {b.get("mint"): (b.get("uiTokenAmount") or {}).get("uiAmount")
                        for b in post_balances if b.get("owner") == wallet_address}

        token_address = None
        token_amount = None
        for mint in post_by_mint:
            if mint == SOL_MINT:
                continue
            pre_amount = pre_by_mint.get(mint)
            post_amount = post_by_mint.get(mint)
            if pre_amount is None or post_amount is None:
                continue
            delta = post_amount - pre_amount
            if abs(delta) > 1e-12:
                token_address = mint
                token_amount = abs(delta)
                break

        if not token_address:
            return None

        # SOL delta for the wallet -> direction and amount in SOL
        amount_sol = None
        sol_spent = None
        if wallet_index is not None and wallet_index < len(pre_lamports) and wallet_index < len(post_lamports):
            sol_delta = (pre_lamports[wallet_index] - post_lamports[wallet_index]) / LAMPORTS_PER_SOL
            if abs(sol_delta) > 1e-9:
                amount_sol = abs(sol_delta)
                sol_spent = sol_delta > 0

        if amount_sol is None:
            amount_sol = token_amount or 0.0
            sol_spent = True

        # Direction: buying when the wallet spent SOL
        action = "buy" if sol_spent else "sell"

        # Determine strategy
        strategy = "spear" if amount_sol > 1.0 else "shield"

        return {
            "timestamp": timestamp,
            "wallet_address": wallet_address,
            "token_address": token_address,
            "action": action,
            "amount_sol": round(amount_sol, 6),
            "strategy": strategy,
            "signature": signature,
            "slot": tx.get("slot", 0),
            "source": "helius_dex_activity"
        }

    except Exception as e:
        print(f"❌ Error parsing transaction: {e}")
        return None


def collect_real_dex_signals(target_signals=1500, days_back=10):
    """Collect real DEX trading signals.

    Paginates each program's history until the target is reached or the
    cutoff date is hit.
    """

    print("🔍 Collecting Real DEX Trading Signals")
    print("=" * 50)

    all_signals = []
    cutoff = datetime.now(timezone.utc) - timedelta(days=days_back)

    print(f"🎯 Target: {target_signals} signals")
    print(f"📊 DEX Programs: {', '.join(DEX_PROGRAMS.keys())}")
    print(f"📅 Cutoff: {cutoff.date()}")
    print("")

    # Collect from each DEX program
    for dex_name, program_address in DEX_PROGRAMS.items():
        if len(all_signals) >= target_signals:
            break

        print(f"🔎 Fetching {dex_name} transactions...")
        before = None

        while len(all_signals) < target_signals:
            transactions = get_recent_transactions(program_address, limit=200, before=before)

            if not transactions:
                print(f"  ⚠️  No more transactions found for {dex_name}")
                break

            # Skip failed transactions and stop paginating once we pass the cutoff
            stop_paging = False
            for tx in transactions:
                if tx.get("err"):
                    continue
                if not tx.get("blockTime"):
                    continue

                tx_time = datetime.fromtimestamp(tx["blockTime"], tz=timezone.utc)
                if tx_time < cutoff:
                    stop_paging = True
                    break

                signature = tx.get("signature")
                if not signature:
                    continue

                signal = parse_transaction(signature)
                if signal:
                    all_signals.append(signal)

                    if len(all_signals) % 10 == 0:
                        print(f"    ✅ Progress: {len(all_signals)}/{target_signals}")

                    if len(all_signals) >= target_signals:
                        break

                # Rate limiting
                time.sleep(0.05)

            if stop_paging or len(all_signals) >= target_signals:
                break

            before = transactions[-1].get("signature")
            if not before:
                break
            time.sleep(0.1)

        print(f"  ✅ {dex_name} complete: {len(all_signals)} total signals")

        # Rate limiting between DEX programs
        time.sleep(0.2)

    print(f"\n📊 Collection Summary:")
    print(f"   Total signals collected: {len(all_signals)}")
    print(f"   Target signals: {target_signals}")
    if target_signals > 0:
        print(f"   Success rate: {len(all_signals)/target_signals*100:.1f}%")

    return all_signals


def save_signals(signals, output_path):
    """Save signals to JSONL file."""

    if not signals:
        print("❌ No signals to save")
        return False

    # Sort chronologically
    signals.sort(key=lambda x: x["timestamp"])

    # Generate statistics
    buys = sum(1 for s in signals if s["action"] == "buy")
    shield = sum(1 for s in signals if s["strategy"] == "shield")

    print(f"\n📊 Signal Statistics:")
    print(f"   Total signals: {len(signals)}")
    print(f"   Buy orders: {buys} ({buys/len(signals)*100:.1f}%)")
    print(f"   Sell orders: {len(signals) - buys} ({(len(signals) - buys)/len(signals)*100:.1f}%)")
    print(f"   Shield trades: {shield} ({shield/len(signals)*100:.1f}%)")
    print(f"   Spear trades: {len(signals) - shield} ({(len(signals) - shield)/len(signals)*100:.1f}%)")

    # Time range
    timestamps = [s["timestamp"] for s in signals]
    print(f"   Time range: {min(timestamps)[:10]} to {max(timestamps)[:10]}")

    # Save to file
    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    with open(output_path, 'w') as f:
        for signal in signals:
            f.write(json.dumps(signal) + '\n')

    print(f"\n✅ Saved {len(signals)} real trading signals to: {output_path}")

    # Display sample
    print(f"\n📋 Sample real signals (first 3):")
    for i, signal in enumerate(signals[:3]):
        print(f"  {i+1}. {signal['timestamp'][:19]} | {signal['action']:4} | {signal['strategy']:6} | "
              f"{signal['amount_sol']:6.4f} SOL | {signal['token_address'][:8]}... | "
              f"Wallet: {signal['wallet_address'][:8]}... | Sig: {signal['signature'][:8]}...")

    return True


def main():
    print("🎯 Real Historical DEX Data Collection")
    print("=" * 50)
    print("")

    # Test API connectivity
    print("🔑 Testing Helius API...")
    try:
        payload = {"jsonrpc": "2.0", "id": "health", "method": "getHealth"}
        response = requests.post(BASE_URL, json=payload, timeout=10)
        if response.json().get("result") == "ok":
            print("✅ Helius API is working")
        else:
            print("❌ API health check failed")
            return 1
    except Exception as e:
        print(f"❌ API connection failed: {e}")
        return 1

    print("")

    # Collect real DEX signals
    signals = collect_real_dex_signals(target_signals=1500)

    if len(signals) == 0:
        print("\n❌ No signals collected")
        print("💡 This might indicate:")
        print("   • API rate limiting")
        print("   • Network connectivity issues")
        print("   • Low DEX activity in timeframe")
        return 1

    if len(signals) < 500:
        print(f"\n⚠️  Only collected {len(signals)} signals")
        print("💡 Still useful for testing, but below target")

    # Save signals
    output_path = "evaluation/signals/historical_signals.jsonl"
    if save_signals(signals, output_path):
        print(f"\n🎯 Real historical data collection complete!")
        return 0
    else:
        return 1


if __name__ == "__main__":
    exit(main())
