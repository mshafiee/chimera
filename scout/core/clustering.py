"""
Wallet clustering and deduplication for Scout roster management.

Groups wallets by common funder address to prevent a single trader's
sybil wallets from dominating the roster with correlated risk.

When clustering is enabled, only the top-WQS wallet from each cluster
is kept in the final ACTIVE roster.
"""

import os
import logging
from typing import Dict, List, Optional

logger = logging.getLogger(__name__)


async def cluster_and_dedup(
    records: List,
    top_n: int = 50,
) -> List:
    """
    Group wallets by shared funder address and return only the top-WQS
    wallet from each cluster.

    Wallets without a known funder are treated as singleton clusters.

    Args:
        records: List of WalletRecord objects with status="ACTIVE"
        top_n: Maximum number of active records to return after dedup

    Returns:
        Deduplicated list of WalletRecord objects (ACTIVE only, reduced)
    """
    import asyncio

    enabled = os.getenv("SCOUT_CLUSTER_DEDUP", "true").lower() == "true"
    if not enabled:
        return records

    active = [r for r in records if r.status == "ACTIVE"]
    if len(active) <= 1:
        return records

    # Fetch funders for all active wallets in batch
    funder_map: Dict[str, Optional[str]] = {}
    # Use existing funder cache or Helius client if available
    try:
        from .helius_client import HeliusClient
        api_key = os.getenv("HELIUS_API_KEY")
        if api_key:
            helius = HeliusClient(api_key=api_key)
            coros = [helius.get_wallet_funder(r.address) for r in active]
            results = await asyncio.gather(*coros, return_exceptions=True)
            for record, funder in zip(active, results):
                if isinstance(funder, Exception) or funder is None:
                    funder_map[record.address] = None
                else:
                    funder_map[record.address] = funder
    except ImportError:
        pass

    if not funder_map:
        return records  # No clustering possible without funder data

    # Build clusters: {funder_address: [WalletRecord, ...]}
    # Wallets without a funder are kept as singletons
    clusters: Dict[str, List] = {}
    singleton_count = 0

    for record in active:
        funder = funder_map.get(record.address)
        if funder:
            clusters.setdefault(funder, []).append(record)
        else:
            clusters.setdefault(f"__singleton_{singleton_count}", []).append(record)
            singleton_count += 1

    # Select top-WQS wallet from each cluster
    deduped = []
    for cluster_records in clusters.values():
        best = max(cluster_records, key=lambda r: r.wqs_score or 0)
        deduped.append(best)

    # Sort by WQS descending and limit
    deduped.sort(key=lambda r: r.wqs_score or 0, reverse=True)
    deduped = deduped[:top_n]

    removed = len(active) - len(deduped)
    if removed > 0:
        print(f"[Clustering] Removed {removed} correlated wallets "
              f"(same funder), retained {len(deduped)} top-WQS representatives")

    # Update the original records list: demote removed ACTIVE to CANDIDATE
    deduped_addresses = {r.address for r in deduped}
    for record in records:
        if record.status == "ACTIVE" and record.address not in deduped_addresses:
            record.status = "CANDIDATE"
            record.notes = (record.notes or "") + " | Demoted: cluster dedup (same funder as higher-WQS wallet)"

    return records
