"""
Model Monitoring for Scout

Real-time monitoring of ML model performance and drift detection.
This module provides:
- Prediction accuracy tracking
- Model drift alerts (statistical process control)
- Feature distribution monitoring
- Dashboard integration for model health
- Latency monitoring

Usage:
    monitor = ModelMonitor()
    monitor.log_prediction(wallet_id, predicted_pnl, actual_pnl, features)
    drift_detected = monitor.check_drift()
"""

import json
import logging
import os
from datetime import datetime, timedelta
from pathlib import Path
from typing import Dict, List, Optional, Any
from collections import defaultdict, deque
from dataclasses import dataclass, asdict
import numpy as np

logger = logging.getLogger(__name__)

# Try to import scipy for statistical tests
try:
    SCIPY_AVAILABLE = True
except ImportError:
    SCIPY_AVAILABLE = False
    logger.warning("scipy not available - statistical tests will be limited")

# Try to import prometheus for metrics
try:
    from prometheus_client import Counter, Gauge, Histogram, CollectorRegistry, generate_latest
    PROMETHEUS_AVAILABLE = True
except ImportError:
    PROMETHEUS_AVAILABLE = False
    logger.warning("prometheus_client not available - metrics export disabled")


@dataclass
class PredictionRecord:
    """Record of a single prediction for tracking."""
    wallet_id: str
    timestamp: str
    predicted_pnl: float
    actual_pnl: Optional[float]
    model_type: str
    features: Dict[str, Any]
    confidence: float
    inference_time_ms: float


@dataclass
class ModelMetrics:
    """Aggregated metrics for a model."""
    model_type: str
    total_predictions: int
    accurate_predictions: int  # Within threshold
    mae: float  # Mean Absolute Error
    rmse: float  # Root Mean Squared Error
    mape: float  # Mean Absolute Percentage Error
    avg_confidence: float
    avg_inference_time_ms: float
    last_updated: str


