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
import os
import sys
from datetime import datetime, timezone
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

# --- Trailing windows (2026-08-28) ------------------------------------------
# Lifetime aggregates promoted stale edges and hid decay: a wallet whose edge
# died months ago stays ACTIVE as long as its lifetime net stays positive, and
# a wallet with a strong recent book but weak lifetime stays unpromotable.
# Promotion now reads the trailing promote window; demotion reads a shorter
# trailing window so below-cost drift is caught while evidence is fresh.
# Dormancy guard: an ACTIVE wallet with fewer than DEMOTE_MIN_SAMPLES exits in
# the demote window is NOT demoted for absence of evidence — removing its
# webhook coverage when it goes quiet is the 2026-08-17 star-wallet blackout
# failure mode (coverage loss made the platform's best wallet invisible for
# 11 days). Dormant wallets cost nothing; bleeding wallets get cut.
PROMOTE_WINDOW_DAYS = int(os.environ.get("SCOUT_PROMOTE_WINDOW_DAYS", "30"))
DEMOTE_WINDOW_DAYS = int(os.environ.get("SCOUT_DEMOTE_WINDOW_DAYS", "14"))
DEMOTE_MIN_SAMPLES = int(os.environ.get("SCOUT_DEMOTE_MIN_SAMPLES", "10"))

# --- Candidate-proving lane (2026-08-28) -------------------------------------
# The operator processes PROVING wallets in shadow-only mode (decisions +
# shadow forks, never queued live), so discovered candidates can finally
# accrue the trailing evidence promotion needs. This module manages the lane:
# fill it with the highest-WQS CANDIDATE wallets, recycle stagnant provers
# (zero evidence after PROVE_STAGNATION_DAYS) back to CANDIDATE, and promote
# provers whose trailing book clears the post-cost bar. PROVING wallets also
# get webhook coverage (consolidate_webhooks.sh + operator eligibility accept
# the status) — without coverage they can never be sampled.
PROVING_ROSTER_SIZE = int(os.environ.get("SCOUT_PROVING_ROSTER_SIZE", "30"))
PROVE_STAGNATION_DAYS = int(os.environ.get("SCOUT_PROVE_STAGNATION_DAYS", "14"))

# Promotion recency guard (2026-08-29): a wallet whose newest shadow exit is
# older than this has a DORMANT whale — the operator's dormancy rotation
# (GREATEST(promoted_at, last_trade_at) anchor) reclaims it within days, and
# a dormant whale generates no signals to copy. Requiring recent edge stops
# the promote→demote→promote flap (measured: 8jfDh7hABX/9bzPrKYb flapped
# every 2h cycle on 2026-08-28/29) and stops re-promoting stale books the
# trailing window still scores highly.
PROMOTE_RECENCY_DAYS = int(os.environ.get("SCOUT_PROMOTE_RECENCY_DAYS", "7"))


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
    # Age (days) of the wallet's newest deduped shadow exit. None = unknown
    # (no exits) — treated as stale by the promotion recency guard.
    last_exit_age_days: Optional[float] = None


def fetch_shadow_performance(window_days: Optional[int] = None) -> list[WalletPerf]:
    """Per-wallet mirror_main shadow PnL joined to current roster status.

    Also carries notional (for post-cost net expectancy) and max_win (for the
    not-one-lucky-trade guard). `window_days` restricts evidence to exits in
    the trailing window (exit time = when the edge was realized); None keeps
    the lifetime aggregate for back-compat callers.
    """
    window_sql = ""
    params: tuple = ()
    if window_days is not None:
        window_sql = "AND se.exited_at > NOW() - (%s || ' days')::INTERVAL"
        params = (str(window_days),)
    rows = execute_and_fetchall(
        f"""
        SELECT sp.wallet_address, w.status,
               COUNT(*)                                   AS samples,
               SUM(se.pnl_sol)                            AS total_pnl,
               AVG(se.pnl_sol)                            AS avg_pnl,
               COUNT(*) FILTER (WHERE se.pnl_sol > 0)::FLOAT / NULLIF(COUNT(*), 0) AS win_rate,
               COALESCE(SUM(COALESCE(sp.entry_amount_sol, 0)), 0) AS notional,
               MAX(se.pnl_sol)                            AS max_win,
               MAX(se.exited_at)                          AS last_exit_at
        FROM shadow_positions sp
        JOIN shadow_exits se ON se.shadow_id = sp.shadow_id
        LEFT JOIN wallets w ON w.address = sp.wallet_address
        WHERE se.exit_strategy = 'mirror_main' AND se.pnl_sol IS NOT NULL
        {window_sql}
        GROUP BY sp.wallet_address, w.status
        """,
        params,
    )
    out: list[WalletPerf] = []
    for r in rows:
        if isinstance(r, dict):
            addr, status = r["wallet_address"], r.get("status")
            samples, total, avg, win = r["samples"], r["total_pnl"], r["avg_pnl"], r["win_rate"]
            notional, max_win = r["notional"], r["max_win"]
            last_exit = r.get("last_exit_at")
        else:  # tuple
            addr, status, samples, total, avg, win, notional, max_win, last_exit = r
        last_exit_age_days: Optional[float] = None
        if last_exit is not None:
            last_exit_age_days = (datetime.now(timezone.utc) - last_exit).total_seconds() / 86400.0
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
                last_exit_age_days=last_exit_age_days,
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


