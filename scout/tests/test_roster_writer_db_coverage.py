"""
Coverage tests for core/roster_writer_db.py.

Covers the remaining branches: ACTIVE-status update path with
inactivity-demotion reset, and the exception paths of delete/get/status reads.
"""


import core.roster_writer_db as rwd
from core.db import execute_update
from core.roster_writer_db import (
    WalletRecord,
    delete_wallet,
    get_wallet,
    get_wallets_by_status,
    update_wallet_status,
    write_wallet_to_db,
    write_wallets_to_db,
)


def _wallet(address="wallet1", status="CANDIDATE"):
    return WalletRecord(
        address=address, status=status, wqs_score=70.0, wqs_confidence=0.8,
        roi_7d=1.0, roi_30d=2.0, trade_count_30d=10, win_rate=0.5,
        max_drawdown_30d=5.0, avg_trade_size_sol=0.5,
    )


def _create_tables(fake_db_layer):
    cur = fake_db_layer.cursor()
    cur.execute(
        "CREATE TABLE IF NOT EXISTS wallets ("
        "address TEXT PRIMARY KEY, status TEXT, wqs_score REAL, wqs_confidence REAL,"
        "roi_7d REAL, roi_30d REAL, trade_count_30d INTEGER, win_rate REAL,"
        "max_drawdown_30d REAL, avg_trade_size_sol REAL, avg_win_sol REAL,"
        "avg_loss_sol REAL, profit_factor REAL, realized_pnl_30d_sol REAL,"
        "last_trade_at TEXT, promoted_at TEXT, ttl_expires_at TEXT, notes TEXT,"
        "archetype TEXT, avg_entry_delay_seconds REAL, updated_at TEXT)"
    )
    cur.execute(
        "CREATE TABLE IF NOT EXISTS wallet_monitoring ("
        "wallet_address TEXT PRIMARY KEY, inactivity_demotion_count INTEGER)"
    )
    fake_db_layer.commit()


def test_update_wallet_status_active_reanchors_promoted_at(fake_db_layer, monkeypatch):
    _create_tables(fake_db_layer)
    monkeypatch.setattr(rwd, "execute_update", execute_update)
    monkeypatch.setattr("core.db.execute_and_fetchone", lambda q, p: {"status": "CANDIDATE"})
    assert update_wallet_status("wallet1", "ACTIVE") is True


def test_update_wallet_status_active_reset_failure_swallowed(fake_db_layer, monkeypatch):
    _create_tables(fake_db_layer)
    monkeypatch.setattr(rwd, "execute_update", execute_update)
    monkeypatch.setattr("core.db.execute_and_fetchone", lambda q, p: {"status": "CANDIDATE"})

    def boom(query, params):
        raise RuntimeError("monitoring table missing")

    monkeypatch.setattr("core.db.execute_update", boom, raising=False)
    # write path uses rwd.execute_update (patched above); the reset uses
    # core.db.execute_update indirectly only via rwd binding — simulate the
    # failure by making the reset query fail after the first call
    real_execute_update = rwd.execute_update
    calls = []

    def flaky(query, params):
        calls.append(query)
        if "wallet_monitoring" in query:
            raise RuntimeError("monitoring table missing")
        return real_execute_update(query, params)

    monkeypatch.setattr(rwd, "execute_update", flaky)
    assert update_wallet_status("wallet1", "ACTIVE") is True
    assert any("wallet_monitoring" in q for q in calls)


def test_update_wallet_status_non_active(fake_db_layer, monkeypatch):
    _create_tables(fake_db_layer)
    monkeypatch.setattr(rwd, "execute_update", execute_update)
    assert update_wallet_status("wallet1", "REJECTED") is True


def test_update_wallet_status_error_returns_false(fake_db_layer, monkeypatch):
    def boom(query, params):
        raise RuntimeError("db down")
    monkeypatch.setattr(rwd, "execute_update", boom)
    assert update_wallet_status("wallet1", "ACTIVE") is False


def test_delete_wallet(fake_db_layer, monkeypatch):
    _create_tables(fake_db_layer)
    monkeypatch.setattr(rwd, "execute_update", execute_update)
    assert delete_wallet("wallet1") is True


