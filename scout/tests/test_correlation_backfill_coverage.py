"""
Coverage tests for core/correlation_backfill.py.

Uses the fake_db_layer fixture (in-memory SQLite) and patches the module's
own bindings, since correlation_backfill imports Connection/execute_query/
execute_update at module import time.
"""

import core.correlation_backfill as cb


def _wallet_rows():
    return [
        {"wallet_address": "walletA", "promoted_at": "2025-01-01T00:00:00"},
        {"wallet_address": "walletB", "promoted_at": "2025-01-01T00:00:00"},
        {"wallet_address": "walletC", "promoted_at": "2025-01-01T00:00:00"},
    ]


def _pnl_rows():
    return {
        "walletA": {
            "wallet_address": "walletA",
            "copy_pnl_7d": 1.5,
            "copy_pnl_30d": 2.5,
            "copy_pnl_all": 10.0,
            "count_7d": 2,
            "count_30d": 3,
            "count_all": 7,
        },
        "walletB": {
            "wallet_address": "walletB",
            "copy_pnl_7d": None,
            "copy_pnl_30d": None,
            "copy_pnl_all": None,
            "count_7d": 0,
            "count_30d": 0,
            "count_all": 1,
        },
    }


def test_backfill_correlation_pnl_updates_records(fake_db_layer, monkeypatch, capsys):
    monkeypatch.setattr(cb, "Connection", lambda db_path: fake_db_layer)
    updates = []

    def fake_execute_query(conn, query, params=None):
        if "FROM wqs_pnl_correlation" in query and "IN" not in query:
            return _FakeCursor(_wallet_rows())
        if "UPDATE wqs_pnl_correlation" in query:
            updates.append(params)
            return _FakeCursor([])
        return _FakeCursor(_pnl_rows())

    monkeypatch.setattr(cb, "execute_query", fake_execute_query)
    monkeypatch.setattr(cb, "execute_update", lambda query, params: None)

    result = cb.backfill_correlation_pnl("data/test.db")
    # walletA updated; walletB has NULL 30d+all (skipped); walletC has no pnl row
    assert result == 1
    assert len(updates) == 1
    # walletA gets its real pnl values
    assert updates[0][0] == 1.5
    assert updates[0][5] == 7
    out = capsys.readouterr().out
    assert "Backfilled PnL for 1 wallets" in out


def test_backfill_correlation_pnl_skips_wallet_with_no_pnl_data(
    fake_db_layer, monkeypatch, capsys
):
    monkeypatch.setattr(cb, "Connection", lambda db_path: fake_db_layer)
    updates = []

    def fake_execute_query(conn, query, params=None):
        if "FROM wqs_pnl_correlation" in query and "IN" not in query:
            return _FakeCursor(_wallet_rows())
        if "UPDATE wqs_pnl_correlation" in query:
            updates.append(params)
            return _FakeCursor([])
        return _FakeCursor({"walletA": _pnl_rows()["walletA"]})

    monkeypatch.setattr(cb, "execute_query", fake_execute_query)
    monkeypatch.setattr(cb, "execute_update", lambda query, params: None)
    result = cb.backfill_correlation_pnl("data/test.db")
    # walletB and walletC have no pnl rows at all -> skipped; walletA updated
    assert result == 1
    assert len(updates) == 1
    assert updates[0][-1] == "walletA"


def test_backfill_correlation_pnl_all_null_skips_row(fake_db_layer, monkeypatch):
    monkeypatch.setattr(cb, "Connection", lambda db_path: fake_db_layer)
    monkeypatch.setattr(
        cb, "execute_query",
        lambda conn, query, params=None: _FakeCursor(
            _wallet_rows() if "FROM wqs_pnl_correlation" in query and "IN" not in query
            else {"walletB": _pnl_rows()["walletB"]}
        ),
    )
    monkeypatch.setattr(cb, "execute_update", lambda query, params: None)
    result = cb.backfill_correlation_pnl("data/test.db")
    # walletB's 30d and all pnl are both None -> skipped (line 84-85)
    assert result == 0


