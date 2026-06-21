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
    helius_client = None,
) -> List:
    """
    Group wallets by shared funder address and return only the top-WQS
    wallet from each cluster.

    Wallets without a known funder are treated as singleton clusters.

    Args:
        records: List of WalletRecord objects with status="ACTIVE"
        top_n: Maximum number of active records to return after dedup
        helius_client: Optional existing HeliusClient instance (reuses session)

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
    # Use existing HeliusClient if provided, otherwise create one
    created_client = False
    try:
        from .helius_client import HeliusClient
        if helius_client is None:
            api_key = os.getenv("HELIUS_API_KEY")
            if api_key:
                helius_client = HeliusClient(api_key=api_key)
                created_client = True
        if helius_client:
            coros = [helius_client.get_wallet_funder(r.address) for r in active]
            results = await asyncio.gather(*coros, return_exceptions=True)
            for record, funder in zip(active, results):
                if isinstance(funder, Exception) or funder is None:
                    funder_map[record.address] = None
                else:
                    funder_map[record.address] = funder
    except ImportError:
        pass
    finally:
        if created_client and helius_client:
            try:
                await helius_client.close()
            except Exception:
                pass  # Non-critical

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
            setattr(record, 'cluster_id', funder)
        else:
            clusters.setdefault(f"__singleton_{singleton_count}", []).append(record)
            setattr(record, 'cluster_id', f"__singleton_{singleton_count}")
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


def apply_cross_wallet_token_correlation(
    records: List,
    wallet_tokens: Dict[str, set],
    max_overlap_ratio: float = 0.70,
    funder_map: Optional[Dict[str, Optional[str]]] = None,
) -> int:
    """
    Detect ACTIVE wallets with >max_overlap_ratio shared tokens and demote
    the lower-WQS wallet to CANDIDATE. This prevents a roster of wallets
    all trading the same pool of tokens (correlated risk).

    When funder_map is provided (wallet_address -> funder_address), wallets
    that share the same funder are demoted at a lower overlap threshold (50%)
    because shared funding is an additional sybil signal.

    Args:
        records: List of WalletRecord objects
        wallet_tokens: Dict mapping wallet_address -> set of token_addresses
        max_overlap_ratio: Maximum allowed token overlap before demotion
        funder_map: Optional dict mapping wallet_address -> funder_address

    Returns:
        Number of wallets demoted
    """
    active = [r for r in records if r.status == "ACTIVE" and r.address in wallet_tokens]
    if len(active) < 2:
        return 0

    demoted = 0
    demoted_addresses = set()

    active.sort(key=lambda r: r.wqs_score or 0, reverse=True)

    for i, r1 in enumerate(active):
        if r1.address in demoted_addresses:
            continue
        tokens1 = wallet_tokens.get(r1.address, set())
        if len(tokens1) < 2:
            continue
        for r2 in active[i + 1:]:
            if r2.address in demoted_addresses:
                continue
            tokens2 = wallet_tokens.get(r2.address, set())
            if len(tokens2) < 2:
                continue
            if not tokens1 & tokens2:
                continue
            overlap = len(tokens1 & tokens2)
            min_size = min(len(tokens1), len(tokens2))
            if min_size == 0:
                continue
            overlap_ratio = overlap / min_size

            # Determine threshold: shared funder lowers the bar to 50%
            share_funder = (
                funder_map is not None
                and funder_map.get(r1.address)
                and funder_map.get(r1.address) == funder_map.get(r2.address)
            )
            threshold = 0.50 if share_funder else max_overlap_ratio

            if overlap_ratio > threshold:
                reason = "funder" if share_funder else "token"
                r2.status = "CANDIDATE"
                r2.notes = (r2.notes or "") + (
                    f" | Demoted: >{threshold*100:.0f}% {reason} overlap "
                    f"({overlap}/{min_size}) with higher-WQS ACTIVE wallet {r1.address[:8]}..."
                )
                demoted_addresses.add(r2.address)
                demoted += 1

    if demoted > 0:
        print(f"[Clustering] Cross-wallet token correlation: demoted {demoted} "
              f"wallets with >{max_overlap_ratio*100:.0f}% token overlap")

    return demoted
