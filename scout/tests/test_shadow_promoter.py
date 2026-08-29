"""Tests for shadow_promoter selection logic (pure, no DB)."""

from decimal import Decimal

import pytest

import core.shadow_promoter as sp
from core.shadow_promoter import (
    WalletPerf,
    optimize_paper_roster,
    select_demotions,
    select_promotions,
)


@pytest.fixture(autouse=True)
def _zero_cost(monkeypatch):
    """No-cost default so legacy gross-semantics assertions hold; cost-aware
    tests pass an explicit cost_per_sol."""
    monkeypatch.setattr(sp, "observed_cost_per_sol", lambda: Decimal("0"))


def _perf(address, status, samples, total, win=0.5, notional=0.0, max_win=None, last_exit_age_days=1.0):
    return WalletPerf(
        address=address,
        status=status,
        samples=samples,
        total_pnl=Decimal(str(total)),
        avg_pnl=Decimal(str(total)) / Decimal(samples) if samples else Decimal("0"),
        win_rate=win,
        notional=Decimal(str(notional)),
        max_win=Decimal(str(max_win)) if max_win is not None else None,
        last_exit_age_days=last_exit_age_days,
    )


def test_promotes_proven_candidate_winners():
    perf = [
        _perf("WINNER_BIG", "CANDIDATE", 70, 195.0, win=0.08),   # moonshot wallet
        _perf("WINNER_OK", "CANDIDATE", 33, 7.0, win=0.42),
        _perf("TOO_FEW", "CANDIDATE", 5, 100.0),                 # not enough samples
        _perf("SMALL_PROFIT", "CANDIDATE", 30, 0.5),             # below pnl floor
        _perf("ALREADY_ACTIVE", "ACTIVE", 50, 50.0),             # wrong status
    ]
    promos = select_promotions(perf)
    assert {p.address for p in promos} == {"WINNER_BIG", "WINNER_OK"}
    # Ranked by total PnL descending.
    assert promos[0].address == "WINNER_BIG"


def test_promotion_ignores_win_rate_uses_expected_value():
    # The 8%-win moonshot wallet must still promote (high EV, low win rate).
    perf = [_perf("MOONSHOT", "CANDIDATE", 25, 50.0, win=0.08)]
    assert select_promotions(perf)[0].address == "MOONSHOT"


def test_promotion_respects_safety_cap():
    perf = [_perf(f"w{i}", "CANDIDATE", 30, 10.0) for i in range(40)]
    assert len(select_promotions(perf, max_promotions=25)) == 25


def test_demotes_proven_active_losers():
    perf = [
        _perf("LOSER", "ACTIVE", 66, -1.99, win=0.28),
        _perf("BIG_LOSER", "ACTIVE", 200, -7.6, win=0.25),
        _perf("MILD_LOSS", "ACTIVE", 30, -0.4),    # above demote floor, keep
        _perf("TOO_FEW_LOSS", "ACTIVE", 9, -5.0),  # not enough samples
        _perf("CANDIDATE_LOSS", "CANDIDATE", 50, -5.0),  # wrong status
    ]
    demos = select_demotions(perf)
    assert {p.address for p in demos} == {"LOSER", "BIG_LOSER"}
    # Worst loss first.
    assert demos[0].address == "BIG_LOSER"


def test_neither_promotes_nor_demotes_marginal_wallets():
    perf = [
        _perf("FLAT_CAND", "CANDIDATE", 30, 1.0),   # positive but below promote floor
        _perf("FLAT_ACT", "ACTIVE", 30, -0.5),      # negative but above demote floor
    ]
    assert select_promotions(perf) == []
    assert select_demotions(perf) == []


def test_promotion_requires_post_cost_clearance():
    # cost 2% per 1 SOL notional.
    thin = _perf("THIN", "CANDIDATE", 30, 5.0, notional=200)   # net 1.0 -> 0.5% < 1.5
    clear = _perf("CLEAR", "CANDIDATE", 30, 8.0, notional=100)  # net 6.0 -> 6% >= 1.5
    promos = select_promotions([thin, clear], cost_per_sol=Decimal("0.02"))
    assert {p.address for p in promos} == {"CLEAR"}


