"""
Hydra Phase 2.1: In-memory co-funding graph (FundingOriginTracker).

Traces each wallet's native-SOL funding source up to 2 hops via
HeliusClient.get_wallet_funder, then builds a `funder -> [wallets]` inverted
index so the detector can find cabals sharing a root dispenser / CEX
sub-account within a tight window.

Reuses clustering._resolve_funder_root stop-lists; adds TTL edge cache +
budget cap so roster-scale scans don't bleed Helius credits.
"""

from __future__ import annotations

import logging
import time
from collections import defaultdict
from dataclasses import dataclass, field
from decimal import Decimal
from typing import Dict, List, Optional, Set

logger = logging.getLogger(__name__)

FRESH_WALLET_MAX_PRIOR_TXS = 5
FUNDING_WINDOW_CEX_HOURS = 4.0
FUNDING_WINDOW_DISPENSER_HOURS = 24.0


@dataclass
class FundingEdge:
    wallet: str
    funder: Optional[str]
    root: Optional[str]
    hops: int
    funded_at_ts: Optional[float] = None
    amount_sol: Optional[Decimal] = None
    is_fresh: bool = False


@dataclass
class FundingOriginTracker:
    """BFS 2-hop funder resolver with TTL cache and inverted index."""

    max_hops: int = 2
    ttl_secs: float = 3600.0
    max_wallets_per_run: int = 200

    _edges: Dict[str, FundingEdge] = field(default_factory=dict)
    _cached_at: Dict[str, float] = field(default_factory=dict)
    _by_funder: Dict[str, Set[str]] = field(
        default_factory=lambda: defaultdict(set)
    )

    def _is_fresh_cache(self, wallet: str) -> bool:
        ts = self._cached_at.get(wallet)
        return ts is not None and (time.time() - ts) < self.ttl_secs

    async def resolve_wallet(
        self, client, wallet: str, depth: Optional[int] = None
    ) -> FundingEdge:
        """Resolve one wallet's funding root (cached)."""
        if self._is_fresh_cache(wallet) and wallet in self._edges:
            return self._edges[wallet]

        hops = depth if depth is not None else self.max_hops
        cache: Dict[tuple, Optional[str]] = {}
        funder: Optional[str] = None
        root: Optional[str] = None
        try:
            # Local import to avoid cycle at module load.
            from scout.core.clustering import _resolve_funder_root  # type: ignore
            from core.clustering import _resolve_funder_root as _alt  # type: ignore
        except Exception:
            _resolve_funder_root = None  # type: ignore
            _alt = None  # type: ignore

        resolver = None
        try:
            from core.clustering import _resolve_funder_root as r  # noqa

            resolver = r
        except Exception:
            try:
                from scout.core.clustering import _resolve_funder_root as r2  # noqa

                resolver = r2
            except Exception:
                resolver = None

        if resolver is not None:
            try:
                root = await resolver(client, wallet, hops, cache)
            except Exception as exc:
                logger.warning("funder resolve failed for %s: %s", wallet, exc)
        if funder is None and root is None:
            try:
                funder = await client.get_wallet_funder(wallet)
                root = funder
            except Exception as exc:
                logger.warning("get_wallet_funder failed for %s: %s", wallet, exc)

        # Fresh-wallet heuristic: <5 lifetime txs prior to funding.
        is_fresh = False
        try:
            sigs = await client.get_signatures_for_address(wallet, limit=10)
            if isinstance(sigs, list) and len(sigs) < FRESH_WALLET_MAX_PRIOR_TXS + 1:
                is_fresh = True
        except Exception:
            pass

        edge = FundingEdge(
            wallet=wallet, funder=funder, root=root, hops=hops, is_fresh=is_fresh
        )
        self._edges[wallet] = edge
        self._cached_at[wallet] = time.time()
        if root:
            self._by_funder[root].add(wallet)
        elif funder:
            self._by_funder[funder].add(wallet)
        return edge

    async def build_graph(self, client, wallets: List[str]) -> Dict[str, FundingEdge]:
        """Resolve up to max_wallets_per_run wallets (credit-bounded)."""
        cohort = wallets[: self.max_wallets_per_run]
        for w in cohort:
            try:
                await self.resolve_wallet(client, w)
            except Exception as exc:
                logger.warning("build_graph skip %s: %s", w, exc)
        return dict(self._edges)

    def cluster_candidates(self, min_shared: int = 3) -> List[Set[str]]:
        """Return wallet sets sharing a funding root (size >= min_shared)."""
        return [set(v) for v in self._by_funder.values() if len(v) >= min_shared]

    def clear(self) -> None:
        self._edges.clear()
        self._cached_at.clear()
        self._by_funder.clear()
