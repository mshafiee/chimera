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


def _perf(address, status, samples, total, win=0.5, notional=0.0, max_win=None):
    return WalletPerf(
        address=address,
        status=status,
        samples=samples,
        total_pnl=Decimal(str(total)),
        avg_pnl=Decimal(str(total)) / Decimal(samples),
        win_rate=win,
        notional=Decimal(str(notional)),
        max_win=Decimal(str(max_win)) if max_win is not None else None,
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


def test_run_cycle_applies_paper_optimal_roster(monkeypatch):
    # The scheduled cycle keeps the paper copy set optimal: promote the CLEAR
    # candidate, demote the ACTIVE cost-burner (net <= 0), caps applied.
    perf = [
        _perf("CLEAR_CAND", "CANDIDATE", 30, 5.0, notional=100),   # net 3 -> 3% -> promote
        _perf("BURNER_ACT", "ACTIVE", 30, 0.5, notional=100),      # net -1.5 <= 0 -> demote
        _perf("BELOW_SAMPLES", "ACTIVE", 9, -5.0, notional=100),   # under min samples
    ]
    monkeypatch.setattr(sp, "fetch_shadow_performance", lambda: perf)
    monkeypatch.setattr(sp, "observed_cost_per_sol", lambda: Decimal("0.02"))
    applied = []
    monkeypatch.setattr(sp, "update_wallet_status", lambda addr, status: applied.append((addr, status)))
    summary = sp.run_cycle(dry_run=False)
    assert ("CLEAR_CAND", "ACTIVE") in applied
    assert ("BURNER_ACT", "CANDIDATE") in applied
    assert ("BELOW_SAMPLES", "CANDIDATE") not in applied
    assert summary["promote"] == ["CLEAR_CAND"]
    assert summary["demote"] == ["BURNER_ACT"]
