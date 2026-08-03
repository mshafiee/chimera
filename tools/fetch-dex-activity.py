#!/usr/bin/env python3
"""
Enhanced version to fetch real DEX trading activity using Helius APIs.
Focuses on actual DEX program activity rather than specific wallets.
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


def get_biggest_transactions(limit=50):
    """Get recent big transactions from Helius."""
    payload = {
        "jsonrpc": "2.0",
        "id": "big-txs",
        "method": "getBigTransactions",
        "params": [limit]
    }

    try:
        response = requests.post(BASE_URL, json=payload, timeout=30)
        response.raise_for_status()
        data = response.json()

        if "result" in data:
            return data["result"]
        return []

    except Exception as e:
        print(f"❌ Error fetching big transactions: {e}")
        return []


def parse_transaction_activity(tx_data):
    """Parse transaction to extract trading signal.

    Only transactions that touch a known DEX program are considered; the
    action, token and amount are derived from actual balance deltas.
    """

    if not tx_data or "result" not in tx_data:
        return None

    tx = tx_data["result"]

    if not tx or not tx.get("meta") or not tx.get("transaction"):
        return None

    # Skip failed transactions
    if tx.get("meta", {}).get("err"):
        return None

    if not tx.get("blockTime"):
        return None

    # Only consider transactions that actually touch a DEX program
    accounts = tx["transaction"]["message"].get("accountKeys", [])
    account_pubkeys = {a["pubkey"] if isinstance(a, dict) else a for a in accounts}
    dex_name = None
    for name, program in DEX_PROGRAMS.items():
        if program in account_pubkeys:
            dex_name = name
            break
    if not dex_name:
        return None

    # Extract basic info
    timestamp = datetime.fromtimestamp(tx.get("blockTime", time.time()), tz=timezone.utc).isoformat()

    # Get the first account as the wallet/trader
    if not accounts:
        return None

    wallet_address = accounts[0]["pubkey"] if isinstance(accounts[0], dict) else accounts[0]

    # Determine if it's a swap/DEX transaction from the token balance deltas
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

    # Find the traded token via balance deltas for this wallet
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

    # SOL delta determines direction and amount in SOL
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

    action = "buy" if sol_spent else "sell"

    # Strategy based on amount
    strategy = "spear" if amount_sol > 1.0 else "shield"

    return {
        "timestamp": timestamp,
        "wallet_address": wallet_address,
        "token_address": token_address,
        "action": action,
        "amount_sol": round(amount_sol, 6),
        "strategy": strategy,
        "signature": tx.get("signature", ""),
        "slot": tx.get("slot", 0),
        "dex": dex_name
    }


def collect_real_dex_activity(days_back=10, target_signals=1500):
    """Collect real DEX trading activity."""

    print("🔍 Collecting Real DEX Trading Activity")
    print("=" * 50)

    signals = []
    seen_signatures = set()
    attempts = 0
    # Each attempt yields at most `limit` signals; allow enough attempts to
    # actually reach the target
    max_attempts = max(20, (target_signals // 50) * 2)
    cutoff = datetime.now(timezone.utc) - timedelta(days=days_back)

    print(f"🎯 Target: {target_signals} signals")
    print(f"📅 Time range: Last {days_back} days")
    print("")

    while len(signals) < target_signals and attempts < max_attempts:
        attempts += 1
        print(f"📡 Attempt {attempts}/{max_attempts}...")

        # Get recent big transactions
        big_txs = get_biggest_transactions(limit=50)

        if not big_txs:
            print("  ⚠️  No transactions returned")
            time.sleep(1)
            continue

        print(f"  🔍 Processing {len(big_txs)} transactions...")

        for tx in big_txs:
            try:
                # Get full transaction details
                if "signature" not in tx:
                    continue

                signature = tx["signature"]
                # Never process the same transaction twice
                if signature in seen_signatures:
                    continue
                seen_signatures.add(signature)

                payload = {
                    "jsonrpc": "2.0",
                    "id": f"tx-{len(signals)}",
                    "method": "getTransaction",
                    "params": [signature, "json", {"maxSupportedTransactionVersion": 0}]
                }

                response = requests.post(BASE_URL, json=payload, timeout=15)
                tx_data = response.json()

                signal = parse_transaction_activity(tx_data)
                if signal:
                    # Honor the requested time window
                    try:
                        sig_time = datetime.fromisoformat(signal["timestamp"])
                    except ValueError:
                        sig_time = datetime.now(timezone.utc)
                    if sig_time < cutoff:
                        continue

                    signals.append(signal)
                    print(f"    ✅ Signal {len(signals)}: {signal['timestamp'][:19]} | {signal['action']:4} | {signal['strategy']:6}")

                    if len(signals) >= target_signals:
                        break

                # Rate limiting
                time.sleep(0.1)

            except Exception as e:
                print(f"    ❌ Error processing transaction: {e}")
                continue

        print(f"  📊 Progress: {len(signals)}/{target_signals} signals")

        # Rate limiting between attempts
        if len(signals) < target_signals:
            time.sleep(0.5)

    print(f"\n📊 Collection Summary:")
    print(f"   Total signals collected: {len(signals)}")
    print(f"   API attempts made: {attempts}")
    if target_signals > 0:
        print(f"   Success rate: {len(signals)/target_signals*100:.1f}%")

    return signals


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
              f"Sig: {signal['signature'][:8]}...")


def main():
    print("🎯 Enhanced Real Historical Data Collection")
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

    # Collect real DEX activity
    signals = collect_real_dex_activity(days_back=10, target_signals=1500)

    if len(signals) == 0:
        print("\n❌ No signals collected")
        print("💡 This might indicate:")
        print("   • API rate limiting")
        print("   • Low DEX activity in timeframe")
        print("   • Network connectivity issues")
        return 1

    if len(signals) < 100:
        print(f"\n⚠️  Only collected {len(signals)} signals (below target 1500)")
        print("💡 This is still useful for testing the evaluation system")

    # Save the signals
    output_path = "evaluation/signals/historical_signals.jsonl"
    save_signals(signals, output_path)

    print(f"\n🎯 Real historical data collection complete!")
    print(f"📊 Collected: {len(signals)} authentic trading signals")
    print(f"📁 Location: {output_path}")

    return 0


if __name__ == "__main__":
    exit(main())
