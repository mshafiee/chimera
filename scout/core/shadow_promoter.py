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
from core.copy_backtest import observed_cost_per_sol

logger = logging.getLogger(__name__)

# --- Thresholds (data-driven, conservative) ---------------------------------
MIN_SAMPLES = 20          # need enough shadow signals to trust the PnL
PROMOTE_MIN_PNL = 2.0     # legacy SOL gross floor (used when cost basis unknown)
DEMOTE_MAX_PNL = -1.0     # SOL of proven loss before demoting an ACTIVE wallet
MAX_PROMOTIONS = 25       # safety cap per cycle (don't flood ACTIVE at once)
MAX_DEMOTIONS = 50        # safety cap per cycle
PRUNE_MIN_AGE_DAYS = 14   # only prune candidates idle this long
# Post-cost gate (Phase 2G): recon showed the ~1.4% round-trip cost floor eats
# thin gross edges, so promotion now requires NET expectancy that clears the
# floor with margin (net_pct = net pnl / notional * 100).
PROMOTE_MIN_NET_PCT = 1.5


@dataclass(frozen=True)
class WalletPerf:
    address: str
    status: Optional[str]
    samples: int
    total_pnl: Decimal
    avg_pnl: Decimal
    win_rate: float
    notional: Decimal = Decimal("0")
    max_win: Optional[Decimal] = None


