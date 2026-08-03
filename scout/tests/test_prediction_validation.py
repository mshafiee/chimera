"""
Tests for Prediction Validation System

Tests the prediction logger, matcher, metrics calculator, and reporter.
"""

import json
import os
import tempfile
import unittest
from datetime import datetime, timedelta
from pathlib import Path

import pytest

import sys

# Add parent directory to path
sys.path.insert(0, str(Path(__file__).parent.parent))

from core.prediction_logger import PredictionLogger
from core.prediction_matcher import PredictionMatcher
from core.validation_metrics import ValidationMetricsCalculator
from core.validation_reporter import ValidationReporter


_ML_PREDICTIONS_DDL = """
CREATE TABLE IF NOT EXISTS ml_predictions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_address TEXT NOT NULL,
    prediction_timestamp TIMESTAMP NOT NULL,
    model_type TEXT NOT NULL,
    predicted_pnl_sol TEXT NOT NULL
        CHECK (predicted_pnl_sol = CAST(predicted_pnl_sol AS NUMERIC)),
    predicted_class TEXT,
    confidence REAL CHECK (confidence IS NULL OR confidence BETWEEN 0 AND 1),
    features_json TEXT,
    strategy TEXT,
    wqs_score_at_prediction REAL,
    wqs_components_json TEXT,
    actual_pnl_sol TEXT
        CHECK (actual_pnl_sol IS NULL OR actual_pnl_sol = CAST(actual_pnl_sol AS NUMERIC)),
    actual_pnl_7d_sol TEXT
        CHECK (actual_pnl_7d_sol IS NULL OR actual_pnl_7d_sol = CAST(actual_pnl_7d_sol AS NUMERIC)),
    actual_pnl_30d_sol TEXT
        CHECK (actual_pnl_30d_sol IS NULL OR actual_pnl_30d_sol = CAST(actual_pnl_30d_sol AS NUMERIC)),
    match_timestamp TIMESTAMP,
    days_to_match INTEGER CHECK (days_to_match IS NULL OR days_to_match >= 0),
    status TEXT DEFAULT 'PENDING' CHECK (status IN ('PENDING', 'MATCHED', 'EXPIRED', 'INVALID')),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(wallet_address, prediction_timestamp, model_type)
)
"""

_WQS_PNL_CORRELATION_DDL = """
CREATE TABLE IF NOT EXISTS wqs_pnl_correlation (
    wallet_address TEXT PRIMARY KEY,
    wqs_score_at_promotion REAL NOT NULL
        CHECK (wqs_score_at_promotion BETWEEN 0 AND 100),
    actual_copy_pnl_7d_sol TEXT
        CHECK (actual_copy_pnl_7d_sol IS NULL OR actual_copy_pnl_7d_sol = CAST(actual_copy_pnl_7d_sol AS NUMERIC)),
    actual_copy_pnl_30d_sol TEXT
        CHECK (actual_copy_pnl_30d_sol IS NULL OR actual_copy_pnl_30d_sol = CAST(actual_copy_pnl_30d_sol AS NUMERIC)),
    actual_copy_pnl_all_sol TEXT
        CHECK (actual_copy_pnl_all_sol IS NULL OR actual_copy_pnl_all_sol = CAST(actual_copy_pnl_all_sol AS NUMERIC)),
    copy_trade_count_7d INTEGER NOT NULL DEFAULT 0 CHECK (copy_trade_count_7d >= 0),
    copy_trade_count_30d INTEGER NOT NULL DEFAULT 0 CHECK (copy_trade_count_30d >= 0),
    copy_trade_count_all INTEGER NOT NULL DEFAULT 0 CHECK (copy_trade_count_all >= 0),
    strategy TEXT NOT NULL DEFAULT 'SHIELD'
        CHECK(strategy IN ('SHIELD', 'SPEAR')),
    wqs_components_json TEXT,
    promoted_at TEXT NOT NULL,
    last_updated_at TEXT NOT NULL
        CHECK (last_updated_at >= promoted_at)
)
"""


