"""
Hydra Phase 2.2: Temporal buying-cluster trigger (the "Golden Gate").

Rule: >=3 clustered wallets (shared funding root OR caller-supplied cluster
set) buying the same mint within <=120s with collective volume >= $7,500
(or >=40 SOL) emits a ClusterAdmissionPayload for `cluster_signals`.

Pure function `detect_clusters` is unit-tested; DB writer is thin psycopg3.
"""

from __future__ import annotations

import logging
import uuid
from dataclasses import dataclass
from decimal import Decimal
from typing import Dict, List, Optional, Sequence, Set

logger = logging.getLogger(__name__)

CLUSTER_MIN_WALLETS = 3
CLUSTER_WINDOW_SECS = 120.0
CLUSTER_MIN_USD = Decimal("7500")
CLUSTER_MIN_SOL = Decimal("40")


@dataclass(frozen=True)
class ClusterBuy:
    wallet: str
    mint: str
    ts: float  # epoch seconds
    amount_sol: Decimal


@dataclass(frozen=True)
class ClusterAdmissionPayload:
    signal_id: str
    token_mint: str
    cluster_id: str
    wallet_count: int
    total_cluster_sol: Decimal


def detect_clusters(
    buys: Sequence[ClusterBuy],
    wallet_clusters: Optional[Sequence[Set[str]]] = None,
    sol_price_usd: Optional[Decimal] = None,
) -> List[ClusterAdmissionPayload]:
    """Scan buys grouped by mint for Hydra-qualifying clusters."""
    by_mint: Dict[str, List[ClusterBuy]] = {}
    for b in buys:
        by_mint.setdefault(b.mint, []).append(b)

    # Optional cluster-membership lookup: wallet -> cluster idx.
    member_of: Dict[str, int] = {}
    if wallet_clusters:
        for i, s in enumerate(wallet_clusters):
            for w in s:
                member_of[w] = i

    out: List[ClusterAdmissionPayload] = []
    for mint, legs in by_mint.items():
        legs = sorted(legs, key=lambda b: b.ts)
        n = len(legs)
        # Sliding window: maximal wallet-distinct sets within 120s.
        for i in range(n):
            seen: Dict[str, ClusterBuy] = {}
            for j in range(i, n):
                if legs[j].ts - legs[i].ts > CLUSTER_WINDOW_SECS:
                    break
                seen.setdefault(legs[j].wallet, legs[j])
                if len(seen) < CLUSTER_MIN_WALLETS:
                    continue
                wallets = set(seen.keys())
                # If cluster sets supplied, require >=3 from ONE funding root.
                if member_of:
                    roots: Dict[int, int] = {}
                    for w in wallets:
                        if w in member_of:
                            roots[member_of[w]] = roots.get(member_of[w], 0) + 1
                    if not any(c >= CLUSTER_MIN_WALLETS for c in roots.values()):
                        continue
                total_sol = sum((b.amount_sol for b in seen.values()), Decimal("0"))
                total_usd = (
                    total_sol * sol_price_usd if sol_price_usd is not None else None
                )
                if total_sol >= CLUSTER_MIN_SOL or (
                    total_usd is not None and total_usd >= CLUSTER_MIN_USD
                ):
                    out.append(
                        ClusterAdmissionPayload(
                            signal_id=str(uuid.uuid4()),
                            token_mint=mint,
                            cluster_id=str(uuid.uuid4()),
                            wallet_count=len(wallets),
                            total_cluster_sol=total_sol,
                        )
                    )
                    break  # one signal per start index; dedup by (mint, window)
    return out


def insert_cluster_signal(conn, payload: ClusterAdmissionPayload) -> None:
    """Insert one payload into cluster_signals (idempotent on cluster_id)."""
    with conn.cursor() as cur:
        cur.execute(
            """
            INSERT INTO cluster_signals
                (signal_id, token_mint, cluster_id, wallet_count,
                 total_cluster_sol, status)
            VALUES (%s, %s, %s, %s, %s, 'PENDING')
            ON CONFLICT (cluster_id) DO NOTHING
            """,
            (
                str(payload.signal_id),
                payload.token_mint,
                str(payload.cluster_id),
                payload.wallet_count,
                str(payload.total_cluster_sol),
            ),
        )
