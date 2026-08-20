"""
Unit tests for core/price_path.py (Phase 2A on-chain price reconstruction).

Validates the amount parsing and the SOL<->token swap price extraction against
representative Helius tokenTransfer payloads.
"""

from decimal import Decimal

import core.price_path as pp


def test_ui_amount_raw_token_amount():
    tr = {"mint": "abc", "rawTokenAmount": {"tokenAmount": "1000000", "decimals": 6}}
    assert pp.ui_amount(tr) == Decimal("1")


def test_ui_amount_ui_amount_string():
    tr = {"mint": "abc", "tokenAmount": {"uiAmountString": "1.5"}}
    assert pp.ui_amount(tr) == Decimal("1.5")


def test_ui_amount_scalar():
    tr = {"mint": "abc", "tokenAmount": 2.0}
    assert pp.ui_amount(tr) == Decimal("2")


def test_swap_price_buy():
    # buy: 1 SOL (wSOL) in, 1000 token out -> payable 0.001 SOL/token
    transfers = [
        {"mint": pp.WSOL_MINT, "rawTokenAmount": {"tokenAmount": str(10**9), "decimals": 9}},
        {"mint": "TOKENMINT", "rawTokenAmount": {"tokenAmount": str(1000 * 10 ** 6), "decimals": 6}},
    ]
    assert pp.swap_price_from_transfers(transfers, "TOKENMINT") == Decimal("0.001")


def test_swap_price_sell_reverse_direction():
    # sell: token in, SOL out — direction-agnostic sum yields the same price
    transfers = [
        {"mint": "TOKENMINT", "rawTokenAmount": {"tokenAmount": str(1000 * 10 ** 6), "decimals": 6}},
        {"mint": pp.WSOL_MINT, "rawTokenAmount": {"tokenAmount": str(10**9), "decimals": 9}},
    ]
    assert pp.swap_price_from_transfers(transfers, "TOKENMINT") == Decimal("0.001")


def test_no_direct_sol_leg_returns_none():
    # Jupiter multi-hop with no WSOL leg -> no direct price
    transfers = [
        {"mint": "OTHERTOKEN", "rawTokenAmount": {"tokenAmount": "100", "decimals": 6}},
        {"mint": "TOKENMINT", "rawTokenAmount": {"tokenAmount": "50", "decimals": 6}},
    ]
    assert pp.swap_price_from_transfers(transfers, "TOKENMINT") is None


def test_finalize_sorts_and_dedups():
    pts = [(10, Decimal("2")), (5, Decimal("1")), (10, Decimal("3"))]
    got = pp._finalize(pts)
    assert got == [(5, Decimal("1")), (10, Decimal("2"))]


def test_parse_ohlcv_close():
    # rows: [start_ts, o, h, l, close, volume]
    rows = [
        [1700000100, "1", "2", "0.5", "1.5", "100"],
        [1700000160, "1.5", "3", "1", "2.0", "200"],
        [1700000160, "9", "9", "9", "2.0", "200"],  # duplicate ts -> dropped
    ]
    got = pp.parse_ohlcv_close(rows)
    assert got == [(1700000100, Decimal("1.5")), (1700000160, Decimal("2.0"))]


def test_parse_ohlcv_close_drops_bad_rows():
    rows = [
        ["x", "1", "2", "3", "4", "5"],  # bad ts
        [1700000100, "1", "2", "3", "0", "5"],  # non-positive close
        [1700000200, "1", "2", "3", "7", "5"],
    ]
    got = pp.parse_ohlcv_close(rows)
    assert got == [(1700000200, Decimal("7"))]


def test_as_resource_list_shapes():
    assert pp._as_resource_list([{"id": "a"}]) == [{"id": "a"}]
    assert pp._as_resource_list({"id": "a"}) == [{"id": "a"}]
    assert pp._as_resource_list(["oops", {"id": "a"}]) == [{"id": "a"}]
    assert pp._as_resource_list(None) == []
    assert pp._as_resource_list({}) == [{}]


