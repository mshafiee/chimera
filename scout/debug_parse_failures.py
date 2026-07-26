#!/usr/bin/env python3
"""
Debug script to analyze parse failures and understand the structure of failing transactions.
Run this on the production server to get detailed information about parse failures.
"""

import json
import sys

def analyze_parse_failures():
    """Analyze the parse failure logs from the scout."""

    # Read scout logs
    with open('/opt/chimera/logs/scout.log', 'r') as f:
        logs = f.read()

    # Find all parse failures
    parse_failures = []

    import re
    pattern = r'\[([^\]]+)\] Parse fail #(\d+): type=(\w+), sig=([^\s,]+), reason=(\w+)\s*\n\s*-\s+tokenTransfers: (\d+) items\n\s*-\s+nativeTransfers: (\d+) items\n\s*-\s+accountData: (\d+) items\n\s*-\s+events: (\[.*?\])?'
    matches = re.finditer(pattern, logs)

    for match in matches:
        wallet, fail_num, tx_type, sig, reason, token_transfers, native_transfers, account_data, events = match.groups()

        # Extract more details if available
        details = {}
        detail_pattern = rf'Parse fail #{fail_num}: type={tx_type}, sig={sig}, reason={reason}.*?-\s+events: (\[.*?\])?'
        detail_match = re.search(detail_pattern, logs[match.start():match.start()+500])

        if detail_match:
            events_str = detail_match.group(1)
            if events_str:
                try:
                    events = json.loads(events_str.replace("'", '"'))
                    details['events'] = events
                except:
                    pass

        parse_failures.append({
            'wallet': wallet,
            'fail_num': int(fail_num),
            'tx_type': tx_type,
            'signature': sig,
            'reason': reason,
            'token_transfers': int(token_transfers),
            'native_transfers': int(native_transfers),
            'account_data': int(account_data),
            'events': events
        })

    # Count failures by reason
    from collections import Counter
    reason_counts = Counter(f['reason'] for f in parse_failures)

    print("=" * 80)
    print("PARSING FAILURE ANALYSIS")
    print("=" * 80)
    print(f"Total parse failures analyzed: {len(parse_failures)}")
    print(f"\nFailures by reason:")
    for reason, count in reason_counts.most_common():
        print(f"  {reason:20s}: {count:5d}")

    # Analyze no_primary_token failures
    no_primary = [f for f in parse_failures if f['reason'] == 'no_primary_token']
    print(f"\nno_primary_token failures: {len(no_primary)}")
    print(f"  Avg tokenTransfers: {sum(f['token_transfers'] for f in no_primary) / len(no_primary):.1f}")
    print(f"  Avg nativeTransfers: {sum(f['native_transfers'] for f in no_primary) / len(no_primary):.1f}")
    print(f"  Avg accountData: {sum(f['account_data'] for f in no_primary) / len(no_primary):.1f}")

    # Show sample no_primary_token failure
    if no_primary:
        sample = no_primary[0]
        print(f"\nSample no_primary_token failure:")
        print(f"  Wallet: {sample['wallet']}")
        print(f"  Signature: {sample['signature']}")
        print(f"  tokenTransfers: {sample['token_transfers']}")
        print(f"  nativeTransfers: {sample['native_transfers']}")
        print(f"  accountData: {sample['account_data']}")
        if sample['events']:
            print(f"  events: {sample['events']}")

    # Analyze direction_ambiguous failures
    direction_amb = [f for f in parse_failures if f['reason'] == 'direction_ambiguous']
    print(f"\ndirection_ambiguous failures: {len(direction_amb)}")
    if direction_amb:
        sample = direction_amb[0]
        print(f"  Sample: {sample['wallet']} - {sample['signature']}")

    print("\n" + "=" * 80)

if __name__ == '__main__':
    analyze_parse_failures()
