"""
Coverage tests for core/validation_reporter.py.

Covers report generation (incl. model discovery + failure), issue detection
with drift, recommendations, alerts with webhooks, HTML reports, and the
reporter singleton.
"""

import json

import pytest

from core.prediction_logger import PredictionLogger
from core.validation_metrics import ValidationMetrics
from core.validation_reporter import (
    AlertConfig,
    ValidationReporter,
    get_validation_reporter,
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
def reporter(fake_db_layer, tmp_path):
    return ValidationReporter(
        "data/reporter.db",
        AlertConfig(alert_dir=str(tmp_path / "alerts")),
    )


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
    r = ValidationReporter()
    assert str(r.db_path).endswith("chimera.db")


def test_load_env_config(monkeypatch, fake_db_layer):
    monkeypatch.setenv("SCOUT_ALERT_WEBHOOK_URL", "https://example.com/hook")
    monkeypatch.setenv("SCOUT_ALERT_HIGH_ERROR_THRESHOLD", "0.9")
    monkeypatch.setenv("SCOUT_ALERT_DRIFT_THRESHOLD", "0.3")
    monkeypatch.setenv("SCOUT_ALERT_LOW_ACCURACY_THRESHOLD", "0.4")
    monkeypatch.setenv("SCOUT_ALERT_DIR", "/tmp/alertdir")
    r = ValidationReporter()
    assert r.alert_config.webhook_url == "https://example.com/hook"
    assert r.alert_config.high_error_threshold == 0.9
    assert r.alert_config.drift_threshold == 0.3
    assert r.alert_config.low_accuracy_threshold == 0.4
    assert r.alert_config.alert_dir == "/tmp/alertdir"


def test_generate_report_with_model_types(reporter, monkeypatch):
    monkeypatch.setattr(
        reporter.metrics_calculator, "calculate_metrics",
        lambda **kw: _metrics(kw.get("model_type", "xgboost")),
    )
    report = reporter.generate_report(model_types=["xgboost"], time_window="7d")
    assert report["model_types_analyzed"] == ["xgboost"]
    assert report["models_with_data"] == ["xgboost"]
    assert "summary" in report
    assert "comparison" in report
    assert "recommendations" in report
    assert report["recent_errors"] == []


def test_generate_report_json_output(reporter, monkeypatch):
    monkeypatch.setattr(
        reporter.metrics_calculator, "calculate_metrics",
        lambda **kw: _metrics("xgboost"),
    )
    out = reporter.generate_report(model_types=["xgboost"], output_format="json")
    assert isinstance(out, str)
    parsed = json.loads(out)
    assert parsed["models_with_data"] == ["xgboost"]


def test_generate_report_discovers_model_types(fake_db_layer, monkeypatch):
    logger = PredictionLogger("data/reporter.db")
    logger.log_prediction(
        wallet_address="w1", predicted_pnl_sol=0.1, model_type="xgboost",
        features={}, confidence=0.8, strategy="SHIELD", wqs_score=75.0,
        wqs_components={},
    )
    reporter = ValidationReporter("data/reporter.db")
    monkeypatch.setattr(
        reporter.metrics_calculator, "calculate_metrics",
        lambda **kw: _metrics(kw.get("model_type", "xgboost")),
    )
    report = reporter.generate_report(time_window="7d")
    # Only MATCHED rows are discovered; the row is PENDING -> empty list
    assert report["model_types_analyzed"] == []


def test_generate_report_db_failure_raises(reporter, monkeypatch):
    def boom(*args, **kwargs):
        raise RuntimeError("db down")
    # The reporter module does `from .db import get_connection` at call time;
    # patch both namespace copies so the failure surfaces regardless.
    monkeypatch.setattr("core.db.get_connection", boom)
    try:
        monkeypatch.setattr("scout.core.db.get_connection", boom)
    except Exception:
        pass
    with pytest.raises(RuntimeError):
        reporter.generate_report()


def test_generate_report_skips_models_without_metrics(reporter, monkeypatch):
    monkeypatch.setattr(reporter.metrics_calculator, "calculate_metrics", lambda **kw: None)
    report = reporter.generate_report(model_types=["xgboost"])
    assert report["models_with_data"] == []
    assert report["summary"]["total_models"] == 0


def test_generate_summary_empty(reporter):
    summary = reporter._generate_summary({})
    assert summary["total_models"] == 0
    assert "best_model_by_rmse" not in summary


def test_generate_summary_weighted(reporter):
    metrics = {
        "a": _metrics("a", rmse=0.5, correlation=0.9, direction_accuracy=0.8,
                      matched_predictions=10, pending_predictions=2, expired_predictions=1),
        "b": _metrics("b", rmse=2.0, correlation=0.3, direction_accuracy=0.4,
                      matched_predictions=5),
    }
    summary = reporter._generate_summary(metrics)
    assert summary["total_models"] == 2
    assert summary["total_predictions"] == 20
    assert summary["total_matched"] == 15
    assert summary["total_pending"] == 2
    assert summary["total_expired"] == 1
    assert summary["best_model_by_rmse"] == "a"
    assert summary["best_model_by_correlation"] == "a"


def test_generate_comparison_single_model(reporter):
    comparison = reporter._generate_comparison({"a": _metrics("a")})
    assert "Need at least 2 models" in comparison["note"]


def test_generate_comparison_two_models(reporter):
    comparison = reporter._generate_comparison({
        "a": _metrics("a", rmse=0.5, correlation=0.9, direction_accuracy=0.8),
        "b": _metrics("b", rmse=2.0, correlation=0.3, direction_accuracy=0.4),
    })
    assert comparison["rmse_ranking"][0]["model"] == "a"
    assert comparison["correlation_ranking"][0]["model"] == "a"
    assert comparison["direction_accuracy_ranking"][0]["model"] == "a"


def test_detect_issues_all_types(reporter, monkeypatch):
    prev = _metrics("a", rmse=0.5)
    monkeypatch.setattr(
        reporter.metrics_calculator, "calculate_metrics", lambda **kw: prev
    )
    issues = reporter._detect_issues({
        "a": _metrics("a", rmse=1.2, direction_accuracy=0.3, missing_actual_rate=0.8),
    })
    types = {i["type"] for i in issues}
    assert "high_error_rate" in types
    assert "low_direction_accuracy" in types
    assert "high_pending_rate" in types
    assert "rmse_drift" in types


def test_detect_issues_drift_prev_zero(reporter, monkeypatch):
    prev = _metrics("a", rmse=0.0)
    monkeypatch.setattr(
        reporter.metrics_calculator, "calculate_metrics", lambda **kw: prev
    )
    issues = reporter._detect_issues({
        "a": _metrics("a", rmse=0.8, direction_accuracy=0.9, missing_actual_rate=0.0),
    })
    assert any(i["type"] == "rmse_drift" for i in issues)


def test_detect_issues_drift_exception_debug(reporter, monkeypatch, caplog):
    import logging

    def boom(**kwargs):
        raise RuntimeError("drift calc failed")
    monkeypatch.setattr(reporter.metrics_calculator, "calculate_metrics", boom)
    with caplog.at_level(logging.DEBUG, logger="core.validation_reporter"):
        issues = reporter._detect_issues({
            "a": _metrics("a", rmse=0.5, direction_accuracy=0.9, missing_actual_rate=0.0),
        })
    assert issues == []


def test_get_recent_errors_sorting(reporter, monkeypatch):
    from core.prediction_matcher import MatchedPrediction

    fake_matched = [
        MatchedPrediction(
            prediction_id=1, wallet_address="w1", model_type="xgboost",
            predicted_pnl_sol=0.1, actual_pnl_sol=0.5,
            prediction_timestamp="2025-01-01T00:00:00",
            match_timestamp="2025-01-02T00:00:00", days_to_match=1,
            error=0.4, abs_error=0.4, direction_correct=True,
        ),
        MatchedPrediction(
            prediction_id=2, wallet_address="w2", model_type="xgboost",
            predicted_pnl_sol=0.1, actual_pnl_sol=2.0,
            prediction_timestamp="2025-01-01T00:00:00",
            match_timestamp="2025-01-02T00:00:00", days_to_match=1,
            error=1.9, abs_error=1.9, direction_correct=True,
        ),
    ]
    monkeypatch.setattr(
        reporter.prediction_matcher, "get_matched_predictions",
        lambda **kw: fake_matched,
    )
    errors = reporter._get_recent_errors(["xgboost"], limit=10)
    assert len(errors) == 2
    assert errors[0]["abs_error"] == 1.9  # sorted descending


def test_generate_recommendations_branches(reporter):
    report = {
        "summary": {
            "avg_rmse": 2.0,  # > high_error_threshold
            "avg_direction_accuracy": 0.3,  # < low_accuracy_threshold
            "best_model_by_rmse": "xgboost",
            "total_pending": 150,
        },
        "models_with_data": ["xgboost", "lightgbm"],
        "issues": [{"severity": "high", "message": "RMSE too high"}],
        "model_metrics": {
            "xgboost": {"correlation": 0.1, "mean_predicted_profit": 1.0,
                        "mean_actual_profit": -0.5},
        },
    }
    recs = reporter._generate_recommendations(report)
    text = "\n".join(recs)
    assert "[HIGH] RMSE too high" in text
    assert "retraining" in text
    assert "feature engineering" in text
    assert "primary model" in text
    assert "pending predictions" in text
    assert "Low correlation" in text
    assert "overestimating profitability" in text


def test_generate_recommendations_healthy(reporter):
    report = {
        "summary": {"avg_rmse": 0.1, "avg_direction_accuracy": 0.9,
                    "best_model_by_rmse": None, "total_pending": 0},
        "models_with_data": ["xgboost"],
        "issues": [],
        "model_metrics": {"xgboost": {"correlation": 0.9,
                                      "mean_predicted_profit": 0.1,
                                      "mean_actual_profit": 0.1}},
    }
    recs = reporter._generate_recommendations(report)
    assert any("healthy" in r for r in recs)


def test_send_alert_writes_files(reporter, tmp_path):
    reporter.send_alert("high_error", {"message": "RMSE too high"}, alert_level="error")
    log_file = tmp_path / "alerts" / "validation_alerts.log"
    assert log_file.exists()
    content = log_file.read_text()
    assert "[ERROR]" in content
    assert "RMSE too high" in content
    alerts = list((tmp_path / "alerts").glob("alert_*.json"))
    assert len(alerts) == 1
    data = json.loads(alerts[0].read_text())
    assert data["condition"] == "high_error"


def test_send_webhook_success(reporter, monkeypatch):
    class FakeResponse:
        status = 200

        def __enter__(self):
            return self

        def __exit__(self, *args):
            return False

    called = {}

    def fake_urlopen(req, timeout):
        called["req"] = req
        return FakeResponse()

    monkeypatch.setattr("urllib.request.urlopen", fake_urlopen)
    reporter.alert_config.webhook_url = "https://example.com/hook"
    reporter.send_alert("drift", {"message": "drift detected"})
    assert called["req"] is not None


def test_send_webhook_non_200(reporter, monkeypatch):
    class FakeResponse:
        status = 500

        def __enter__(self):
            return self

        def __exit__(self, *args):
            return False

    monkeypatch.setattr("urllib.request.urlopen", lambda req, timeout: FakeResponse())
    reporter.alert_config.webhook_url = "https://example.com/hook"
    reporter.send_alert("drift", {"message": "drift detected"})


def test_send_webhook_error(reporter, monkeypatch):
    def boom(req, timeout):
        raise OSError("connection refused")
    monkeypatch.setattr("urllib.request.urlopen", boom)
    reporter.alert_config.webhook_url = "https://example.com/hook"
    reporter.send_alert("drift", {"message": "drift detected"})


def test_save_report_default_path(reporter, monkeypatch, tmp_path):
    monkeypatch.chdir(tmp_path)
    path = reporter.save_report({"a": 1})
    assert path.startswith("data/validation_reports")
    assert json.load(open(path))["a"] == 1


def test_save_report_explicit_path(reporter, tmp_path):
    out = str(tmp_path / "reports" / "my_report.json")
    path = reporter.save_report({"b": 2}, output_path=out)
    assert path == out
    assert json.load(open(path))["b"] == 2


def test_generate_html_report(reporter, monkeypatch, tmp_path):
    monkeypatch.setattr(
        reporter.metrics_calculator, "calculate_metrics",
        lambda **kw: _metrics(kw.get("model_type", "xgboost"),
                              missing_actual_rate=0.8),
    )
    out = str(tmp_path / "html" / "report.html")
    reporter.generate_html_report(out, model_types=["xgboost"])
    html = open(out).read()
    assert "<html>" in html
    assert "Scout ML Validation Report" in html
    assert "xgboost" in html


def test_report_to_html_with_issues_and_recs():
    reporter = ValidationReporter("data/x.db")
    report = {
        "generated_at": "now", "time_window": "7d",
        "summary": {"total_models": 1, "total_predictions": 5, "total_matched": 3,
                    "avg_rmse": 0.5, "avg_correlation": 0.5,
                    "avg_direction_accuracy": 0.7},
        "model_metrics": {"xgboost": {"total_predictions": 5, "matched_predictions": 3,
                                      "rmse": 0.5, "correlation": 0.5,
                                      "direction_accuracy": 0.7,
                                      "mean_days_to_match": 2.0}},
        "issues": [{"severity": "high", "message": "RMSE too high", "model": "x",
                    "value": 0.5}],
        "recommendations": ["Retrain models"],
    }
    html = reporter._report_to_html(report)
    assert "Issues Detected" in html
    assert "Recommendations" in html
    assert "Retrain models" in html
    assert "HIGH:" in html


def test_get_validation_reporter_singleton(fake_db_layer, monkeypatch):
    import core.validation_reporter as vr
    prev = dict(vr._global_reporters)
    vr._global_reporters.clear()
    try:
        r1 = get_validation_reporter("data/singleton.db")
        r2 = get_validation_reporter("data/singleton.db")
        assert r1 is r2
        default = get_validation_reporter()
        assert str(default.db_path).endswith("chimera.db")
    finally:
        vr._global_reporters.clear()
        vr._global_reporters.update(prev)
