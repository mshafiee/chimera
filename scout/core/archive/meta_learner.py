"""
Meta-Learner Stacking for Scout

Combines multiple prediction models using stacking for improved accuracy.
This module provides:
- Ensemble combination of existing and new models
- Logistic regression meta-learner for optimal blending
- Dynamic model weighting based on recent performance
- Cross-validation for robust training

Usage:
    meta_learner = MetaLearner()
    prediction = meta_learner.predict_profitability(wallet_features)
"""

import json
import logging
import os
import pickle
import time
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Any
import numpy as np

logger = logging.getLogger(__name__)

# Try to import sklearn for meta-learner
try:
    from sklearn.linear_model import LogisticRegression, Ridge
    from sklearn.metrics import mean_squared_error, mean_absolute_error
    SKLEARN_AVAILABLE = True
except ImportError:
    SKLEARN_AVAILABLE = False
    logger.warning("scikit-learn not available - install with: pip install scikit-learn")


class MetaLearner:
    """
    Meta-learner for stacking ensemble predictions.

    Combines predictions from multiple base models using a meta-model
    that learns optimal weighting based on historical performance.

    Features:
    - Stacking with cross-validation
    - Dynamic model weighting
    - Performance-based model selection
    - Automatic fallback to best single model
    """

    def __init__(
        self,
        model_path: Optional[str] = None,
        meta_model_type: str = "ridge",
        cv_folds: int = 5,
        latency_budget_ms: int = 50
    ):
        """
        Initialize the meta-learner.

        Args:
            model_path: Path to save/load trained models
            meta_model_type: Type of meta-model ("ridge", "logistic", "mean")
            cv_folds: Number of cross-validation folds
            latency_budget_ms: Maximum acceptable inference latency
        """
        self.meta_model_type = meta_model_type
        self.cv_folds = cv_folds
        self.latency_budget_ms = latency_budget_ms

        # Set up model paths
        if model_path is None:
            model_dir = Path(os.getenv("SCOUT_MODEL_DIR", "../models"))
            model_dir.mkdir(parents=True, exist_ok=True)
            self.model_path = model_dir / "meta_learner.pkl"
            self.weights_path = model_dir / "meta_weights.json"
        else:
            self.model_path = Path(model_path)
            self.weights_path = self.model_path.parent / "meta_weights.json"

        # Base model tracking
        self.base_models = {}  # name -> model_instance
        self.base_model_performance = {}  # name -> performance_metrics
        self.base_model_weights = {}  # name -> weight

        # Meta-model
        self.meta_model = None

        # Feature names for meta-model
        self.meta_feature_names = []

        # Training metadata
        self.training_samples = 0
        self.last_trained = None
        self.last_updated = None

        # Latency tracking
        self.inference_times_ms = []

        # Configuration
        self.config = {
            'use_dynamic_weighting': os.getenv("SCOUT_META_DYNAMIC_WEIGHTING", "true").lower() == "true",
            'weight_decay_rate': float(os.getenv("SCOUT_META_WEIGHT_DECAY", "0.95")),
            'min_samples_for_weight_update': int(os.getenv("SCOUT_META_MIN_WEIGHT_UPDATE", "20")),
            'performance_window': int(os.getenv("SCOUT_META_PERFORMANCE_WINDOW", "50")),
        }

        # Load existing models
        self._load_meta_model()

    def register_base_model(
        self,
        name: str,
        model: Any,
        predict_func: Optional[callable] = None
    ):
        """
        Register a base model for ensemble.

        Args:
            name: Model identifier
            model: Model instance or predictor
            predict_func: Optional custom predict function (default: model.predict_profitability)
        """
        self.base_models[name] = {
            'model': model,
            'predict_func': predict_func or self._default_predict_func,
            'enabled': True,
        }

        logger.info(f"Registered base model: {name}")

    def _default_predict_func(self, model: Any, features: Dict[str, Any]) -> float:
        """Default prediction function for standard models."""
        if hasattr(model, 'predict_profitability'):
            result = model.predict_profitability(features)
            return result.get('predicted_pnl_sol', 0.0)
        elif hasattr(model, 'predict'):
            return float(model.predict(features))
        else:
            logger.warning(f"Model {type(model)} has no predict method")
            return 0.0

    def predict_profitability(
        self,
        wallet_features: Dict[str, Any],
        return_contributions: bool = False
    ) -> Dict[str, Any]:
        """
        Predict profitability using ensemble of base models.

        Args:
            wallet_features: Dictionary of wallet features
            return_contributions: Whether to return individual model contributions

        Returns:
            Dictionary with predicted_pnl_sol, confidence, and metadata
        """
        start_time = time.perf_counter()

        if not self.base_models:
            logger.error("No base models registered for meta-learner")
            return {
                'predicted_pnl_sol': 0.0,
                'confidence': 0.0,
                'error': 'no_base_models',
            }

        # Get predictions from all base models
        predictions = {}
        contributions = {}

        for name, model_info in self.base_models.items():
            if not model_info['enabled']:
                continue

            try:
                pred = model_info['predict_func'](model_info['model'], wallet_features)
                predictions[name] = float(pred)
                contributions[name] = float(pred)
            except Exception as e:
                logger.warning(f"Prediction failed for model {name}: {e}")
                contributions[name] = None

        if not predictions:
            logger.error("No base models produced valid predictions")
            return {
                'predicted_pnl_sol': 0.0,
                'confidence': 0.0,
                'error': 'all_models_failed',
            }

        # Combine predictions
        if self.meta_model is not None and SKLEARN_AVAILABLE:
            # Use trained meta-model
            combined_pred = self._meta_predict(predictions)
            method = 'meta_model'
        elif self.base_model_weights:
            # Use learned weights
            combined_pred = self._weighted_average(predictions)
            method = 'weighted_average'
        else:
            # Simple average
            combined_pred = sum(predictions.values()) / len(predictions)
            method = 'simple_average'

        # Calculate confidence
        confidence = self._calculate_confidence(predictions, combined_pred)

        # Track latency
        inference_time_ms = (time.perf_counter() - start_time) * 1000
        self._track_latency(inference_time_ms)

        result = {
            'predicted_pnl_sol': float(combined_pred),
            'confidence': float(confidence),
            'prediction_timestamp': datetime.utcnow().isoformat(),
            'combination_method': method,
            'base_predictions': predictions,
            'base_weights': self.base_model_weights.copy(),
            'inference_time_ms': round(inference_time_ms, 2),
            'training_samples': self.training_samples,
            'last_trained': self.last_trained,
        }

        if return_contributions:
            result['contributions'] = contributions

        return result

    def _meta_predict(self, predictions: Dict[str, float]) -> float:
        """Predict using trained meta-model."""
        if not SKLEARN_AVAILABLE or self.meta_model is None:
            return 0.0

        # Prepare features for meta-model
        features = []
        for name in self.meta_feature_names:
            features.append(predictions.get(name, 0.0))

        X = np.array(features).reshape(1, -1)

        try:
            prediction = self.meta_model.predict(X)[0]
            return float(prediction)
        except Exception as e:
            logger.warning(f"Meta-model prediction failed: {e}")
            # Fallback to weighted average
            return self._weighted_average(predictions)

    def _weighted_average(self, predictions: Dict[str, float]) -> float:
        """Calculate weighted average of predictions."""
        if not self.base_model_weights:
            return sum(predictions.values()) / len(predictions)

        total_weight = 0.0
        weighted_sum = 0.0

        for name, pred in predictions.items():
            weight = self.base_model_weights.get(name, 1.0)
            weighted_sum += pred * weight
            total_weight += weight

        if total_weight == 0:
            return sum(predictions.values()) / len(predictions)

        return weighted_sum / total_weight

    def _calculate_confidence(
        self,
        predictions: Dict[str, float],
        combined_pred: float
    ) -> float:
        """
        Calculate ensemble confidence.

        Based on:
        - Agreement between models (lower variance = higher confidence)
        - Number of models
        - Individual model confidence if available
        """
        if not predictions:
            return 0.0

        # Calculate prediction variance
        preds = list(predictions.values())
        variance = np.var(preds) if len(preds) > 1 else 0.0

        # Higher variance = lower confidence
        variance_penalty = min(1.0, variance / 0.5)  # Normalize

        # More models = higher confidence (up to a point)
        model_count_bonus = min(1.0, len(preds) / 5.0)

        confidence = 1.0 - variance_penalty * 0.5 + model_count_bonus * 0.1

        return max(0.1, min(1.0, confidence))

    def _track_latency(self, inference_time_ms: float):
        """Track inference latency."""
        self.inference_times_ms.append(inference_time_ms)

        # Keep only last 1000 measurements
        if len(self.inference_times_ms) > 1000:
            self.inference_times_ms = self.inference_times_ms[-1000:]

    def train_from_history(
        self,
        historical_data: List[Dict[str, Any]],
        base_model_predictors: Optional[Dict[str, Any]] = None,
        validation_split: float = 0.2
    ) -> Dict[str, Any]:
        """
        Train the meta-learner from historical data.

        Args:
            historical_data: List of dicts with features and actual_pnl_sol
            base_model_predictors: Dict of base model predictors
            validation_split: Fraction of data for validation

        Returns:
            Dictionary with training metrics
        """
        if not SKLEARN_AVAILABLE:
            logger.error("scikit-learn required for meta-learner training")
            return {'error': 'sklearn_not_available'}

        # Register base models if provided
        if base_model_predictors:
            for name, predictor in base_model_predictors.items():
                self.register_base_model(name, predictor)

        if len(self.base_models) < 2:
            logger.warning(f"Meta-learner requires at least 2 base models, got {len(self.base_models)}")
            return {'error': 'insufficient_base_models', 'min_required': 2}

        if len(historical_data) < 20:
            logger.warning(f"Insufficient training data: {len(historical_data)} < 20")
            return {'error': 'insufficient_data', 'min_required': 20}

        # Get predictions from all base models
        X_meta = []  # Meta-features (base model predictions)
        y_true = []  # True values

        logger.info(f"Generating meta-features from {len(historical_data)} samples...")

        for record in historical_data:
            features = {k: v for k, v in record.items() if k != 'actual_pnl_sol'}
            true_value = record.get('actual_pnl_sol')

            if true_value is None:
                continue

            # Get predictions from all base models
            base_predictions = {}
            for name, model_info in self.base_models.items():
                try:
                    pred = model_info['predict_func'](model_info['model'], features)
                    base_predictions[name] = float(pred)
                except Exception as e:
                    logger.debug(f"Base model {name} prediction failed: {e}")
                    base_predictions[name] = 0.0

            if base_predictions:
                X_meta.append(base_predictions)
                y_true.append(float(true_value))

        if len(X_meta) < 10:
            return {'error': 'insufficient_meta_features', 'min_required': 10}

        # Convert to arrays
        X_meta_df = []
        self.meta_feature_names = list(self.base_models.keys())
        for pred_dict in X_meta:
            row = [pred_dict.get(name, 0.0) for name in self.meta_feature_names]
            X_meta_df.append(row)

        X_meta = np.array(X_meta_df)
        y_true = np.array(y_true)

        # Split data
        split_idx = int(len(X_meta) * (1 - validation_split))
        X_train, X_val = X_meta[:split_idx], X_meta[split_idx:]
        y_train, y_val = y_true[:split_idx], y_true[split_idx:]

        # Train meta-model
        metrics = {}

        # Ridge regression meta-learner
        if self.meta_model_type in ['ridge', 'auto']:
            ridge_metrics = self._train_ridge_meta(X_train, y_train, X_val, y_val)
            metrics['ridge'] = ridge_metrics

        # Logistic regression (for classification-style weighting)
        if self.meta_model_type in ['logistic', 'auto']:
            logistic_metrics = self._train_logistic_meta(X_train, y_train, X_val, y_val)
            metrics['logistic'] = logistic_metrics

        # Simple average baseline
        metrics['simple_average'] = self._evaluate_simple_average(X_val, y_val)

        # Select best meta-model
        best_model = min(metrics.keys(), key=lambda k: metrics[k].get('val_rmse', float('inf')))

        if best_model == 'ridge' and SKLEARN_AVAILABLE:
            self.meta_model = self._train_ridge_meta(X_meta, y_true, X_meta, y_true)['model']
        elif best_model == 'logistic' and SKLEARN_AVAILABLE:
            self.meta_model = self._train_logistic_meta(X_meta, y_true, X_meta, y_true)['model']

        # Calculate base model weights
        self._calculate_base_model_weights(X_meta, y_true)

        self.training_samples = len(historical_data)
        self.last_trained = datetime.utcnow().isoformat()

        # Save models
        self._save_meta_model()

        return {
            'best_meta_model': best_model,
            'training_samples': self.training_samples,
            'last_trained': self.last_trained,
            'metrics': metrics,
            'base_model_weights': self.base_model_weights,
            'base_model_performance': self.base_model_performance,
        }

    def train_from_arrays(
        self,
        X_train: np.ndarray,
        y_train: np.ndarray,
        X_val: np.ndarray,
        y_val: np.ndarray,
        feature_names: List[str],
        base_model_predictors: Optional[Dict[str, Any]] = None
    ) -> Dict[str, Any]:
        """
        Train the meta-learner from pre-split numpy arrays.

        This method is designed to work with the TrainingDataLoader output.

        Args:
            X_train: Training features array
            y_train: Training targets array
            X_val: Validation features array
            y_val: Validation targets array
            feature_names: List of feature names
            base_model_predictors: Optional dict of base model predictors

        Returns:
            Dictionary with training metrics
        """
        if not SKLEARN_AVAILABLE:
            logger.error("scikit-learn required for meta-learner training")
            return {'error': 'sklearn_not_available'}

        # Register base models if provided
        if base_model_predictors:
            for name, predictor in base_model_predictors.items():
                self.register_base_model(name, predictor)

        if len(self.base_models) < 2:
            logger.warning(f"Meta-learner requires at least 2 base models, got {len(self.base_models)}")
            return {'error': 'insufficient_base_models', 'min_required': 2}

        if len(X_train) < 10:
            logger.warning(f"Insufficient training data: {len(X_train)} < 10")
            return {'error': 'insufficient_data', 'min_required': 10}

        # Set feature names
        self.meta_feature_names = feature_names

        # Generate meta-features from base models
        logger.info(f"Generating meta-features from {len(X_train) + len(X_val)} samples...")

        # Combine train and val for meta-feature generation
        X_all = np.vstack([X_train, X_val]) if len(X_val) > 0 else X_train
        y_all = np.concatenate([y_train, y_val]) if len(y_val) > 0 else y_train

        X_meta = []
        y_meta = []

        split_idx = len(X_train)

        for i, features_array in enumerate(X_all):
            # Convert array to feature dict
            features = dict(zip(feature_names, features_array))

            # Get predictions from all base models
            base_predictions = {}
            for name, model_info in self.base_models.items():
                try:
                    # Use gradient boost predictor's predict method
                    if hasattr(model_info['model'], 'predict_profitability'):
                        pred = model_info['model'].predict_profitability(features)
                        base_predictions[name] = float(pred.get('predicted_pnl_sol', 0.0))
                    elif hasattr(model_info['model'], 'predict'):
                        pred_array = model_info['model'].predict(features_array.reshape(1, -1))
                        base_predictions[name] = float(pred_array[0])
                    else:
                        base_predictions[name] = 0.0
                except Exception as e:
                    logger.debug(f"Base model {name} prediction failed: {e}")
                    base_predictions[name] = 0.0

            if base_predictions:
                X_meta.append(base_predictions)
                y_meta.append(float(y_all[i]))

        if len(X_meta) < 10:
            return {'error': 'insufficient_meta_features', 'min_required': 10}

        # Convert to arrays
        X_meta_df = []
        for pred_dict in X_meta:
            # Use base model names as feature names
            self.meta_feature_names = list(self.base_models.keys())
            row = [pred_dict.get(name, 0.0) for name in self.meta_feature_names]
            X_meta_df.append(row)

        X_meta = np.array(X_meta_df)
        y_meta = np.array(y_meta)

        # Split meta-features
        X_meta_train, X_meta_val = X_meta[:split_idx], X_meta[split_idx:]
        y_meta_train, y_meta_val = y_meta[:split_idx], y_meta[split_idx:]

        # Train meta-model
        metrics = {}

        # Ridge regression meta-learner
        if self.meta_model_type in ['ridge', 'auto']:
            ridge_metrics = self._train_ridge_meta(X_meta_train, y_meta_train, X_meta_val, y_meta_val)
            metrics['ridge'] = ridge_metrics

        # Logistic regression (for classification-style weighting)
        if self.meta_model_type in ['logistic', 'auto']:
            logistic_metrics = self._train_logistic_meta(X_meta_train, y_meta_train, X_meta_val, y_meta_val)
            metrics['logistic'] = logistic_metrics

        # Simple average baseline
        metrics['simple_average'] = self._evaluate_simple_average(X_meta_val, y_meta_val)

        # Select best meta-model
        best_model = min(metrics.keys(), key=lambda k: metrics[k].get('val_rmse', float('inf')))

        if best_model == 'ridge' and SKLEARN_AVAILABLE:
            self.meta_model = self._train_ridge_meta(X_meta, y_meta, X_meta, y_meta)['model']
        elif best_model == 'logistic' and SKLEARN_AVAILABLE:
            self.meta_model = self._train_logistic_meta(X_meta, y_meta, X_meta, y_meta)['model']

        # Calculate base model weights
        self._calculate_base_model_weights(X_meta, y_meta)

        self.training_samples = len(X_train) + len(X_val)
        self.last_trained = datetime.utcnow().isoformat()

        # Save models
        self._save_meta_model()

        return {
            'best_meta_model': best_model,
            'training_samples': self.training_samples,
            'last_trained': self.last_trained,
            'metrics': metrics,
            'base_model_weights': self.base_model_weights,
            'base_model_performance': self.base_model_performance,
        }

    def _train_ridge_meta(
        self,
        X_train: np.ndarray,
        y_train: np.ndarray,
        X_val: np.ndarray,
        y_val: np.ndarray
    ) -> Dict[str, Any]:
        """Train Ridge regression meta-model."""
        try:
            # Try multiple alpha values
            best_alpha = 1.0
            best_score = float('inf')

            for alpha in [0.01, 0.1, 1.0, 10.0]:
                model = Ridge(alpha=alpha)
                model.fit(X_train, y_train)

                val_preds = model.predict(X_val)
                val_rmse = np.sqrt(mean_squared_error(y_val, val_preds))

                if val_rmse < best_score:
                    best_score = val_rmse
                    best_alpha = alpha

            # Train with best alpha
            best_model = Ridge(alpha=best_alpha)
            best_model.fit(X_train, y_train)

            train_preds = best_model.predict(X_train)
            val_preds = best_model.predict(X_val)

            return {
                'model': best_model,
                'train_rmse': np.sqrt(mean_squared_error(y_train, train_preds)),
                'val_rmse': np.sqrt(mean_squared_error(y_val, val_preds)),
                'train_mae': mean_absolute_error(y_train, train_preds),
                'val_mae': mean_absolute_error(y_val, val_preds),
                'alpha': best_alpha,
                'feature_weights': dict(zip(self.meta_feature_names, best_model.coef_.tolist())),
            }

        except Exception as e:
            logger.error(f"Ridge training failed: {e}")
            return {'status': 'failed', 'error': str(e)}

    def _train_logistic_meta(
        self,
        X_train: np.ndarray,
        y_train: np.ndarray,
        X_val: np.ndarray,
        y_val: np.ndarray
    ) -> Dict[str, Any]:
        """Train logistic regression meta-model (for relative weighting)."""
        try:
            # Convert to binary classification: profitable vs not
            y_train_binary = (y_train > 0).astype(int)
            y_val_binary = (y_val > 0).astype(int)

            model = LogisticRegression(
                max_iter=1000,
                class_weight='balanced',
                random_state=42
            )
            model.fit(X_train, y_train_binary)

            model.predict_proba(X_train)[:, 1]
            model.predict_proba(X_val)[:, 1]

            return {
                'model': model,
                'train_accuracy': model.score(X_train, y_train_binary),
                'val_accuracy': model.score(X_val, y_val_binary),
                'feature_weights': dict(zip(self.meta_feature_names, model.coef_[0].tolist())),
            }

        except Exception as e:
            logger.error(f"Logistic training failed: {e}")
            return {'status': 'failed', 'error': str(e)}

    def _evaluate_simple_average(
        self,
        X_val: np.ndarray,
        y_val: np.ndarray
    ) -> Dict[str, Any]:
        """Evaluate simple average baseline."""
        try:
            # Simple average of all predictions
            avg_preds = np.mean(X_val, axis=1)

            return {
                'val_rmse': np.sqrt(mean_squared_error(y_val, avg_preds)),
                'val_mae': mean_absolute_error(y_val, avg_preds),
            }

        except Exception as e:
            logger.error(f"Simple average evaluation failed: {e}")
            return {'status': 'failed', 'error': str(e)}

    def _calculate_base_model_weights(
        self,
        X_meta: np.ndarray,
        y_true: np.ndarray
    ):
        """Calculate optimal weights for base models."""
        try:
            # Calculate individual model performance
            for i, name in enumerate(self.meta_feature_names):
                model_preds = X_meta[:, i]

                rmse = np.sqrt(mean_squared_error(y_true, model_preds))
                mae = mean_absolute_error(y_true, model_preds)

                # Store performance
                self.base_model_performance[name] = {
                    'rmse': float(rmse),
                    'mae': float(mae),
                }

                # Calculate weight (inverse of error)
                if rmse > 0:
                    weight = 1.0 / rmse
                else:
                    weight = 1.0

                self.base_model_weights[name] = weight

            # Normalize weights
            total_weight = sum(self.base_model_weights.values())
            if total_weight > 0:
                for name in self.base_model_weights:
                    self.base_model_weights[name] /= total_weight

            logger.info(f"Base model weights: {self.base_model_weights}")

        except Exception as e:
            logger.error(f"Weight calculation failed: {e}")

    def update_weights_online(
        self,
        wallet_features: Dict[str, Any],
        actual_pnl: float
    ):
        """
        Update model weights based on recent performance.

        Args:
            wallet_features: Features used for prediction
            actual_pnl: Actual realized PnL
        """
        if not self.config['use_dynamic_weighting']:
            return

        # Get recent predictions
        predictions = {}
        for name, model_info in self.base_models.items():
            try:
                pred = model_info['predict_func'](model_info['model'], wallet_features)
                predictions[name] = float(pred)
            except Exception:
                continue

        if not predictions:
            return

        # Update performance tracking
        for name, pred in predictions.items():
            error = abs(pred - actual_pnl)

            if name not in self.base_model_performance:
                self.base_model_performance[name] = {
                    'recent_errors': [],
                    'avg_error': error,
                }

            # Track recent errors
            perf = self.base_model_performance[name]
            perf['recent_errors'].append(error)

            # Keep only recent window
            window = self.config['performance_window']
            if len(perf['recent_errors']) > window:
                perf['recent_errors'] = perf['recent_errors'][-window:]

            # Update average error
            perf['avg_error'] = np.mean(perf['recent_errors'])

        # Recalculate weights
        total_inverse_error = 0.0
        for name, perf in self.base_model_performance.items():
            if 'avg_error' in perf and perf['avg_error'] > 0:
                self.base_model_weights[name] = 1.0 / perf['avg_error']
                total_inverse_error += self.base_model_weights[name]

        # Normalize
        if total_inverse_error > 0:
            for name in self.base_model_weights:
                self.base_model_weights[name] /= total_inverse_error

        self.last_updated = datetime.utcnow().isoformat()

        logger.debug(f"Updated weights online: {self.base_model_weights}")

    def _save_meta_model(self):
        """Save meta-model and weights to disk."""
        try:
            # Save meta-model
            if self.meta_model is not None:
                with open(self.model_path, 'wb') as f:
                    pickle.dump(self.meta_model, f)
                logger.info(f"Meta-model saved to {self.model_path}")

            # Save weights
            weights_data = {
                'base_model_weights': self.base_model_weights,
                'base_model_performance': self.base_model_performance,
                'meta_feature_names': self.meta_feature_names,
                'training_samples': self.training_samples,
                'last_trained': self.last_trained,
                'last_updated': self.last_updated,
                'config': self.config,
            }

            with open(self.weights_path, 'w') as f:
                json.dump(weights_data, f, indent=2)

            logger.info(f"Meta-learner weights saved to {self.weights_path}")

        except Exception as e:
            logger.error(f"Failed to save meta-learner: {e}")

    def _load_meta_model(self):
        """Load meta-model and weights from disk."""
        try:
            # Load weights
            if self.weights_path.exists():
                with open(self.weights_path, 'r') as f:
                    weights_data = json.load(f)

                self.base_model_weights = weights_data.get('base_model_weights', {})
                self.base_model_performance = weights_data.get('base_model_performance', {})
                self.meta_feature_names = weights_data.get('meta_feature_names', [])
                self.training_samples = weights_data.get('training_samples', 0)
                self.last_trained = weights_data.get('last_trained')
                self.last_updated = weights_data.get('last_updated')

                logger.info(f"Meta-learner weights loaded from {self.weights_path}")

            # Load meta-model
            if self.model_path.exists() and SKLEARN_AVAILABLE:
                with open(self.model_path, 'rb') as f:
                    self.meta_model = pickle.load(f)
                logger.info(f"Meta-model loaded from {self.model_path}")

        except Exception as e:
            logger.warning(f"Failed to load meta-learner: {e}")

    def get_latency_stats(self) -> Dict[str, Any]:
        """Get latency statistics."""
        if not self.inference_times_ms:
            return {'error': 'no_latency_data'}

        times = np.array(self.inference_times_ms)

        return {
            'count': len(times),
            'mean_ms': float(np.mean(times)),
            'p50_ms': float(np.percentile(times, 50)),
            'p95_ms': float(np.percentile(times, 95)),
            'p99_ms': float(np.percentile(times, 99)),
            'max_ms': float(np.max(times)),
            'min_ms': float(np.min(times)),
            'budget_ms': self.latency_budget_ms,
        }

    def get_model_performance(self) -> Dict[str, Any]:
        """Get performance summary of base models."""
        return {
            'base_models': list(self.base_models.keys()),
            'weights': self.base_model_weights,
            'performance': self.base_model_performance,
            'training_samples': self.training_samples,
            'last_trained': self.last_trained,
            'last_updated': self.last_updated,
        }


# Convenience function
def create_meta_learner(
    base_predictors: Dict[str, Any],
    model_type: str = "ridge"
) -> MetaLearner:
    """
    Create a meta-learner with registered base predictors.

    Args:
        base_predictors: Dictionary of name -> predictor
        model_type: Type of meta-model

    Returns:
        Configured MetaLearner instance
    """
    meta_learner = MetaLearner(meta_model_type=model_type)

    for name, predictor in base_predictors.items():
        meta_learner.register_base_model(name, predictor)

    return meta_learner