@pytest.fixture(autouse=True)
def _schema_tables(fake_db_layer, monkeypatch):
    """Pre-create the pipeline tables with native SQLite DDL.

    The production ``_ensure_schema`` path translates the SQLite schema file to
    PostgreSQL DDL (``id SERIAL PRIMARY KEY``, ...) which the SQLite stand-in
    cannot auto-increment, so the pre-created table keeps ``id`` working.
    """
    fake_db_layer.executescript(_ML_PREDICTIONS_DDL)
    fake_db_layer.executescript(_WQS_PNL_CORRELATION_DDL)

    # The SQLite stand-in can't answer information_schema queries, so make
    # table_exists() reflect the (now existing) table directly. Patch both
    # module copies (`core.*` and the `scout.core.*` alias).
    from core.correlation_reader import CorrelationReader as CoreReader
    from scout.core.correlation_reader import CorrelationReader as ScoutReader
    monkeypatch.setattr(CoreReader, "table_exists", lambda self: True)
    monkeypatch.setattr(ScoutReader, "table_exists", lambda self: True)


class TestPredictionLogger(unittest.TestCase):
    """Tests for PredictionLogger."""

    @pytest.fixture(autouse=True)
    def _fake_db(self, fake_db_layer):
        """Run against an in-memory SQLite stand-in for the PG layer."""
        self._fake_db = fake_db_layer

    def setUp(self):
        """Set up test database."""
        self.db_fd, self.db_path = tempfile.mkstemp(suffix='.db')
        self.logger = PredictionLogger(self.db_path)

    def tearDown(self):
        """Clean up test database."""
        os.close(self.db_fd)
        os.unlink(self.db_path)

    def test_log_prediction(self):
        """Test logging a prediction."""
        prediction_id = self.logger.log_prediction(
            wallet_address="test_wallet_1",
            predicted_pnl_sol=0.15,
            model_type="xgboost",
            features={"roi_7d": 0.05, "win_rate": 0.6},
            confidence=0.85,
            strategy="SHIELD",
            wqs_score=75.0,
            wqs_components={"roi": 20, "consistency": 15}
        )

        self.assertIsNotNone(prediction_id)
        self.assertGreater(prediction_id, 0)

    def test_get_pending_predictions(self):
        """Test retrieving pending predictions."""
        # Log a prediction
        self.logger.log_prediction(
            wallet_address="test_wallet_2",
            predicted_pnl_sol=0.10,
            model_type="lightgbm",
            features={"roi_7d": 0.03},
            confidence=0.70,
            strategy="SPEAR",
            wqs_score=65.0,
            wqs_components={}
        )

        # Get pending predictions
        pending = self.logger.get_pending_predictions()

        self.assertEqual(len(pending), 1)
        self.assertEqual(pending[0].wallet_address, "test_wallet_2")
        self.assertEqual(pending[0].status, "PENDING")

    def test_mark_matched(self):
        """Test marking a prediction as matched."""
        # Log a prediction
        prediction_id = self.logger.log_prediction(
            wallet_address="test_wallet_3",
            predicted_pnl_sol=0.20,
            model_type="xgboost",
            features={"roi_7d": 0.08},
            confidence=0.90,
            strategy="SHIELD",
            wqs_score=80.0,
            wqs_components={}
        )

        # Mark as matched
        success = self.logger.mark_matched(
            prediction_id=prediction_id,
            actual_pnl_sol=0.18,
            actual_pnl_7d_sol=0.15,
            actual_pnl_30d_sol=0.25
        )

        self.assertTrue(success)

        # Verify it's marked as matched
        pending = self.logger.get_pending_predictions()
        self.assertEqual(len(pending), 0)  # No pending predictions

        # Verify the actual PnL values were persisted
        row = self._fake_db.execute(
            "SELECT status, actual_pnl_sol, actual_pnl_7d_sol, actual_pnl_30d_sol "
            "FROM ml_predictions WHERE id = ?",
            (prediction_id,),
        ).fetchone()
        self.assertIsNotNone(row, "Prediction row must still exist")
        self.assertEqual(row["status"], "MATCHED")
        # PnL columns are TEXT (Decimal strings) in the schema
        self.assertAlmostEqual(float(row["actual_pnl_sol"]), 0.18)
        self.assertAlmostEqual(float(row["actual_pnl_7d_sol"]), 0.15)
        self.assertAlmostEqual(float(row["actual_pnl_30d_sol"]), 0.25)

    def test_get_statistics(self):
        """Test getting prediction statistics."""
        # Log multiple predictions
        for i in range(5):
            self.logger.log_prediction(
                wallet_address=f"test_wallet_{i}",
                predicted_pnl_sol=0.1 * i,
                model_type="xgboost",
                features={"roi_7d": 0.01 * i},
                confidence=0.8,
                strategy="SHIELD",
                wqs_score=70.0,
                wqs_components={}
            )

        stats = self.logger.get_statistics()

        self.assertEqual(stats['total_predictions'], 5)
        self.assertIn('by_status', stats)
        self.assertIn('by_model', stats)

    def test_mark_expired(self):
        """Test marking old predictions as expired."""
        # Log a prediction and backdate it beyond max_age_days
        prediction_id = self.logger.log_prediction(
            wallet_address="test_wallet_expired",
            predicted_pnl_sol=0.15,
            model_type="xgboost",
            features={"roi_7d": 0.05},
            confidence=0.85,
            strategy="SHIELD",
            wqs_score=75.0,
            wqs_components={}
        )

        old_ts = (datetime.utcnow() - timedelta(days=180)).isoformat()
        self._fake_db.execute(
            "UPDATE ml_predictions SET prediction_timestamp = ? WHERE id = ?",
            (old_ts, prediction_id),
        )
        self._fake_db.commit()

        # A recent prediction must remain PENDING
        self.logger.log_prediction(
            wallet_address="test_wallet_recent",
            predicted_pnl_sol=0.05,
            model_type="xgboost",
            features={},
            confidence=0.8,
            strategy="SHIELD",
            wqs_score=60.0,
            wqs_components={}
        )

        expired_count = self.logger.mark_expired(max_age_days=90)
        self.assertEqual(expired_count, 1)

        pending = self.logger.get_pending_predictions()
        self.assertEqual(len(pending), 1)
        self.assertEqual(pending[0].wallet_address, "test_wallet_recent")


