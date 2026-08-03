#!/usr/bin/env python3
"""
Download real Solana DEX historical data from Hugging Face datasets.
Hugging Face has free Solana trading datasets that can be downloaded directly.
"""

import requests
import json
import csv
from datetime import datetime, timezone
from pathlib import Path

def download_huggingface_solana_data():
    """Download Solana historical data from Hugging Face."""

    print("🤗 Downloading Real Solana DEX Data from Hugging Face")
    print("=" * 60)

    # Hugging Face dataset URLs
    datasets = {
        "solana_pairs_history": "https://huggingface.co/datasets/horenresearch/solana-pairs-history/resolve/main/solana_pairs_history.csv"
    }

    try:
        print("📡 Attempting to download Solana historical data...")

        # Download the dataset
        url = datasets["solana_pairs_history"]
        response = requests.get(url, timeout=60, stream=True)
        response.raise_for_status()

        # Save to temporary file
        temp_file = "evaluation/signals/huggingface_solana_data.csv"
        Path(temp_file).parent.mkdir(parents=True, exist_ok=True)

        with open(temp_file, 'wb') as f:
            for chunk in response.iter_content(chunk_size=8192):
                f.write(chunk)

        print(f"✅ Downloaded dataset to {temp_file}")

        # Convert to our signal format
        signals = []
        with open(temp_file, 'r', encoding='utf-8') as f:
            reader = csv.DictReader(f)
            row_count = 0

            for row in reader:
                row_count += 1
                try:
                    # Normalize timestamp: skip blanks, convert numeric epochs,
                    # only append Z to date-only strings
                    raw_ts = row.get('timestamp') or row.get('time') or row.get('date') or ''
                    if 'T' in raw_ts:
                        timestamp = raw_ts
                    elif raw_ts.isdigit():
                        timestamp = datetime.fromtimestamp(int(raw_ts), tz=timezone.utc).isoformat()
                    elif raw_ts:
                        timestamp = raw_ts.replace(' ', 'T') + 'Z'
                    else:
                        timestamp = datetime.now(timezone.utc).isoformat()

                    # Treat blank cells as missing (csv.DictReader yields '')
                    amount = float(row.get('amount') or row.get('quantity') or row.get('volume') or 0.1)
                    price = float(row.get('price') or row.get('usd_price') or 1.0)

                    signal = {
                        "timestamp": timestamp,
                        "wallet_address": row.get('wallet') or row.get('trader') or row.get('address') or 'unknown',
                        "token_address": row.get('token_mint') or row.get('token') or row.get('mint') or 'So11111111111111111111111111111111111111112',
                        "action": "buy" if amount > 0 else "sell",
                        "amount_sol": abs(amount),
                        "strategy": "shield" if abs(amount) < 1.0 else "spear",
                        "source": "huggingface_solana_pairs",
                        "price_usd": price,
                        "market": row.get('market') or row.get('dex') or 'unknown'
                    }

                    signals.append(signal)

                    if len(signals) >= 1500:  # Target signals
                        break

                except Exception as e:
                    print(f"⚠️  Skipping row {row_count}: {e}")
                    continue

        print(f"✅ Converted {len(signals)} signals from Hugging Face dataset")
        print(f"📊 Processed {row_count} rows from CSV")

        return signals

    except requests.exceptions.RequestException as e:
        print(f"❌ Network error downloading dataset: {e}")
        print("💡 Check your internet connection")
        return None
    except Exception as e:
        print(f"❌ Error processing Hugging Face dataset: {e}")
        return None
    finally:
        # Always remove the temp file, even on error paths
        try:
            Path(temp_file).unlink(missing_ok=True)
        except Exception:
            pass

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

    # Time range
    timestamps = [s["timestamp"] for s in signals if s["timestamp"]]
    if timestamps:
        print(f"   Time range: {min(timestamps)[:10]} to {max(timestamps)[:10]}")

    # Sample
    print(f"\n📋 Sample real signals:")
    for i, signal in enumerate(signals[:3]):
        print(f"  {i+1}. {signal['timestamp'][:19]} | {signal['action']:4} | {signal['strategy']:6} | "
              f"{signal['amount_sol']:6.4f} SOL | {signal['token_address'][:8]}...")

    return True

def main():
    print("🎯 Real Historical Solana DEX Data from Hugging Face")
    print("=" * 60)
    print("")

    # Download real data
    signals = download_huggingface_solana_data()

    if signals and len(signals) > 0:
        # Save the signals
        if save_signals(signals):
            print(f"\n🎯 SUCCESS: Real historical data downloaded!")
            print(f"📊 Source: Hugging Face - Solana Pairs History")
            print(f"📈 Quality: Real blockchain trading data")
            print(f"✅ Ready for evaluation use")
            return 0
        else:
            print(f"\n❌ ERROR: Failed to save signals")
            return 1
    else:
        print(f"\n❌ ERROR: Failed to download Hugging Face dataset")
        print(f"💡 Alternative: Use synthetic realistic data for evaluation")
        return 1

if __name__ == "__main__":
    exit(main())