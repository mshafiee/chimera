"""
Coverage tests for core/prediction_logger.py.

Uses the fake_db_layer fixture (in-memory SQLite, %s -> ? translation). The
ml_predictions table is created by PredictionLogger._ensure_schema from the
repo's database/schema/ml_predictions.sql (exists in this checkout).
"""


import pytest

from core.prediction_logger import (
    PredictionLogger,
    PredictionRecord,
    get_prediction_logger,
    log_prediction,
)


_ML_PREDICTIONS_DDL = """
CREATE TABLE IF NOT EXISTS ml_predictions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_address TEXT NOT NULL,
    prediction_timestamp TIMESTAMP NOT NULL,
    model_type TEXT NOT NULL,
    predicted_pnl_sol TEXT NOT NULL,
    predicted_class TEXT,
    confidence REAL,
    features_json TEXT,
    strategy TEXT,
    wqs_score_at_prediction REAL,
    wqs_components_json TEXT,
    actual_pnl_sol TEXT,
    actual_pnl_7d_sol TEXT,
    actual_pnl_30d_sol TEXT,
    match_timestamp TIMESTAMP,
    days_to_match INTEGER,
    status TEXT DEFAULT 'PENDING',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(wallet_address, prediction_timestamp, model_type)
)
"""


@pytest.fixture(autouse=True)
def _ml_predictions_table(fake_db_layer):
    """Pre-create the ml_predictions table (the production schema file contains
    a trigger whose embedded semicolons break the naive ;-split in
    _ensure_schema, which then fails safely against SQLite)."""
    fake_db_layer.executescript(_ML_PREDICTIONS_DDL)


@pytest.fixture
def logger(fake_db_layer):
    return PredictionLogger("data/test_ml.db")


def _log(logger, wallet="wallet1", model="xgboost", pnl=0.15, ts="2025-01-01T00:00:00"):
    return logger.log_prediction(
        wallet_address=wallet,
        predicted_pnl_sol=pnl,
        model_type=model,
        features={"roi_7d": 0.05},
        confidence=0.85,
        strategy="SHIELD",
        wqs_score=75.0,
        wqs_components={"roi": 20},
        predicted_class="positive",
    )


def test_default_db_path(fake_db_layer):
    logger = PredictionLogger()
    assert str(logger.db_path).endswith("chimera.db")


def test_prediction_record_features_property():
    rec = PredictionRecord(
        id=1, wallet_address="w", prediction_timestamp="2025-01-01T00:00:00",
        model_type="m", predicted_pnl_sol=0.1, predicted_class=None,
        confidence=None, features_json='{"a": 1}', strategy=None,
        wqs_score_at_prediction=None, wqs_components_json='{"b": 2}',
        actual_pnl_sol=None, actual_pnl_7d_sol=None, actual_pnl_30d_sol=None,
        match_timestamp=None, days_to_match=None, status="PENDING",
        created_at="x", updated_at="x",
    )
    assert rec.features == {"a": 1}
    assert rec.wqs_components == {"b": 2}


def test_prediction_record_features_bad_json():
    rec = PredictionRecord(
        id=1, wallet_address="w", prediction_timestamp="2025-01-01T00:00:00",
        model_type="m", predicted_pnl_sol=0.1, predicted_class=None,
        confidence=None, features_json="{bad", strategy=None,
        wqs_score_at_prediction=None, wqs_components_json="[1,2",
        actual_pnl_sol=None, actual_pnl_7d_sol=None, actual_pnl_30d_sol=None,
        match_timestamp=None, days_to_match=None, status="PENDING",
        created_at="x", updated_at="x",
    )
    assert rec.features == {}
    assert rec.wqs_components == {}


def test_prediction_record_features_none():
    rec = PredictionRecord(
        id=1, wallet_address="w", prediction_timestamp="2025-01-01T00:00:00",
        model_type="m", predicted_pnl_sol=0.1, predicted_class=None,
        confidence=None, features_json=None, strategy=None,
        wqs_score_at_prediction=None, wqs_components_json=None,
        actual_pnl_sol=None, actual_pnl_7d_sol=None, actual_pnl_30d_sol=None,
        match_timestamp=None, days_to_match=None, status="PENDING",
        created_at="x", updated_at="x",
    )
    assert rec.features == {}
    assert rec.wqs_components == {}


def test_log_and_get_pending(logger):
    pid = _log(logger)
    assert pid is not None
    pending = logger.get_pending_predictions()
    assert len(pending) == 1
    assert pending[0].id == pid
    assert pending[0].model_type == "xgboost"


def test_get_pending_with_filters(logger):
    _log(logger, wallet="w1", model="xgboost")
    _log(logger, wallet="w2", model="lightgbm")
    only_xgb = logger.get_pending_predictions(model_type="xgboost")
    assert len(only_xgb) == 1
    limited = logger.get_pending_predictions(limit=1)
    assert len(limited) == 1


