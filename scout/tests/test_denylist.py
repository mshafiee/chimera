"""Tests for core/denylist.py - Scam wallet denylist."""

from core.denylist import is_known_scam_address


def test_empty_address():
    assert is_known_scam_address(None) is False
    assert is_known_scam_address("") is False


def test_clean_wallet_not_flagged():
    assert is_known_scam_address("legitimate_wallet_address_1234567890abcdef") is False
