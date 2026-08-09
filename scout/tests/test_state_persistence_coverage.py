"""Coverage completion tests for core/state_persistence.py."""

import time

from unittest.mock import MagicMock, patch

import pytest

from core.multitimeframe_discovery import DiscoveryTimeframe, MultiTimeframeResult, TimeframeResult
from core.state_persistence import (
    BudgetCategory,
    CreditHistory,
    PersistenceConfig,
    ROIMetrics,
    StatePersistence,
    WalletPerformance,
)


def make_credit(date="2026-01-01", total=100, remaining=60, ts=None):
    return CreditHistory(
        date=date,
        total_credits=total,
        credits_by_category={c.value: 10 for c in BudgetCategory},
        credits_remaining=remaining,
        day_of_month=1,
        timestamp=ts if ts is not None else time.time(),
    )


def make_wallet(address="wallet_1", ts=None):
    ts = ts if ts is not None else time.time()
    return WalletPerformance(
        wallet_address=address,
        wqs_score=70.0,
        total_trades=50,
        winning_trades=35,
        total_pnl=120.0,
        avg_pnl=2.4,
        win_rate=0.7,
        roi_score=1.2,
        first_seen=ts - 100,
        last_updated=ts,
    )


def make_roi(category="discovery", ts=None):
    return ROIMetrics(
        category=category,
        credits_consumed=30,
        value_generated=90.0,
        roi_score=3.0,
        operations_count=5,
        period_start=(ts or time.time()) - 3600,
        period_end=ts or time.time(),
    )


def make_mtf_result():
    deep = TimeframeResult(
        timeframe=DiscoveryTimeframe.DEEP,
        wallets_discovered=["w1", "w2"],
        wallet_quality_scores={"w1": 80.0, "w2": 60.0},
        credits_consumed=40,
        execution_time_seconds=5.0,
    )
    fast = TimeframeResult(
        timeframe=DiscoveryTimeframe.FAST,
        wallets_discovered=["w2"],
        wallet_quality_scores={"w2": 70.0},
        credits_consumed=20,
        execution_time_seconds=2.0,
    )
    return MultiTimeframeResult(
        timeframe_results={DiscoveryTimeframe.DEEP: deep, DiscoveryTimeframe.FAST: fast},
        combined_wallets=["w1", "w2"],
        combined_quality_scores={"w1": 80.0, "w2": 70.0},
        cross_timeframe_ranking=[("w1", 80.0), ("w2", 70.0)],
        deduplication_stats={"total_raw_wallets": 3, "deduplication_ratio": 0.667, "multi_timeframe_wallets": 1},
        total_credits_consumed=60,
        total_execution_time_seconds=7.0,
    )


@pytest.fixture
def sp(fake_db_layer, tmp_path):
    config = PersistenceConfig(db_path="test_persistence.db")
    return StatePersistence(config)


class TestInitAndSchema:
    def test_init_creates_tables(self, sp):
        with sp._get_connection() as conn:
            cursor = conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '%_history'"
            )
            names = {row["name"] for row in cursor}
        assert {"credit_history", "wallet_performance_history"} <= names

    def test_get_db_path(self, sp):
        assert sp._get_db_path().endswith("test_persistence.db")


class TestCreditHistory:
    def test_save_and_load(self, sp):
        credit = make_credit()
        sp.save_credit_history(credit)
        records = sp.load_credit_history(days=30)
        assert len(records) == 1
        assert records[0].date == "2026-01-01"
        assert records[0].credits_by_category["discovery"] == 10

    def test_save_upsert_same_date(self, sp):
        sp.save_credit_history(make_credit(total=100))
        sp.save_credit_history(make_credit(total=150, remaining=80))
        records = sp.load_credit_history(days=30)
        assert len(records) == 1
        assert records[0].total_credits == 150

    def test_load_respects_cutoff(self, sp):
        old_ts = time.time() - 60 * 86400
        sp.save_credit_history(make_credit(date="old", ts=old_ts))
        sp.save_credit_history(make_credit(date="new", ts=time.time()))
        assert len(sp.load_credit_history(days=30)) == 1


