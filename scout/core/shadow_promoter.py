"""Promote/demote wallets on PROVEN shadow copy-profitability (mirror_main).

The shadow trader records a counterfactual mirror_main PnL for every signal a
wallet generates — that is the ground truth for "does copying this wallet make
money". The default promotion path gates on WQS (a proxy), and the data shows
WQS is anti-correlated with actual edge: the biggest shadow winner
(+195 SOL over 70 signals) was stuck in CANDIDATE, never promoted, never copied.

This module corrects the roster directly from the shadow evidence:

  promote : CANDIDATE with samples >= MIN_SAMPLES and total_pnl >= PROMOTE_MIN_PNL
            -> ACTIVE            (captures proven winners the WQS gate missed)
  demote  : ACTIVE   with samples >= MIN_SAMPLES and total_pnl <= DEMOTE_MAX_PNL
            -> CANDIDATE         (prunes confirmed losers still being copied)
  prune   : CANDIDATE with zero shadow signals older than PRUNE_MIN_AGE_DAYS
            -> REJECTED          (shrinks the ~11k candidate pool that burns the
                                   daily Helius quota; opt-in via --prune)

Run from the scout/ directory:
  python -m core.shadow_promoter --dry-run            # show what would change
  python -m core.shadow_promoter                       # apply promote + demote
  python -m core.shadow_promoter --prune --max-prune 2000   # also prune idle
"""

from __future__ import annotations

import argparse
import logging
import sys
from dataclasses import dataclass
from decimal import Decimal
from typing import Optional

from core.db import execute_and_fetchall
from core.roster_writer_db import update_wallet_status

logger = logging.getLogger(__name__)

# --- Thresholds (data-driven, conservative) ---------------------------------
MIN_SAMPLES = 20          # need enough shadow signals to trust the PnL
PROMOTE_MIN_PNL = 2.0     # SOL of proven mirror_main profit before promoting
DEMOTE_MAX_PNL = -1.0     # SOL of proven loss before demoting an ACTIVE wallet
MAX_PROMOTIONS = 25       # safety cap per cycle (don't flood ACTIVE at once)
MAX_DEMOTIONS = 50        # safety cap per cycle
PRUNE_MIN_AGE_DAYS = 14   # only prune candidates idle this long


@dataclass(frozen=True)
class WalletPerf:
    address: str
    status: Optional[str]
    samples: int
    total_pnl: Decimal
    avg_pnl: Decimal
    win_rate: float


def fetch_shadow_performance() -> list[WalletPerf]:
    """Per-wallet mirror_main shadow PnL joined to current roster status."""
    rows = execute_and_fetchall(
        """
        SELECT sp.wallet_address, w.status,
               COUNT(*)                                   AS samples,
               SUM(se.pnl_sol)                            AS total_pnl,
               AVG(se.pnl_sol)                            AS avg_pnl,
               COUNT(*) FILTER (WHERE se.pnl_sol > 0)::FLOAT / NULLIF(COUNT(*), 0) AS win_rate
        FROM shadow_positions sp
        JOIN shadow_exits se ON se.shadow_id = sp.shadow_id
        LEFT JOIN wallets w ON w.address = sp.wallet_address
        WHERE se.exit_strategy = 'mirror_main' AND se.pnl_sol IS NOT NULL
        GROUP BY sp.wallet_address, w.status
        """,
    )
    out: list[WalletPerf] = []
    for r in rows:
        if isinstance(r, dict):
            addr, status, samples = r["wallet_address"], r.get("status"), r["samples"]
            total, avg = r["total_pnl"], r["avg_pnl"]
            win = r["win_rate"]
        else:  # tuple
            addr, status, samples, total, avg, win = r
        out.append(
            WalletPerf(
                address=addr,
                status=status,
                samples=int(samples),
                total_pnl=Decimal(total),
                avg_pnl=Decimal(avg),
                win_rate=float(win) if win is not None else 0.0,
            )
        )
    return out


def select_promotions(
    perf: list[WalletPerf],
    min_samples: int = MIN_SAMPLES,
    min_pnl: float = PROMOTE_MIN_PNL,
    max_promotions: int = MAX_PROMOTIONS,
) -> list[WalletPerf]:
    """CANDIDATE wallets with proven profit to promote to ACTIVE.

    Gates on total PnL (expected value), not win rate — the biggest edges are
    high-variance moonshot wallets (e.g. 8% win, +278% avg) that a win-rate gate
    would wrongly reject.
    """
    candidates = [
        p
        for p in perf
        if p.status == "CANDIDATE"
        and p.samples >= min_samples
        and p.total_pnl >= Decimal(str(min_pnl))
    ]
    candidates.sort(key=lambda p: p.total_pnl, reverse=True)
    return candidates[:max_promotions]