def _recent_edge(w: WalletPerf) -> bool:
    """Promotion requires CURRENT edge, not a strong historical book: a wallet
    whose newest shadow exit is older than PROMOTE_RECENCY_DAYS has a dormant
    whale — the operator's dormancy rotation reclaims it within days, and a
    dormant whale generates no signals to copy."""
    if w.last_exit_age_days is None:
        return False
    return w.last_exit_age_days <= PROMOTE_RECENCY_DAYS


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
    recent_net_pct: Optional[dict] = None,
) -> dict:
    """Roster rebalance that maximizes PAPER copy profitability (Phase 2H).

    Paper PnL is dominated by WHICH wallets are copied (entry selection), not
    by exit logic: reconciliation proved the shadow price basis is faithful, so
    the post-cost-CLEAR wallets (net_pct >= min_net_pct, not one lucky trade)
    are the profitable copy set, while ACTIVE wallets whose post-cost net <= 0
    are guaranteed cost-burners (gross edge below the ~1.4% cost floor).

      promote : every CLEAR candidate -> ACTIVE (no 25-rollover cap)
      demote  : ACTIVE with post-cost net <= 0 -> CANDIDATE (cut burners now)

    REJECTED status is respected (never resurrected by this path).

    N3 (2026-08-31, current-flow quality): `recent_net_pct` maps wallet ->
    trailing-7d shadow net %. A candidate whose RECENT book is net-negative
    is held back even when its 30d book clears the bar — measured 2026-08-31:
    all three graduated books (129i4zsF9E, 5jahCqsAv9, 132Tkgf5YE) had zero
    admits from their current flow because the whales' present-tense behavior
    (launch-sniping, thin liquidity) diverged from the historical books the
    promotion bar scored. Promotion must price the flow being delivered NOW,
    not only the flow that existed. None (no recent exits) passes — the
    recency guard already blocks dormant whales."""
    cps = observed_cost_per_sol() if cost_per_sol is None else cost_per_sol
    to_promote: list[tuple[WalletPerf, Decimal]] = []
    to_demote: list[tuple[WalletPerf, Decimal]] = []
    for p in perf:
        if p.samples < min_samples:
            continue
        notional = p.notional or Decimal("0")
        net = _net_pnl(p, cps)
        net_pct = (net / notional * Decimal("100")) if notional > 0 else None
        recent_net = (recent_net_pct or {}).get(p.address)
        recent_ok = recent_net is None or recent_net >= 0
        clear = (
            (net_pct is not None and net_pct >= Decimal(str(min_net_pct)))
            or (net_pct is None and p.total_pnl >= Decimal(str(PROMOTE_MIN_PNL)))
        ) and _not_tail_only(p) and _recent_edge(p) and recent_ok
        if clear and p.status in ("PROVING", "CANDIDATE", None):
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


