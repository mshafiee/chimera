"""
Coverage tests for core/utils.py.
"""

import pytest

from core.utils import parse_utc_timestamp, utcnow


def test_utcnow_returns_aware_utc():
    now = utcnow()
    assert now.tzinfo is not None
    assert now.utcoffset().total_seconds() == 0


def test_parse_naive_timestamp_assumes_utc():
    dt = parse_utc_timestamp("2025-06-18T12:00:00")
    assert dt.tzinfo is not None
    assert dt.utcoffset().total_seconds() == 0
    assert dt.hour == 12


def test_parse_aware_timestamp_normalizes_to_utc():
    dt = parse_utc_timestamp("2025-06-18T12:00:00+05:30")
    assert dt.tzinfo is not None
    assert dt.utcoffset().total_seconds() == 0
    # 12:00 IST = 06:30 UTC
    assert dt.hour == 6
    assert dt.minute == 30


def test_parse_timestamp_utc_offset_passthrough():
    dt = parse_utc_timestamp("2025-06-18T12:00:00+00:00")
    assert dt.utcoffset().total_seconds() == 0
    assert dt.hour == 12


def test_parse_timestamp_invalid_raises():
    with pytest.raises(ValueError):
        parse_utc_timestamp("not-a-timestamp")