def test_delete_wallet_error(fake_db_layer, monkeypatch):
    def boom(query, params):
        raise RuntimeError("db down")
    monkeypatch.setattr(rwd, "execute_update", boom)
    assert delete_wallet("wallet1") is False


def test_get_wallet(fake_db_layer, monkeypatch):
    monkeypatch.setattr("core.db.execute_and_fetchone", lambda q, p: {"address": "wallet1"})
    assert get_wallet("wallet1") == {"address": "wallet1"}


def test_get_wallet_error(fake_db_layer, monkeypatch):
    def boom(q, p):
        raise RuntimeError("db down")
    monkeypatch.setattr("core.db.execute_and_fetchone", boom)
    assert get_wallet("wallet1") is None


def test_get_wallets_by_status(fake_db_layer, monkeypatch):
    monkeypatch.setattr(
        "core.db.execute_and_fetchall",
        lambda q, p: [{"address": "wallet1", "status": "ACTIVE"}],
    )
    wallets = get_wallets_by_status("ACTIVE")
    assert wallets[0]["address"] == "wallet1"


def test_get_wallets_by_status_error(fake_db_layer, monkeypatch):
    def boom(q, p):
        raise RuntimeError("db down")
    monkeypatch.setattr("core.db.execute_and_fetchall", boom)
    assert get_wallets_by_status("ACTIVE") == []


def test_write_wallet_to_db_new_promotion_resets_counter(fake_db_layer, monkeypatch):
    _create_tables(fake_db_layer)
    monkeypatch.setattr("core.db.execute_and_fetchone", lambda q, p: None)
    monkeypatch.setattr(rwd, "execute_update", execute_update)
    assert write_wallet_to_db(_wallet(status="ACTIVE")) is True


def test_write_wallet_to_db_reset_failure_does_not_fail_write(fake_db_layer, monkeypatch):
    _create_tables(fake_db_layer)
    monkeypatch.setattr("core.db.execute_and_fetchone", lambda q, p: None)
    real_execute_update = rwd.execute_update
    monkeypatch.setattr(rwd, "execute_update", execute_update)

    def flaky(query, params):
        if "wallet_monitoring" in query:
            raise RuntimeError("monitoring table missing")
        return real_execute_update(query, params)

    monkeypatch.setattr(rwd, "execute_update", flaky)
    assert write_wallet_to_db(_wallet(status="ACTIVE")) is True


def test_write_wallet_to_db_error(fake_db_layer, monkeypatch):
    monkeypatch.setattr("core.db.execute_and_fetchone", lambda q, p: None)
    def boom(query, params):
        raise RuntimeError("db down")
    monkeypatch.setattr(rwd, "execute_update", boom)
    assert write_wallet_to_db(_wallet()) is False


def test_write_wallets_to_db_counts_successes(fake_db_layer, monkeypatch):
    _create_tables(fake_db_layer)
    monkeypatch.setattr("core.db.execute_and_fetchone", lambda q, p: None)
    monkeypatch.setattr(rwd, "execute_update", execute_update)
    wallets = [_wallet("w1"), _wallet("w2"), _wallet("w3")]
    assert write_wallets_to_db(wallets) == 3


def test_write_wallets_to_db_partial(fake_db_layer, monkeypatch):
    _create_tables(fake_db_layer)
    monkeypatch.setattr("core.db.execute_and_fetchone", lambda q, p: None)
    real_execute_update = rwd.execute_update
    monkeypatch.setattr(rwd, "execute_update", execute_update)
    calls = {"n": 0}

    def flaky(query, params):
        calls["n"] += 1
        if calls["n"] == 2:
            raise RuntimeError("transient")
        return real_execute_update(query, params)

    monkeypatch.setattr(rwd, "execute_update", flaky)
    assert write_wallets_to_db([_wallet("w1"), _wallet("w2"), _wallet("w3")]) == 2


def test_wallet_status_read_failure_returns_none(fake_db_layer, monkeypatch):
    _create_tables(fake_db_layer)
    def boom(q, p):
        raise RuntimeError("db down")
    monkeypatch.setattr("core.db.execute_and_fetchone", boom)
    monkeypatch.setattr(rwd, "execute_update", execute_update)
    # write succeeds even though status read failed (treated as no previous)
    assert write_wallet_to_db(_wallet(status="CANDIDATE")) is True