def test_backfill_correlation_pnl_no_rows(fake_db_layer, monkeypatch):
    monkeypatch.setattr(cb, "Connection", lambda db_path: fake_db_layer)
    monkeypatch.setattr(
        cb, "execute_query",
        lambda conn, query, params=None: _FakeCursor([]),
    )
    assert cb.backfill_correlation_pnl("data/test.db") == 0


def test_backfill_correlation_pnl_single_address(fake_db_layer, monkeypatch):
    monkeypatch.setattr(cb, "Connection", lambda db_path: fake_db_layer)
    captured = {}

    def fake_execute_query(conn, query, params=None):
        if "t.wallet_address IN" in query:
            captured["params"] = params
            return _FakeCursor({"walletA": _pnl_rows()["walletA"]})
        if "UPDATE wqs_pnl_correlation" in query:
            return _FakeCursor([])
        return _FakeCursor([{"wallet_address": "walletA", "promoted_at": "2025-01-01"}])

    monkeypatch.setattr(cb, "execute_query", fake_execute_query)
    monkeypatch.setattr(cb, "execute_update", lambda query, params: None)
    cb.backfill_correlation_pnl("data/test.db")
    # Single-element IN must be a 2-tuple for psycopg
    assert captured["params"][0] == ("walletA", "walletA")


def test_backfill_correlation_pnl_error_reraises(fake_db_layer, monkeypatch, capsys):
    monkeypatch.setattr(cb, "Connection", lambda db_path: fake_db_layer)
    def boom(conn, query, params=None):
        raise RuntimeError("db down")
    monkeypatch.setattr(cb, "execute_query", boom)
    try:
        cb.backfill_correlation_pnl("data/test.db")
        assert False, "expected RuntimeError"
    except RuntimeError:
        pass
    out = capsys.readouterr().out
    assert "PnL backfill failed" in out


def test_write_correlation_record(fake_db_layer, monkeypatch):
    calls = []
    monkeypatch.setattr(cb, "execute_update", lambda query, params: calls.append(params))
    cb.write_correlation_record("walletA", 75.5, "{}", "SHIELD")
    assert len(calls) == 1
    assert calls[0][0] == "walletA"
    assert calls[0][1] == 75.5
    assert calls[0][2] == "{}"
    assert calls[0][4] == "SHIELD"


def test_write_correlation_record_error(fake_db_layer, monkeypatch, capsys):
    def boom(query, params):
        raise RuntimeError("nope")
    monkeypatch.setattr(cb, "execute_update", boom)
    cb.write_correlation_record("walletA", 1.0, "{}", "SHIELD")
    out = capsys.readouterr().out
    assert "Failed to write correlation record" in out


def test_write_promotion_episode(fake_db_layer, monkeypatch):
    calls = []
    monkeypatch.setattr(cb, "execute_update", lambda query, params: calls.append(params))
    monkeypatch.setenv("SCOUT_PROMOTION_POLICY_VERSION", "v3")
    monkeypatch.setenv("GIT_HASH", "abc123")
    cb.write_promotion_episode("walletA", 80.0, 0.9, "{}", "promoted")
    assert len(calls) == 1
    assert calls[0][0] == "walletA"
    assert calls[0][1] == 80.0
    assert calls[0][2] == 0.9
    assert calls[0][4] == "promoted"
    assert calls[0][5] == "v3"
    assert calls[0][6] == "abc123"


def test_write_promotion_episode_defaults(fake_db_layer, monkeypatch):
    calls = []
    monkeypatch.setattr(cb, "execute_update", lambda query, params: calls.append(params))
    cb.write_promotion_episode("walletA", 80.0, None, None)
    assert calls[0][2] is None
    assert calls[0][5] == "default"
    assert calls[0][6] == "unknown"


def test_write_promotion_episode_error(fake_db_layer, monkeypatch, capsys):
    def boom(query, params):
        raise RuntimeError("nope")
    monkeypatch.setattr(cb, "execute_update", boom)
    cb.write_promotion_episode("walletA", 80.0, None, None)
    out = capsys.readouterr().out
    assert "Failed to write promotion episode" in out


class _FakeCursor:
    """Cursor stand-in returning preset rows."""

    def __init__(self, rows):
        self._rows = rows

    def fetchall(self):
        if isinstance(self._rows, dict):
            return [dict(r) for r in self._rows.values()]
        return list(self._rows)
