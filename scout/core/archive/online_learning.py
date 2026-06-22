"""
Online Learning System for Scout

Enables models to adapt to changing market conditions.
This module provides:
- Incremental model updates as new data arrives
- Concept drift detection and alerts
- Automatic retraining triggers
- Adaptive model weighting

Usage:
    online_learner = OnlineLearner(base_model)
    online_learner.update(new_data)
    prediction = online_learner.predict(features)
"""

import logging
from datetime import datetime
from typing import Dict, Optional, Any
from collections import deque
import numpy as np

logger = logging.getLogger(__name__)


class ConceptDriftDetector:
    """
    Detects concept drift in model performance.

    Uses statistical process control to detect when model
    performance degrades significantly.
    """

    def __init__(
        self,
        window_size: int = 100,
        drift_threshold: float = 2.0,  # Standard deviations
        warning_threshold: float = 1.5,
        min_samples: int = 30
    ):
        """
        Initialize concept drift detector.

        Args:
            window_size: Size of sliding window for statistics
            drift_threshold: Standard deviations for drift detection
            warning_threshold: Standard deviations for warning
            min_samples: Minimum samples before detecting drift
        """
        self.window_size = window_size
        self.drift_threshold = drift_threshold
        self.warning_threshold = warning_threshold
        self.min_samples = min_samples

        # Performance tracking
        self.errors = deque(maxlen=window_size * 2)
        self.baseline_mean = None
        self.baseline_std = None
        self.baseline_samples = 0

        # Drift state
        self.drift_detected = False
        self.warning_detected = False
        self.last_drift_time = None
        self.drift_count = 0

    def update(self, error: float) -> Dict[str, Any]:
        """
        Update detector with new error value.

        Args:
            error: Prediction error (actual - predicted)

        Returns:
            Dictionary with drift status
        """
        self.errors.append(error)

        result = {
            'drift_detected': False,
            'warning_detected': False,
            'error': float(error),
            'error_mean': float(np.mean(self.errors)),
            'error_std': float(np.std(self.errors)) if len(self.errors) > 1 else 0.0,
        }

        # Need minimum samples
        if len(self.errors) < self.min_samples:
            return result

        # Calculate baseline if not set
        if self.baseline_mean is None:
            self._establish_baseline()

        # Check for drift
        if self.baseline_std is not None and self.baseline_std > 0:
            recent_errors = list(self.errors)[-self.window_size:]
            recent_mean = np.mean(recent_errors)

            # Z-score
            z_score = abs(recent_mean - self.baseline_mean) / self.baseline_std

            result['z_score'] = float(z_score)

            # Drift detection
            if z_score >= self.drift_threshold:
                self.drift_detected = True
                self.warning_detected = True
                self.last_drift_time = datetime.utcnow()
                self.drift_count += 1

                result['drift_detected'] = True
                result['warning_detected'] = True
                result['drift_count'] = self.drift_count

                logger.warning(
                    f"Concept drift detected! Z-score: {z_score:.2f}, "
                    f"Baseline: {self.baseline_mean:.4f}, Recent: {recent_mean:.4f}"
                )

            elif z_score >= self.warning_threshold:
                self.warning_detected = True
                result['warning_detected'] = True

        return result

    def _establish_baseline(self):
        """Establish baseline error statistics."""
        errors = list(self.errors)
        self.baseline_mean = np.mean(errors)
        self.baseline_std = np.std(errors)
        self.baseline_samples = len(errors)
        logger.info(f"Baseline established: mean={self.baseline_mean:.4f}, std={self.baseline_std:.4f}")

    def reset_baseline(self):
        """Reset baseline with current data."""
        self._establish_baseline()
        self.drift_detected = False
        self.drift_count = 0

    def get_status(self) -> Dict[str, Any]:
        """Get current detector status."""
        return {
            'drift_detected': self.drift_detected,
            'warning_detected': self.warning_detected,
            'last_drift_time': self.last_drift_time.isoformat() if self.last_drift_time else None,
            'drift_count': self.drift_count,
            'baseline_mean': float(self.baseline_mean) if self.baseline_mean else None,
            'baseline_std': float(self.baseline_std) if self.baseline_std else None,
            'baseline_samples': self.baseline_samples,
            'current_samples': len(self.errors),
        }


