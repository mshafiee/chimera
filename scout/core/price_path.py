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


# ── GeckoTerminal OHLCV (public, no key) ────────────────────────────────────
# On the current Helius plan the enriched swap-enabling endpoints are not
# available (address-activity feed returns no tokenTransfers/events.swap; the
# `/v0/transactions` batch-parse returns "Method not found"), so the on-chain
# swap path above is best-effort only. GeckoTerminal's public OHLCV API is the
# available price-path source for tokens with a live pool. The close is in the
# pool quote currency (USDC/USDT for stablecoin pairs), consistent with the
# shadow `entry_price_usd` the replay compares against.

GECKO_BASE = "https://api.geckoterminal.com/api/v2"


def parse_ohlcv_close(ohlcv_list: Sequence[Sequence]) -> List[Tuple[int, Decimal]]:
    """Parse GeckoTerminal `ohlcv_list` rows into sorted (ts_unix, close).

    Each row is ``[start_ts, open, high, low, close, volume]`` (start_ts in
    unix seconds). Rows with a non-positive or missing close are dropped."""
    out: List[Tuple[int, Decimal]] = []
    for row in ohlcv_list or []:
        try:
            ts = int(row[0])
            close = Decimal(str(row[4]))
        except (IndexError, TypeError, ValueError):
            continue
        if close > 0:
            out.append((ts, close))
    return _finalize(out)


async def _gecko_get(session, url: str) -> Tuple[int, dict]:
    """GET a GeckoTerminal URL with a 429 backoff/retry (public API can rate
    limit a long reconstruction run). Returns (status, json_dict)."""
    import asyncio
    import aiohttp

    for attempt in range(3):
        async with session.get(url, timeout=aiohttp.ClientTimeout(total=20)) as r:
            if r.status == 429 and attempt < 2:
                await asyncio.sleep(0.8 * (attempt + 1))
                continue
            try:
                return r.status, await r.json()
            except Exception:
                return r.status, {}
    return 429, {}


async def geckoterminal_ohlcv(token_address: str, timeframe: str = "hour") -> List[Tuple[int, Decimal]]:
    """Return a token's hourly OHLCV close series via GeckoTerminal, or [] if
    the token has no live pool."""
    import aiohttp

    out: List[Tuple[int, Decimal]] = []
    try:
        async with aiohttp.ClientSession() as s:
            status, js = await _gecko_get(
                s, f"{GECKO_BASE}/networks/solana/tokens/{token_address}/pools"
            )
            if status != 200:
                return out
            pools = js.get("data") or []
            if not pools:
                return out
            pool_addr = str(pools[0]["id"]).replace("solana_", "")
            for tf in (timeframe, "hour", "day", "minute"):
                status, js = await _gecko_get(
                    s, f"{GECKO_BASE}/networks/solana/pools/{pool_addr}/ohlcv/{tf}"
                )
                if status != 200:
                    continue
                data = js.get("data")
                items = _as_resource_list(data)
                for item in items:
                    attrs = item.get("attributes") or {}
                    out.extend(parse_ohlcv_close(attrs.get("ohlcv_list") or []))
                if out:
                    break
    except Exception as e:  # noqa: BLE001 - provider errors are recoverable
        print(f"ERROR: geckoterminal_ohlcv {token_address}: {e}")
        return out
    return _finalize(out)


# ── Birdeye OHLCV (keyed, higher rate limit) ────────────────────────────────
# GeckoTerminal's public API is unkeyed and rate-limits a full reconstruction
# run to a crawl (and cannot serve dead/delisted mints). Birdeye's keyed
# `/defi/ohlcv` returns a full hourly candle series in one request, so a
# cohort of N tokens costs ~N requests — feasible within a 60 rpm plan.

async def birdeye_ohlcv(
    token_address: str, time_from: int, time_to: int
) -> List[Tuple[int, Decimal]]:
    """Return a token's hourly close series via Birdeye's keyed OHLCV API.

    One request per token (rate-limited to ~60 rpm by the BIRDEYE_API_KEY
    plan via `BirdeyeClient`). Returns [] if the token is absent from Birdeye
    or the request fails, so the caller can retry on a later batch."""
    from core.birdeye_client import BirdeyeClient

    client = BirdeyeClient()
    try:
        series = await client.get_ohlcv_series(token_address, time_from, time_to)
    except Exception as e:  # noqa: BLE001 - provider errors are recoverable
        print(f"ERROR: birdeye_ohlcv {token_address}: {e}")
        return []
    finally:
        try:
            await client.close()
        except Exception:
            pass
    return _finalize(series)


def _as_resource_list(data) -> List[Dict]:
    """Normalize GeckoTerminal `data` (a resource list, a single resource dict,
    or an error/empty value) into a list of resource dicts."""
    if isinstance(data, list):
        return [d for d in data if isinstance(d, dict)]
    if isinstance(data, dict):
        return [data]
    return []
