"""
Wallet clustering and deduplication for Scout roster management.

Groups wallets by common funder address to prevent a single trader's
sybil wallets from dominating the roster with correlated risk.

When clustering is enabled, only the top-WQS wallet from each cluster
is kept in the final ACTIVE roster.
"""

import os
import logging
from typing import Dict, List, Optional, Set

logger = logging.getLogger(__name__)

# Builtin CEX seed set (minimal - operators extend via file)
_BUILTIN_EXCHANGE_FUNDERS: Set[str] = {
    "BinanceHotWallet1111111111111111111111111111",
    "CoinbaseHotWallet111111111111111111111111111",
    "OKXHotWallet1111111111111111111111111111111",
    "BybitHotWallet11111111111111111111111111111",
    "KrakenHotWallet1111111111111111111111111111",
}

# Exchange funder set (will be loaded from file and unioned with builtin)
_EXCHANGE_FUNDERS: Set[str] = set()


def _load_exchange_funders() -> None:
    """Load exchange funder addresses from a local file."""
    path = os.getenv("SCOUT_EXCHANGE_FUNDERS_PATH", "scout/config/exchange_funders.txt")
    if not os.path.exists(path):
        logger.info(f"Exchange funders file not found at {path}, using builtin seed only")
        _EXCHANGE_FUNDERS.update(_BUILTIN_EXCHANGE_FUNDERS)
        return
    
    try:
        loaded = set()
        with open(path) as f:
            for line in f:
                addr = line.strip().split("#")[0].strip()
                if addr and len(addr) >= 32:
                    loaded.add(addr)
        
        _EXCHANGE_FUNDERS.update(_BUILTIN_EXCHANGE_FUNDERS)
        _EXCHANGE_FUNDERS.update(loaded)
        logger.info(f"Loaded {len(loaded)} exchange funders from {path} "
                   f"(total: {len(_EXCHANGE_FUNDERS)} including builtin)")
    except Exception as exc:
        logger.warning(f"Failed to load exchange funders from {path}: {exc}, using builtin seed only")
        _EXCHANGE_FUNDERS.update(_BUILTIN_EXCHANGE_FUNDERS)


_load_exchange_funders()