def test_promotion_rejects_single_lucky_trade():
    # cost 2%: both net-clear, but TAIL's whole edge is one trade.
    tail = _perf("TAIL", "CANDIDATE", 40, 20.0, notional=100, max_win=20.0)
    spread = _perf("SPREAD", "CANDIDATE", 40, 20.0, notional=100, max_win=5.0)
    promos = select_promotions([tail, spread], cost_per_sol=Decimal("0.02"))
    assert {p.address for p in promos} == {"SPREAD"}


def test_demotion_uses_post_cost_net():
    # cost 2% per 1 SOL notional.
    hidden = _perf("HIDDEN_LOSS", "ACTIVE", 30, -0.5, notional=100)  # net -2.5 -> demote
    mild = _perf("TRUE_MILD", "ACTIVE", 30, -0.5, notional=1)        # net -0.52 -> keep
    demos = select_demotions([hidden, mild], cost_per_sol=Decimal("0.02"))
    assert {p.address for p in demos} == {"HIDDEN_LOSS"}


def test_optimize_paper_promotes_clears_and_demotes_burners():
    # cost 2% per 1 SOL notional.
    clearc = _perf("CLEAR_CAND", "CANDIDATE", 30, 5.0, notional=100)  # net 3 -> 3% -> promote
    burner = _perf("BURNER_ACT", "ACTIVE", 30, 0.5, notional=100)    # net -1.5 <= 0 -> demote
    clear_active = _perf("CLEAR_ACT", "ACTIVE", 30, 8.0, notional=100)  # net 6 -> stays ACTIVE
    few = _perf("TOO_FEW", "CANDIDATE", 5, 50.0, notional=100)       # under min samples
    res = optimize_paper_roster(
        [clearc, burner, clear_active, few], cost_per_sol=Decimal("0.02")
    )
    assert {p.address for p in res["promote"]} == {"CLEAR_CAND"}
    assert {p.address for p in res["demote"]} == {"BURNER_ACT"}
    assert "CLEAR_ACT" not in [p.address for p in res["demote"]]


def test_optimize_paper_respects_rejected_status():
    # cost 2%; a CLEAR wallet that was REJECTED is never resurrected.
    rejected_clear = _perf("REJ_CLEAR", "REJECTED", 30, 8.0, notional=100)
    res = optimize_paper_roster([rejected_clear], cost_per_sol=Decimal("0.02"))
    assert res["promote"] == []


def test_optimize_paper_volume_mode_promotes_marginal():
    # cost 2%: net 1.0 on notional 200 = 0.5% net -> MARGINAL. Volume mode
    # (min_net_pct=0) promotes it; the default CLEAR floor (1.5) does not.
    marginal = _perf("MARGIN_CAND", "CANDIDATE", 30, 5.0, notional=200)
    assert optimize_paper_roster([marginal], cost_per_sol=Decimal("0.02"))["promote"] == []
    res = optimize_paper_roster(
        [marginal], cost_per_sol=Decimal("0.02"), min_net_pct=0.0
    )
    assert {p.address for p in res["promote"]} == {"MARGIN_CAND"}


def test_run_cycle_applies_paper_optimal_roster(monkeypatch):
    # The scheduled cycle keeps the paper copy set optimal: promote the CLEAR
    # candidate, demote the ACTIVE cost-burner (net <= 0), caps applied.
    perf = [
        _perf("CLEAR_CAND", "CANDIDATE", 30, 5.0, notional=100),   # net 3 -> 3% -> promote
        _perf("BURNER_ACT", "ACTIVE", 30, 0.5, notional=100),      # net -1.5 <= 0 -> demote
        _perf("BELOW_SAMPLES", "ACTIVE", 9, -5.0, notional=100),   # under min samples
    ]
    monkeypatch.setattr(sp, "fetch_shadow_performance", lambda window_days: perf)
    monkeypatch.setattr(sp, "observed_cost_per_sol", lambda: Decimal("0.02"))
    applied = []
    monkeypatch.setattr(sp, "update_wallet_status", lambda addr, status: applied.append((addr, status)))
    summary = sp.run_cycle(dry_run=False)
    assert ("CLEAR_CAND", "ACTIVE") in applied
    assert ("BURNER_ACT", "CANDIDATE") in applied
    assert ("BELOW_SAMPLES", "CANDIDATE") not in applied
    assert summary["promote"] == ["CLEAR_CAND"]
    assert summary["demote"] == ["BURNER_ACT"]