class TestWalletPerformance:
    def test_save_and_load_one(self, sp):
        sp.save_wallet_performance(make_wallet("w1"))
        records = sp.load_wallet_performance("w1")
        assert len(records) == 1
        assert records["w1"].win_rate == 0.7

    def test_save_and_load_all(self, sp):
        sp.save_wallet_performance(make_wallet("w1"))
        sp.save_wallet_performance(make_wallet("w2"))
        records = sp.load_wallet_performance()
        assert len(records) == 2

    def test_upsert(self, sp):
        sp.save_wallet_performance(make_wallet("w1", ts=time.time()))
        sp.save_wallet_performance(make_wallet("w1", ts=time.time() + 5))
        records = sp.load_wallet_performance("w1")
        assert records["w1"].last_updated == pytest.approx(time.time() + 5, abs=2.0)

    def test_load_missing_wallet(self, sp):
        assert sp.load_wallet_performance("missing") == {}


class TestRoiMetrics:
    def test_save_and_load(self, sp):
        sp.save_roi_metrics(make_roi("discovery"))
        records = sp.load_roi_metrics()
        assert len(records) == 1
        assert records[0].category == "discovery"

    def test_load_by_category(self, sp):
        sp.save_roi_metrics(make_roi("discovery"))
        sp.save_roi_metrics(make_roi("analysis"))
        records = sp.load_roi_metrics(category="analysis")
        assert len(records) == 1
        assert records[0].category == "analysis"

    def test_load_cutoff(self, sp):
        sp.save_roi_metrics(make_roi(ts=time.time() - 60 * 86400))
        with sp._get_connection() as conn:
            conn.execute("UPDATE roi_metrics SET timestamp = %s", (time.time() - 60 * 86400,))
        assert sp.load_roi_metrics(days=30) == []


class TestMultiTimeframeStats:
    def test_save_and_load(self, sp):
        result = make_mtf_result()
        sp.save_multi_timeframe_discovery_stats(result, parallel=True, discovery_goal="quality")
        records = sp.load_multi_timeframe_discovery_stats(days=30)
        assert len(records) == 1
        assert records[0]["deep_wallets_discovered"] == 2
        assert records[0]["fast_wallets_discovered"] == 1
        assert records[0]["parallel_execution"] is True
        assert records[0]["discovery_goal"] == "quality"
        assert records[0]["cross_timeframe_quality_avg"] == pytest.approx(75.0)

    def test_save_without_timeframe_results(self, sp):
        result = MultiTimeframeResult(
            timeframe_results={},
            combined_wallets=[],
            combined_quality_scores={},
            cross_timeframe_ranking=[],
            deduplication_stats={},
            total_credits_consumed=0,
            total_execution_time_seconds=0.0,
        )
        sp.save_multi_timeframe_discovery_stats(result)
        records = sp.load_multi_timeframe_discovery_stats(days=30)
        assert records[0]["deep_wallets_discovered"] == 0
        assert records[0]["cross_timeframe_quality_avg"] == 0.0

    def test_load_cutoff(self, sp):
        result = make_mtf_result()
        result.timestamp = time.time() - 60 * 86400
        sp.save_multi_timeframe_discovery_stats(result)
        assert sp.load_multi_timeframe_discovery_stats(days=30) == []

    def test_get_multi_timeframe_summary(self, sp):
        result = make_mtf_result()
        sp.save_multi_timeframe_discovery_stats(result)
        sp.save_multi_timeframe_discovery_stats(result)
        summary = sp.get_multi_timeframe_summary(days=30)
        assert summary["total_runs"] == 2
        assert summary["avg_unique_wallets"] == 2
        assert summary["avg_deduplication_ratio"] == pytest.approx(0.667)
        assert summary["avg_cross_timeframe_quality"] == pytest.approx(75.0)

    def test_get_multi_timeframe_summary_empty(self, sp):
        summary = sp.get_multi_timeframe_summary(days=30)
        assert summary["total_runs"] == 0
        assert summary["avg_unique_wallets"] == 0


class TestCreditSummary:
    def test_get_credit_summary(self, sp):
        sp.save_credit_history(make_credit(total=100, remaining=60))
        sp.save_credit_history(make_credit(date="2026-01-02", total=200, remaining=150, ts=time.time() + 5))
        summary = sp.get_credit_summary(days=7)
        assert summary["period_days"] == 7
        assert summary["total_credits"] == 300
        assert summary["by_category"]["discovery"] == 20
        assert summary["max_daily"] == 200
        assert summary["min_daily"] == 100

    def test_get_credit_summary_empty(self, sp):
        summary = sp.get_credit_summary(days=7)
        assert summary["total_credits"] == 0


