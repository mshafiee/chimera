#!/usr/bin/env python3
"""
Download real Solana DEX historical trading data from Vybe Network API.
Vybe Network provides free access to historical trade data for Solana DEXs.
"""

import requests
import json
from datetime import datetime, timezone
from pathlib import Path

def download_vybe_historical_trades():
    """Download historical DEX trades from Vybe Network API."""

    print("🎯 Vybe Network - Real Solana DEX Historical Data")
    print("=" * 60)

    # Vybe Network API endpoint
    vybe_api_url = "https://solana-historical-trade-data-api.vybenetwork.com/api"

    try:
        print("📡 Testing Vybe Network API connectivity...")

        # Test API connectivity
        response = requests.get(f"{vybe_api_url}/health", timeout=30)
        if response.status_code == 200:
            print("✅ Vybe Network API is accessible")
        else:
            print(f"⚠️  API returned status: {response.status_code}")

        print("\n🔍 Available Vybe Network endpoints:")

        # Get available DEXs
        response = requests.get(f"{vybe_api_url}/dexes", timeout=30)
        if response.status_code == 200:
            dexes = response.json()
            print(f"   DEXs: {', '.join(dexes.get('dexes', []))}")

        # Get supported tokens
        response = requests.get(f"{vybe_api_url}/tokens", timeout=30)
        if response.status_code == 200:
            tokens = response.json()
            print(f"   Tokens available: {len(tokens.get('tokens', []))} different tokens")

        print("\n💡 Vybe Network provides:")
        print("   • Historical trade data for Raydium, Orca, Jupiter, and more")
        print("   • SPL and Token-2022 support")
        print("   • CSV export functionality")
        print("   • Real-time and historical data")

        print("\n📊 To download data, you can:")
        print("   1. Visit: https://solana-historical-trade-data-api.vybenetwork.com/")
        print("   2. Use their web interface to download CSV files")
        print("   3. Or use their API programmatically (requires authentication)")

        print("\n⚠️  For this evaluation, let's try programmatic access...")

        # Try to get recent trades (this might require authentication)
        print("\n📡 Attempting to fetch recent DEX trades...")

        # Try getting recent Raydium trades
        raydium_address = "9WzDXwBbnkgPm3iZnZPF7yYAZ8dBBz9rBqEMLn5b5Sqs"

        trade_params = {
            "program": raydium_address,
            "limit": 100,
            "offset": 0
        }

        response = requests.get(
            f"{vybe_api_url}/trades",
            params=trade_params,
            timeout=30,
            headers={"Accept": "application/json"}
        )

        if response.status_code == 200:
            trades_data = response.json()
            print(f"✅ Successfully fetched {len(trades_data.get('trades', []))} trades")

            return convert_vybe_to_signals(trades_data), None

        elif response.status_code == 401:
            print("⚠️  Authentication required for API access")
            print("💡 Sign up for free API key at: https://www.vybenetwork.com/")
            return None, "auth_required"

        elif response.status_code == 404:
            print("⚠️  Endpoint not found - API structure may have changed")
            print("💡 Check Vybe Network documentation for current endpoints")
            return None, "endpoint_changed"

        else:
            print(f"⚠️  API returned: {response.status_code} - {response.text[:100]}")
            return None, f"http_{response.status_code}"

    except requests.exceptions.RequestException as e:
        print(f"❌ Network error: {e}")
        print("💡 Check your internet connection")
        return None, "network_error"
    except Exception as e:
        print(f"❌ Error accessing Vybe API: {e}")
        return None, "unknown"

