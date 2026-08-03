"""Regression tests for the 2026-08-02 post-review remediation.

Covers the safety-critical scout fixes that previously had no test coverage:
  - WQS penalty precedence vs cap ordering (precedence must run BEFORE the cap)
  - WQS penalty cap enforcement
  - adjusted_score keeps penalties at full strength (confidence scales only the
    bonus side) — guards against the silent scoring-distribution shift
  - RealtimeProfitTracker trade-id dedup uses FIFO eviction (not unordered set
    slicing that can evict recently-seen ids)
  - CircuitBreaker reentrant lock paths no longer deadlock (threading.RLock)
"""

import sys
import threading
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent))

from decimal import Decimal

from core.wqs import (
    PenaltyCategory,
    ScoreTracker,
    WalletMetrics,
    _calculate_raw_score,
    calculate_wqs_with_confidence,
)


# ---------------------------------------------------------------------------
# WQS: penalty precedence + cap
# ---------------------------------------------------------------------------

def test_penalty_precedence_keeps_most_severe_per_category():
    """Precedence collapses duplicate-category entries to the single most
    severe one and recomputes the running negative total."""
    tracker = ScoreTracker()
    for amt in (20.0, 10.0, 5.0):
        tracker.add_neg(PenaltyCategory.MARTINGALE, amt)
    tracker.add_neg(PenaltyCategory.CVAR, 8.0)

    tracker._apply_penalty_precedence()

    # MARTINGALE keeps only the most severe (-20); the +10/+5 are dropped.
    assert tracker.components[PenaltyCategory.MARTINGALE] == -20.0
    martingale_entries = [a for c, a in tracker._penalty_entries if c == PenaltyCategory.MARTINGALE]
    assert martingale_entries == [20.0]
    # CVAR is a single entry, untouched.
    assert tracker.components[PenaltyCategory.CVAR] == -8.0
    # negative reflects most-severe-per-category only (20 + 8).
    assert tracker.negative == 28.0


def test_precedence_before_cap_not_after():
    """Regression for the precedence/cap ordering bug.

    Precedence re-reads the raw `_penalty_entries` and writes the uncapped
    most-severe value back into `components`. If it runs AFTER the cap it
    resurrects an uncapped value and defeats SCOUT_MAX_TOTAL_PENALTY. Running
    it BEFORE the cap honors the cap. This test exercises the real
    `ScoreTracker._apply_penalty_precedence` under both orderings.
    """
    cap = 5.0

    def _scale_to_cap(t: ScoreTracker) -> None:
        # Mirrors the cappable scale step from `_calculate_raw_score`.
        cappable = abs(sum(v for v in t.components.values() if v < 0))
        if cappable > 0:
            scale = cap / cappable
            for k in list(t.components.keys()):
                if t.components[k] < 0:
                    t.components[k] *= scale
            t.negative = abs(sum(v for v in t.components.values() if v < 0))

    def build() -> ScoreTracker:
        t = ScoreTracker()
        for amt in (20.0, 10.0, 5.0):  # duplicated cappable category
            t.add_neg(PenaltyCategory.MARTINGALE, amt)
        return t

    # FIXED ordering: precedence THEN cap -> cap honored.
    fixed = build()
    fixed._apply_penalty_precedence()
    _scale_to_cap(fixed)
    assert fixed.negative <= cap + 1e-9

    # BUGGY ordering: cap THEN precedence resurrects the uncapped most-severe.
    buggy = build()
    _scale_to_cap(buggy)
    buggy._apply_penalty_precedence()
    assert buggy.negative > cap, "precedence-after-cap must resurrect uncapped value (documents the hazard)"


def test_penalty_cap_enforced(monkeypatch):
    """The real `_calculate_raw_score` caps total cappable penalty at
    SCOUT_MAX_TOTAL_PENALTY."""
    monkeypatch.setenv("SCOUT_MAX_TOTAL_PENALTY", "5.0")
    wallet = WalletMetrics(
        address="cap_test",
        roi_30d=40.0,
        roi_7d=8.0,
        win_streak_consistency=0.8,
        trade_count_30d=25,
        max_drawdown_30d=50.0,   # drawdown penalty = 50 * 0.2 = 10 (> cap of 5)
        avg_trade_size_sol=Decimal("0.5"),
        profit_factor=2.0,       # avoid pf_wr penalty; no scam/sniper conditions
    )
    components = _calculate_raw_score(wallet)
    assert components.negative <= 5.0 + 1e-9