class TestCleanup:
    def test_cleanup_old_history(self, sp):
        old_ts = time.time() - 200 * 86400
        sp.save_credit_history(make_credit(date="old", ts=old_ts))
        sp.save_credit_history(make_credit(date="new", ts=time.time()))
        sp.save_roi_metrics(make_roi(ts=old_ts))
        with sp._get_connection() as conn:
            conn.execute("UPDATE roi_metrics SET timestamp = %s", (old_ts,))
        deleted = sp.cleanup_old_history()
        assert deleted == 2
        assert len(sp.load_credit_history(days=30)) == 1

    def test_vacuum_database(self, sp):
        sp.vacuum_database()
        assert sp._get_db_path()


class TestBackup:
    def test_backup_requires_database_url(self, sp, monkeypatch):
        monkeypatch.delenv("DATABASE_URL", raising=False)
        with pytest.raises(ValueError, match="DATABASE_URL is required"):
            sp.backup_database("backup.sql")

    def test_backup_success_with_path(self, sp, monkeypatch):
        monkeypatch.setenv("DATABASE_URL", "postgresql://u:p@h/db")
        fake_result = MagicMock(returncode=0, stderr="")
        with patch("subprocess.run", return_value=fake_result) as mock_run:
            path = sp.backup_database("backup.sql")
        assert path == "backup.sql"
        mock_run.assert_called_once()

    def test_backup_default_path(self, sp, monkeypatch):
        monkeypatch.setenv("DATABASE_URL", "postgresql://u:p@h/db")
        fake_result = MagicMock(returncode=0, stderr="")
        with patch("subprocess.run", return_value=fake_result):
            path = sp.backup_database()
        assert path.endswith(".sql")
        assert "scout_persistence_backup_" in path

    def test_backup_pgdump_fails(self, sp, monkeypatch):
        monkeypatch.setenv("DATABASE_URL", "postgresql://u:p@h/db")
        fake_result = MagicMock(returncode=1, stderr="boom")
        with patch("subprocess.run", return_value=fake_result):
            with pytest.raises(RuntimeError, match="pg_dump failed"):
                sp.backup_database("backup.sql")

    def test_backup_pgdump_missing(self, sp, monkeypatch):
        monkeypatch.setenv("DATABASE_URL", "postgresql://u:p@h/db")
        with patch("subprocess.run", side_effect=FileNotFoundError):
            with pytest.raises(RuntimeError, match="pg_dump not found"):
                sp.backup_database("backup.sql")


class FakeStatsConn:
    """Connection stand-in returning COUNT(*) rows keyed as 'count'."""

    def __init__(self, conn):
        self._real = conn

    class FakeCursor:
        def fetchone(self):
            return {"count": 1}

    def execute(self, sql, params=None):
        return self.FakeCursor()

    def commit(self):
        return None

    def close(self):
        return None


class TestDatabaseStats:
    def test_get_database_stats(self, sp, monkeypatch):
        import importlib

        mod = importlib.import_module(sp._get_connection.__func__.__module__)
        monkeypatch.setattr(mod, "get_connection", lambda *a, **k: FakeStatsConn(None))
        sp.save_credit_history(make_credit())
        sp.save_wallet_performance(make_wallet("w1"))
        sp.save_roi_metrics(make_roi())
        stats = sp.get_database_stats()
        assert stats["credit_history_records"] == 1
        assert stats["wallet_performance_records"] == 1
        assert stats["roi_metrics_records"] == 1
        assert stats["multi_timeframe_discovery_records"] == 1
        assert stats["total_records"] == 4
        assert stats["database_size_mb"] >= 0


class TestImportFallback:
    """Reloads the module with a poisoned multitimeframe import — must run last."""

    def test_import_fallback_sets_none(self, monkeypatch):
        import importlib
        import sys

        # Another suite may have unloaded the module (reload-based tests pop
        # sys.modules entries) — ensure it is present before reloading.
        if "core.state_persistence" not in sys.modules:
            importlib.import_module("core.state_persistence")
        import core.state_persistence as sp

        # The dual core.* / scout.core.* import topology can register a
        # different module object than `import` returns; reload must target
        # the object actually registered in sys.modules.
        registered = sys.modules.get("core.state_persistence")
        if registered is not None:
            sp = registered

        monkeypatch.setitem(sys.modules, "core.multitimeframe_discovery", None)
        importlib.reload(sp)
        try:
            assert sp.DiscoveryTimeframe is None
            assert sp.MultiTimeframeResult is None
        finally:
            monkeypatch.undo()
            importlib.reload(sp)
            assert sp.DiscoveryTimeframe is not None
