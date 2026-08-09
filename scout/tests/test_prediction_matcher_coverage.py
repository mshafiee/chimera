"""
Coverage tests for core/prediction_matcher.py.

Uses the fake_db_layer fixture; tables are pre-created with SQLite DDL and
CorrelationReader.table_exists is patched to reflect the SQLite stand-in
(which cannot answer information_schema queries).
"""

import pytest

from core.prediction_matcher import (
    MatchedPrediction,
    MatchingResults,
    PredictionMatcher,
    get_prediction_matcher,
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

_CORR_DDL = """
CREATE TABLE IF NOT EXISTS wqs_pnl_correlation (
    wallet_address TEXT PRIMARY KEY,
    wqs_score_at_promotion REAL NOT NULL,
    actual_copy_pnl_7d_sol TEXT,
    actual_copy_pnl_30d_sol TEXT,
    actual_copy_pnl_all_sol TEXT,
    copy_trade_count_7d INTEGER DEFAULT 0,
    copy_trade_count_30d INTEGER DEFAULT 0,
    copy_trade_count_all INTEGER DEFAULT 0,
    strategy TEXT DEFAULT 'SHIELD',
    wqs_components_json TEXT,
    promoted_at TEXT NOT NULL,
    last_updated_at TEXT NOT NULL
)
"""


@pytest.fixture(autouse=True)
def _tables(fake_db_layer, monkeypatch):
    fake_db_layer.executescript(_ML_DDL)
    fake_db_layer.executescript(_CORR_DDL)
    from core.correlation_reader import CorrelationReader as CoreReader
    from scout.core.correlation_reader import CorrelationReader as ScoutReader
    monkeypatch.setattr(CoreReader, "table_exists", lambda self: True)
    monkeypatch.setattr(ScoutReader, "table_exists", lambda self: True)


@pytest.fixture
def matcher(fake_db_layer):
    return PredictionMatcher("data/matcher.db")


def _log(matcher, wallet="wallet1", pnl=0.15, model="xgboost"):
    return matcher.prediction_logger.log_prediction(
        wallet_address=wallet,
        predicted_pnl_sol=pnl,
        model_type=model,
        features={"roi_7d": 0.05},
        confidence=0.85,
        strategy="SHIELD",
        wqs_score=75.0,
        wqs_components={},
    )


def _add_correlation(fake_db_layer, wallet="wallet1", pnl_7d="0.5", pnl_30d=None,
                     pnl_all=None, promoted_at=None):
    from datetime import datetime
    promoted_at = promoted_at or datetime.utcnow().isoformat()
    cur = fake_db_layer.cursor()
    cur.execute(
        "INSERT INTO wqs_pnl_correlation "
        "(wallet_address, wqs_score_at_promotion, actual_copy_pnl_7d_sol,"
        " actual_copy_pnl_30d_sol, actual_copy_pnl_all_sol, copy_trade_count_7d,"
        " copy_trade_count_30d, copy_trade_count_all, strategy, wqs_components_json,"
        " promoted_at, last_updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (wallet, 75.0, pnl_7d, pnl_30d, pnl_all, 1, 2, 3, "SHIELD", "{}",
         promoted_at, promoted_at),
    )
    fake_db_layer.commit()


def test_default_db_path(fake_db_layer):
    m = PredictionMatcher()
    assert str(m.db_path).endswith("chimera.db")


def test_no_pending_predictions(matcher):
    result = matcher.match_predictions_to_actuals(lookback_days=7)
    assert isinstance(result, MatchingResults)
    assert result.total_pending == 0
    assert result.matched_count == 0


def test_correlation_table_missing(matcher, monkeypatch):
    _log(matcher)
    monkeypatch.setattr(type(matcher.correlation_reader), "table_exists", lambda self: False)
    result = matcher.match_predictions_to_actuals()
    assert result.skipped_count == 1


def test_correlation_load_failure(matcher, monkeypatch):
    _log(matcher)

    def boom(*args, **kwargs):
        raise RuntimeError("db down")
    monkeypatch.setattr(matcher.correlation_reader, "get_all_records", boom)
    result = matcher.match_predictions_to_actuals()
    assert result.skipped_count == 1