class TestPredictionMatcher(unittest.TestCase):
    """Tests for PredictionMatcher."""

    @pytest.fixture(autouse=True)
    def _fake_db(self, fake_db_layer):
        """Run against an in-memory SQLite stand-in for the PG layer."""
        self._fake_db = fake_db_layer

    def setUp(self):
        """Set up test database."""
        self.db_fd, self.db_path = tempfile.mkstemp(suffix='.db')
        self.logger = PredictionLogger(self.db_path)
        self.matcher = PredictionMatcher(self.db_path)

    def tearDown(self):
        """Clean up test database."""
        os.close(self.db_fd)
        os.unlink(self.db_path)

    def test_match_predictions_to_actuals(self):
        """Test matching predictions to actuals."""
        # Log a prediction
        self.logger.log_prediction(
            wallet_address="test_wallet_match",
            predicted_pnl_sol=0.15,
            model_type="xgboost",
            features={"roi_7d": 0.05},
            confidence=0.85,
            strategy="SHIELD",
            wqs_score=75.0,
            wqs_components={}
        )

        # Seed correlation data so the matcher has something to match against
        # (7d PnL must be present: the 7-day lookback window scores against it)
        self._fake_db.execute(
            "INSERT INTO wqs_pnl_correlation "
            "(wallet_address, wqs_score_at_promotion, actual_copy_pnl_7d_sol, "
            "actual_copy_pnl_30d_sol, copy_trade_count_30d, strategy, promoted_at, "
            "last_updated_at) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            ("test_wallet_match", 75.0, 0.12, 0.10, 12, "SHIELD",
             datetime.utcnow().isoformat(), datetime.utcnow().isoformat()),
        )
        self._fake_db.commit()

        results = self.matcher.match_predictions_to_actuals(
            lookback_days=7,
            dry_run=False
        )

        self.assertIsNotNone(results)
        self.assertEqual(results.matched_count, 1)
        self.assertIn('processing_time_seconds', results.to_dict())

        # The prediction must have transitioned to MATCHED
        row = self._fake_db.execute(
            "SELECT status, actual_pnl_sol FROM ml_predictions "
            "WHERE wallet_address = ?",
            ("test_wallet_match",),
        ).fetchone()
        self.assertIsNotNone(row)
        self.assertEqual(row["status"], "MATCHED")

    def test_get_matched_predictions(self):
        """Test retrieving matched predictions."""
        # Log and match a prediction
        prediction_id = self.logger.log_prediction(
            wallet_address="test_wallet_retrieve",
            predicted_pnl_sol=0.12,
            model_type="lightgbm",
            features={"roi_7d": 0.04},
            confidence=0.75,
            strategy="SPEAR",
            wqs_score=68.0,
            wqs_components={}
        )

        self.logger.mark_matched(
            prediction_id=prediction_id,
            actual_pnl_sol=0.10
        )

        # Get matched predictions
        matched = self.matcher.get_matched_predictions()

        self.assertEqual(len(matched), 1)
        self.assertEqual(matched[0].wallet_address, "test_wallet_retrieve")
        self.assertTrue(matched[0].direction_correct)


