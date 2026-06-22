"""Tests for core/auto_merge.py - Roster merge via operator API."""

from core.auto_merge import merge_via_sighup


def test_merge_via_sighup_no_container():
    result = merge_via_sighup(operator_container="nonexistent-container")
    assert result is not None
    assert len(result) == 2
    assert isinstance(result[0], bool)
    assert isinstance(result[1], str)
