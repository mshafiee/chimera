"""Tests for the pre-registered cluster re-validation logic (Gate 1)."""

import datetime as dt
import itertools

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from scout.scripts.cluster_revalidation import (
    bootstrap_ci,
    bucket_for_count,
    evaluate_go_bar,
    passes_dispersion,
)

UTC = dt.timezone.utc
BASE = dt.datetime(2026, 9, 1, tzinfo=UTC)


# ── passes_dispersion: exists i<j<k with span>=min_span, gaps>=min_gap ──
def test_dispersion_three_wallets_exact_bounds():
    arrivals = [BASE, BASE + dt.timedelta(seconds=5), BASE + dt.timedelta(seconds=185)]
    assert passes_dispersion(arrivals, min_gap_s=5, min_span_s=180) is True


def test_dispersion_rejects_short_span():
    arrivals = [BASE, BASE + dt.timedelta(seconds=5), BASE + dt.timedelta(seconds=35)]
    assert passes_dispersion(arrivals, min_gap_s=5, min_span_s=180) is False


def test_dispersion_rejects_sub_gap_triple_but_accepts_valid_subset():
    # t=0, 3s, 200s, 300s: consecutive triple (0,3,200) fails the 5s gap,
    # but subset {0, 200, 300} is valid → must return True.
    arrivals = [
        BASE,
        BASE + dt.timedelta(seconds=3),
        BASE + dt.timedelta(seconds=200),
        BASE + dt.timedelta(seconds=300),
    ]
    assert passes_dispersion(arrivals, min_gap_s=5, min_span_s=180) is True


def test_dispersion_fewer_than_three_wallets():
    assert passes_dispersion([BASE, BASE + dt.timedelta(seconds=300)], 5, 180) is False


@settings(max_examples=200, deadline=None)
@given(st.lists(st.integers(min_value=0, max_value=10_000), min_size=0, max_size=12))
def test_dispersion_matches_brute_force_definition(offsets):
    """Brute-force oracle: any 3-combination with span & gaps in bounds."""

    arrivals = [BASE + dt.timedelta(seconds=s) for s in sorted(set(offsets))]

    def brute(secs):
        for a, b, c in itertools.combinations(secs, 3):
            if c - a >= 180 and b - a >= 5 and c - b >= 5:
                return True
        return False

    assert passes_dispersion(arrivals, 5, 180) == brute(sorted(set(offsets)))


# ── bucket_for_count ──────────────────────────────────────────────────
@pytest.mark.parametrize(
    "count,expected",
    [(1, "1"), (2, "2"), (3, "3+"), (7, "3+"), (None, "unattributed")],
)
def test_bucket_for_count(count, expected):
    assert bucket_for_count(count) == expected


# ── bootstrap_ci: deterministic under seed, percentile bounds ─────────
def test_bootstrap_ci_deterministic_and_sane():
    vals = [1.0, 2.0, 3.0, 4.0, 5.0]
    lo, hi = bootstrap_ci(vals, n_boot=2000, seed=42)
    assert lo <= hi
    lo2, hi2 = bootstrap_ci(vals, n_boot=2000, seed=42)
    assert (lo, hi) == (lo2, hi2)


def test_bootstrap_ci_empty():
    assert bootstrap_ci([], n_boot=100, seed=1) == (0.0, 0.0)


# ── Pivot-A: mirror shadow-validation on the dune cohort ────────────────


def test_evaluate_go_bar_frozen_thresholds():
    # Below n bar → fail even with great stats.
    assert evaluate_go_bar({"n": 299, "avg_pnl": 5.0, "ci_lo": 1.0}) is False
    # Negative mean → fail even with big n.
    assert evaluate_go_bar({"n": 400, "avg_pnl": -1.0, "ci_lo": 0.5}) is False
    # CI-lo <= 0 → fail.
    assert evaluate_go_bar({"n": 400, "avg_pnl": 2.0, "ci_lo": 0.0}) is False
    assert evaluate_go_bar({"n": 400, "avg_pnl": 2.0, "ci_lo": -0.1}) is False
    # All three cleared → pass.
    assert evaluate_go_bar({"n": 400, "avg_pnl": 2.0, "ci_lo": 0.3}) is True


