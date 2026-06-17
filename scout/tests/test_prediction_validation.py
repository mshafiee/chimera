"""
Tests for Prediction Validation System

Tests the prediction logger, matcher, metrics calculator, and reporter.
"""

import json
import os
import sqlite3
import tempfile
import unittest
from datetime import datetime, timedelta
from pathlib import Path

import sys

# Add parent directory to path
sys.path.insert(0, str(Path(__file__).parent.parent.parent))

from scout.core.prediction_logger import PredictionLogger, PredictionRecord
from scout.core.prediction_matcher import PredictionMatcher, MatchingResults
from scout.core.validation_metrics import ValidationMetricsCalculator
from scout.core.validation_reporter import ValidationReporter


class TestPredictionLogger(unittest.TestCase):
    """Tests for PredictionLogger."""

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
        # This test would require mocking time or setting old timestamps
        # For now, just verify the method exists and doesn't error
        expired_count = self.logger.mark_expired(max_age_days=90)
        self.assertGreaterEqual(expired_count, 0)


class TestPredictionMatcher(unittest.TestCase):
    """Tests for PredictionMatcher."""

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

        # Try to match (will have no correlation data, so will skip)
        results = self.matcher.match_predictions_to_actuals(
            lookback_days=7,
            dry_run=False
        )

        self.assertIsNotNone(results)
        self.assertGreaterEqual(results.matched_count, 0)
        self.assertIn('processing_time_seconds', results.to_dict())

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

        for wallet, predicted, actual, error in test_cases:
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
        """Test calculating validation metrics."""
        metrics = self.calculator.calculate_metrics(
            model_type="xgboost",
            time_window="7d",
            min_predictions=1
        )

        self.assertIsNotNone(metrics)
        self.assertEqual(metrics.model_type, "xgboost")
        self.assertEqual(metrics.matched_predictions, 5)
        self.assertGreater(metrics.rmse, 0)
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