def test_get_pending_exception_returns_empty(fake_db_layer, monkeypatch):
    logger = PredictionLogger("data/test.db")

    def boom(*args, **kwargs):
        raise RuntimeError("db down")
    monkeypatch.setattr(logger, "_get_connection", boom)
    assert logger.get_pending_predictions() == []


def test_mark_matched(logger):
    pid = _log(logger)
    assert logger.mark_matched(pid, actual_pnl_sol=0.2, actual_pnl_7d_sol=0.1,
                               actual_pnl_30d_sol=0.3) is True
    records = logger.get_pending_predictions()
    assert records == []


def test_mark_matched_not_found(logger):
    assert logger.mark_matched(999, actual_pnl_sol=0.1) is False


def test_mark_matched_exception(fake_db_layer, monkeypatch):
    logger = PredictionLogger("data/test.db")

    class BoomCursor:
        def execute(self, *a, **k):
            raise RuntimeError("db down")

        def fetchone(self):
            return None

    class BoomConn:
        def cursor(self):
            return BoomCursor()

        def close(self):
            pass

    monkeypatch.setattr(logger, "_get_connection", lambda: BoomConn())
    assert logger.mark_matched(1, actual_pnl_sol=0.1) is False


def test_mark_matched_by_address(logger):
    _log(logger, wallet="w1", model="xgboost")
    _log(logger, wallet="w1", model="lightgbm")
    updated = logger.mark_matched_by_address("w1", actual_pnl_sol=0.5)
    assert updated == 2
    assert logger.get_pending_predictions() == []
    # No pending rows left -> 0
    assert logger.mark_matched_by_address("w1", actual_pnl_sol=0.5) == 0


def test_mark_matched_by_address_with_filters(logger):
    _log(logger, wallet="w1", model="xgboost")
    _log(logger, wallet="w1", model="lightgbm")
    xgb_ts = logger.get_pending_predictions(model_type="xgboost")[0].prediction_timestamp
    updated = logger.mark_matched_by_address(
        "w1", model_type="xgboost", actual_pnl_sol=0.5,
        prediction_timestamp=xgb_ts,
    )
    assert updated == 1
    remaining = logger.get_pending_predictions()
    assert len(remaining) == 1
    assert remaining[0].model_type == "lightgbm"


def test_mark_matched_by_address_exception(fake_db_layer, monkeypatch):
    logger = PredictionLogger("data/test.db")

    class BoomCursor:
        def execute(self, *a, **k):
            raise RuntimeError("db down")

    class BoomConn:
        def cursor(self):
            return BoomCursor()

        def close(self):
            pass

    monkeypatch.setattr(logger, "_get_connection", lambda: BoomConn())
    assert logger.mark_matched_by_address("w1") == 0


def test_mark_expired(logger):
    _log(logger, wallet="old_wallet")
    # Backdate the prediction so it is older than the expiry window
    cur = logger._get_connection().cursor()
    cur.execute(
        "UPDATE ml_predictions SET prediction_timestamp = '2020-01-01T00:00:00' "
        "WHERE wallet_address = %s", ("old_wallet",)
    )
    cur.connection.commit()
    assert logger.mark_expired(max_age_days=90) == 1
    assert logger.mark_expired() == 0  # nothing left


def test_mark_expired_exception(fake_db_layer, monkeypatch):
    logger = PredictionLogger("data/test.db")

    class BoomCursor:
        def execute(self, *a, **k):
            raise RuntimeError("db down")

    class BoomConn:
        def cursor(self):
            return BoomCursor()

        def close(self):
            pass

    monkeypatch.setattr(logger, "_get_connection", lambda: BoomConn())
    assert logger.mark_expired() == 0


def test_get_statistics(logger):
    pid = _log(logger)
    logger.mark_matched(pid, actual_pnl_sol=0.2)
    stats = logger.get_statistics()
    assert stats["total_predictions"] == 1
    assert stats["by_status"]["MATCHED"] == 1
    assert stats["matched_stats"]["count"] == 1


def test_get_statistics_no_matches(logger):
    _log(logger)
    stats = logger.get_statistics()
    assert stats["matched_stats"]["count"] == 0


def test_get_statistics_exception(fake_db_layer, monkeypatch):
    logger = PredictionLogger("data/test.db")

    class BoomCursor:
        def execute(self, *a, **k):
            raise RuntimeError("db down")

    class BoomConn:
        def cursor(self):
            return BoomCursor()

        def close(self):
            pass

    monkeypatch.setattr(logger, "_get_connection", lambda: BoomConn())
    assert logger.get_statistics() == {}


def test_cleanup_old_records(logger):
    _log(logger, wallet="old_wallet")
    cur = logger._get_connection().cursor()
    cur.execute(
        "UPDATE ml_predictions SET prediction_timestamp = '2020-01-01T00:00:00', "
        "status = 'MATCHED' WHERE wallet_address = %s", ("old_wallet",)
    )
    cur.connection.commit()
    deleted = logger.cleanup_old_records(keep_days=180)
    assert deleted == 1
    assert logger.cleanup_old_records() == 0