# ---------------------------------------------------------------------------
# WQS: adjusted_score keeps penalties at full strength
# ---------------------------------------------------------------------------

def test_adjusted_score_penalties_not_confidence_discounted():
    """Regression for the reverted `adjusted_score` formula.

    The buggy form `raw_score * confidence` discounted penalties by confidence,
    silently promoting low-confidence wallets. The correct form scales only the
    bonus side: `positive * confidence - negative`. With penalties present and
    confidence < 1, the correct adjusted score is strictly lower than the buggy
    `raw_score * confidence`.
    """
    wallet = WalletMetrics(
        address="adjusted_test",
        roi_30d=60.0,
        roi_7d=12.0,
        win_streak_consistency=0.8,
        trade_count_30d=5,        # confidence strictly < 1
        max_drawdown_30d=30.0,    # penalties present (negative > 0)
        avg_trade_size_sol=Decimal("0.5"),
        profit_factor=2.0,
    )
    res = calculate_wqs_with_confidence(wallet)
    assert 0.0 < res.confidence < 1.0

    buggy = max(0.0, min(res.score * res.confidence, 100.0))
    # Both values land inside (0, 100) for these metrics, so the strict
    # inequality holds iff penalties are at full strength.
    assert 0.0 < res.adjusted_score < 100.0
    assert 0.0 < buggy < 100.0
    assert res.adjusted_score < buggy - 1e-9, (
        f"penalties must not be confidence-discounted: adjusted={res.adjusted_score} "
        f"buggy(raw*conf)={buggy}"
    )


# ---------------------------------------------------------------------------
# RealtimeProfitTracker: FIFO dedup
# ---------------------------------------------------------------------------

def test_realtime_profit_tracker_dedup_evicts_oldest_first():
    """Dedup must evict the OLDEST trade ids (FIFO), not an arbitrary subset,
    so recently-seen ids are never re-counted."""
    from core.realtime_profit_tracker import RealtimeProfitTracker, TrackerConfig

    tracker = RealtimeProfitTracker(config=TrackerConfig())
    # Lower the bound so the test is fast and deterministic.
    tracker._max_seen_trade_ids = 5

    for i in range(5):
        tracker.update_profit(f"trade_{i}", pnl=1.0)
    # All five are distinct; capital reflects all five (no double count).
    assert tracker._current_capital == TrackerConfig.STARTING_CAPITAL + 5.0

    # Inserting a 6th evicts the OLDEST (trade_0), not a recent one.
    tracker.update_profit("trade_5", pnl=1.0)
    assert "trade_0" not in tracker._seen_trade_ids
    assert "trade_5" in tracker._seen_trade_ids
    # trade_4 (most recent before the 6th) must still be present.
    assert "trade_4" in tracker._seen_trade_ids


# ---------------------------------------------------------------------------
# CircuitBreaker: reentrant lock paths
# ---------------------------------------------------------------------------

def test_circuit_breaker_reentrant_paths_do_not_deadlock():
    """can_trade_wallet -> check_circuit_breaker and record_trade_result ->
    blacklist_wallet both re-acquire the lock. With threading.RLock these
    complete; a plain Lock would self-deadlock. Run on a worker thread with a
    timeout so a regression hangs the test instead of the whole suite."""
    from core.circuit_breaker import CircuitBreaker, CircuitBreakerConfig

    config = CircuitBreakerConfig(MAX_WALLET_FAILURES=2)
    cb = CircuitBreaker(config=config)

    result = {}

    def _worker():
        try:
            # Reentrant: can_trade_wallet acquires the lock, then calls
            # check_circuit_breaker which acquires it again.
            can, _ = cb.can_trade_wallet("wallet_deadlock")
            result["can_trade"] = can
            # Reentrant: record_trade_result acquires the lock, and after
            # MAX_WALLET_FAILURES it calls blacklist_wallet (lock again).
            for _ in range(config.MAX_WALLET_FAILURES):
                cb.record_trade_result(False, wallet_address="wallet_deadlock")
            result["done"] = True
        except Exception as exc:  # pragma: no cover - surface unexpected errors
            result["error"] = repr(exc)

    t = threading.Thread(target=_worker, daemon=True)
    t.start()
    t.join(timeout=10.0)
    assert not t.is_alive(), "CircuitBreaker reentrant path deadlocked"
    assert result.get("done") is True, f"worker did not complete cleanly: {result}"
    assert result.get("can_trade") is True
