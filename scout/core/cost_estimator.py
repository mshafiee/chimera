"""
Dynamic execution cost estimation for backtesting.

Replaces static priority_fee_sol_per_trade / jito_tip_sol_per_trade with
percentile-based fees queried from Helius getPriorityFeeEstimate so that
the backtester's cost model matches current market conditions.
"""

from __future__ import annotations

import logging
import os
import time
from decimal import Decimal
from typing import Dict, List, Optional, Tuple

import aiohttp

logger = logging.getLogger(__name__)

# Default fallback values (SOL)
DEFAULT_PRIORITY_FEE_SOL = Decimal("0.00005")
DEFAULT_JITO_TIP_SOL = Decimal("0.0001")

# Percentile to use per strategy
SHIELD_FEE_PERCENTILE = 75
SPEAR_FEE_PERCENTILE = 90

# Cache TTL in seconds (default 300 = 5 min, covers a full Scout run)
CACHE_TTL_SECONDS = float(os.getenv("SCOUT_FEE_CACHE_TTL_SECONDS", "300"))


class CostEstimator:
    """
    Estimates current priority fees and Jito tips for execution cost modeling.

    Queries Helius ``getPriorityFeeEstimate`` at configurable percentiles and
    caches results briefly so that a single Scout run reuses one snapshot.
    """

    def __init__(self, helius_api_key: Optional[str] = None):
        self._api_key = helius_api_key or os.getenv("HELIUS_API_KEY")
        self._rpc_url = self._build_rpc_url()
        self._cache: Dict[str, Tuple[float, Tuple[float, ...]]] = {}  # (ts, cached_fee_levels_tuple)
        self._session: Optional[aiohttp.ClientSession] = None

    def _build_rpc_url(self) -> Optional[str]:
        """Derive the Helius RPC URL from available configuration."""
        url = os.getenv("CHIMERA_RPC__PRIMARY_URL") or os.getenv("SOLANA_RPC_URL", "")
        if url:
            return url
        if self._api_key:
            return "https://mainnet.helius-rpc.com/"
        return None

    def _get_rpc_params(self) -> Dict[str, str]:
        """Return RPC URL query params (api-key as separate param, not in URL)."""
        if self._api_key:
            return {"api-key": self._api_key}
        return {}

    async def _get_session(self) -> aiohttp.ClientSession:
        if self._session is None:
            timeout = aiohttp.ClientTimeout(total=10, connect=5)
            self._session = aiohttp.ClientSession(timeout=timeout)
        return self._session

    async def close(self):
        if self._session:
            await self._session.close()
            self._session = None

    async def get_priority_fee_estimate(
        self,
        percentile: Optional[int] = None,
        strategy: str = "SHIELD",
    ) -> Decimal:
        """
        Return the estimated priority fee (SOL) per trade.

        Caches the raw fee table for ~10 seconds; individual percentiles are
        computed from the cached table.

        Args:
            percentile: Which percentile to query (overrides strategy default).
            strategy: SHIELD (p75) or SPEAR (p90).
        """
        if percentile is None:
            percentile = SHIELD_FEE_PERCENTILE if strategy.upper() == "SHIELD" else SPEAR_FEE_PERCENTILE

        raw = await self._fetch_raw_fee_estimates()
        if raw is None:
            return DEFAULT_PRIORITY_FEE_SOL

        return self._percentile(raw, float(percentile)) if raw else DEFAULT_PRIORITY_FEE_SOL

    async def get_jito_tip_estimate(self, strategy: str = "SHIELD") -> Decimal:
        """
        Return the estimated Jito tip (SOL).

        Currently based on priority fee at SPEAR percentile + 20% markup
        to model the competitive tip environment.
        """
        fee_at_p90 = await self.get_priority_fee_estimate(percentile=SPEAR_FEE_PERCENTILE)
        markup = Decimal("1.2") if strategy.upper() == "SPEAR" else Decimal("1.0")
        return max(DEFAULT_JITO_TIP_SOL, fee_at_p90 * markup)

    async def get_all_estimates(self, strategy: str = "SHIELD") -> Tuple[Decimal, Decimal]:
        """Return (priority_fee_sol, jito_tip_sol) for the given strategy."""
        prio = await self.get_priority_fee_estimate(strategy=strategy)
        jito = await self.get_jito_tip_estimate(strategy=strategy)
        return prio, jito

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    async def _fetch_raw_fee_estimates(self) -> Optional[List[float]]:
        """Fetch raw priority fee levels (SOL) from Helius RPC."""
        if not self._rpc_url:
            return None

        cache_key = "raw_fees"
        now = time.monotonic()
        if cache_key in self._cache:
            cached_ts, cached_value = self._cache[cache_key]
            if now - cached_ts < CACHE_TTL_SECONDS:
                # Return the cached fee levels list, not the single Decimal.
                # The cached_value is the full sorted list of fees as a tuple.
                if isinstance(cached_value, (list, tuple)):
                    return list(cached_value)
                return None

        if not self._api_key:
            return None

        try:
            session = await self._get_session()
            payload = {
                "jsonrpc": "2.0",
                "id": "scout-fee-estimate",
                "method": "getPriorityFeeEstimate",
                "params": [
                    {
                        "accountKeys": [],
                    },
                    {"percentile": 0},
                ],
            }
            async with session.post(self._rpc_url, json=payload, params=self._get_rpc_params()) as resp:
                if resp.status != 200:
                    logger.warning("getPriorityFeeEstimate HTTP %d", resp.status)
                    return None
                data = await resp.json()
                levels = self._parse_fee_response(data)
                if levels is not None:
                    # Cache the full sorted list so percentiles can be computed from cache
                    self._cache[cache_key] = (now, tuple(levels))
                return levels
        except Exception as exc:
            logger.warning("getPriorityFeeEstimate failed: %s", self._redact(str(exc)))
            return None

    @staticmethod
    def _redact(text: str) -> str:
        """Remove API keys from error messages before logging."""
        import re
        return re.sub(r'api-key=[^&\s"]+', 'api-key=REDACTED', text)

    @staticmethod
    def _parse_fee_response(data: dict) -> Optional[List[float]]:
        """Extract priority fee levels from the Helius RPC response."""
        result = data.get("result")
        if not result:
            return None
        if isinstance(result, dict):
            # Newer format: {"percentiles": {pct: fee, ...}} or {"priorityFeeLevels": {...}}
            levels = result.get("priorityFeeLevels") or result.get("percentiles")
            if levels:
                fees = [float(v) for v in levels.values() if v is not None]
                return sorted(fees) if fees else None
            # Single estimate: {"priorityFeeEstimate": 1234}
            est = result.get("priorityFeeEstimate")
            if est is not None:
                return [float(est)]
            return None
        if isinstance(result, (int, float)):
            return [float(result)]
        return None

    @staticmethod
    def _percentile(values: List[float], p: float) -> Decimal:
        """Compute the p-th percentile from a sorted list of values."""
        if not values:
            return DEFAULT_PRIORITY_FEE_SOL
        xs = sorted(values)
        if len(xs) == 1:
            return Decimal(str(xs[0]))
        k = (len(xs) - 1) * (p / 100.0)
        f = int(k)
        c = min(f + 1, len(xs) - 1)
        if f == c:
            return Decimal(str(xs[f]))
        d = k - f
        return Decimal(str(xs[f] * (1.0 - d) + xs[c] * d))
