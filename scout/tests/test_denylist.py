import pytest
from core.denylist import is_known_scam_address, check_wallet_correlation

def test_empty_address():
    assert is_known_scam_address(None) is False
    assert is_known_scam_address("") is False

def test_clean_wallet_not_flagged():
    assert is_known_scam_address("legitimate_wallet_address_1234567890abcdef") is False

def test_case_sensitivity():
    addr = "ABC"
    assert is_known_scam_address(addr) is is_known_scam_address(addr.lower())

def test_short_address_not_loaded():
    assert is_known_scam_address("a" * 31) is False

def test_long_address_not_flagged():
    assert is_known_scam_address("a" * 44) is False

def test_whitespace_not_loaded():
    assert is_known_scam_address("   ") is False

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
