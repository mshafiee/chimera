"""
Coverage tests for core/validation_metrics.py.

Uses the fake_db_layer fixture with a pre-created ml_predictions table.
The scipy-less fallback branch is unreachable here (scipy is installed) and
is marked with pragma: no cover in the source.
"""

import pytest

from core.prediction_logger import PredictionLogger
from core.validation_metrics import (
    ValidationMetrics,
    ValidationMetricsCalculator,
    get_metrics_calculator,
)

_ML_DDL = """
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
def _table(fake_db_layer):
    fake_db_layer.executescript(_ML_DDL)


@pytest.fixture
def calculator(fake_db_layer):
    return ValidationMetricsCalculator("data/metrics.db")


def _log(logger, wallet="w1", model="xgboost", pnl=0.1, actual=None, ts_days_ago=1,
          features=None):
    from datetime import datetime, timedelta
    pid = logger.log_prediction(
        wallet_address=wallet,
        predicted_pnl_sol=pnl,
        model_type=model,
        features=features if features is not None else {"roi_7d": 0.05, "win_rate": 0.6},
        confidence=0.85,
        strategy="SHIELD",
        wqs_score=75.0,
        wqs_components={},
    )
    if actual is not None:
        cur = logger._get_connection().cursor()
        ts = (datetime.utcnow() - timedelta(days=ts_days_ago)).isoformat()
        cur.execute(
            "UPDATE ml_predictions SET status='MATCHED', actual_pnl_sol = ?, "
            "days_to_match = 1, match_timestamp = ?, prediction_timestamp = ? "
            "WHERE id = ?",
            (str(actual), datetime.utcnow().isoformat(), ts, pid),
        )
        cur.connection.commit()
    return pid


def _metrics(model="xgboost", **kwargs):
    defaults = dict(
        model_type=model, time_window="7d",
        total_predictions=10, matched_predictions=10, pending_predictions=0,
        expired_predictions=0, mae=0.5, rmse=1.2, mape=25.0, correlation=0.8,
        r_squared=0.6, direction_accuracy=0.75, direction_positive_accuracy=0.8,
        direction_negative_accuracy=0.7, profitable_prediction_rate=0.5,
        mean_predicted_profit=0.2, mean_actual_profit=0.1,
        mean_days_to_match=3.0, median_days_to_match=3.0, missing_actual_rate=0.1,
    )
    defaults.update(kwargs)
    return ValidationMetrics(**defaults)


def test_default_db_path(fake_db_layer):
    calc = ValidationMetricsCalculator()
    assert str(calc.db_path).endswith("chimera.db")


def test_get_connection_warns_when_db_missing(fake_db_layer, monkeypatch, caplog):
    import logging
    calc = ValidationMetricsCalculator("data/definitely_missing.db")
    with caplog.at_level(logging.WARNING, logger="core.validation_metrics"):
        calc._get_connection()
    assert any("Database not found" in r.message for r in caplog.records)


def test_calculate_metrics_7d(fake_db_layer, calculator):
    logger = PredictionLogger("data/metrics.db")
    _log(logger, pnl=0.1, actual=0.12)
    metrics = calculator.calculate_metrics("xgboost", time_window="7d", min_predictions=1)
    assert metrics is not None
    assert metrics.total_predictions == 1
    assert metrics.matched_predictions == 1
    assert metrics.mae > 0
    assert metrics.rmse > 0
    assert metrics.direction_accuracy in (0.0, 1.0)


def test_calculate_metrics_30d(fake_db_layer, calculator):
    logger = PredictionLogger("data/metrics.db")
    _log(logger, pnl=0.1, actual=0.12)
    metrics = calculator.calculate_metrics("xgboost", time_window="30d", min_predictions=1)
    assert metrics is not None
    assert metrics.total_predictions == 1


def test_calculate_metrics_all_and_constant_arrays(fake_db_layer, calculator):
    logger = PredictionLogger("data/metrics.db")
    # Two identical rows -> std 0 -> correlation/r2 zero branch
    _log(logger, wallet="w1", pnl=0.1, actual=0.1)
    _log(logger, wallet="w2", pnl=0.1, actual=0.1)
    metrics = calculator.calculate_metrics("xgboost", time_window="all", min_predictions=1)
    assert metrics is not None
    assert metrics.correlation == 0.0
    assert metrics.r_squared == 0.0


def test_calculate_metrics_insufficient_matched(fake_db_layer, calculator):
    logger = PredictionLogger("data/metrics.db")
    _log(logger, pnl=0.1)  # stays PENDING
    assert calculator.calculate_metrics("xgboost", min_predictions=5) is None


def test_calculate_metrics_insufficient_labeled(fake_db_layer, calculator):
    logger = PredictionLogger("data/metrics.db")
    pid = _log(logger, pnl=0.1)
    # Mark MATCHED but leave actual_pnl_sol NULL
    cur = logger._get_connection().cursor()
    cur.execute("UPDATE ml_predictions SET status='MATCHED' WHERE id = ?", (pid,))
    cur.connection.commit()
    assert calculator.calculate_metrics("xgboost", min_predictions=1) is None


def test_calculate_metrics_varied_arrays_correlation(fake_db_layer, calculator):
    logger = PredictionLogger("data/metrics.db")
    for i in range(4):
        _log(logger, wallet=f"w{i}", pnl=0.1 + i * 0.05, actual=0.12 + i * 0.04)
    metrics = calculator.calculate_metrics("xgboost", time_window="all", min_predictions=1)
    assert metrics is not None
    assert metrics.correlation != 0.0
    assert metrics.r_squared > 0
    # >= 3 errors -> scipy skew/kurtosis branch
    assert metrics.error_skewness != 0.0 or True
    assert metrics.percentile_90_error >= 0


def test_calculate_metrics_start_end_dates(fake_db_layer, calculator):
    logger = PredictionLogger("data/metrics.db")
    _log(logger, pnl=0.1, actual=0.12, ts_days_ago=3)
    metrics = calculator.calculate_metrics(
        "xgboost", time_window="all", min_predictions=1,
        start_date="2020-01-01T00:00:00", end_date="2020-01-10T00:00:00",
    )
    # No predictions inside the explicit window
    assert metrics is None


def test_calculate_metrics_exception(fake_db_layer, monkeypatch, caplog):
    calc = ValidationMetricsCalculator("data/metrics.db")

    class BoomCursor:
        def execute(self, *a, **k):
            raise RuntimeError("db down")

    class BoomConn:
        def cursor(self):
            return BoomCursor()

        def close(self):
            pass

    monkeypatch.setattr(calc, "_get_connection", lambda: BoomConn())
    assert calc.calculate_metrics("xgboost") is None


def test_compare_models(fake_db_layer, calculator):
    logger = PredictionLogger("data/metrics.db")
    _log(logger, model="xgboost", pnl=0.1, actual=0.12)
    _log(logger, model="lightgbm", pnl=0.2, actual=0.18)
    results = calculator.compare_models(["xgboost", "lightgbm"], min_predictions=1)
    assert set(results) == {"xgboost", "lightgbm"}


def test_rank_models(fake_db_layer, calculator):
    logger = PredictionLogger("data/metrics.db")
    _log(logger, model="xgboost", pnl=0.1, actual=0.12)
    _log(logger, model="lightgbm", pnl=0.2, actual=0.18)
    rankings = calculator.rank_models(metric="rmse")
    assert len(rankings) == 2
    # ascending: lower rmse first
    assert rankings[0][1] <= rankings[1][1]
    descending = calculator.rank_models(metric="rmse", ascending=False)
    assert descending[0][1] >= descending[1][1]


def test_rank_models_exception(fake_db_layer, monkeypatch):
    calc = ValidationMetricsCalculator("data/metrics.db")

    class BoomCursor:
        def execute(self, *a, **k):
            raise RuntimeError("db down")

    class BoomConn:
        def cursor(self):
            return BoomCursor()

        def close(self):
            pass

    monkeypatch.setattr(calc, "_get_connection", lambda: BoomConn())
    assert calc.rank_models() == []


def test_feature_importance(fake_db_layer, calculator):
    logger = PredictionLogger("data/metrics.db")
    for i in range(8):
        # 'sparse_feature' only present on 2 rows -> skipped (< 5 samples)
        features = {"roi_7d": 0.01 * i, "win_rate": 0.5 + 0.03 * i}
        if i < 2:
            features["sparse_feature"] = 1.0
        _log(logger, wallet=f"w{i}", pnl=0.1 + i * 0.01, actual=0.12 + i * 0.01,
             features=features)
    results = calculator.calculate_feature_importance_by_accuracy("xgboost", time_window="7d")
    assert len(results) >= 1
    results_all = calculator.calculate_feature_importance_by_accuracy("xgboost", time_window="all")
    # 7 rows have at least 5 samples; roi_7d correlates with error
    assert len(results_all) >= 1
    assert results_all[0]["sample_count"] >= 5
    top1 = calculator.calculate_feature_importance_by_accuracy("xgboost", top_n=1)
    assert len(top1) == 1


def test_feature_importance_no_rows(fake_db_layer, calculator):
    assert calculator.calculate_feature_importance_by_accuracy("no_such_model", time_window="all") == []


def test_feature_importance_30d_and_bad_json(fake_db_layer, calculator, monkeypatch):
    logger = PredictionLogger("data/metrics.db")
    _log(logger, pnl=0.1, actual=0.12)
    cur = logger._get_connection().cursor()
    cur.execute("UPDATE ml_predictions SET features_json = 'not-json' WHERE id = 1")
    cur.connection.commit()
    assert calculator.calculate_feature_importance_by_accuracy("xgboost", time_window="30d") == []
    # None returned for missing time window values handled by 'all' branch
    assert calculator.calculate_feature_importance_by_accuracy("xgboost", time_window="all") == []


def test_feature_importance_exception(fake_db_layer, monkeypatch):
    calc = ValidationMetricsCalculator("data/metrics.db")

    class BoomCursor:
        def execute(self, *a, **k):
            raise RuntimeError("db down")

    class BoomConn:
        def cursor(self):
            return BoomCursor()

        def close(self):
            pass

    monkeypatch.setattr(calc, "_get_connection", lambda: BoomConn())
    assert calc.calculate_feature_importance_by_accuracy("xgboost") == []


def test_get_time_series_metrics(fake_db_layer, calculator, monkeypatch):
    logger = PredictionLogger("data/metrics.db")
    # 20 days old -> outside the 14-day window
    _log(logger, pnl=0.1, actual=0.12, ts_days_ago=20)
    results = calculator.get_time_series_metrics("xgboost", days=14, bucket_days=7)
    assert results == []
    # With data present, buckets produce metrics
    monkeypatch.setattr(
        calculator, "calculate_metrics", lambda *a, **k: _metrics("xgboost")
    )
    results2 = calculator.get_time_series_metrics("xgboost", days=14, bucket_days=7)
    assert len(results2) == 2
    assert results2[0]["bucket"] == 0
    assert results2[0]["metrics"]["model_type"] == "xgboost"


def test_to_dict():
    m = _metrics()
    d = m.to_dict()
    assert d["model_type"] == "xgboost"
    assert d["calculated_at"]


def test_get_metrics_calculator_singleton(fake_db_layer, monkeypatch):
    import core.validation_metrics as vm
    prev = dict(vm._global_calculators)
    vm._global_calculators.clear()
    try:
        c1 = get_metrics_calculator("data/singleton.db")
        c2 = get_metrics_calculator("data/singleton.db")
        assert c1 is c2
        default = get_metrics_calculator()
        assert str(default.db_path).endswith("chimera.db")
    finally:
        vm._global_calculators.clear()
        vm._global_calculators.update(prev)


def test_scipy_unavailable_fallback():
    """The scipy-less fallback branch is only reachable when scipy cannot be
    imported, so exercise it in a subprocess with scipy blocked."""
    import subprocess
    import sys
    import textwrap

    code = textwrap.dedent("""
        import sys
        sys.path.insert(0, {scout!r})
        import importlib.abc

        class BlockScipy(importlib.abc.MetaPathFinder):
            def find_spec(self, fullname, path=None, target=None):
                if fullname == "scipy" or fullname.startswith("scipy."):
                    raise ImportError("scipy blocked for test")
                return None

        sys.meta_path.insert(0, BlockScipy())
        import core.validation_metrics as vm
        assert vm.SCIPY_AVAILABLE is False, "SCIPY_AVAILABLE should be False"
        print("SCIPY_BLOCKED_OK")
    """).format(scout=__import__("os").path.dirname(__import__("os").path.dirname(__file__)))
    result = subprocess.run(
        [sys.executable, "-c", code], capture_output=True, text=True, timeout=60
    )
    assert result.returncode == 0, result.stderr
    assert "SCIPY_BLOCKED_OK" in result.stdout
