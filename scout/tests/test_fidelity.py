"""Unit tests for core/fidelity.py (Phase 2B cross-check fidelity)."""

import math

import core.fidelity as fid


def test_perfect_correlation_pass():
    rec = [1.0, 2.0, 3.0, 4.0, 5.0]
    ref = [1.0, 2.0, 3.0, 4.0, 5.0]
    corr, m, ok = fid.fidelity(rec, ref)
    assert ok is True
    assert corr == 1.0
    assert m == 0.0


def test_scale_invariant_corr():
    # same shape scaled 100x -> correlation 1.0, MAPE huge (so fails gate)
    rec = [1.0, 2.0, 3.0]
    ref = [100.0, 200.0, 300.0]
    assert fid.pearson_corr(rec, ref) == 1.0
    _, m, ok = fid.fidelity(rec, ref)
    assert ok is False
    assert m is not None and m > 0.2


def test_under_sampled_returns_fail():
    corr, m, ok = fid.fidelity([1.0, 2.0], [1.0, 2.0])
    assert ok is False
    assert corr is None


def test_opposite_trend_fails():
    rec = [1.0, 2.0, 3.0]
    ref = [3.0, 2.0, 1.0]
    corr, _, ok = fid.fidelity(rec, ref)
    assert ok is False
    assert corr < 0


def test_mape_defined():
    assert fid.mape([2.0, 4.0], [2.0, 4.0]) == 0.0
    assert fid.mape([2.0, 4.0], [1.0, 2.0]) == 1.0