def fetch_shadow_performance() -> list[WalletPerf]:
    """Per-wallet mirror_main shadow PnL joined to current roster status.

    Also carries notional (for post-cost net expectancy) and max_win (for the
    not-one-lucky-trade guard)."""
    rows = execute_and_fetchall(
        """
        SELECT sp.wallet_address, w.status,
               COUNT(*)                                   AS samples,
               SUM(se.pnl_sol)                            AS total_pnl,
               AVG(se.pnl_sol)                            AS avg_pnl,
               COUNT(*) FILTER (WHERE se.pnl_sol > 0)::FLOAT / NULLIF(COUNT(*), 0) AS win_rate,
               COALESCE(SUM(COALESCE(sp.entry_amount_sol, 0)), 0) AS notional,
               MAX(se.pnl_sol)                            AS max_win
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
            addr, status = r["wallet_address"], r.get("status")
            samples, total, avg, win = r["samples"], r["total_pnl"], r["avg_pnl"], r["win_rate"]
            notional, max_win = r["notional"], r["max_win"]
        else:  # tuple
            addr, status, samples, total, avg, win, notional, max_win = r
        out.append(
            WalletPerf(
                address=addr,
                status=status,
                samples=int(samples),
                total_pnl=Decimal(total),
                avg_pnl=Decimal(avg),
                win_rate=float(win) if win is not None else 0.0,
                notional=Decimal(notional or 0),
                max_win=Decimal(max_win) if max_win is not None else None,
            )
        )
    return out


def _net_pnl(w: WalletPerf, cost_per_sol: Decimal) -> Decimal:
    """Post-cost shadow PnL = gross mirror_main PnL minus notional * cost."""
    return w.total_pnl - (w.notional or Decimal("0")) * cost_per_sol


def _not_tail_only(w: WalletPerf) -> bool:
    """Guard: the edge must not be a single lucky trade. When max_win is
    known, exclude the best exit — the wallet must still be gross-positive."""
    if w.max_win is None:
        return True
    return (w.total_pnl - w.max_win) > Decimal("0")


def select_promotions(
    perf: list[WalletPerf],
    min_samples: int = MIN_SAMPLES,
    min_net_pct: float = PROMOTE_MIN_NET_PCT,
    max_promotions: int = MAX_PROMOTIONS,
    cost_per_sol: Optional[Decimal] = None,
) -> list[WalletPerf]:
    """CANDIDATE wallets with PROVEN POST-COST profit to promote to ACTIVE.

    With a known cost basis (notional > 0) a wallet must clear the cost floor
    with margin: net_pct (net pnl / notional * 100) >= `min_net_pct`, AND the
    edge must not be a single lucky trade. Without a cost basis (legacy data)
    it falls back to the gross `PROMOTE_MIN_PNL` floor. Ranked by net PnL.
    """
    cps = observed_cost_per_sol() if cost_per_sol is None else cost_per_sol
    candidates: list[tuple[WalletPerf, Decimal]] = []
    for p in perf:
        if p.status != "CANDIDATE" or p.samples < min_samples or not _not_tail_only(p):
            continue
        notional = p.notional or Decimal("0")
        if notional > 0:
            net = _net_pnl(p, cps)
            net_pct = net / notional * Decimal("100")
            if net_pct < Decimal(str(min_net_pct)):
                continue
        else:
            if p.total_pnl < Decimal(str(PROMOTE_MIN_PNL)):
                continue
        candidates.append((p, _net_pnl(p, cps)))
    candidates.sort(key=lambda x: x[1], reverse=True)
    return [p for p, _ in candidates[:max_promotions]]


def select_demotions(
    perf: list[WalletPerf],
    min_samples: int = MIN_SAMPLES,
    max_net_pnl: float = DEMOTE_MAX_PNL,
    max_demotions: int = MAX_DEMOTIONS,
    cost_per_sol: Optional[Decimal] = None,
) -> list[WalletPerf]:
    """ACTIVE wallets with proven POST-COST losses to demote to CANDIDATE."""
    cps = observed_cost_per_sol() if cost_per_sol is None else cost_per_sol
    candidates: list[tuple[WalletPerf, Decimal]] = []
    for p in perf:
        if p.status != "ACTIVE" or p.samples < min_samples:
            continue
        net = _net_pnl(p, cps)
        if net <= Decimal(str(max_net_pnl)):
            candidates.append((p, net))
    candidates.sort(key=lambda x: x[1])  # worst first
    return [p for p, _ in candidates[:max_demotions]]


def optimize_paper_roster(
    perf: list[WalletPerf],
    min_samples: int = MIN_SAMPLES,
    min_net_pct: float = PROMOTE_MIN_NET_PCT,
    cost_per_sol: Optional[Decimal] = None,
) -> dict:
    """Roster rebalance that maximizes PAPER copy profitability (Phase 2H).

    Paper PnL is dominated by WHICH wallets are copied (entry selection), not
    by exit logic: reconciliation proved the shadow price basis is faithful, so
    the post-cost-CLEAR wallets (net_pct >= min_net_pct, not one lucky trade)
    are the profitable copy set, while ACTIVE wallets whose post-cost net <= 0
    are guaranteed cost-burners (gross edge below the ~1.4% cost floor).

      promote : every CLEAR candidate -> ACTIVE (no 25-rollover cap)
      demote  : ACTIVE with post-cost net <= 0 -> CANDIDATE (cut burners now)

    REJECTED status is respected (never resurrected by this path)."""
    cps = observed_cost_per_sol() if cost_per_sol is None else cost_per_sol
    to_promote: list[tuple[WalletPerf, Decimal]] = []
    to_demote: list[tuple[WalletPerf, Decimal]] = []
    for p in perf:
        if p.samples < min_samples:
            continue
        notional = p.notional or Decimal("0")
        net = _net_pnl(p, cps)
        net_pct = (net / notional * Decimal("100")) if notional > 0 else None
        clear = (
            (net_pct is not None and net_pct >= Decimal(str(min_net_pct)))
            or (net_pct is None and p.total_pnl >= Decimal(str(PROMOTE_MIN_PNL)))
        ) and _not_tail_only(p)
        if clear and p.status in ("CANDIDATE", None):
            to_promote.append((p, net))
        elif p.status == "ACTIVE" and net <= Decimal("0"):
            to_demote.append((p, net))
    to_promote.sort(key=lambda x: x[1], reverse=True)
    to_demote.sort(key=lambda x: x[1])  # worst first
    return {
        "promote": [p for p, _ in to_promote],
        "demote": [p for p, _ in to_demote],
    }


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
    cost_per_sol = observed_cost_per_sol()
    promotions = select_promotions(perf, cost_per_sol=cost_per_sol)
    demotions = select_demotions(perf, cost_per_sol=cost_per_sol)

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
    parser.add_argument(
        "--optimize-paper", action="store_true",
        help="roster rebalance for paper profitability: promote every post-cost "
        "CLEAR wallet -> ACTIVE, demote ACTIVE wallets with post-cost net <= 0 "
        "(cost-burners). Default is dry-run; combine with --apply to commit.",
    )
    parser.add_argument(
        "--apply", action="store_true",
        help="commit the --optimize-paper roster changes",
    )
    args = parser.parse_args(argv)

    if args.optimize_paper:
        perf = fetch_shadow_performance()
        cps = observed_cost_per_sol()
        res = optimize_paper_roster(perf, cost_per_sol=cps)
        print(
            f"paper roster rebalance: promote {len(res['promote'])}  "
            f"demote {len(res['demote'])}  apply={args.apply}"
        )
        for p in res["promote"]:
            print(f"  PROMOTE -> ACTIVE    {p.address}  n={p.samples} net={float(_net_pnl(p, cps)):.2f}")
        for p in res["demote"]:
            print(f"  DEMOTE -> CANDIDATE  {p.address}  n={p.samples} net={float(_net_pnl(p, cps)):.2f}")
        if args.apply:
            for p in res["promote"]:
                update_wallet_status(p.address, "ACTIVE")
            for p in res["demote"]:
                update_wallet_status(p.address, "CANDIDATE")
        return 0

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