class TestValidationMetricsCalculator(unittest.TestCase):
    """Tests for ValidationMetricsCalculator."""

    @pytest.fixture(autouse=True)
    def _fake_db(self, fake_db_layer):
        """Run against an in-memory SQLite stand-in for the PG layer."""
        self._fake_db = fake_db_layer

    def setUp(self):
        """Set up test database."""
        self.db_fd, self.db_path = tempfile.mkstemp(suffix='.db')
        self.logger = PredictionLogger(self.db_path)
        self.calculator = ValidationMetricsCalculator(self.db_path)

        # Create test data
        self._create_test_data()

    def tearDown(self):
        """Clean up test database."""
        os.close(self.db_fd)
        os.unlink(self.db_path)

    def _create_test_data(self):
        """Create test prediction data."""
        # Log predictions with varying accuracy
        test_cases = [
            ("wallet_1", 0.10, 0.12, 0.02),  # Good prediction
            ("wallet_2", 0.15, 0.10, -0.05),  # Overestimated
            ("wallet_3", -0.05, -0.03, 0.02),  # Direction correct
            ("wallet_4", 0.20, 0.18, -0.02),  # Good prediction
            ("wallet_5", 0.08, 0.25, 0.17),  # Underestimated
        ]

        for wallet, predicted, actual, _ in test_cases:
            prediction_id = self.logger.log_prediction(
                wallet_address=wallet,
                predicted_pnl_sol=predicted,
                model_type="xgboost",
                features={"roi_7d": 0.05},
                confidence=0.8,
                strategy="SHIELD",
                wqs_score=70.0,
                wqs_components={}
            )
            self.logger.mark_matched(
                prediction_id=prediction_id,
                actual_pnl_sol=actual
            )

    def test_calculate_metrics(self):
        """Test calculating validation metrics with exact expected values."""
        metrics = self.calculator.calculate_metrics(
            model_type="xgboost",
            time_window="7d",
            min_predictions=1
        )

        self.assertIsNotNone(metrics)
        self.assertEqual(metrics.model_type, "xgboost")
        self.assertEqual(metrics.matched_predictions, 5)
        # Hand-computed from the seeded pairs:
        # predicted [0.10, 0.15, -0.05, 0.20, 0.08] vs actual [0.12, 0.10, -0.03, 0.18, 0.25]
        self.assertAlmostEqual(metrics.mae, 0.056, places=3)
        self.assertAlmostEqual(metrics.rmse, 0.0807, places=3)
        self.assertAlmostEqual(metrics.direction_accuracy, 1.0, places=3)
        self.assertGreaterEqual(metrics.correlation, -1)
        self.assertLessEqual(metrics.correlation, 1)

    def test_compare_models(self):
        """Test comparing metrics across models."""
        # Add lightgbm predictions
        prediction_id = self.logger.log_prediction(
            wallet_address="wallet_lgb",
            predicted_pnl_sol=0.10,
            model_type="lightgbm",
            features={"roi_7d": 0.05},
            confidence=0.8,
            strategy="SHIELD",
            wqs_score=70.0,
            wqs_components={}
        )
        self.logger.mark_matched(
            prediction_id=prediction_id,
            actual_pnl_sol=0.12
        )

        model_metrics = self.calculator.compare_models(
            model_types=["xgboost", "lightgbm"],
            time_window="7d",
            min_predictions=1
        )

        self.assertIn("xgboost", model_metrics)
        self.assertIn("lightgbm", model_metrics)