def test_run_cycle_uses_trailing_windows(monkeypatch):
    # Promotion must read the trailing promote window (30d); demotion the
    # shorter demote window (14d) — lifetime aggregates promoted stale edges
    # and hid decay (2026-08-28 windowing fix).
    seen_windows = []

    def fake_fetch(window_days):
        seen_windows.append(window_days)
        return []

    monkeypatch.setattr(sp, "fetch_shadow_performance", fake_fetch)
    monkeypatch.setattr(sp, "observed_cost_per_sol", lambda: Decimal("0.02"))
    monkeypatch.setattr(sp, "update_wallet_status", lambda addr, status: None)
    sp.run_cycle(dry_run=True)
    assert seen_windows == [sp.PROMOTE_WINDOW_DAYS, sp.DEMOTE_WINDOW_DAYS]


def test_demotion_needs_min_trailing_samples(monkeypatch):
    # Demotion is protective: it must fire on a REAL trailing bleeding book
    # (>= DEMOTE_MIN_SAMPLES exits) and must NOT fire on thin or dormant
    # evidence — cutting a quiet wallet removes its webhook coverage, which is
    # the 2026-08-17 star-wallet blackout failure mode.
    perf = [
        _perf("BLEEDER", "ACTIVE", 14, -3.0, notional=200),   # net -3.28 -> demote
        _perf("THIN_LOSER", "ACTIVE", 5, -3.0, notional=100),  # under demote floor -> keep
        _perf("DORMANT", "ACTIVE", 0, 0.0, notional=0),        # no evidence -> keep
    ]
    monkeypatch.setattr(sp, "observed_cost_per_sol", lambda: Decimal("0.02"))
    demotions = sp.optimize_paper_roster(
        perf, cost_per_sol=Decimal("0.02"), min_samples=sp.DEMOTE_MIN_SAMPLES,
    )["demote"]
    assert [p.address for p in demotions] == ["BLEEDER"]


def test_promotion_reads_promote_window_only(monkeypatch):
    # A CANDIDATE clear in the promote window is promoted even if absent from
    # the demote window; an ACTIVE burner in the demote window is demoted even
    # if absent from the promote window — the two windows are independent.
    promote_perf = [_perf("NEW_CLEAR", "CANDIDATE", 25, 6.0, notional=100)]
    demote_perf = [_perf("STALE_BURNER", "ACTIVE", 20, -2.0, notional=100)]
    monkeypatch.setattr(
        sp, "fetch_shadow_performance",
        lambda window_days: promote_perf if window_days == sp.PROMOTE_WINDOW_DAYS else demote_perf,
    )
    monkeypatch.setattr(sp, "observed_cost_per_sol", lambda: Decimal("0.02"))
    applied = []
    monkeypatch.setattr(sp, "update_wallet_status", lambda addr, status: applied.append((addr, status)))
    summary = sp.run_cycle(dry_run=False)
    assert ("NEW_CLEAR", "ACTIVE") in applied
    assert ("STALE_BURNER", "CANDIDATE") in applied
    assert summary["promote"] == ["NEW_CLEAR"]
    assert summary["demote"] == ["STALE_BURNER"]


def test_promotes_proving_wallets(monkeypatch):
    # PROVING wallets are first-class promotion candidates: the lane exists so
    # their trailing book can be judged — a clear prover graduates to ACTIVE.
    perf = [_perf("PROVEN_PROVER", "PROVING", 25, 6.0, notional=100)]
    monkeypatch.setattr(sp, "observed_cost_per_sol", lambda: Decimal("0.02"))
    res = sp.optimize_paper_roster(perf, cost_per_sol=Decimal("0.02"))
    assert [p.address for p in res["promote"]] == ["PROVEN_PROVER"]


