"""Unit tests for core/replay_harness.py JSON shapes (Phase 2C/2D)."""

from decimal import Decimal

import core.replay_harness as rh


def test_replay_input_json_shape():
    positions = [
        {
            "entry_price": Decimal("0.000123"),
            "opened_at": 1787000000,
            "strategy": "SHIELD",
            "size_sol": Decimal("1.0"),
            "points": [(1700000000, Decimal("0.000120")), (1700000060, Decimal("0.000150"))],
        }
    ]
    out = rh.replay_input_json(positions)
    assert out["overrides"] == {}
    p = out["positions"][0]
    assert p["entry_price"] == "0.000123"
    assert p["strategy"] == "SHIELD"
    assert p["opened_at"] == 1787000000
    assert p["points"] == [[1700000000, "0.000120"], [1700000060, "0.000150"]]