def test_happy_path_match(fake_db_layer, matcher):
    pid = _log(matcher, wallet="wallet1", pnl=0.15)
    _add_correlation(fake_db_layer, wallet="wallet1", pnl_7d="0.5")
    result = matcher.match_predictions_to_actuals(lookback_days=7)
    assert result.matched_count == 1
    # Prediction updated to MATCHED with actual pnl
    cur = fake_db_layer.cursor()
    cur.execute("SELECT status, actual_pnl_sol FROM ml_predictions WHERE id = ?", (pid,))
    row = cur.fetchone()
    assert row["status"] == "MATCHED"
    assert float(row["actual_pnl_sol"]) == 0.5


def test_dry_run_does_not_update(fake_db_layer, matcher):
    pid = _log(matcher, wallet="wallet1", pnl=0.15)
    _add_correlation(fake_db_layer, wallet="wallet1", pnl_7d="0.5")
    result = matcher.match_predictions_to_actuals(dry_run=True)
    assert result.matched_count == 1
    cur = fake_db_layer.cursor()
    cur.execute("SELECT status FROM ml_predictions WHERE id = ?", (pid,))
    assert cur.fetchone()["status"] == "PENDING"


def test_skip_wallet_without_correlation(matcher):
    _log(matcher, wallet="no_corr_wallet")
    result = matcher.match_predictions_to_actuals()
    assert result.skipped_count == 1


def test_select_actual_pnl_fills_total_from_all(fake_db_layer, matcher):
    # lookback 7d chooses the 7d column, but it is NULL; the all-time value
    # must fill the total so the prediction can still match.
    _log(matcher, wallet="wallet1")
    _add_correlation(fake_db_layer, wallet="wallet1", pnl_7d=None, pnl_all="5.0")
    result = matcher.match_predictions_to_actuals(lookback_days=7)
    assert result.matched_count == 1


def test_to_dict(matcher):
    result = matcher.match_predictions_to_actuals()
    d = result.to_dict()
    assert d["total_pending"] == 0
    assert d["lookback_days"] == 7


def test_skip_missing_actual_pnl(fake_db_layer, matcher):
    _log(matcher, wallet="wallet1")
    _add_correlation(fake_db_layer, wallet="wallet1", pnl_7d=None, pnl_30d=None, pnl_all=None)
    result = matcher.match_predictions_to_actuals()
    assert result.skipped_count == 1


def test_mark_matched_failure_counts_failed(matcher, monkeypatch):
    _log(matcher, wallet="wallet1")
    monkeypatch.setattr(
        matcher.prediction_logger, "mark_matched", lambda *a, **k: False
    )
    # Provide a correlation record so the flow reaches mark_matched
    from core.prediction_matcher import MatchedPrediction  # noqa
    from core.correlation_reader import WqsCorrelationRecord

    from datetime import datetime
    fake_records = [WqsCorrelationRecord(
        wallet_address="wallet1", wqs_score_at_promotion=75.0,
        actual_copy_pnl_7d_sol=0.5, actual_copy_pnl_30d_sol=0.7,
        actual_copy_pnl_all_sol=2.0, copy_trade_count_7d=1,
        copy_trade_count_30d=2, copy_trade_count_all=3, strategy="SHIELD",
        wqs_components_json="{}", promoted_at=datetime.utcnow().isoformat(),
        last_updated_at=datetime.utcnow().isoformat(),
    )]
    monkeypatch.setattr(matcher.correlation_reader, "get_all_records", lambda: fake_records)
    result = matcher.match_predictions_to_actuals()
    assert result.failed_count == 1


def test_per_prediction_exception_counts_failed(matcher, monkeypatch):
    _log(matcher, wallet="wallet1")

    def boom(*args, **kwargs):
        raise RuntimeError("boom")
    monkeypatch.setattr(matcher, "_find_best_match", boom)
    from core.correlation_reader import WqsCorrelationRecord
    from datetime import datetime
    monkeypatch.setattr(matcher.correlation_reader, "get_all_records",
                        lambda: [WqsCorrelationRecord(
                            wallet_address="wallet1", wqs_score_at_promotion=75.0,
                            actual_copy_pnl_7d_sol=0.5, actual_copy_pnl_30d_sol=None,
                            actual_copy_pnl_all_sol=None, copy_trade_count_7d=1,
                            copy_trade_count_30d=0, copy_trade_count_all=1,
                            strategy="SHIELD", wqs_components_json="{}",
                            promoted_at=datetime.utcnow().isoformat(),
                            last_updated_at=datetime.utcnow().isoformat(),
                        )])
    result = matcher.match_predictions_to_actuals()
    assert result.failed_count == 1