def test_rebalance_recycles_stagnant_and_fills(monkeypatch):
    # Stagnant provers (zero shadow evidence past the window) recycle to
    # CANDIDATE; the deficit is filled with the highest-WQS candidates.
    def fake_fetch(query, params=()):
        if "NOT EXISTS" in query:
            return [{"address": "STAGNANT_1"}]
        if "count(*)" in query:
            return [{"count": 3}]  # 3 proving, 1 recycling -> deficit = 30 - 2 = 28
        return [{"address": f"CAND_{i}"} for i in range(28)]

    monkeypatch.setattr(sp, "execute_and_fetchall", fake_fetch)
    res = sp.rebalance_proving_pool(stagnation_days=14, target_size=30)
    assert res["to_candidate"] == ["STAGNANT_1"]
    assert len(res["to_proving"]) == 28
    assert res["to_proving"][0] == "CAND_0"


def test_rebalance_no_fill_when_pool_full(monkeypatch):
    def fake_fetch(query, params=()):
        if "NOT EXISTS" in query:
            return []
        return [{"count": 30}]

    monkeypatch.setattr(sp, "execute_and_fetchall", fake_fetch)
    res = sp.rebalance_proving_pool(stagnation_days=14, target_size=30)
    assert res["to_proving"] == []
    assert res["to_candidate"] == []


def test_run_cycle_rebalances_proving_pool(monkeypatch):
    # The cycle applies the proving rebalance (PROVING entry + recycle) before
    # promote/demote selection.
    monkeypatch.setattr(
        sp, "rebalance_proving_pool",
        lambda stagnation_days=14, target_size=30: {
            "to_proving": ["FRESH_1"], "to_candidate": ["STALE_1"],
        },
    )
    monkeypatch.setattr(sp, "fetch_shadow_performance", lambda window_days: [])
    monkeypatch.setattr(sp, "observed_cost_per_sol", lambda: Decimal("0.02"))
    applied = []
    monkeypatch.setattr(sp, "update_wallet_status", lambda addr, status: applied.append((addr, status)))
    summary = sp.run_cycle(dry_run=False)
    assert ("STALE_1", "CANDIDATE") in applied
    assert ("FRESH_1", "PROVING") in applied
    assert summary["to_proving"] == ["FRESH_1"]
    assert summary["to_candidate"] == ["STALE_1"]


def test_proving_pool_stats_counts_evidence(monkeypatch):
    def fake_fetch(query, params=()):
        if "has_evidence" in query:
            return [{"provers": 30, "with_evidence": 12}]
        raise AssertionError(f"unexpected query: {query}")

    monkeypatch.setattr(sp, "execute_and_fetchall", fake_fetch)
    stats = sp.proving_pool_stats()
    assert stats == {"provers": 30, "with_evidence": 12}


# ── Promotion recency guard (2026-08-29) ────────────────────────────────────

def test_promotion_requires_recent_edge():
    # Strong historical book but the whale went quiet 8 days ago: the
    # operator's dormancy rotation reclaims it within days — promoting it
    # just flaps the roster. Recency guard must hold it back.
    stale_book = _perf(
        "STALE_QUIET", "CANDIDATE", 81, 2.477, win=0.46,
        notional=60.0, last_exit_age_days=8.0,
    )
    res = sp.optimize_paper_roster([stale_book], cost_per_sol=Decimal("0.02"))
    assert res["promote"] == []


def test_promotion_allows_recent_edge():
    # Same book, newest exit 1 day old: clear to promote.
    fresh_book = _perf(
        "FRESH_QUIET", "CANDIDATE", 81, 2.477, win=0.46,
        notional=60.0, last_exit_age_days=1.0,
    )
    res = sp.optimize_paper_roster([fresh_book], cost_per_sol=Decimal("0.02"))
    assert [p.address for p in res["promote"]] == ["FRESH_QUIET"]


def test_promotion_rejects_unknown_recency():
    # No exit timestamp (None) = no evidence of current edge — treat as stale.
    unknown = _perf(
        "UNKNOWN_RECENCY", "CANDIDATE", 81, 2.477, win=0.46,
        notional=60.0, last_exit_age_days=None,
    )
    res = sp.optimize_paper_roster([unknown], cost_per_sol=Decimal("0.02"))
    assert res["promote"] == []