def rebalance_proving_pool(
    stagnation_days: int = PROVE_STAGNATION_DAYS,
    target_size: int = PROVING_ROSTER_SIZE,
) -> dict:
    """Keep the PROVING pool full of the most promising CANDIDATE wallets.

    - Recycle: PROVING wallets with ZERO shadow evidence and promoted_at older
      than `stagnation_days` go back to CANDIDATE — they had their chance to
      generate signals; a fresh discovery class gets the slot.
    - Fill: highest-WQS CANDIDATE wallets (oldest promoted_at first on ties)
      move to PROVING until the pool holds `target_size` wallets.

    Returns {"to_proving": [...], "to_candidate": [...]} (planned actions;
    the caller applies them via update_wallet_status).
    """
    # Stagnant provers recycle first (they vacate pool slots).
    rows = execute_and_fetchall(
        """
        SELECT w.address
        FROM wallets w
        WHERE w.status = 'PROVING'
          AND w.promoted_at < NOW() - (%s || ' days')::INTERVAL
          AND NOT EXISTS (
              SELECT 1 FROM shadow_positions sp WHERE sp.wallet_address = w.address
          )
        ORDER BY w.promoted_at ASC
        """,
        (str(stagnation_days),),
    )
    to_candidate = [r["address"] if isinstance(r, dict) else r[0] for r in rows]

    count_rows = execute_and_fetchall(
        "SELECT count(*) FROM wallets WHERE status = 'PROVING'",
    )
    current = count_rows[0]["count"] if isinstance(count_rows[0], dict) else count_rows[0][0]
    deficit = max(0, target_size - (int(current) - len(to_candidate)))
    to_proving: list[str] = []
    if deficit > 0:
        rows = execute_and_fetchall(
            """
            SELECT w.address
            FROM wallets w
            WHERE w.status = 'CANDIDATE'
            ORDER BY w.wqs_score DESC NULLS LAST, w.promoted_at ASC NULLS LAST
            LIMIT %s
            """,
            (str(deficit),),
        )
        to_proving = [r["address"] if isinstance(r, dict) else r[0] for r in rows]
    return {"to_proving": to_proving, "to_candidate": to_candidate}


def proving_pool_stats() -> dict:
    """Pool visibility: PROVING size and how many provers hold any shadow
    evidence. Logged every cycle so a starved lane (provers trading but
    zero evidence — the 2026-08-28 cache-starve class) is visible in the
    scout log without touching the DB by hand."""
    rows = execute_and_fetchall(
        """
        SELECT COUNT(*) AS provers,
               COUNT(*) FILTER (WHERE has_evidence) AS with_evidence
        FROM (
            SELECT w.status,
                   EXISTS (
                       SELECT 1 FROM shadow_positions sp
                       WHERE sp.wallet_address = w.address
                   ) AS has_evidence
            FROM wallets w
            WHERE w.status = 'PROVING'
        ) t
        """,
    )
    r = rows[0]
    if isinstance(r, dict):
        return {"provers": int(r["provers"]), "with_evidence": int(r["with_evidence"])}
    return {"provers": int(r[0]), "with_evidence": int(r[1])}


def fetch_recent_shadow_net(window_days: int = 7) -> dict:
    """Trailing-7d shadow net % per wallet — the CURRENT-flow quality signal.

    N3 (2026-08-31): a 30d book can be excellent while the whale's present-
    tense flow is toxic (launch-sniping, thin liquidity) — all three graduated
    books had zero gate-passing signals from their current flow. Promotion
    prices both: the 30d bar AND a non-negative 7d book."""
    rows = execute_and_fetchall(
        """
        SELECT sp.wallet_address, w.status,
               SUM(se.pnl_sol) AS pnl, COALESCE(SUM(sp.entry_amount_sol),0) AS notional
        FROM shadow_positions sp
        JOIN shadow_exits se ON se.shadow_id = sp.shadow_id
        LEFT JOIN wallets w ON w.address = sp.wallet_address
        WHERE se.exit_strategy = 'mirror_main' AND se.pnl_sol IS NOT NULL
          AND se.exited_at > NOW() - (%s || ' days')::INTERVAL
        GROUP BY sp.wallet_address, w.status
        """,
        (str(window_days),),
    )
    out: dict = {}
    for r in rows:
        if isinstance(r, dict):
            addr, pnl, notional = r["wallet_address"], r["pnl"], r["notional"]
        else:
            addr, pnl, notional = r[0], r[1], r[2]
        if notional and Decimal(str(notional)) > 0:
            out[addr] = (Decimal(str(pnl)) - Decimal(str(notional)) * Decimal("0.0065")) / Decimal(str(notional)) * 100
        else:
            out[addr] = None
    return out


