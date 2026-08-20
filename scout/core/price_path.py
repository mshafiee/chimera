"""
Price-path reconstruction for the copy-engine replay harness (Phase 2A).

The realize-vs-price gap cannot be tuned without a per-position price series,
and no price path is persisted in-system. This module reconstructs one from
on-chain data: for a token mint, paginate its Helius Enhanced Transactions and,
for each swap that moves the token directly against SOL/WSOL, derive the
payable SOL per token. A swap gives the ground-truth exchange rate, and works
even for dead/delisted pump.fun mints because the data lives in tx history.

Reconstructed points are stored as ``(ts_unix, payable_sol_per_token)``. At
replay time the exit rails (stop/recovery/trailing/time) only need the *ratio*
of current to entry, so SOL-per-token (no USD) is the self-consistent unit.

All money math stays in :class:`decimal.Decimal`.
"""

from __future__ import annotations

from decimal import Decimal
from typing import Dict, List, Optional, Sequence, Tuple

WSOL_MINT = "So11111111111111111111111111111111111111111"
_PAGE = 100


def ui_amount(transfer: Dict) -> Decimal:
    """UI token amount from a Helius tokenTransfer object (parity with the
    operator's parser). Returns 0 on parse failure so a malformed transfer
    doesn't abort the batch."""
    raw = transfer.get("rawTokenAmount")
    if isinstance(raw, dict):
        raw_amt = raw.get("tokenAmount")
        dec = int(raw.get("decimals", 0))
        if raw_amt is not None:
            try:
                return Decimal(str(raw_amt)).scaleb(-dec)
            except Exception:
                pass
    ta = transfer.get("tokenAmount")
    if isinstance(ta, dict):
        for key in ("uiAmount", "uiAmountString"):
            if ta.get(key) is not None:
                try:
                    return Decimal(str(ta[key]))
                except Exception:
                    pass
        if ta.get("amount") is not None:
            try:
                return Decimal(str(ta["amount"])).scaleb(-int(ta.get("decimals", 0)))
            except Exception:
                pass
    if ta is not None:
        try:
            return Decimal(str(ta))
        except Exception:
            pass
    return Decimal("0")


def swap_price_from_transfers(
    transfers: Sequence[Dict],
    token_mint: str,
) -> Optional[Decimal]:
    """Payable SOL per token from a tx's tokenTransfers, or None if not a
    direct SOL<->token swap (e.g. a Jupiter multi-hop with no SOL leg).

    A swap is described by two tokenTransfer entries: one moving WSOL and one
    moving the token mint (regardless of direction relative to the pool).
    Total payable = total WSOL moved / total token moved; summing both legs is
    direction-agnostic and robust.
    """
    sol = Decimal("0")
    token = Decimal("0")
    for tr in transfers:
        mint = str(tr.get("mint", ""))
        amt = ui_amount(tr)
        if amt <= 0:
            continue
        if mint == WSOL_MINT:
            sol += amt
        elif mint == token_mint:
            token += amt
    if sol > 0 and token > 0:
        return sol / token
    return None


def _finalize(points: List[Tuple[int, Decimal]]) -> List[Tuple[int, Decimal]]:
    # sort ascending by ts and drop any duplicate timestamps (keep first)
    points.sort(key=lambda p: p[0])
    out: List[Tuple[int, Decimal]] = []
    seen = set()
    for ts, price in points:
        if ts in seen:
            continue
        seen.add(ts)
        out.append((ts, price))
    return out


async def reconstruct_price_path(
    helius,
    token_mint: str,
    time_from: int,
    time_to: int,
) -> List[Tuple[int, Decimal]]:
    """Paginate the token's Helius transactions and return sorted
    ``(ts_unix, payable_sol_per_token)`` points within [time_from, time_to]."""
    points: List[Tuple[int, Decimal]] = []
    before: Optional[str] = None
    for _ in range(200):
        params: Dict = {
            "api-key": helius.api_key,
            "limit": _PAGE,
            "order": "desc",
        }
        if before:
            params["before"] = before
        data = await helius._make_request(
            f"/addresses/{token_mint}/transactions",
            params,
            use_retry=True,
        )
        if not data or not isinstance(data, list) or not data:
            break
        saw_in_window = False
        for tx in data:
            ts = tx.get("timestamp")
            if ts is None:
                continue
            if ts > time_to:
                continue  # too recent for this position's window
            saw_in_window = True
            if ts < time_from:
                # descending order: past the window, stop.
                return _finalize(points)
            price = swap_price_from_transfers(tx.get("tokenTransfers", []) or [], token_mint)
            if price and price > 0:
                points.append((int(ts), price))
        if not saw_in_window:
            # all remaining pages are newer than time_to — nothing in window.
            return _finalize(points)
        before = data[-1].get("signature")
        if not before:
            break
    return _finalize(points)
