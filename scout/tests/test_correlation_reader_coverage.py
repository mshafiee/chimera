"""Coverage completion tests for core/correlation_reader.py."""

import json

from types import SimpleNamespace

import pytest

from tests.conftest import _SqliteConn
from core.correlation_reader import CorrelationReader, CorrelationStats


def patch_execute_query(monkeypatch, fake):
    """Patch execute_query in BOTH module copies (core.* and scout.core.*)."""
    import importlib

    for pkg_name in ("core", "scout.core"):
        try:
            mod = importlib.import_module(f"{pkg_name}.correlation_reader")
            monkeypatch.setattr(mod, "execute_query", fake)
        except Exception:
            pass


def make_table_exists_query(monkeypatch, exists=True):
    """Route information_schema queries to a fake cursor; SQLite has none."""
    from core import correlation_reader as cr

    real_execute_query = cr.execute_query

    def fake_execute_query(conn, query, params=None, cursor=None):
        if "information_schema" in query:
            return SimpleNamespace(fetchone=lambda: ("1",) if exists else None)
        return real_execute_query(conn, query, params, cursor)

    patch_execute_query(monkeypatch, fake_execute_query)


def wrap(fake_conn):
    """Wrap the raw sqlite3 connection so %s placeholders are translated."""
    return _SqliteConn(fake_conn)