def run_cycle(
    dry_run: bool = False,
    prune: bool = False,
    max_prune: int = 2000,
) -> dict:
    """Execute one promotion/demotion (and optional prune) cycle.

    Returns a summary dict of actions taken/planned.

    The candidate-proving pool is rebalanced first (fill with highest-WQS
    candidates, recycle stagnant provers). Promotion then reads the trailing
    PROMOTE_WINDOW_DAYS (default 30d) shadow book; demotion reads the shorter
    DEMOTE_WINDOW_DAYS (default 14d) book with a DEMOTE_MIN_SAMPLES floor so
    dormant wallets are never cut for absence of evidence.
    """
    try:
        proving = rebalance_proving_pool()
    except Exception as e:  # noqa: BLE001 — pool rebalance is advisory; a DB
        # hiccup here must not kill the promote/demote cycle.
        logger.warning("proving pool rebalance failed — skipping lane this cycle: %s", e)
        proving = {"to_proving": [], "to_candidate": []}
    if proving["to_proving"] or proving["to_candidate"]:
        logger.info(
            "proving pool: %d -> PROVING, %d -> CANDIDATE (stagnant)",
            len(proving["to_proving"]), len(proving["to_candidate"]),
        )
        if not dry_run:
            for addr in proving["to_candidate"]:
                update_wallet_status(addr, "CANDIDATE")
            for addr in proving["to_proving"]:
                update_wallet_status(addr, "PROVING")
    try:
        pool_stats = proving_pool_stats()
        logger.info(
            "proving pool stats: size=%d, with_evidence=%d",
            pool_stats["provers"], pool_stats["with_evidence"],
        )
    except Exception as e:  # noqa: BLE001 — stats are advisory visibility.
        logger.warning("proving pool stats failed: %s", e)
    promote_perf = fetch_shadow_performance(PROMOTE_WINDOW_DAYS)
    demote_perf = fetch_shadow_performance(DEMOTE_WINDOW_DAYS)
    # N3 (2026-08-30/31): current-flow quality — a 7d-negative book holds a
    # candidate back even when its 30d book clears the bar.
    try:
        recent_net_pct = fetch_recent_shadow_net(7)
    except Exception as e:  # noqa: BLE001 — advisory; promote on 30d bar alone.
        logger.warning("recent-flow fetch failed — promotion uses 30d book only: %s", e)
        recent_net_pct = None
    cost_per_sol = observed_cost_per_sol()    # Keep the PAPER copy set at the post-cost-CLEAR optimum every scheduled
    # cycle (Phase 2H): promote CLEAR candidates from the trailing promote
    # window, demote ACTIVE cost-burners (net <= 0) from the shorter trailing
    # demote window. Caps remain as guardrails against roster flapping.
    promote_roster = optimize_paper_roster(
        promote_perf, cost_per_sol=cost_per_sol, recent_net_pct=recent_net_pct,
    )
    demote_roster = optimize_paper_roster(
        demote_perf, cost_per_sol=cost_per_sol, min_samples=DEMOTE_MIN_SAMPLES,
    )
    promotions = promote_roster["promote"][:MAX_PROMOTIONS]
    demotions = demote_roster["demote"][:MAX_DEMOTIONS]

    summary: dict = {
        "promote": [p.address for p in promotions],
        "demote": [p.address for p in demotions],
        "to_proving": proving["to_proving"],
        "to_candidate": proving["to_candidate"],
        "prune": [],
        "dry_run": dry_run,
    }

    logger.info(
        "shadow promotion cycle: %d promote (trailing %dd), %d demote (trailing %dd, min %d samples) (dry_run=%s)",
        len(promotions), PROMOTE_WINDOW_DAYS,
        len(demotions), DEMOTE_WINDOW_DAYS, DEMOTE_MIN_SAMPLES,
        dry_run,
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
        "--promote-min-net-pct", type=float, default=PROMOTE_MIN_NET_PCT,
        help="net-expectancy floor (%% of notional) for promotion. Default 1.5 "
        "(CLEAR). Lower it toward 0 to grow signal volume: any net-positive "
        "wallet becomes promotable (paper-volume mode).",
    )
    parser.add_argument(
        "--apply", action="store_true",
        help="commit the --optimize-paper roster changes",
    )
    args = parser.parse_args(argv)

    if args.optimize_paper:
        perf = fetch_shadow_performance(PROMOTE_WINDOW_DAYS)
        cps = observed_cost_per_sol()
        res = optimize_paper_roster(
            perf, cost_per_sol=cps, min_net_pct=args.promote_min_net_pct,
        )
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
