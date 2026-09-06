"""Gate-1 retest forward-capture: record cluster triggers + entry prices.

Runs on a cron (every 15 min on the operator host):
    docker compose --profile mainnet-prod exec -T scout python -m scripts.cluster_trigger_capture

For each dispersion-valid cluster trigger (>= 3 distinct tracked wallets,
BUY, trailing 12h — the held plan's rule) not yet recorded, inserts a
cluster_triggers row. Idempotent via UNIQUE (token_address, trigger_at).
Entry-price backfill is done by the price-path reconstruction (GeckoTerminal
OHLCV) on the next run; no Dune/Helius heavy calls here.

FEASIBILITY measurement only — triggers recorded here are NOT traded. The
Gate-1 retest verdict consumes (cluster_triggers JOIN forward prices) after
the 14-day window (2026-09-06 → 2026-09-20).
"""

from __future__ import annotations

import itertools
import os
import sys

import psycopg

DSN = os.environ.get("DATABASE_URL") or os.environ.get("CHIMERA_DB_URL")
MIN_GAP_S = 5
MIN_SPAN_S = 180
MIN_WALLETS = 3

TRIGGERS_SQL = """
SELECT s.token_address, s.block_time, s.wallet_address, s.tx_signature
FROM smart_money_signals s
WHERE s.side = 'BUY'
  AND s.block_time > NOW() - INTERVAL '25 hours'
ORDER BY s.token_address, s.block_time
"""


def dispersion_ok(secs: list[int]) -> bool:
    return any(
        c - a >= MIN_SPAN_S and b - a >= MIN_GAP_S and c - b >= MIN_GAP_S
        for a, b, c in itertools.combinations(secs, 3)
    )


def main() -> int:
    if not DSN:
        print("DATABASE_URL/CHIMERA_DB_URL not set", file=sys.stderr)
        return 2
    conn = psycopg.connect(DSN)
    cur = conn.cursor()

    cur.execute(TRIGGERS_SQL)
    events: dict[str, list[tuple[int, str, str]]] = {}
    for token, bt, wallet, sig in cur.fetchall():
        events.setdefault(token, []).append((int(bt.timestamp()), wallet, sig))

    inserted = 0
    for token, arrivals in events.items():
        arrivals.sort()
        # first valid trigger only (matching the probe's dedupe)
        for ts, _w, _s in arrivals:
            window = [(t, w, s) for t, w, s in arrivals
                      if ts - 12 * 3600 <= t <= ts]
            wallets = {w for _, w, _ in window}
            sigs = sorted({s for _, _, s in window})
            if len(wallets) >= MIN_WALLETS and dispersion_ok(
                [t for t, _, _ in window]
            ):
                import datetime as dt

                trigger_at = dt.datetime.fromtimestamp(ts, dt.timezone.utc)
                cur.execute(
                    """
                    INSERT INTO cluster_triggers
                        (token_address, trigger_at, wallet_count, arrival_signatures)
                    VALUES (%s, %s, %s, %s)
                    ON CONFLICT (token_address, trigger_at) DO NOTHING
                    """,
                    (token, trigger_at, len(wallets), sigs),
                )
                inserted += cur.rowcount
                break

    conn.commit()

    # entry-price backfill for triggers missing one: GeckoTerminal hourly close
    cur.execute(
        "SELECT id, token_address, trigger_at FROM cluster_triggers "
        "WHERE entry_price_sol IS NULL AND trigger_at > NOW() - INTERVAL '25 hours'"
    )
    priced = 0
    pending = cur.fetchall()
    if pending:
        asyncio_run_backfill(cur, conn, pending)
        priced = len(pending)

    cur.execute("SELECT COUNT(*) FROM cluster_triggers")
    total = cur.fetchone()[0]
    print(f"cluster_triggers: total={total} inserted_now={inserted} "
          f"price_backfill_attempted={priced}")
    conn.close()
    return 0


def asyncio_run_backfill(cur, conn, pending) -> None:
    import asyncio
    import datetime as dt  # noqa: F401

    import aiohttp

    sys.path.insert(0, "/app")
    os.chdir("/app")
    from core.price_path import geckoterminal_ohlcv

    async def run():
        async with aiohttp.ClientSession() as session:
            for tid, token, trigger_at in pending:
                ts = int(trigger_at.timestamp())
                try:
                    candles = await geckoterminal_ohlcv(token, timeframe="hour")
                except Exception as e:  # noqa: BLE001
                    print(f"ohlcv failed {token[:8]}: {e}")
                    continue
                after = [p for t, p in candles if t >= ts]
                if after:
                    cur.execute(
                        "UPDATE cluster_triggers SET entry_price_sol = %s "
                        "WHERE id = %s",
                        (str(after[0]), tid),
                    )
                conn.commit()

    asyncio.run(run())


if __name__ == "__main__":
    sys.exit(main())