def test_find_best_match_skips_invalid_timestamp(fake_db_layer, matcher):
    _log(matcher, wallet="wallet1")
    _add_correlation(fake_db_layer, wallet="wallet1", pnl_7d="0.5",
                     promoted_at="not-a-timestamp")
    # Prediction timestamp is now; a garbage record timestamp is skipped
    result = matcher.match_predictions_to_actuals()
    assert result.skipped_count == 1


def test_select_actual_pnl_30d_branch(fake_db_layer, matcher):
    _log(matcher, wallet="wallet1")
    _add_correlation(fake_db_layer, wallet="wallet1", pnl_7d="0.5", pnl_30d="1.5")
    result = matcher.match_predictions_to_actuals(lookback_days=30)
    assert result.matched_count == 1


def test_select_actual_pnl_all_branch_with_fillins(fake_db_layer, matcher):
    _log(matcher, wallet="wallet1")
    _add_correlation(fake_db_layer, wallet="wallet1", pnl_7d="0.5", pnl_30d="1.5",
                     pnl_all="5.0")
    result = matcher.match_predictions_to_actuals(lookback_days=90)
    assert result.matched_count == 1


def test_get_matched_predictions(fake_db_layer, matcher):
    _log(matcher, wallet="wallet1", pnl=0.15)
    _log(matcher, wallet="wallet2", pnl=0.2, model="lightgbm")
    _add_correlation(fake_db_layer, wallet="wallet1", pnl_7d="0.5")
    _add_correlation(fake_db_layer, wallet="wallet2", pnl_7d="0.3")
    matcher.match_predictions_to_actuals()
    matched = matcher.get_matched_predictions()
    assert len(matched) == 2
    assert isinstance(matched[0], MatchedPrediction)
    only_xgb = matcher.get_matched_predictions(model_type="xgboost")
    assert len(only_xgb) == 1
    limited = matcher.get_matched_predictions(limit=1)
    assert len(limited) == 1


def test_get_matched_predictions_error(monkeypatch):
    def boom(*args, **kwargs):
        raise RuntimeError("db down")
    # The dual core.*/scout.core.* import paths create duplicate module
    # objects; patch the class's own globals so the lookup definitely hits.
    monkeypatch.setitem(
        PredictionMatcher.get_matched_predictions.__globals__, "get_connection", boom
    )
    m = PredictionMatcher("data/matcher.db")
    assert m.get_matched_predictions() == []


def test_get_match_summary_empty(matcher):
    summary = matcher.get_match_summary()
    assert summary["total_matched"] == 0
    assert summary["mean_error"] == 0.0


def test_get_match_summary_with_data(fake_db_layer, matcher):
    _log(matcher, wallet="wallet1", pnl=0.15)
    _log(matcher, wallet="wallet2", pnl=-0.1)
    _add_correlation(fake_db_layer, wallet="wallet1", pnl_7d="0.5")
    _add_correlation(fake_db_layer, wallet="wallet2", pnl_7d="-0.2")
    matcher.match_predictions_to_actuals()
    summary = matcher.get_match_summary()
    assert summary["total_matched"] == 2
    assert summary["positive_predictions"] == 1
    assert summary["negative_predictions"] == 1
    assert summary["mean_abs_error"] > 0
    assert 0.0 <= summary["direction_accuracy"] <= 1.0


def test_get_prediction_matcher_singleton(fake_db_layer, monkeypatch):
    import core.prediction_matcher as pm
    prev = dict(pm._global_matchers)
    pm._global_matchers.clear()
    try:
        m1 = get_prediction_matcher("data/singleton.db")
        m2 = get_prediction_matcher("data/singleton.db")
        assert m1 is m2
        default = get_prediction_matcher()
        assert str(default.db_path).endswith("chimera.db")
    finally:
        pm._global_matchers.clear()
        pm._global_matchers.update(prev)


def test_check_direction_correct():
    from core.prediction_matcher import PredictionMatcher as PM
    assert PM._check_direction_correct(0.5, 0.3) is True
    assert PM._check_direction_correct(-0.5, -0.3) is True
    assert PM._check_direction_correct(0.5, -0.3) is False
    assert PM._check_direction_correct(-0.5, 0.3) is False