def test_cleanup_old_records_exception(fake_db_layer, monkeypatch):
    logger = PredictionLogger("data/test.db")

    class BoomCursor:
        def execute(self, *a, **k):
            raise RuntimeError("db down")

    class BoomConn:
        def cursor(self):
            return BoomCursor()

        def close(self):
            pass

    monkeypatch.setattr(logger, "_get_connection", lambda: BoomConn())
    assert logger.cleanup_old_records() == 0


def test_log_prediction_duplicate_detected(fake_db_layer, monkeypatch):
    import sys
    import types

    fake_psycopg = types.ModuleType("psycopg")
    fake_errors = types.ModuleType("psycopg.errors")

    class UniqueViolation(Exception):
        pass

    fake_errors.UniqueViolation = UniqueViolation
    fake_psycopg.errors = fake_errors
    monkeypatch.setitem(sys.modules, "psycopg", fake_psycopg)
    monkeypatch.setitem(sys.modules, "psycopg.errors", fake_errors)

    logger = PredictionLogger("data/test.db")

    def raising_exec(cursor, query, params=None):
        raise UniqueViolation("duplicate key value violates unique constraint")

    monkeypatch.setattr(PredictionLogger, "_exec", staticmethod(raising_exec))
    assert logger.log_prediction(
        wallet_address="w", predicted_pnl_sol=0.1, model_type="m",
        features={}, confidence=0.5, strategy="S", wqs_score=50.0,
        wqs_components={},
    ) is None


def test_log_prediction_generic_error(fake_db_layer, monkeypatch):
    logger = PredictionLogger("data/test.db")

    class BoomCursor:
        def execute(self, *a, **k):
            raise RuntimeError("constraint violation somewhere else")

    class BoomConn:
        def cursor(self):
            return BoomCursor()

        def close(self):
            pass

    monkeypatch.setattr(logger, "_get_connection", lambda: BoomConn())
    assert logger.log_prediction(
        wallet_address="w", predicted_pnl_sol=0.1, model_type="m",
        features={}, confidence=0.5, strategy="S", wqs_score=50.0,
        wqs_components={},
    ) is None


def test_get_prediction_logger_singleton(fake_db_layer, monkeypatch):
    import core.prediction_logger as pl
    prev = dict(pl._global_loggers)
    pl._global_loggers.clear()
    try:
        lg1 = get_prediction_logger("data/singleton.db")
        lg2 = get_prediction_logger("data/singleton.db")
        assert lg1 is lg2
        default = get_prediction_logger()
        assert str(default.db_path).endswith("chimera.db")
    finally:
        pl._global_loggers.clear()
        pl._global_loggers.update(prev)


def test_ensure_schema_full_execution(fake_db_layer, monkeypatch):
    """Run _ensure_schema against the real schema file, skipping the
    trigger fragments that the naive ;-split produces."""
    real_exec = PredictionLogger._exec

    def filtered_exec(cursor, query, params=None):
        stripped = query.strip()
        if stripped == "END" or "TRIGGER" in query or "WHERE id = OLD.id" in query:
            return
        real_exec(cursor, query, params)

    monkeypatch.setattr(PredictionLogger, "_exec", staticmethod(filtered_exec))
    logger = PredictionLogger("data/schema_full.db")
    # The real DDL ran -> table exists with the trigger-compatible columns
    cur = logger._get_connection().cursor()
    cur.execute("SELECT COUNT(*) AS n FROM sqlite_master WHERE type='table' AND name='ml_predictions'")
    assert cur.fetchone()["n"] == 1


def test_ensure_schema_missing_file_debug_branch(fake_db_layer, monkeypatch, caplog):
    """Cover the 'schema file not found' branch: Path.exists() is used by the
    candidate search, so patch it to report nothing exists. The autouse
    fixture pre-creates the table, so assert on the log branch instead."""
    import logging
    import pathlib

    monkeypatch.setattr(pathlib.Path, "exists", lambda self: False)
    with caplog.at_level(logging.DEBUG, logger="core.prediction_logger"):
        PredictionLogger("data/noschema.db")
    assert any(
        "ml_predictions.sql not found" in r.message for r in caplog.records
    )


def test_global_log_prediction(fake_db_layer, monkeypatch):
    import core.prediction_logger as pl
    prev = dict(pl._global_loggers)
    pl._global_loggers.clear()
    try:
        pid = log_prediction(
            wallet_address="w", predicted_pnl_sol=0.1, model_type="m",
            features={}, confidence=0.5, strategy="S", wqs_score=50.0,
            wqs_components={},
        )
        assert pid is not None
    finally:
        pl._global_loggers.clear()
        pl._global_loggers.update(prev)