def select_demotions(
    perf: list[WalletPerf],
    min_samples: int = MIN_SAMPLES,
    max_pnl: float = DEMOTE_MAX_PNL,
    max_demotions: int = MAX_DEMOTIONS,
) -> list[WalletPerf]:
    """ACTIVE wallets with proven losses to demote to CANDIDATE."""
    candidates = [
        p
        for p in perf
        if p.status == "ACTIVE"
        and p.samples >= min_samples
        and p.total_pnl <= Decimal(str(max_pnl))
    ]
    candidates.sort(key=lambda p: p.total_pnl)  # worst first
    return candidates[:max_demotions]


def fetch_idle_candidates(min_age_days: int = PRUNE_MIN_AGE_DAYS) -> list[str]:
    """CANDIDATE wallets with zero shadow signals, older than min_age_days.

    These consume Helius quota (monitored/polled) without ever producing a
    copyable signal. Pruning them is the main lever for cutting daily quota burn.
    """
    rows = execute_and_fetchall(
        """
        SELECT w.address
        FROM wallets w
        WHERE w.status = 'CANDIDATE'
          AND w.promoted_at < NOW() - (%s || ' days')::INTERVAL
          AND NOT EXISTS (
              SELECT 1 FROM shadow_positions sp WHERE sp.wallet_address = w.address
          )
        ORDER BY w.promoted_at ASC
        """,
        (str(min_age_days),),
    )
    return [r["address"] if isinstance(r, dict) else r[0] for r in rows]


def run_cycle(
    dry_run: bool = False,
    prune: bool = False,
    max_prune: int = 2000,
) -> dict:
    """Execute one promotion/demotion (and optional prune) cycle.

    Returns a summary dict of actions taken/planned.
    """
    perf = fetch_shadow_performance()
    promotions = select_promotions(perf)
    demotions = select_demotions(perf)

    summary: dict = {
        "promote": [p.address for p in promotions],
        "demote": [p.address for p in demotions],
        "prune": [],
        "dry_run": dry_run,
    }

    logger.info(
        "shadow promotion cycle: %d promote, %d demote (dry_run=%s)",
        len(promotions), len(demotions), dry_run,
    )
    for p in promotions:
        logger.info(
            "PROMOTE %s  samples=%d total_pnl=%.3f win=%.0f%%",
            p.address, p.samples, float(p.total_pnl), p.win_rate * 100,
        )
    for p in demotions:
        logger.info(
            "DEMOTE  %s  samples=%d total_pnl=%.3f win=%.0f%%",
            p.address, p.samples, float(p.total_pnl), p.win_rate * 100,
        )

    if not dry_run:
        for p in promotions:
            update_wallet_status(p.address, "ACTIVE")
        for p in demotions:
            update_wallet_status(p.address, "CANDIDATE")

    if prune:
        idle = fetch_idle_candidates()[:max_prune]
        summary["prune"] = idle
        logger.info("prune: %d idle CANDIDATE wallets (cap %d, dry_run=%s)",
                    len(idle), max_prune, dry_run)
        if not dry_run:
            for addr in idle:
                update_wallet_status(addr, "REJECTED")

    return summary


def main(argv: list[str]) -> int:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    )
    parser = argparse.ArgumentParser(description="Shadow-profitability roster promoter")
    parser.add_argument("--dry-run", action="store_true", help="show actions without applying")
    parser.add_argument("--prune", action="store_true", help="also reject idle CANDIDATE wallets")
    parser.add_argument("--max-prune", type=int, default=2000, help="cap on pruned wallets")
    args = parser.parse_args(argv)

    summary = run_cycle(dry_run=args.dry_run, prune=args.prune, max_prune=args.max_prune)
    print(f"promote: {len(summary['promote'])}  demote: {len(summary['demote'])}  "
          f"prune: {len(summary['prune'])}  dry_run: {summary['dry_run']}")
    for addr in summary["promote"]:
        print(f"  PROMOTE -> ACTIVE   {addr}")
    for addr in summary["demote"]:
        print(f"  DEMOTE  -> CANDIDATE {addr}")
    if args.prune:
        print(f"  (prune list: {len(summary['prune'])} wallets)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
