#!/usr/bin/env python3
"""
Generate synthetic historical trading signals for 10-day evaluation.
Creates realistic trading patterns for Solana copy trading platform.
"""

import json
import random
from datetime import datetime, timedelta
from pathlib import Path

# Configuration
DAYS = 10
SIGNALS_PER_DAY = 150  # ~15 signals per hour over 10 hours
START_DATE = datetime.now() - timedelta(days=DAYS)

# Realistic Solana token addresses (well-known tokens)
KNOWN_TOKENS = [
    "So11111111111111111111111111111111111111112",  # SOL
    "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",  # USDC
    "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",  # USDT
    "7vfCXTUXx5WJV5JADk17DUJ4ksgau7utNKj4b963voxs",  # RAY
    "JUPyiwrYwFq2aXtLguiPtoGQuLiqBOMkGeVxLvDj8jqj",  # JUP
    "D2aRAnSZTZaRP2iwhTvHfa9hVsAkq0cL4LwFVc8fTCs7",  # BONK
]

# Realistic wallet addresses (format: 32-44 character base58)
WALLET_ADDRESSES = [
    "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83YGJP5RxYt1",
    "9WzDXwBbnkgPm3iZnZPF7yYAZ8dBBz9rBqEMLn5b5Sqs",
    "5G5UXGXKcRKGcMA5VWWCBZc5JHPn5gTxDRo2rRNbh5Gv",
    "3HC5Uyt3UWb36dUhvaaC1UGqXj7cEQLNbLdcqNm2EYu2",
    "7u1XfFGz6mYYqAKWdjA6kQgVDqZXJDnhLgLPpT4A9ZE",
]

STRATEGIES = ["shield", "spear"]
ACTIONS = ["buy", "sell"]

def generate_realistic_signals():
    """Generate realistic trading signals with proper patterns."""
    signals = []

    for day in range(DAYS):
        day_signals = []
        current_date = START_DATE + timedelta(days=day)

        # Generate signals throughout the day (9 AM - 7 PM UTC)
        for hour in range(9, 19):
            signals_in_hour = SIGNALS_PER_DAY // 10  # Distribute evenly

            for _ in range(signals_in_hour):
                # Add some randomness to timing
                minute = random.randint(0, 59)
                second = random.randint(0, 59)
                timestamp = current_date.replace(
                    hour=hour, minute=minute, second=second
                )

                # Realistic trading patterns
                strategy = random.choices(
                    STRATEGIES,
                    weights=[0.7, 0.3],  # 70% shield, 30% spear
                    k=1
                )[0]

                # Shield trades are smaller, spear trades are larger
                if strategy == "shield":
                    amount_sol = round(random.uniform(0.1, 2.0), 4)
                else:
                    amount_sol = round(random.uniform(1.0, 5.0), 4)

                # Realistic action distribution (slightly more buys)
                action = random.choices(ACTIONS, weights=[0.55, 0.45], k=1)[0]

                # Realistic token distribution
                token_address = random.choices(
                    KNOWN_TOKENS,
                    weights=[0.3, 0.25, 0.2, 0.1, 0.1, 0.05],  # SOL and stablecoins more common
                    k=1
                )[0]

                signal = {
                    "timestamp": timestamp.isoformat() + "Z",
                    "wallet_address": random.choice(WALLET_ADDRESSES),
                    "token_address": token_address,
                    "action": action,
                    "amount_sol": abs(amount_sol),
                    "strategy": strategy,
                    "price_usd": round(random.uniform(0.5, 150.0), 2)  # Realistic token prices
                }

                day_signals.append(signal)

        # Shuffle to remove perfect hourly pattern
        random.shuffle(day_signals)
        signals.extend(day_signals)

    # Sort chronologically
    signals.sort(key=lambda x: x["timestamp"])

    return signals

def validate_signals(signals):
    """Validate signal format and content."""
    required_fields = ["timestamp", "wallet_address", "token_address", "action", "amount_sol", "strategy"]

    for i, signal in enumerate(signals):
        # Check required fields
        for field in required_fields:
            if field not in signal:
                print(f"❌ Signal {i}: Missing field '{field}'")
                return False

        # Validate field types
        if not isinstance(signal["timestamp"], str):
            print(f"❌ Signal {i}: Invalid timestamp type")
            return False

        if not isinstance(signal["amount_sol"], (int, float)):
            print(f"❌ Signal {i}: Invalid amount_sol type")
            return False

        if signal["action"] not in ACTIONS:
            print(f"❌ Signal {i}: Invalid action '{signal['action']}'")
            return False

        if signal["strategy"] not in STRATEGIES:
            print(f"❌ Signal {i}: Invalid strategy '{signal['strategy']}'")
            return False

    print(f"✅ All {len(signals)} signals validated successfully")
    return True

def write_signals_jsonl(signals, output_path):
    """Write signals to JSONL file."""
    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    with open(output_path, 'w') as f:
        for signal in signals:
            f.write(json.dumps(signal) + '\n')

    print(f"✅ Wrote {len(signals)} signals to {output_path}")

def generate_summary(signals):
    """Generate summary statistics."""
    total_signals = len(signals)
    buys = sum(1 for s in signals if s["action"] == "buy")
    sells = total_signals - buys
    shield_trades = sum(1 for s in signals if s["strategy"] == "shield")
    spear_trades = total_signals - shield_trades

    print(f"""
📊 Historical Signal Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Total Signals: {total_signals}
Duration: {DAYS} days
Signals per Day: ~{total_signals // DAYS}

Trading Distribution:
  Buy Orders: {buys} ({buys/total_signals*100:.1f}%)
  Sell Orders: {sells} ({sells/total_signals*100:.1f}%)

Strategy Distribution:
  Shield (Low-risk): {shield_trades} ({shield_trades/total_signals*100:.1f}%)
  Spear (High-reward): {spear_trades} ({spear_trades/total_signals*100:.1f}%)

Timeline: {START_DATE.strftime('%Y-%m-%d')} to {(START_DATE + timedelta(days=DAYS-1)).strftime('%Y-%m-%d')}
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
""")

def main():
    print("🎯 Generating Historical Trading Signals for 10-Day Evaluation")
    print("=" * 60)

    # Generate signals
    print("📈 Generating realistic trading patterns...")
    signals = generate_realistic_signals()

    # Validate signals
    print("🔍 Validating signal format...")
    if not validate_signals(signals):
        print("❌ Signal validation failed")
        return 1

    # Generate summary
    generate_summary(signals)

    # Write to file
    output_path = "evaluation/signals/historical_signals.jsonl"
    write_signals_jsonl(signals, output_path)

    # Sample signals
    print("\n📋 Sample signals (first 3):")
    for i, signal in enumerate(signals[:3]):
        print(f"  {i+1}. {signal['timestamp'][:19]} | {signal['action']:4} | "
              f"{signal['strategy']:6} | {signal['amount_sol']:6.4f} SOL | "
              f"{signal['token_address'][:8]}...")

    print(f"\n✅ Historical signals ready for evaluation!")
    print(f"📁 Location: {output_path}")

    return 0

if __name__ == "__main__":
    exit(main())