import pytest
import core.denylist as denylist_module
from core.denylist import is_known_scam_address, check_wallet_correlation


def test_empty_address():
    assert is_known_scam_address(None) is False
    assert is_known_scam_address("") is False


def test_clean_wallet_not_flagged():
    assert is_known_scam_address("legitimate_wallet_address_1234567890abcdef") is False


def test_case_sensitivity(monkeypatch):
    # Inject a known scam address; membership is exact-match (case-sensitive)
    known = "AbC1234567890123456789012345678901234567890"
    monkeypatch.setattr(denylist_module, "_ALL_SCAM",
                       frozenset(denylist_module._ALL_SCAM | {known}))
    assert is_known_scam_address(known) is True
    # A different case is a different address and must not match
    assert is_known_scam_address(known.lower()) is False


def test_short_address_not_loaded():
    assert is_known_scam_address("a" * 31) is False


def test_long_address_not_flagged(monkeypatch):
    # A 44-char address is a plausible encoded address and would be flagged
    # when present in the denylist (the loader adds any address >= 32 chars)
    addr = "a" * 44
    monkeypatch.setattr(denylist_module, "_ALL_SCAM",
                       frozenset(denylist_module._ALL_SCAM | {addr}))
    assert is_known_scam_address(addr) is True


def test_whitespace_not_loaded():
    assert is_known_scam_address("   ") is False


def test_known_scam_address_detected(monkeypatch):
    """Positive test: a known scam address must be flagged."""
    known = "ScamWallet0000000000000000000000000000000000"
    monkeypatch.setattr(denylist_module, "_ALL_SCAM",
                       frozenset(denylist_module._ALL_SCAM | {known}))
    assert is_known_scam_address(known) is True


@pytest.mark.asyncio
async def test_check_wallet_correlation_clean():
    result = await check_wallet_correlation(
        "legitimate_wallet_address_1234567890abcdef",
        funder="some_funder_address_1234567890abcdef",
    )
    assert result is True


@pytest.mark.asyncio
async def test_check_wallet_correlation_no_funder():
    result = await check_wallet_correlation(
        "legitimate_wallet_address_1234567890abcdef"
    )
    assert result is True


@pytest.mark.asyncio
async def test_check_wallet_correlation_scam_wallet(monkeypatch):
    """Positive test: a scam wallet must fail correlation."""
    known = "ScamWallet0000000000000000000000000000000000"
    monkeypatch.setattr(denylist_module, "_ALL_SCAM",
                       frozenset(denylist_module._ALL_SCAM | {known}))
    result = await check_wallet_correlation(known, funder="funder_xyz")
    assert result is False


@pytest.mark.asyncio
async def test_check_wallet_correlation_scam_funder(monkeypatch):
    """Positive test: a scam funder must fail correlation."""
    scam_funder = "ScamFunder000000000000000000000000000000000"
    monkeypatch.setattr(denylist_module, "_ALL_SCAM",
                       frozenset(denylist_module._ALL_SCAM | {scam_funder}))
    result = await check_wallet_correlation(
        "legitimate_wallet_address_1234567890abcdef",
        funder=scam_funder,
    )
    assert result is False


@pytest.mark.asyncio
async def test_check_wallet_correlation_scam_counterparty(monkeypatch):
    """Positive test: a scam counterparty must fail correlation."""
    scam_cp = "ScamCounterparty000000000000000000000000000"
    monkeypatch.setattr(denylist_module, "_ALL_SCAM",
                       frozenset(denylist_module._ALL_SCAM | {scam_cp}))
    result = await check_wallet_correlation(
        "legitimate_wallet_address_1234567890abcdef",
        funder="some_funder_address_1234567890abcdef",
        counterparties={"other_wallet", scam_cp},
    )
    assert result is False
