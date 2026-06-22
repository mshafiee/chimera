"""
Validation Metrics Calculator for Scout ML Models

Calculates comprehensive validation metrics from matched predictions.
This module provides the analytical layer for model validation.

Usage:
    calculator = ValidationMetricsCalculator(db_path="data/chimera.db")
    metrics = calculator.calculate_metrics(model_type="xgboost", time_window="7d")
    print(f"RMSE: {metrics.rmse}, Correlation: {metrics.correlation}")
"""

import json
import logging
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path
from typing import Dict, Any, Optional, List, Tuple

import numpy as np

from .db import get_connection

try:
    from scipy import stats
    SCIPY_AVAILABLE = True
except ImportError:
    SCIPY_AVAILABLE = False
    logging.warning("scipy not available - some metrics will be limited")

logger = logging.getLogger(__name__)


@dataclass
class ValidationMetrics:
    """Comprehensive validation metrics for a model."""
    model_type: str
    time_window: str  # '7d', '30d', 'all'

    # Sample counts
    total_predictions: int
    matched_predictions: int
    pending_predictions: int
    expired_predictions: int

    # Accuracy metrics
    mae: float  # Mean Absolute Error
    rmse: float  # Root Mean Squared Error
    mape: float  # Mean Absolute Percentage Error
    correlation: float  # Pearson correlation
    r_squared: float  # R-squared

    # Direction accuracy
    direction_accuracy: float
    direction_positive_accuracy: float
    direction_negative_accuracy: float

    # Profit metrics
    profitable_prediction_rate: float
    mean_predicted_profit: float
    mean_actual_profit: float

    # Timing
    mean_days_to_match: float
    median_days_to_match: float

    # Data quality
    missing_actual_rate: float

    # Classification metrics (if applicable) - moved to end with defaults
    precision: Optional[float] = None
    recall: Optional[float] = None
    f1_score: Optional[float] = None

    # Error distribution - moved to end with defaults
    error_skewness: float = 0.0
    error_kurtosis: float = 0.0
    percentile_90_error: float = 0.0
    percentile_95_error: float = 0.0
    percentile_99_error: float = 0.0

    # Timestamp
    calculated_at: str = field(default_factory=lambda: datetime.utcnow().isoformat())

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary."""
        from dataclasses import asdict
        return asdict(self)


class ValidationMetricsCalculator:
    """
    Calculates validation metrics from matched predictions.

    This class:
    - Retrieves matched predictions from database
    - Calculates comprehensive accuracy metrics
    - Compares metrics across models
    - Analyzes feature importance by accuracy
    """

    def __init__(self, db_path: Optional[str] = None):
        """
        Initialize the metrics calculator.

        Args:
            db_path: Path to SQLite database
        """
        if db_path is None:
            db_path = "data/chimera.db"

        self.db_path = Path(db_path)

    def _get_connection(self):
        """Get a database connection with row factory."""
        if not self.db_path.exists():
            logger.warning(f"Database not found at {self.db_path}")

        conn = get_connection(str(self.db_path))
        return conn

    def calculate_metrics(
        self,
        model_type: str,
        time_window: str = '7d',
        min_predictions: int = 5,
        start_date: Optional[str] = None,
        end_date: Optional[str] = None
    ) -> Optional[ValidationMetrics]:
        """
        Calculate validation metrics for a model.

        Args:
            model_type: Model type (xgboost, lightgbm, meta_learner, etc.)
            time_window: Time window for analysis ('7d', '30d', 'all')
            min_predictions: Minimum number of predictions required
            start_date: Optional start date filter (ISO format)
            end_date: Optional end date filter (ISO format)

        Returns:
            ValidationMetrics object or None if insufficient data
        """
        try:
            conn = self._get_connection()
            cursor = conn.cursor()

            # Build query with filters
            query = """
                SELECT
                    id, wallet_address, prediction_timestamp, model_type,
                    predicted_pnl_sol, actual_pnl_sol, actual_pnl_7d_sol,
                    actual_pnl_30d_sol, match_timestamp, days_to_match,
                    status, features_json
                FROM ml_predictions
                WHERE model_type = ?
                AND status = 'MATCHED'
            """
            params = [model_type]

            # Apply time window filter
            if end_date:
                query += " AND prediction_timestamp <= ?"
                params.append(end_date)
            elif time_window == '7d':
                threshold = (datetime.utcnow() - timedelta(days=7)).isoformat()
                query += " AND prediction_timestamp >= ?"
                params.append(threshold)
            elif time_window == '30d':
                threshold = (datetime.utcnow() - timedelta(days=30)).isoformat()
                query += " AND prediction_timestamp >= ?"
                params.append(threshold)

            # Apply start date if specified
            if start_date:
                query += " AND prediction_timestamp >= ?"
                params.append(start_date)

            query += " ORDER BY prediction_timestamp DESC"

            cursor.execute(query, params)
            rows = cursor.fetchall()

            # Get status counts
            cursor.execute(
                """
                SELECT status, COUNT(*) as count
                FROM ml_predictions
                WHERE model_type = ?
                GROUP BY status
                """,
                (model_type,)
            )
            status_counts = {row['status']: row['count'] for row in cursor.fetchall()}

            conn.close()

            matched = [dict(row) for row in rows]

            if len(matched) < min_predictions:
                logger.warning(
                    f"Insufficient matched predictions for {model_type}: "
                    f"{len(matched)} < {min_predictions}"
                )
                return None

            # Extract arrays for calculation
            predicted = np.array([float(r['predicted_pnl_sol']) for r in matched])
            actual = np.array([float(r['actual_pnl_sol'] or 0) for r in matched])
            days_to_match = np.array([int(r['days_to_match'] or 0) for r in matched])

            # Calculate metrics
            errors = actual - predicted
            abs_errors = np.abs(errors)

            # Basic accuracy metrics
            mae = float(np.mean(abs_errors))
            rmse = float(np.sqrt(np.mean(errors ** 2)))

            # MAPE (handle division by zero)
            nonzero_actual = np.abs(actual) > 1e-8
            mape = float(np.mean(np.abs(errors[nonzero_actual] / actual[nonzero_actual])) * 100) if np.any(nonzero_actual) else 0.0

            # Correlation and R²
            if len(predicted) > 1 and np.std(predicted) > 0 and np.std(actual) > 0:
                correlation = float(np.corrcoef(predicted, actual)[0, 1])
                ss_res = np.sum(errors ** 2)
                ss_tot = np.sum((actual - np.mean(actual)) ** 2)
                r_squared = float(1 - (ss_res / ss_tot)) if ss_tot > 0 else 0.0
            else:
                correlation = 0.0
                r_squared = 0.0

            # Direction accuracy
            pred_direction = np.sign(predicted)
            actual_direction = np.sign(actual)
            direction_accuracy = float(np.mean(pred_direction == actual_direction))

            # Split by direction
            positive_mask = predicted > 0
            negative_mask = predicted < 0
            direction_positive_accuracy = float(np.mean(pred_direction[positive_mask] == actual_direction[positive_mask])) if np.any(positive_mask) else 0.0
            direction_negative_accuracy = float(np.mean(pred_direction[negative_mask] == actual_direction[negative_mask])) if np.any(negative_mask) else 0.0

            # Profit metrics
            profitable_mask = actual > 0
            profitable_prediction_rate = float(np.mean(profitable_mask)) if len(actual) > 0 else 0.0
            mean_predicted_profit = float(np.mean(predicted))
            mean_actual_profit = float(np.mean(actual))

            # Timing metrics
            mean_days_to_match = float(np.mean(days_to_match))
            median_days_to_match = float(np.median(days_to_match))

            # Data quality
            total_preds = status_counts.get('MATCHED', 0) + status_counts.get('PENDING', 0) + status_counts.get('EXPIRED', 0)
            missing_actual_rate = float(status_counts.get('PENDING', 0) / total_preds) if total_preds > 0 else 0.0

            # Error distribution
            if SCIPY_AVAILABLE:
                error_skewness = float(stats.skew(errors))
                error_kurtosis = float(stats.kurtosis(errors))
            else:
                error_skewness = 0.0
                error_kurtosis = 0.0

            percentile_90_error = float(np.percentile(abs_errors, 90))
            percentile_95_error = float(np.percentile(abs_errors, 95))
            percentile_99_error = float(np.percentile(abs_errors, 99))

            # Get status counts
            total_predictions = sum(status_counts.values())
            matched_predictions = status_counts.get('MATCHED', 0)
            pending_predictions = status_counts.get('PENDING', 0)
            expired_predictions = status_counts.get('EXPIRED', 0)

            return ValidationMetrics(
                model_type=model_type,
                time_window=time_window,
                total_predictions=total_predictions,
                matched_predictions=matched_predictions,
                pending_predictions=pending_predictions,
                expired_predictions=expired_predictions,
                mae=mae,
                rmse=rmse,
                mape=mape,
                correlation=correlation,
                r_squared=r_squared,
                direction_accuracy=direction_accuracy,
                direction_positive_accuracy=direction_positive_accuracy,
                direction_negative_accuracy=direction_negative_accuracy,
                profitable_prediction_rate=profitable_prediction_rate,
                mean_predicted_profit=mean_predicted_profit,
                mean_actual_profit=mean_actual_profit,
                mean_days_to_match=mean_days_to_match,
                median_days_to_match=median_days_to_match,
                missing_actual_rate=missing_actual_rate,
                error_skewness=error_skewness,
                error_kurtosis=error_kurtosis,
                percentile_90_error=percentile_90_error,
                percentile_95_error=percentile_95_error,
                percentile_99_error=percentile_99_error,
            )

        except Exception as e:
            logger.error(f"Failed to calculate metrics for {model_type}: {e}")
            return None

    def compare_models(
        self,
        model_types: List[str],
        time_window: str = '7d',
        min_predictions: int = 5
    ) -> Dict[str, ValidationMetrics]:
        """
        Compare metrics across multiple models.

        Args:
            model_types: List of model types to compare
            time_window: Time window for analysis
            min_predictions: Minimum predictions required per model

        Returns:
            Dictionary mapping model_type -> ValidationMetrics
        """
        results = {}

        for model_type in model_types:
            metrics = self.calculate_metrics(
                model_type=model_type,
                time_window=time_window,
                min_predictions=min_predictions
            )
            if metrics:
                results[model_type] = metrics

        return results

    def rank_models(
        self,
        metric: str = 'rmse',
        time_window: str = '7d',
        ascending: bool = True
    ) -> List[Tuple[str, float]]:
        """
        Rank models by a specific metric.

        Args:
            metric: Metric name ('rmse', 'mae', 'correlation', 'direction_accuracy')
            time_window: Time window for analysis
            ascending: Sort order (True = lower is better)

        Returns:
            List of (model_type, metric_value) tuples, sorted
        """
        # Get all model types
        try:
            conn = self._get_connection()
            cursor = conn.cursor()
            cursor.execute("SELECT DISTINCT model_type FROM ml_predictions WHERE status = 'MATCHED'")
            model_types = [row[0] for row in cursor.fetchall()]
            conn.close()
        except Exception as e:
            logger.error(f"Failed to get model types: {e}")
            return []

        # Calculate metrics for each model
        rankings = []
        for model_type in model_types:
            metrics = self.calculate_metrics(
                model_type=model_type,
                time_window=time_window,
                min_predictions=1  # Allow all models
            )
            if metrics and hasattr(metrics, metric):
                rankings.append((model_type, getattr(metrics, metric)))

        # Sort
        rankings.sort(key=lambda x: x[1], reverse=not ascending)
        return rankings

    def calculate_feature_importance_by_accuracy(
        self,
        model_type: str,
        time_window: str = '7d',
        top_n: int = 10
    ) -> List[Dict[str, Any]]:
        """
        Analyze which features correlate with prediction accuracy.

        Args:
            model_type: Model type to analyze
            time_window: Time window for analysis
            top_n: Number of top features to return

        Returns:
            List of feature importance results
        """
        try:
            conn = self._get_connection()
            cursor = conn.cursor()

            # Get matched predictions with features
            query = """
                SELECT predicted_pnl_sol, actual_pnl_sol, features_json
                FROM ml_predictions
                WHERE model_type = ?
                AND status = 'MATCHED'
                AND features_json IS NOT NULL
            """

            if time_window == '7d':
                threshold = (datetime.utcnow() - timedelta(days=7)).isoformat()
                query += " AND prediction_timestamp >= ?"
                cursor.execute(query, (model_type, threshold))
            elif time_window == '30d':
                threshold = (datetime.utcnow() - timedelta(days=30)).isoformat()
                query += " AND prediction_timestamp >= ?"
                cursor.execute(query, (model_type, threshold))
            else:
                cursor.execute(query, (model_type,))

            rows = cursor.fetchall()
            conn.close()

            if not rows:
                return []

            # Parse features and calculate errors
            feature_values = {}  # feature_name -> list of (value, error)
            errors = []

            for row in rows:
                predicted = float(row['predicted_pnl_sol'])
                actual = float(row['actual_pnl_sol'])
                error = abs(actual - predicted)
                errors.append(error)

                try:
                    features = json.loads(row['features_json'])
                    for feature_name, feature_value in features.items():
                        if isinstance(feature_value, (int, float)):
                            if feature_name not in feature_values:
                                feature_values[feature_name] = []
                            feature_values[feature_name].append((feature_value, error))
                except (json.JSONDecodeError, TypeError):
                    continue

            # Calculate correlation between feature values and errors
            results = []
            for feature_name, values in feature_values.items():
                if len(values) < 5:  # Need minimum samples
                    continue

                feature_vals = np.array([v[0] for v in values])
                error_vals = np.array([v[1] for v in values])

                # Calculate correlation
                if np.std(feature_vals) > 0 and np.std(error_vals) > 0:
                    correlation = float(np.corrcoef(feature_vals, error_vals)[0, 1])
                    results.append({
                        'feature': feature_name,
                        'error_correlation': correlation,
                        'sample_count': len(values),
                        'mean_feature_value': float(np.mean(feature_vals)),
                        'std_feature_value': float(np.std(feature_vals)),
                    })

            # Sort by absolute correlation (higher = more predictive of error)
            results.sort(key=lambda x: abs(x['error_correlation']), reverse=True)
            return results[:top_n]

        except Exception as e:
            logger.error(f"Failed to calculate feature importance: {e}")
            return []

    def get_time_series_metrics(
        self,
        model_type: str,
        days: int = 30,
        bucket_days: int = 7
    ) -> List[Dict[str, Any]]:
        """
        Get metrics over time for trend analysis.

        Args:
            model_type: Model type to analyze
            days: Total time period to analyze
            bucket_days: Size of time buckets

        Returns:
            List of metrics per time bucket
        """
        results = []
        end_date = datetime.utcnow()
        start_date = end_date - timedelta(days=days)

        current_start = start_date
        bucket_num = 0

        while current_start < end_date:
            current_end = current_start + timedelta(days=bucket_days)

            # Calculate metrics for this bucket
            metrics = self.calculate_metrics(
                model_type=model_type,
                time_window='all',
                min_predictions=1,
                start_date=current_start.isoformat(),
                end_date=current_end.isoformat()
            )

            if metrics:
                results.append({
                    'bucket': bucket_num,
                    'start_date': current_start.isoformat(),
                    'end_date': current_end.isoformat(),
                    'metrics': metrics.to_dict(),
                })

            current_start = current_end
            bucket_num += 1

        return results


# Global instance
_global_calculator = None


def get_metrics_calculator(db_path: Optional[str] = None) -> ValidationMetricsCalculator:
    """Get or create global metrics calculator instance."""
    global _global_calculator
    if _global_calculator is None:
        _global_calculator = ValidationMetricsCalculator(db_path)
    return _global_calculator
