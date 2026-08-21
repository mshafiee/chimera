"""Unit tests for analysis.deferral_evidence.classify_position (pure, no DB)."""

import analysis.deferral_evidence as de


def _marks(*prices):
    return [(i * 5, p) for i, p in enumerate(prices)]


def test_dip_then_recover_is_deferral_candidate():
    # entry 100; dips to 92 (-8%), recovers to 99 (-1%), exits at 93.5 (-6.5%).
    c = de.classify_position(_marks(100, 92, 95, 99, 98), 100.0, 93.5, 1.0, min_marks=4)
    assert c["type"] == "dip_then_recover"
    assert c["deferral_candidate"] is True
    assert c["recoverable_pct"] == 5.5  # peak -1 - exit -6.5
    assert c["recoverable_sol"] == 0.055  # 1.0 notional


def test_kept_falling_never_recovers():
    # dips to 91 (-9%), recovers only to 92 (-8%): recovery 1pp < 5pp.
    c = de.classify_position(_marks(100, 91, 92, 92), 100.0, 92.0, 1.0, min_marks=4)
    assert c["type"] == "kept_falling"
    assert c["deferral_candidate"] is False
    assert c["recoverable_sol"] == 0.0


def test_clean_position_no_dip():
    c = de.classify_position(_marks(100, 99, 100, 101), 100.0, 101.0, 1.0, min_marks=4)
    assert c["type"] == "clean"
    assert c["deferral_candidate"] is False


def test_insufficient_marks_not_classified():
    c = de.classify_position(_marks(100, 92, 99, 98, 95), 100.0, 95.0, 1.0)
    assert c["type"] == "insufficient"
    assert c["deferral_candidate"] is False


def test_dip_still_falling_at_close_is_not_candidate():
    # recovers to 96 (-4%) then a fresh dip to 90 (-10%) at the very end: the
    # min is the last mark, so nothing recovered to defer toward.
    c = de.classify_position(_marks(100, 94, 96, 90), 100.0, 90.5, 1.0, min_marks=4)
    assert c["type"] == "dip_then_recover"
    assert c["deferral_candidate"] is False