def convert_vybe_to_signals(trades_data):
    """Convert Vybe Network trade data to our signal format."""

    if not trades_data or "trades" not in trades_data:
        return []

    signals = []
    trades = trades_data["trades"]

    for trade in trades:
        try:
            # Skip records that lack the fields we need; never fabricate values
            raw_ts = trade.get("timestamp") or trade.get("time") or ''
            wallet = trade.get("wallet") or trade.get("trader") or trade.get("authority") or ''
            token = trade.get("token_mint") or trade.get("mint") or ''
            signature = trade.get("signature") or trade.get("tx_id") or ''
            amount_raw = trade.get("amount", trade.get("quantity"))

            if not raw_ts or not wallet or not token or amount_raw is None:
                print("⚠️  Skipping trade: missing required fields")
                continue

            # Normalize timestamp to a string (API may return epoch or null)
            if isinstance(raw_ts, (int, float)):
                timestamp = datetime.fromtimestamp(raw_ts, tz=timezone.utc).isoformat()
            else:
                timestamp = str(raw_ts)

            amount = float(amount_raw)

            # Prefer the API's explicit direction field when present;
            # otherwise only infer from the sign of a signed amount
            direction = (trade.get("side") or trade.get("direction") or '').lower()
            if direction in ("buy", "sell"):
                action = direction
            elif amount != 0:
                action = "buy" if amount > 0 else "sell"
            else:
                print("⚠️  Skipping trade: cannot determine direction")
                continue

            # Determine strategy based on amount
            strategy = "spear" if abs(amount) > 1.0 else "shield"

            signal = {
                "timestamp": timestamp,
                "wallet_address": wallet,
                "token_address": token,
                "action": action,
                "amount_sol": abs(amount),
                "strategy": strategy,
                "price_usd": float(trade.get("price") or trade.get("usd_price") or 1.0),
                "market": trade.get("market") or trade.get("dex") or "unknown",
                "source": "vybe_network_api",
                "signature": signature
            }

            signals.append(signal)

        except Exception as e:
            print(f"⚠️  Skipping trade due to error: {e}")
            continue

    print(f"✅ Converted {len(signals)} trades to signal format")
    return signals

def save_signals(signals, output_path="evaluation/signals/historical_signals.jsonl"):
    """Save signals to JSONL file."""

    if not signals or len(signals) == 0:
        print("❌ No signals to save")
        return False

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    with open(output_path, 'w') as f:
        for signal in signals:
            f.write(json.dumps(signal) + '\n')

    # Statistics
    buys = sum(1 for s in signals if s["action"] == "buy")
    shield = sum(1 for s in signals if s["strategy"] == "shield")

    print(f"\n✅ Saved {len(signals)} real signals to: {output_path}")
    print(f"📊 Signal Statistics:")
    print(f"   Buy orders: {buys} ({buys/len(signals)*100:.1f}%)")
    print(f"   Sell orders: {len(signals) - buys} ({(len(signals) - buys)/len(signals)*100:.1f}%)")
    print(f"   Shield trades: {shield} ({shield/len(signals)*100:.1f}%)")
    print(f"   Spear trades: {len(signals) - shield} ({(len(signals) - shield)/len(signals)*100:.1f}%)")

    # Sample
    print(f"\n📋 Sample real signals:")
    for i, signal in enumerate(signals[:3]):
        print(f"  {i+1}. {signal['timestamp'][:19]} | {signal['action']:4} | {signal['strategy']:6} | "
              f"{signal['amount_sol']:6.4f} SOL | {signal['token_address'][:8]}...")

    return True

def main():
    print("🎯 Download Real Solana DEX Historical Data")
    print("=" * 60)
    print("")

    # Download from Vybe Network
    signals, failure_reason = download_vybe_historical_trades()

    if signals and len(signals) > 0:
        # Save the signals
        if save_signals(signals):
            print(f"\n🎯 SUCCESS: Real historical data downloaded!")
            print(f"📊 Source: Vybe Network API")
            print(f"📈 Quality: Real blockchain DEX trades")
            print(f"✅ Ready for evaluation use")
            return 0
        else:
            print(f"\n❌ ERROR: Failed to save signals")
            return 1
    else:
        if failure_reason == "auth_required":
            print(f"\n⚠️  Vybe Network API requires authentication")
            print(f"💡 RECOMMENDED APPROACH:")
            print(f"   1. Visit: https://www.vybenetwork.com/")
            print(f"   2. Sign up for free API access")
            print(f"   3. Download CSV files manually from their dashboard")
            print(f"   4. Import the downloaded CSV into evaluation/signals/")
        else:
            print(f"\n⚠️  Vybe Network download failed: {failure_reason or 'no data'}")
            print("💡 Check the error above (network, endpoint, or data availability)")
        print(f"\n💡 ALTERNATIVE: Use realistic synthetic data for evaluation")
        return 1

if __name__ == "__main__":
    exit(main())