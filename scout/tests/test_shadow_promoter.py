"""Tests for shadow_promoter selection logic (pure, no DB)."""

from decimal import Decimal

from core.shadow_promoter import WalletPerf, select_demotions, select_promotions


def _perf(address, status, samples, total, win=0.5):
    return WalletPerf(
        address=address,
        status=status,
        samples=samples,
        total_pnl=Decimal(str(total)),
        avg_pnl=Decimal(str(total)) / Decimal(samples),
        win_rate=win,
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