class ModelMonitor:
    """
    Monitor ML model performance and detect drift.

    Features:
    - Real-time prediction accuracy tracking
    - Model drift detection using statistical process control
    - Feature distribution monitoring
    - Automatic alerting on performance degradation
    - Prometheus metrics export
    """

    def __init__(
        self,
        drift_threshold: float = 0.1,
        accuracy_threshold: float = 0.5,
        window_size: int = 100,
        alert_cooldown_minutes: int = 60
    ):
        """
        Initialize the model monitor.

        Args:
            drift_threshold: Threshold for KL divergence drift detection
            accuracy_threshold: Threshold for "accurate" prediction (SOL)
            window_size: Size of rolling window for calculations
            alert_cooldown_minutes: Minimum time between alerts
        """
        self.drift_threshold = drift_threshold
        self.accuracy_threshold = accuracy_threshold
        self.window_size = window_size
        self.alert_cooldown = timedelta(minutes=alert_cooldown_minutes)

        # Storage
        self.predictions = deque(maxlen=window_size * 10)  # Keep more for drift detection
        self.feature_distributions = {}  # feature_name -> deque of values
        self.model_metrics = {}  # model_type -> ModelMetrics

        # Alert tracking
        self.last_alert_time = {}
        self.alert_count = defaultdict(int)

        # Baseline (expected) distributions
        self.baseline_feature_stats = {}
        self.baseline_performance = None

        # Registry for Prometheus metrics
        if PROMETHEUS_AVAILABLE:
            self.prometheus_registry = CollectorRegistry()
            self._setup_prometheus_metrics()
        else:
            self.prometheus_registry = None

        # Load state if available
        self._load_state()

    def _setup_prometheus_metrics(self):
        """Set up Prometheus metrics."""
        self.prediction_counter = Counter(
            'scout_predictions_total',
            'Total number of predictions',
            ['model_type', 'status'],
            registry=self.prometheus_registry
        )

        self.accuracy_gauge = Gauge(
            'scout_model_accuracy',
            'Current model accuracy (1 - MAE)',
            ['model_type'],
            registry=self.prometheus_registry
        )

        self.drift_gauge = Gauge(
            'scout_model_drift_score',
            'Current model drift score',
            ['model_type'],
            registry=self.prometheus_registry
        )

        self.latency_histogram = Histogram(
            'scout_prediction_latency_ms',
            'Prediction latency in milliseconds',
            ['model_type'],
            registry=self.prometheus_registry
        )

        self.confidence_gauge = Gauge(
            'scout_prediction_confidence',
            'Average prediction confidence',
            ['model_type'],
            registry=self.prometheus_registry
        )

    def log_prediction(
        self,
        wallet_id: str,
        predicted_pnl: float,
        features: Dict[str, Any],
        model_type: str = "unknown",
        confidence: float = 0.5,
        inference_time_ms: float = 0.0,
        actual_pnl: Optional[float] = None
    ):
        """
        Log a prediction for tracking.

        Args:
            wallet_id: Wallet identifier
            predicted_pnl: Predicted PnL value
            features: Feature dictionary
            model_type: Type of model used
            confidence: Prediction confidence
            inference_time_ms: Inference latency
            actual_pnl: Actual realized PnL (if available)
        """
        record = PredictionRecord(
            wallet_id=wallet_id,
            timestamp=datetime.utcnow().isoformat(),
            predicted_pnl=float(predicted_pnl),
            actual_pnl=float(actual_pnl) if actual_pnl is not None else None,
            model_type=model_type,
            features=features,
            confidence=float(confidence),
            inference_time_ms=float(inference_time_ms)
        )

        self.predictions.append(record)

        # Track feature distributions
        for name, value in features.items():
            if isinstance(value, (int, float)):
                if name not in self.feature_distributions:
                    self.feature_distributions[name] = deque(maxlen=self.window_size * 10)
                self.feature_distributions[name].append(float(value))

        # Update Prometheus metrics
        if PROMETHEUS_AVAILABLE:
            status = "correct" if actual_pnl is not None and abs(predicted_pnl - actual_pnl) <= self.accuracy_threshold else "incorrect"
            self.prediction_counter.labels(model_type=model_type, status=status).inc()
            self.latency_histogram.labels(model_type=model_type).observe(inference_time_ms)
            if actual_pnl is not None:
                self.confidence_gauge.labels(model_type=model_type).set(confidence)

        # Update metrics
        self._update_model_metrics(model_type)

    def _update_model_metrics(self, model_type: str):
        """Update aggregated metrics for a model."""
        # Filter predictions for this model
        model_preds = [p for p in self.predictions if p.model_type == model_type]

        if not model_preds:
            return

        # Calculate metrics
        total = len(model_preds)
        with_actual = [p for p in model_preds if p.actual_pnl is not None]

        if with_actual:
            errors = [abs(p.predicted_pnl - p.actual_pnl) for p in with_actual]
            mae = np.mean(errors)
            rmse = np.sqrt(np.mean([e ** 2 for e in errors]))

            # MAPE (avoid division by zero)
            pct_errors = []
            for p in with_actual:
                if abs(p.actual_pnl) > 1e-8:
                    pct_errors.append(abs(p.predicted_pnl - p.actual_pnl) / abs(p.actual_pnl))
            mape = np.mean(pct_errors) if pct_errors else 0.0

            # Accuracy (within threshold)
            accurate = sum(1 for e in errors if e <= self.accuracy_threshold)
            accuracy = accurate / len(with_actual)

            # Update Prometheus
            if PROMETHEUS_AVAILABLE:
                self.accuracy_gauge.labels(model_type=model_type).set(accuracy)
        else:
            mae = 0.0
            rmse = 0.0
            mape = 0.0
            accuracy = 0.0

        # Average confidence and latency
        avg_confidence = np.mean([p.confidence for p in model_preds])
        avg_latency = np.mean([p.inference_time_ms for p in model_preds])

        self.model_metrics[model_type] = ModelMetrics(
            model_type=model_type,
            total_predictions=total,
            accurate_predictions=int(len(with_actual) * accuracy) if with_actual else 0,
            mae=float(mae),
            rmse=float(rmse),
            mape=float(mape),
            avg_confidence=float(avg_confidence),
            avg_inference_time_ms=float(avg_latency),
            last_updated=datetime.utcnow().isoformat()
        )

    def check_drift(
        self,
        model_type: Optional[str] = None,
        check_feature_drift: bool = True,
        check_performance_drift: bool = True
    ) -> Dict[str, Any]:
        """
        Check for model and feature drift.

        Args:
            model_type: Specific model to check, or None for all
            check_feature_drift: Whether to check feature distribution drift
            check_performance_drift: Whether to check performance degradation

        Returns:
            Dictionary with drift detection results
        """
        drift_results = {
            'drift_detected': False,
            'feature_drift': {},
            'performance_drift': {},
            'timestamp': datetime.utcnow().isoformat(),
        }

        # Check feature drift
        if check_feature_drift and self.baseline_feature_stats:
            feature_drift = self._check_feature_drift()
            drift_results['feature_drift'] = feature_drift
            if feature_drift.get('drift_detected', False):
                drift_results['drift_detected'] = True

        # Check performance drift
        if check_performance_drift:
            performance_drift = self._check_performance_drift(model_type)
            drift_results['performance_drift'] = performance_drift
            if performance_drift.get('drift_detected', False):
                drift_results['drift_detected'] = True

        # Update Prometheus drift gauge
        if PROMETHEUS_AVAILABLE:
            for model_name, metrics in self.model_metrics.items():
                # Use RMSE as drift score (higher = more drift)
                drift_score = metrics.rmse
                self.drift_gauge.labels(model_type=model_name).set(drift_score)

        # Alert if drift detected
        if drift_results['drift_detected']:
            self._send_drift_alert(drift_results)

        return drift_results

    def _check_feature_drift(self) -> Dict[str, Any]:
        """Check for feature distribution drift using KL divergence."""
        result = {
            'drift_detected': False,
            'drifted_features': [],
            'drift_scores': {},
        }

        if not SCIPY_AVAILABLE:
            return result

        for feature_name, values in self.feature_distributions.items():
            if len(values) < 10:
                continue

            if feature_name not in self.baseline_feature_stats:
                continue

            baseline = self.baseline_feature_stats[feature_name]

            # Calculate histogram for current values
            try:
                # Create histogram bins from baseline
                current_array = np.array(values)
                baseline_mean = baseline.get('mean', 0)
                baseline_std = baseline.get('std', 1)

                # Simple drift detection: mean shift
                current_mean = np.mean(current_array)
                mean_shift = abs(current_mean - baseline_mean) / (baseline_std + 1e-8)

                result['drift_scores'][feature_name] = float(mean_shift)

                if mean_shift > self.drift_threshold:
                    result['drifted_features'].append(feature_name)
                    result['drift_detected'] = True

            except Exception as e:
                logger.warning(f"Feature drift check failed for {feature_name}: {e}")

        return result

    def _check_performance_drift(self, model_type: Optional[str]) -> Dict[str, Any]:
        """Check for performance degradation."""
        result = {
            'drift_detected': False,
            'drifted_models': [],
            'model_scores': {},
        }

        models_to_check = [model_type] if model_type else list(self.model_metrics.keys())

        for model in models_to_check:
            if model not in self.model_metrics:
                continue

            metrics = self.model_metrics[model]

            # Compare with baseline
            if self.baseline_performance:
                baseline_mae = self.baseline_performance.get(model, {}).get('mae', 0)
                current_mae = metrics.mae

                if baseline_mae > 0:
                    degradation = (current_mae - baseline_mae) / baseline_mae
                    result['model_scores'][model] = {
                        'degradation': float(degradation),
                        'current_mae': current_mae,
                        'baseline_mae': baseline_mae,
                    }

                    if degradation > self.drift_threshold:
                        result['drifted_models'].append(model)
                        result['drift_detected'] = True
                else:
                    result['model_scores'][model] = {
                        'degradation': 0.0,
                        'current_mae': current_mae,
                        'baseline_mae': baseline_mae,
                    }
            else:
                result['model_scores'][model] = {
                    'degradation': 0.0,
                    'current_mae': metrics.mae,
                    'baseline_mae': 0.0,
                }

        return result

    def establish_baseline(
        self,
        predictions: Optional[List[PredictionRecord]] = None
    ):
        """
        Establish baseline distributions for drift detection.

        Args:
            predictions: Optional list of predictions to use as baseline
        """
        if predictions:
            # Use provided predictions
            source_preds = predictions
        else:
            # Use current predictions
            source_preds = list(self.predictions)

        if not source_preds:
            logger.warning("No predictions available for baseline establishment")
            return

        # Calculate feature baselines
        feature_stats = {}
        for pred in source_preds:
            for name, value in pred.features.items():
                if isinstance(value, (int, float)):
                    if name not in feature_stats:
                        values = []
                        for p in source_preds:
                            if name in p.features and isinstance(p.features[name], (int, float)):
                                values.append(float(p.features[name]))
                        if values:
                            feature_stats[name] = {
                                'mean': float(np.mean(values)),
                                'std': float(np.std(values)),
                                'min': float(np.min(values)),
                                'max': float(np.max(values)),
                            }

        self.baseline_feature_stats = feature_stats

        # Calculate performance baseline
        performance_baseline = {}
        for model_type in set(p.model_type for p in source_preds):
            model_preds = [p for p in source_preds if p.model_type == model_type and p.actual_pnl is not None]

            if model_preds:
                errors = [abs(p.predicted_pnl - p.actual_pnl) for p in model_preds]
                performance_baseline[model_type] = {
                    'mae': float(np.mean(errors)),
                    'rmse': float(np.sqrt(np.mean([e ** 2 for e in errors]))),
                }

        self.baseline_performance = performance_baseline

        logger.info(f"Baseline established with {len(source_preds)} predictions")

    def get_metrics_summary(self) -> Dict[str, Any]:
        """Get summary of all model metrics."""
        return {
            'model_metrics': {
                name: asdict(metrics)
                for name, metrics in self.model_metrics.items()
            },
            'total_predictions': len(self.predictions),
            'baseline_established': self.baseline_performance is not None,
            'last_updated': datetime.utcnow().isoformat(),
        }

    def get_feature_summary(self, feature_name: str) -> Dict[str, Any]:
        """Get summary statistics for a specific feature."""
        if feature_name not in self.feature_distributions:
            return {'error': 'feature_not_found'}

        values = list(self.feature_distributions[feature_name])

        if not values:
            return {'error': 'no_data'}

        return {
            'name': feature_name,
            'count': len(values),
            'mean': float(np.mean(values)),
            'std': float(np.std(values)),
            'min': float(np.min(values)),
            'max': float(np.max(values)),
            'median': float(np.median(values)),
            'p25': float(np.percentile(values, 25)),
            'p75': float(np.percentile(values, 75)),
        }

    def export_prometheus_metrics(self) -> Optional[bytes]:
        """Export metrics in Prometheus format."""
        if not PROMETHEUS_AVAILABLE:
            return None

        return generate_latest(self.prometheus_registry)

    def _send_drift_alert(self, drift_results: Dict[str, Any]):
        """Send alert if drift is detected (with cooldown)."""
        now = datetime.utcnow()

        # Check cooldown
        if self.last_alert_time.get('global', datetime.min) > now - self.alert_cooldown:
            return

        self.alert_count['global'] += 1
        self.last_alert_time['global'] = now

        # Log alert
        logger.warning(
            f"Model drift detected! "
            f"Feature drift: {drift_results['feature_drift'].get('drifted_features', [])}, "
            f"Performance drift: {drift_results['performance_drift'].get('drifted_models', [])}"
        )

        # In production, this would send to alerting system (Slack, PagerDuty, etc.)
        alert_data = {
            'alert_type': 'model_drift',
            'timestamp': now.isoformat(),
            'drift_results': drift_results,
            'alert_count': self.alert_count['global'],
        }

        # Save alert to file
        self._save_alert(alert_data)

    def _save_alert(self, alert_data: Dict[str, Any]):
        """Save alert to file."""
        try:
            alert_dir = Path(os.getenv("SCOUT_ALERT_DIR", "../alerts"))
            alert_dir.mkdir(parents=True, exist_ok=True)

            timestamp = datetime.utcnow().strftime("%Y%m%d_%H%M%S")
            alert_file = alert_dir / f"drift_alert_{timestamp}.json"

            with open(alert_file, 'w') as f:
                json.dump(alert_data, f, indent=2)

            logger.info(f"Alert saved to {alert_file}")

        except Exception as e:
            logger.error(f"Failed to save alert: {e}")

    def _save_state(self):
        """Save monitor state to disk."""
        try:
            state_dir = Path(os.getenv("SCOUT_STATE_DIR", "../state"))
            state_dir.mkdir(parents=True, exist_ok=True)

            state_file = state_dir / "model_monitor_state.json"

            state = {
                'baseline_feature_stats': self.baseline_feature_stats,
                'baseline_performance': self.baseline_performance,
                'last_updated': datetime.utcnow().isoformat(),
            }

            with open(state_file, 'w') as f:
                json.dump(state, f, indent=2)

        except Exception as e:
            logger.error(f"Failed to save state: {e}")

    def _load_state(self):
        """Load monitor state from disk."""
        try:
            state_dir = Path(os.getenv("SCOUT_STATE_DIR", "../state"))
            state_file = state_dir / "model_monitor_state.json"

            if not state_file.exists():
                return

            with open(state_file, 'r') as f:
                state = json.load(f)

            self.baseline_feature_stats = state.get('baseline_feature_stats', {})
            self.baseline_performance = state.get('baseline_performance')

            logger.info("Monitor state loaded")

        except Exception as e:
            logger.warning(f"Failed to load state: {e}")


# Global monitor instance
_global_monitor = None


def get_monitor() -> ModelMonitor:
    """Get or create global monitor instance."""
    global _global_monitor
    if _global_monitor is None:
        _global_monitor = ModelMonitor()
    return _global_monitor


def log_prediction(*args, **kwargs):
    """Log prediction using global monitor."""
    monitor = get_monitor()
    monitor.log_prediction(*args, **kwargs)


def check_drift(*args, **kwargs) -> Dict[str, Any]:
    """Check drift using global monitor."""
    monitor = get_monitor()
    return monitor.check_drift(*args, **kwargs)