def test_summarize_mirror_uses_frozen_cohort_definition(monkeypatch):
    # Frozen cohort (2026-09-07) = `dune_%` positions OR positions belonging to
    # a bootstrap-set wallet. Regression guard: a prefix-only filter dropped the
    # wallet-membership half and collapsed the live sample to n=0 (2026-09-19).
    from scout.scripts import cluster_revalidation as cr

    rows = [
        ("dune_1", "wA", 12.0),   # dune-keyed position -> cohort
        ("uuid_1", "wB", -3.0),   # bootstrap-set wallet, live key -> cohort
        ("uuid_2", "wZ", 100.0),  # non-cohort wallet -> excluded
    ]
    monkeypatch.setattr(cr, "load_mirror_exits", lambda days: rows)
    monkeypatch.setattr(cr, "load_dune_cohort_wallets", lambda: {"wA", "wB"})
    out = cr.summarize_mirror(days=14)
    assert out["n"] == 2, "non-cohort rows must be excluded"
    assert out["win_rate"] == 0.5
    assert out["avg_pnl"] == 4.5


# ── Mirror robustness gate (frozen 2026-09-19) ──────────────────────────


def test_robustness_excludes_no_price_and_caps_moonshots(monkeypatch):
    from scout.scripts import cluster_revalidation as cr

    rows = [
        ("dune_1", "wA", 5000.0, "profit_target_5"),  # capped to +100
        ("dune_2", "wA", -50.0, "stop_loss"),
        ("dune_3", "wA", 0.0, "no_price"),  # excluded from the priced sample
        ("uuid_1", "wB", 30.0, "profit_target_5"),
    ]
    monkeypatch.setattr(cr, "load_cohort_exits", lambda days, s: rows)
    monkeypatch.setattr(cr, "load_dune_cohort_wallets", lambda: {"wA", "wB"})
    m = cr.summarize_rail_robust(days=14, exit_strategy="mirror_main")
    assert m["n_total"] == 4
    assert m["n_priced"] == 3, "no_price rows must not count toward n"
    assert abs(m["mean_winsorized"] - (100.0 - 50.0 + 30.0) / 3) < 1e-9
    assert m["median"] == 30.0
    assert abs(m["win_rate"] - 2 / 3) < 1e-9


def test_robustness_requires_live_rail(monkeypatch):
    from scout.scripts import cluster_revalidation as cr

    def fake(days, exit_strategy):
        if exit_strategy == "mirror_main":
            return [("dune_1", "wA", 10.0, "profit_target_5")] * 400
        return [("uuid_1", "wB", -1.0, "stop_loss")] * 400

    monkeypatch.setattr(cr, "load_cohort_exits", fake)
    monkeypatch.setattr(cr, "load_dune_cohort_wallets", lambda: {"wA", "wB"})
    out = cr.run_robustness_validation(days=14)
    assert out["rails"]["mirror_main"]["n_priced"] == 400
    assert out["rails"]["wallet_sell"]["median"] == -1.0
    assert out["meets_robustness_bar"] is False, "live rail fails G4"


def test_robustness_passes_when_both_rails_clean(monkeypatch):
    from scout.scripts import cluster_revalidation as cr

    monkeypatch.setattr(
        cr, "load_cohort_exits", lambda days, s: [("dune_1", "wA", 5.0, "profit_target_5")] * 400
    )
    monkeypatch.setattr(cr, "load_dune_cohort_wallets", lambda: {"wA", "wB"})
    out = cr.run_robustness_validation(days=14)
    assert out["meets_robustness_bar"] is True
