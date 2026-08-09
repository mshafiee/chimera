"""
Coverage tests for core/denylist.py.

Existing tests (test_denylist.py) cover the membership functions; this file
covers the custom denylist file loader, which is only exercised when
SCOUT_DENYLIST_PATH points at an existing file.
"""

import asyncio
import logging

import core.denylist as denylist_module
from core.denylist import check_wallet_correlation, is_known_scam_address


def _run(coro):
    return asyncio.run(coro)


def _load(tmp_path, monkeypatch, content):
    path = tmp_path / "denylist.txt"
    path.write_text(content)
    monkeypatch.setenv("SCOUT_DENYLIST_PATH", str(path))
    denylist_module._KNOWN_SCAM_ADDRESSES.clear()
    denylist_module._KNOWN_SCAM_FUNDERS.clear()
    denylist_module._load_custom_denylist()
    # Rebuild the combined set exactly like the module does at import time
    denylist_module._ALL_SCAM = frozenset(
        denylist_module._KNOWN_SCAM_ADDRESSES | denylist_module._KNOWN_SCAM_FUNDERS
    )
    return path


def test_load_custom_denylist_parses_sections(tmp_path, monkeypatch):
    addr = "9xLONGWALLETADDRESS12345678901234567890123456789012345678"
    funder = "FUNDERADDRESS987654321098765432109876543210987654321098"
    _load(
        tmp_path,
        monkeypatch,
        f"# comment line\n\n{addr}\nFUNDERS:\n{funder}\n",
    )
    assert addr in denylist_module._KNOWN_SCAM_ADDRESSES
    assert funder in denylist_module._KNOWN_SCAM_FUNDERS
    assert is_known_scam_address(addr)
    assert is_known_scam_address(funder)


def test_load_custom_denylist_bracket_headers(tmp_path, monkeypatch):
    addr = "BKTADDR1111111111111111111111111111111111111111111111111"
    funder = "BKFUNDER2222222222222222222222222222222222222222222222"
    _load(
        tmp_path,
        monkeypatch,
        f"[FUNDERS]\n{funder}\n[ADDRESSES]\n{addr}\n",
    )
    assert funder in denylist_module._KNOWN_SCAM_FUNDERS
    assert addr in denylist_module._KNOWN_SCAM_ADDRESSES


def test_load_custom_denylist_skips_short_and_inline_comments(tmp_path, monkeypatch):
    addr = "SHORTADDR1234567890123456789012345678901234567890123456"
    _load(
        tmp_path,
        monkeypatch,
        f"short_addr_ignored\n{addr} # inline comment\n",
    )
    assert addr in denylist_module._KNOWN_SCAM_ADDRESSES
    assert "short_addr_ignored" not in denylist_module._KNOWN_SCAM_ADDRESSES


def test_load_custom_denylist_missing_file(monkeypatch, tmp_path):
    monkeypatch.setenv("SCOUT_DENYLIST_PATH", str(tmp_path / "nope.txt"))
    denylist_module._KNOWN_SCAM_ADDRESSES.clear()
    denylist_module._KNOWN_SCAM_FUNDERS.clear()
    denylist_module._load_custom_denylist()
    assert not denylist_module._KNOWN_SCAM_ADDRESSES


def test_load_custom_denylist_exception_swallowed(tmp_path, monkeypatch, caplog):
    # A directory path makes open() raise IsADirectoryError
    monkeypatch.setenv("SCOUT_DENYLIST_PATH", str(tmp_path))
    denylist_module._KNOWN_SCAM_ADDRESSES.clear()
    with caplog.at_level(logging.WARNING, logger="core.denylist"):
        denylist_module._load_custom_denylist()
    assert any("Failed to load denylist" in r.message for r in caplog.records)


def test_check_wallet_correlation_clean(tmp_path, monkeypatch):
    _load(tmp_path, monkeypatch, "")
    assert _run(check_wallet_correlation("clean_wallet_12345")) is True


def test_check_wallet_correlation_funder_hit(tmp_path, monkeypatch):
    funder = "CORRFUNDER333333333333333333333333333333333333333333333"
    _load(tmp_path, monkeypatch, f"FUNDERS:\n{funder}\n")
    assert _run(check_wallet_correlation("wallet_abc", funder=funder)) is False


def test_check_wallet_correlation_counterparty_hit(tmp_path, monkeypatch):
    cp = "CORRCP444444444444444444444444444444444444444444444444444"
    _load(tmp_path, monkeypatch, cp)
    assert _run(check_wallet_correlation("wallet_abc", counterparties={"other", cp})) is False
    assert _run(check_wallet_correlation("wallet_abc", counterparties={"other"})) is True


def test_check_wallet_correlation_wallet_hit(tmp_path, monkeypatch):
    scam = "CORRWALLET55555555555555555555555555555555555555555555555"
    _load(tmp_path, monkeypatch, scam)
    assert _run(check_wallet_correlation(scam)) is False


def test_is_known_scam_address_none():
    assert is_known_scam_address(None) is False
    assert is_known_scam_address("") is False