class OnlineLearner:
    """
    Online learning system for model adaptation.

    Features:
    - Incremental model updates
    - Concept drift detection
    - Automatic retraining triggers
    - Adaptive model weighting
    """

    def __init__(
        self,
        base_model: Any = None,
        model_type: str = "xgboost",
        update_frequency: int = 10,
        min_samples_for_update: int = 5,
        drift_threshold: float = 2.0,
        retrain_threshold: int = 3,
        max_samples: int = 1000
    ):
        """
        Initialize the online learner.

        Args:
            base_model: Initial model to adapt
            model_type: Type of model ("xgboost", "lightgbm", "linear")
            update_frequency: How often to update weights (in samples)
            min_samples_for_update: Minimum samples before first update
            drift_threshold: Standard deviations for drift detection
            retrain_threshold: Number of drift detections before retraining
            max_samples: Maximum samples to keep for retraining
        """
        self.base_model = base_model
        self.model_type = model_type
        self.update_frequency = update_frequency
        self.min_samples_for_update = min_samples_for_update
        self.retrain_threshold = retrain_threshold
        self.max_samples = max_samples

        # Data storage
        self.X_buffer = deque(maxlen=max_samples)
        self.y_buffer = deque(maxlen=max_samples)

        # Concept drift detector
        self.drift_detector = ConceptDriftDetector(
            drift_threshold=drift_threshold
        )

        # State tracking
        self.samples_seen = 0
        self.updates_performed = 0
        self.last_update_time = None
        self.last_retrain_time = None

        # Model state
        self.current_model = base_model
        self.model_version = 0

    def update(
        self,
        features: Dict[str, Any],
        actual_value: float,
        predicted_value: Optional[float] = None
    ) -> Dict[str, Any]:
        """
        Update the model with new data.

        Args:
            features: Feature dictionary
            actual_value: Actual target value
            predicted_value: Optional predicted value (for drift detection)

        Returns:
            Dictionary with update status
        """
        # Store data
        self.X_buffer.append(features)
        self.y_buffer.append(actual_value)
        self.samples_seen += 1

        result = {
            'samples_seen': self.samples_seen,
            'update_performed': False,
            'retrain_triggered': False,
            'drift_status': {},
        }

        # Check for concept drift if prediction available
        if predicted_value is not None:
            error = actual_value - predicted_value
            drift_status = self.drift_detector.update(error)
            result['drift_status'] = drift_status

            # Check if retraining is needed
            if drift_status.get('drift_detected'):
                if self.drift_detector.drift_count >= self.retrain_threshold:
                    result['retrain_triggered'] = True
                    logger.info("Retraining triggered due to concept drift")

        # Update model weights
        if self.samples_seen >= self.min_samples_for_update:
            if self.samples_seen % self.update_frequency == 0:
                update_result = self._update_weights()
                result['update_performed'] = True
                result['update_result'] = update_result
                self.updates_performed += 1
                self.last_update_time = datetime.utcnow()

        return result

    def _update_weights(self) -> Dict[str, Any]:
        """Update model weights with new data."""
        if not self.X_buffer or not self.y_buffer:
            return {'error': 'no_data'}

        try:
            # Convert buffers to arrays
            X = self._prepare_features(self.X_buffer)
            y = np.array(list(self.y_buffer))

            if X is None or len(X) != len(y):
                return {'error': 'data_mismatch'}

            # Update based on model type
            if self.model_type == "linear":
                result = self._update_linear(X, y)
            elif self.model_type == "xgboost":
                result = self._update_xgboost(X, y)
            elif self.model_type == "lightgbm":
                result = self._update_lightgbm(X, y)
            else:
                result = {'error': 'unsupported_model_type'}

            if 'error' not in result:
                self.model_version += 1

            return result

        except Exception as e:
            logger.error(f"Weight update failed: {e}")
            return {'error': str(e)}

    def _prepare_features(self, feature_buffer: deque) -> Optional[np.ndarray]:
        """Convert feature buffer to array."""
        if not feature_buffer:
            return None

        # Get feature names from first sample
        feature_names = list(feature_buffer[0].keys())

        # Build array
        X = []
        for features in feature_buffer:
            row = []
            for name in feature_names:
                value = features.get(name, 0.0)
                if value is None:
                    value = 0.0
                row.append(float(value))
            X.append(row)

        return np.array(X)

    def _update_linear(self, X: np.ndarray, y: np.ndarray) -> Dict[str, Any]:
        """Update linear model using incremental learning."""
        try:
            from sklearn.linear_model import SGDRegressor

            if not hasattr(self, 'linear_model') or self.linear_model is None:
                self.linear_model = SGDRegressor(
                    learning_rate='adaptive',
                    eta0=0.01,
                    max_iter=1000,
                    warm_start=True
                )

            # Partial fit
            self.linear_model.partial_fit(X, y)
            self.current_model = self.linear_model

            return {
                'status': 'success',
                'model_type': 'linear',
                'samples_used': len(X),
                'current_version': self.model_version,
            }

        except ImportError:
            return {'error': 'sklearn_not_available'}
        except Exception as e:
            return {'error': str(e)}

    def _update_xgboost(self, X: np.ndarray, y: np.ndarray) -> Dict[str, Any]:
        """Update XGBoost model (requires full retrain)."""
        try:
            import xgboost as xgb

            # For XGBoost, we need to retrain (no true incremental learning)
            # Use recent data for faster training
            recent_samples = min(len(X), 100)
            X_recent = X[-recent_samples:]
            y_recent = y[-recent_samples:]

            dtrain = xgb.DMatrix(X_recent, label=y_recent)

            params = {
                'objective': 'reg:squarederror',
                'max_depth': 6,
                'eta': 0.1,
            }

            self.current_model = xgb.train(params, dtrain, num_boost_round=50)

            return {
                'status': 'success',
                'model_type': 'xgboost',
                'samples_used': recent_samples,
                'current_version': self.model_version,
                'note': 'xgboost_retrained',
            }

        except ImportError:
            return {'error': 'xgboost_not_available'}
        except Exception as e:
            return {'error': str(e)}

    def _update_lightgbm(self, X: np.ndarray, y: np.ndarray) -> Dict[str, Any]:
        """Update LightGBM model."""
        try:
            import lightgbm as lgb

            recent_samples = min(len(X), 100)
            X_recent = X[-recent_samples:]
            y_recent = y[-recent_samples:]

            train_data = lgb.Dataset(X_recent, label=y_recent)

            params = {
                'objective': 'regression',
                'metric': 'rmse',
                'max_depth': 6,
                'learning_rate': 0.1,
                'verbose': -1,
            }

            self.current_model = lgb.train(
                params,
                train_data,
                num_boost_round=50
            )

            return {
                'status': 'success',
                'model_type': 'lightgbm',
                'samples_used': recent_samples,
                'current_version': self.model_version,
            }

        except ImportError:
            return {'error': 'lightgbm_not_available'}
        except Exception as e:
            return {'error': str(e)}

    def predict(self, features: Dict[str, Any]) -> Dict[str, Any]:
        """
        Make a prediction with the current model.

        Args:
            features: Feature dictionary

        Returns:
            Dictionary with prediction
        """
        if self.current_model is None:
            return {
                'error': 'no_model',
                'prediction': None,
            }

        try:
            # Prepare features
            X = self._prepare_single_features(features)

            if X is None:
                return {'error': 'feature_preparation_failed'}

            # Predict based on model type
            if self.model_type == "xgboost":
                import xgboost as xgb
                dmatrix = xgb.DMatrix(X)
                prediction = self.current_model.predict(dmatrix)
            elif self.model_type == "lightgbm":
                prediction = self.current_model.predict(X)
            elif self.model_type == "linear":
                prediction = self.current_model.predict(X)
            else:
                prediction = None

            return {
                'prediction': float(prediction[0]) if prediction is not None else None,
                'model_version': self.model_version,
                'samples_seen': self.samples_seen,
            }

        except Exception as e:
            logger.error(f"Prediction failed: {e}")
            return {'error': str(e)}

    def _prepare_single_features(self, features: Dict[str, Any]) -> Optional[np.ndarray]:
        """Prepare single feature dict for prediction."""
        if not self.X_buffer:
            # Use provided features
            feature_names = list(features.keys())
            X = [[float(features.get(name, 0.0)) for name in feature_names]]
        else:
            # Use features from buffer
            feature_names = list(self.X_buffer[0].keys())
            X = [[float(features.get(name, 0.0)) for name in feature_names]]

        return np.array(X)

    def trigger_retrain(self) -> bool:
        """
        Trigger a full model retrain.

        Returns:
            True if successful
        """
        if len(self.X_buffer) < 10:
            logger.warning("Insufficient data for retraining")
            return False

        try:
            result = self._update_weights()

            if 'error' not in result:
                self.last_retrain_time = datetime.utcnow()
                self.drift_detector.reset_baseline()
                logger.info("Model retrained successfully")
                return True

        except Exception as e:
            logger.error(f"Retrain failed: {e}")

        return False

    def get_status(self) -> Dict[str, Any]:
        """Get current learner status."""
        return {
            'samples_seen': self.samples_seen,
            'updates_performed': self.updates_performed,
            'model_version': self.model_version,
            'last_update_time': self.last_update_time.isoformat() if self.last_update_time else None,
            'last_retrain_time': self.last_retrain_time.isoformat() if self.last_retrain_time else None,
            'buffer_size': len(self.X_buffer),
            'drift_detector': self.drift_detector.get_status(),
        }


# Global online learner instances
_online_learners = {}


def get_online_learner(
    model_name: str = "default",
    model_type: str = "xgboost",
    **kwargs
) -> OnlineLearner:
    """
    Get or create online learner instance.

    Args:
        model_name: Name for the learner
        model_type: Type of model
        **kwargs: Additional arguments for OnlineLearner

    Returns:
        OnlineLearner instance
    """
    if model_name not in _online_learners:
        _online_learners[model_name] = OnlineLearner(
            model_type=model_type,
            **kwargs
        )

    return _online_learners[model_name]
