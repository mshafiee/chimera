"""Unit tests for admission_tracker diff (pure, no DB)."""

import analysis.admission_tracker as at


def _snap(prefix):
    return {
        "taken_at": f"{prefix}-t",
        "gate": {
            "ADMITTED": {"n": 188, "sum_pnl": -2.99, "win_pct": 32.4},
            "WQS_TOO_LOW": {"n": 482, "sum_pnl": 40.31, "win_pct": 43.6},
        },
        "gap": {"predicted_win_pct": 62.6, "realized_win_pct": 18.8,
                "predicted_n": 16554, "realized_n": 186},
        "trades_per_day_last_14d": [{"d": "2026-08-07", "n": 8}],
        "active_roster": 35,
        "signaling_wallets_7d": 12,
    }


def test_diff_snapshots_deltas():
    before = _snap("b")
    after = _snap("a")
    after["gate"]["ADMITTED"]["n"] = 260
    after["gate"]["ADMITTED"]["sum_pnl"] = 1.5
    after["active_roster"] = 41
    after["signaling_wallets_7d"] = 18
    after["trades_per_day_last_14d"] = [{"d": "2026-08-07", "n": 12}]
    out = at.diff_snapshots(before, after)
    assert "188->260" in out
    assert "-2.99->" in out
    assert "35 -> 41" in out
    assert "12 -> 18" in out
    assert "8 -> 12" in out