async def _resolve_funder_root(
    client,
    address: str,
    depth: int,
    cache: Dict[tuple, Optional[str]],
    visited: Optional[Set[str]] = None,
) -> Optional[str]:
    """
    Resolve the root funder of a wallet by following funding chains up to depth hops.
    
    Stop conditions:
    - Funder is None (no funding source found)
    - Funder is in exchange funders set
    - Funder is in HeliusClient.SYSTEM_ACCOUNTS
    - Funder is in HeliusClient.NON_WALLET_ADDRESSES
    - Cycle detected (same address appears twice in chain)
    
    Args:
        client: HeliusClient instance
        address: Wallet address to resolve
        depth: Maximum hops to follow (usually 2)
        cache: TTL cache keyed by (address, depth) -> root funder
        visited: Set of addresses visited in current chain (for cycle detection)
    
    Returns:
        Root funder address if non-system/non-exchange, None otherwise (singleton)
    """
    if visited is None:
        visited = set()
    
    # Check cache first
    cache_key = (address, depth)
    if cache_key in cache:
        return cache[cache_key]
    
    # Cycle detection
    if address in visited:
        logger.debug(f"Cycle detected at {address[:8]}, returning None")
        cache[cache_key] = None
        return None
    
    visited.add(address)
    
    # Import SYSTEM_ACCOUNTS and NON_WALLET_ADDRESSES
    from .helius_client import HeliusClient
    
    # Check if current address is a stop condition
    if address in _EXCHANGE_FUNDERS:
        logger.debug(f"Exchange funder found: {address[:8]}, returning None")
        result = None
    elif address in HeliusClient.SYSTEM_ACCOUNTS:
        logger.debug(f"System account found: {address[:8]}, returning None")
        result = None
    elif address in HeliusClient.NON_WALLET_ADDRESSES:
        logger.debug(f"Non-wallet address found: {address[:8]}, returning None")
        result = None
    elif depth <= 0:
        # Reached max depth without hitting stop condition, use current address as root
        result = address
    else:
        # Fetch next funder
        funder = await client.get_wallet_funder(address)
        if funder is None:
            # No funder found, this is a root
            result = address
        else:
            # Recurse to resolve the funder's root
            result = await _resolve_funder_root(client, funder, depth - 1, cache, visited)
    
    # Cache and return
    cache[cache_key] = result
    return result


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

    # Get config for multi-hop detection
    try:
        from config import ScoutConfig
        hops = ScoutConfig.get_sybil_hops()
        multihop_max = ScoutConfig.get_sybil_multihop_max()
    except ImportError:
        hops = 2
        multihop_max = 20

    # Sort by WQS descending and identify top-K for multi-hop
    active.sort(key=lambda r: r.wqs_score or 0, reverse=True)
    top_k_active = active[:multihop_max]

    # Fetch funders for all active wallets in batch
    funder_map: Dict[str, Optional[str]] = {}
    root_map: Dict[str, Optional[str]] = {}  # address -> resolved root (for multi-hop)
    
    # Use existing HeliusClient if provided, otherwise create one
    created_client = False
    use_multihop = False
    
    try:
        from .helius_client import HeliusClient
        from .helius_credit_tracker import get_credit_tracker, CreditCost, RequestPriority
        
        if helius_client is None:
            api_key = os.getenv("HELIUS_API_KEY")
            if api_key:
                helius_client = HeliusClient(api_key=api_key)
                created_client = True
        
        if helius_client:
            # Pre-flight budget check for multi-hop
            cost_per_wallet = (CreditCost.SIGNATURES.value + CreditCost.GET_TRANSACTION.value) * hops
            estimated_cost = cost_per_wallet * min(multihop_max, len(active))
            
            tracker = get_credit_tracker()
            can_proceed, reason = tracker.can_make_request(
                estimated_cost,
                category="analysis",
                priority=RequestPriority.MEDIUM,
                expected_value=0.5
            )
            
            if can_proceed:
                use_multihop = True
                logger.info(f"Multi-hop sybil detection: {hops} hops for up to {len(top_k_active)} top-K wallets "
                           f"(estimated cost: {estimated_cost} credits)")
                
                # Phase 1: Fetch hop-1 funders for all wallets in parallel
                all_wallets = active
                coros = [helius_client.get_wallet_funder(r.address) for r in all_wallets]
                results = await asyncio.gather(*coros, return_exceptions=True)
                for record, funder in zip(all_wallets, results):
                    if isinstance(funder, Exception) or funder is None:
                        funder_map[record.address] = None
                    else:
                        funder_map[record.address] = funder
                
                # Phase 2: Resolve roots for top-K wallets using multi-hop
                cache: Dict[tuple, Optional[str]] = {}
                for record in top_k_active:
                    funder = funder_map.get(record.address)
                    if funder:
                        root = await _resolve_funder_root(helius_client, funder, hops, cache)
                        root_map[record.address] = root
                    else:
                        root_map[record.address] = None
            else:
                logger.info(f"Multi-hop sybil detection: budget denied ({reason}), using single-hop for all wallets")
                # Fall back to single-hop for all wallets
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

    # Build clusters: {cluster_key: [WalletRecord, ...]}
    # Cluster key = resolved root (if multi-hop and root exists), otherwise direct funder
    # Wallets without a funder/root are kept as singletons
    clusters: Dict[str, List] = {}
    singleton_count = 0

    for record in active:
        if use_multihop and record.address in root_map:
            root = root_map[record.address]
            if root:
                clusters.setdefault(root, []).append(record)
                setattr(record, 'cluster_id', root)
            else:
                # Exchange/system root or no root, treat as singleton
                clusters.setdefault(f"__singleton_{singleton_count}", []).append(record)
                setattr(record, 'cluster_id', f"__singleton_{singleton_count}")
                singleton_count += 1
        else:
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
        mode_str = "multi-hop" if use_multihop else "single-hop"
        print(f"[Clustering] Removed {removed} correlated wallets "
              f"(same funder/root, {mode_str}), retained {len(deduped)} top-WQS representatives")

    # Update the original records list: demote removed ACTIVE to CANDIDATE
    deduped_addresses = {r.address for r in deduped}
    for record in records:
        if record.status == "ACTIVE" and record.address not in deduped_addresses:
            mode_str = "multi-hop" if use_multihop else "single-hop"
            record.status = "CANDIDATE"
            record.notes = (record.notes or "") + f" | Demoted: cluster dedup (same funder/root as higher-WQS wallet, {mode_str})"

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