def create_table(conn):
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS wqs_pnl_correlation (
            wallet_address TEXT PRIMARY KEY,
            wqs_score_at_promotion REAL NOT NULL,
            actual_copy_pnl_7d_sol REAL,
            actual_copy_pnl_30d_sol REAL,
            actual_copy_pnl_all_sol REAL,
            copy_trade_count_7d INTEGER DEFAULT 0,
            copy_trade_count_30d INTEGER DEFAULT 0,
            copy_trade_count_all INTEGER DEFAULT 0,
            strategy TEXT NOT NULL,
            wqs_components_json TEXT,
            promoted_at TEXT NOT NULL,
            last_updated_at TEXT NOT NULL
        )
        """
    )


def insert_row(raw_conn, **overrides):
    conn = wrap(raw_conn)
    row = {
        "wallet_address": "wallet_1",
        "wqs_score_at_promotion": 60.0,
        "actual_copy_pnl_7d_sol": 1.0,
        "actual_copy_pnl_30d_sol": 2.0,
        "actual_copy_pnl_all_sol": 3.0,
        "copy_trade_count_7d": 5,
        "copy_trade_count_30d": 10,
        "copy_trade_count_all": 20,
        "strategy": "SHIELD",
        "wqs_components_json": json.dumps({"roi_score": 1.5, "win_rate_score": 1.2}),
        "promoted_at": "2026-01-01T00:00:00Z",
        "last_updated_at": "2026-01-02T00:00:00Z",
    }
    row.update(overrides)
    columns = ", ".join(row.keys())
    placeholders = ", ".join(["%s"] * len(row))
    conn.execute(
        f"INSERT INTO wqs_pnl_correlation ({columns}) VALUES ({placeholders})",
        tuple(row.values()),
    )
    return row


class TestTableExists:
    def test_table_exists_true(self, fake_db_layer, monkeypatch):
        create_table(fake_db_layer)
        make_table_exists_query(monkeypatch, exists=True)
        assert CorrelationReader().table_exists() is True

    def test_table_exists_false(self, fake_db_layer):
        assert CorrelationReader().table_exists() is False

    def test_table_exists_exception(self, fake_db_layer, monkeypatch):
        from core import correlation_reader as cr

        def boom(*args, **kwargs):
            raise RuntimeError("db down")

        monkeypatch.setattr(cr, "Connection", boom)
        assert CorrelationReader().table_exists() is False


class TestGetAllRecords:
    def test_returns_records(self, fake_db_layer):
        create_table(fake_db_layer)
        insert_row(fake_db_layer, wallet_address="w1", strategy="SHIELD",
                   promoted_at="2026-01-01T00:00:00Z")
        insert_row(fake_db_layer, wallet_address="w2", strategy="SPEAR",
                   promoted_at="2026-01-02T00:00:00Z")
        records = CorrelationReader().get_all_records()
        assert len(records) == 2
        assert records[0].wallet_address == "w2"
        assert records[0].strategy == "SPEAR"
        assert records[0].copy_trade_count_all == 20

    def test_strategy_filter(self, fake_db_layer):
        create_table(fake_db_layer)
        insert_row(fake_db_layer, wallet_address="w1", strategy="SHIELD")
        insert_row(fake_db_layer, wallet_address="w2", strategy="SPEAR")
        records = CorrelationReader().get_all_records(strategy="SHIELD")
        assert len(records) == 1
        assert records[0].wallet_address == "w1"

    def test_min_trades_filter(self, fake_db_layer):
        create_table(fake_db_layer)
        insert_row(fake_db_layer, wallet_address="w1", copy_trade_count_all=0)
        records = CorrelationReader().get_all_records(min_trades=1)
        assert records == []

    def test_null_counts_default_zero(self, fake_db_layer):
        create_table(fake_db_layer)
        insert_row(fake_db_layer, wallet_address="w1", copy_trade_count_7d=None)
        records = CorrelationReader().get_all_records()
        assert records[0].copy_trade_count_7d == 0

    def test_exception_returns_empty(self, fake_db_layer, monkeypatch):
        from core import correlation_reader as cr

        def boom(*args, **kwargs):
            raise RuntimeError("db down")

        monkeypatch.setattr(cr, "Connection", boom)
        assert CorrelationReader().get_all_records() == []


class TestCorrelationStats:
    def test_empty_records_returns_none(self, fake_db_layer):
        create_table(fake_db_layer)
        assert CorrelationReader().get_correlation_stats() is None

    def test_stats_computed(self, fake_db_layer):
        create_table(fake_db_layer)
        insert_row(fake_db_layer, wallet_address="w1", strategy="SHIELD",
                   actual_copy_pnl_7d_sol=1.0, actual_copy_pnl_30d_sol=5.0,
                   actual_copy_pnl_all_sol=6.0, wqs_score_at_promotion=60.0)
        insert_row(fake_db_layer, wallet_address="w2", strategy="SHIELD",
                   actual_copy_pnl_7d_sol=3.0, actual_copy_pnl_30d_sol=None,
                   actual_copy_pnl_all_sol=2.0, wqs_score_at_promotion=70.0)
        insert_row(fake_db_layer, wallet_address="w3", strategy="SPEAR",
                   actual_copy_pnl_7d_sol=None, actual_copy_pnl_30d_sol=-1.0,
                   actual_copy_pnl_all_sol=None, wqs_score_at_promotion=50.0)
        stats = CorrelationReader().get_correlation_stats()
        assert isinstance(stats, CorrelationStats)
        assert stats.total_wallets == 3
        assert stats.wallets_with_pnl == 2
        assert stats.mean_pnl_7d_sol == 2.0
        assert stats.mean_pnl_30d_sol == 2.0
        assert stats.mean_wqs_at_promotion == 60.0
        shield = stats.strategy_breakdown["SHIELD"]
        assert shield["count"] == 2
        assert shield["n_with_pnl"] == 1
        assert shield["mean_pnl_30d"] == 5.0
        assert shield["profit_rate"] == 1.0
        spear = stats.strategy_breakdown["SPEAR"]
        assert spear["mean_pnl_30d"] == -1.0
        assert spear["profit_rate"] == 0.0

    def test_stats_with_no_pnl_values(self, fake_db_layer):
        create_table(fake_db_layer)
        insert_row(fake_db_layer, wallet_address="w1", actual_copy_pnl_7d_sol=None,
                   actual_copy_pnl_30d_sol=None)
        stats = CorrelationReader().get_correlation_stats()
        assert stats is not None
        assert stats.mean_pnl_7d_sol == 0.0
        assert stats.mean_pnl_30d_sol == 0.0
        assert stats.strategy_breakdown["SHIELD"]["mean_pnl_30d"] == 0.0

    def test_stats_strategy_filter(self, fake_db_layer):
        create_table(fake_db_layer)
        insert_row(fake_db_layer, wallet_address="w1", strategy="SHIELD")
        insert_row(fake_db_layer, wallet_address="w2", strategy="SPEAR")
        stats = CorrelationReader().get_correlation_stats(strategy="SPEAR")
        assert stats.total_wallets == 1
        assert "SPEAR" in stats.strategy_breakdown


class TestTopComponentPredictors:
    def test_insufficient_records(self, fake_db_layer):
        create_table(fake_db_layer)
        insert_row(fake_db_layer, wallet_address="w1")
        assert CorrelationReader().get_top_component_predictors(min_samples=5) == []

    def test_predictors_ranked(self, fake_db_layer):
        create_table(fake_db_layer)
        for i in range(5):
            insert_row(
                fake_db_layer,
                wallet_address=f"w{i}",
                actual_copy_pnl_30d_sol=float(i),
                wqs_components_json=json.dumps({"roi_score": float(i), "activity": 5.0 - i}),
            )
        insert_row(
            fake_db_layer,
            wallet_address="w_solo",
            actual_copy_pnl_30d_sol=1.0,
            wqs_components_json=json.dumps({"solo_component": 9.0}),
        )
        results = CorrelationReader().get_top_component_predictors(min_samples=3)
        assert len(results) == 2
        assert results[0]["component"] == "roi_score"
        assert abs(results[0]["correlation"] - 1.0) < 1e-6
        assert all(r["component"] != "solo_component" for r in results)

    def test_skips_null_pnl_and_corrupt_json(self, fake_db_layer):
        create_table(fake_db_layer)
        for i in range(6):
            kwargs = {
                "wallet_address": f"w{i}",
                "actual_copy_pnl_30d_sol": float(i),
                "wqs_components_json": json.dumps({"roi_score": float(i)}),
            }
            if i == 0:
                kwargs["actual_copy_pnl_30d_sol"] = None
            if i == 1:
                kwargs["wqs_components_json"] = "{corrupt"
            if i == 2:
                kwargs["wqs_components_json"] = "[1,2,3]"
            if i == 3:
                kwargs["wqs_components_json"] = json.dumps({"str_val": "x"})
            insert_row(fake_db_layer, **kwargs)
        results = CorrelationReader().get_top_component_predictors(min_samples=2)
        assert any(r["component"] == "roi_score" for r in results)


class TestPearson:
    def test_too_few_samples(self):
        assert CorrelationReader._pearson_correlation([1.0], [2.0]) == 0.0

    def test_zero_variance(self):
        assert CorrelationReader._pearson_correlation([1.0, 1.0, 1.0], [1.0, 2.0, 3.0]) == 0.0

    def test_perfect_correlation(self):
        corr = CorrelationReader._pearson_correlation([1.0, 2.0, 3.0], [2.0, 4.0, 6.0])
        assert abs(corr - 1.0) < 1e-9

    def test_unequal_lengths(self):
        corr = CorrelationReader._pearson_correlation([1.0, 2.0, 3.0, 4.0], [2.0, 4.0, 6.0])
        assert corr == pytest.approx(0.5070925528371099)


class TestPrintSummary:
    def test_no_table(self, fake_db_layer, capsys):
        CorrelationReader().print_correlation_summary()
        out = capsys.readouterr().out
        assert "does not exist" in out

    def test_empty_table(self, fake_db_layer, capsys, monkeypatch):
        create_table(fake_db_layer)
        make_table_exists_query(monkeypatch, exists=True)
        CorrelationReader().print_correlation_summary()
        out = capsys.readouterr().out
        assert "no data" in out

    def test_with_data(self, fake_db_layer, capsys, monkeypatch):
        create_table(fake_db_layer)
        make_table_exists_query(monkeypatch, exists=True)
        insert_row(fake_db_layer, wallet_address="w1", strategy="SHIELD",
                   actual_copy_pnl_7d_sol=1.0, actual_copy_pnl_30d_sol=2.0,
                   wqs_components_json=json.dumps({"roi_score": 1.0, "activity": 0.5}))
        insert_row(fake_db_layer, wallet_address="w2", strategy="SPEAR",
                   actual_copy_pnl_7d_sol=3.0, actual_copy_pnl_30d_sol=4.0,
                   wqs_components_json=json.dumps({"roi_score": 2.0, "activity": 1.5}))
        insert_row(fake_db_layer, wallet_address="w3", strategy="SHIELD",
                   actual_copy_pnl_7d_sol=5.0, actual_copy_pnl_30d_sol=6.0,
                   wqs_components_json=json.dumps({"roi_score": 3.0, "activity": 2.5}))
        insert_row(fake_db_layer, wallet_address="w4", strategy="SHIELD",
                   actual_copy_pnl_7d_sol=7.0, actual_copy_pnl_30d_sol=8.0,
                   wqs_components_json=json.dumps({"roi_score": 4.0, "activity": 3.5}))
        insert_row(fake_db_layer, wallet_address="w5", strategy="SHIELD",
                   actual_copy_pnl_7d_sol=9.0, actual_copy_pnl_30d_sol=10.0,
                   wqs_components_json=json.dumps({"roi_score": 5.0, "activity": 4.5}))
        CorrelationReader().print_correlation_summary()
        out = capsys.readouterr().out
        assert "Total wallets tracked: 5" in out
        assert "SHIELD" in out
        assert "Top WQS component predictors" in out