class TestValidationReporter(unittest.TestCase):
    """Tests for ValidationReporter."""

    @pytest.fixture(autouse=True)
    def _fake_db(self, fake_db_layer):
        """Run against an in-memory SQLite stand-in for the PG layer."""
        self._fake_db = fake_db_layer

    def setUp(self):
        """Set up test database."""
        self.db_fd, self.db_path = tempfile.mkstemp(suffix='.db')
        self.logger = PredictionLogger(self.db_path)
        self.reporter = ValidationReporter(self.db_path)

        # Create test data
        self._create_test_data()

    def tearDown(self):
        """Clean up test database."""
        os.close(self.db_fd)
        os.unlink(self.db_path)

    def _create_test_data(self):
        """Create test prediction data."""
        for i in range(3):
            prediction_id = self.logger.log_prediction(
                wallet_address=f"report_wallet_{i}",
                predicted_pnl_sol=0.1 * i,
                model_type="xgboost",
                features={"roi_7d": 0.05},
                confidence=0.8,
                strategy="SHIELD",
                wqs_score=70.0,
                wqs_components={}
            )
            self.logger.mark_matched(
                prediction_id=prediction_id,
                actual_pnl_sol=0.1 * i + 0.01
            )

    def test_generate_report(self):
        """Test generating validation report."""
        report = self.reporter.generate_report(
            model_types=["xgboost"],
            time_window="7d"
        )

        self.assertIn('generated_at', report)
        self.assertIn('summary', report)
        self.assertIn('model_metrics', report)
        self.assertIn('issues', report)
        self.assertIn('recommendations', report)

        summary = report['summary']
        self.assertGreater(summary['total_models'], 0)

    def test_save_report(self):
        """Test saving report to file."""
        report = self.reporter.generate_report(
            model_types=["xgboost"],
            time_window="7d"
        )

        # Create temp output path
        with tempfile.NamedTemporaryFile(mode='w', delete=False, suffix='.json') as f:
            output_path = f.name

        try:
            saved_path = self.reporter.save_report(report, output_path)
            self.assertTrue(os.path.exists(saved_path))

            # Verify file content
            with open(saved_path, 'r') as f:
                loaded_report = json.load(f)

            self.assertIn('summary', loaded_report)

        finally:
            if os.path.exists(output_path):
                os.unlink(output_path)


if __name__ == '__main__':
    unittest.main()
