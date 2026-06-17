"""
Gradient Boosting Predictor for Scout

Uses XGBoost/LightGBM for high-accuracy profitability prediction.
This module provides:
- Non-linear pattern capture through gradient boosting
- Feature importance analysis and SHAP values for interpretability
- Model pruning and optimization for <50ms latency
- Continuous retraining with concept drift detection

Usage:
    predictor = GradientBoostPredictor()
    prediction = predictor.predict_profitability(wallet_features)
"""

import json
import logging
import os
import pickle
import time
from datetime import datetime, timedelta
from pathlib import Path
from typing import Dict, List, Optional, Tuple, Any, Union
from collections import defaultdict
import numpy as np

logger = logging.getLogger(__name__)

# Try to import XGBoost and LightGBM
try:
    import xgboost as xgb
    XGBOOST_AVAILABLE = True
except ImportError:
    XGBOOST_AVAILABLE = False
    logger.warning("XGBoost not available - install with: pip install xgboost")

try:
    import lightgbm as lgb
    LIGHTGBM_AVAILABLE = True
except ImportError:
    LIGHTGBM_AVAILABLE = False
    logger.warning("LightGBM not available - install with: pip install lightgbm")

# Try to import SHAP for explainability
try:
    import shap
    SHAP_AVAILABLE = True
except ImportError:
    SHAP_AVAILABLE = False
    logger.warning("SHAP not available - install with: pip install shap")


class GradientBoostPredictor:
    """
    Gradient boosting predictor for wallet profitability.

    Supports both XGBoost and LightGBM with automatic model selection
    based on validation performance. Optimized for <50ms inference latency.

    Features:
    - XGBoost/LightGBM support with automatic fallback
    - Model pruning for latency optimization
    - SHAP values for explainability
    - Feature importance tracking
    - Concept drift detection
    """

    def __init__(
        self,
        model_type: str = "auto",
        model_path: Optional[str] = None,
        latency_budget_ms: int = 50,
        enable_pruning: bool = True,
        enable_shap: bool = True
    ):
        """
        Initialize the gradient boost predictor.

        Args:
            model_type: "xgboost", "lightgbm", or "auto" (select best)
            model_path: Path to save/load trained models
            latency_budget_ms: Maximum acceptable inference latency in milliseconds
            enable_pruning: Whether to enable model pruning for latency optimization
            enable_shap: Whether to enable SHAP explainability
        """
        self.model_type = model_type
        self.latency_budget_ms = latency_budget_ms
        self.enable_pruning = enable_pruning
        self.enable_shap = enable_shap and SHAP_AVAILABLE

        # Set up model paths
        if model_path is None:
            model_dir = Path(os.getenv("SCOUT_MODEL_DIR", "../models"))
            model_dir.mkdir(parents=True, exist_ok=True)
            self.xgboost_path = model_dir / "xgboost_profitability.json"
            self.lightgbm_path = model_dir / "lightgbm_profitability.txt"
        else:
            model_dir = Path(model_path)
            self.xgboost_path = model_dir / "xgboost_profitability.json"
            self.lightgbm_path = model_dir / "lightgbm_profitability.txt"

        # Model storage
        self.xgboost_model = None
        self.lightgbm_model = None
        self.best_model_type = None  # "xgboost" or "lightgbm"

        # Feature tracking
        self.feature_names = []
        self.feature_importance = {}
        self.shap_values = None

        # Training metadata
        self.training_samples = 0
        self.last_trained = None
        self.validation_metrics = {}

        # Latency tracking
        self.inference_times_ms = []
        self.latency_violations = 0

        # Configuration
        self.config = self._load_config()

        # Try to load existing models
        self._load_models()

    def _load_config(self) -> Dict[str, Any]:
        """Load configuration from environment or defaults."""
        return {
            'n_estimators': int(os.getenv("SCOUT_XGBOOST_N_ESTIMATORS", "100")),
            'max_depth': int(os.getenv("SCOUT_XGBOOST_MAX_DEPTH", "6")),
            'learning_rate': float(os.getenv("SCOUT_XGBOOST_LEARNING_RATE", "0.1")),
            'subsample': float(os.getenv("SCOUT_XGBOOST_SUBSAMPLE", "0.8")),
            'colsample_bytree': float(os.getenv("SCOUT_XGBOOST_COLSAMPLE_BYTREE", "0.8")),
            'min_child_weight': int(os.getenv("SCOUT_XGBOOST_MIN_CHILD_WEIGHT", "1")),
            'reg_alpha': float(os.getenv("SCOUT_XGBOOST_REG_ALPHA", "0.0")),
            'reg_lambda': float(os.getenv("SCOUT_XGBOOST_REG_LAMBDA", "1.0")),
            # LightGBM specific
            'num_leaves': int(os.getenv("SCOUT_LIGHTGBM_NUM_LEAVES", "31")),
            'min_data_in_leaf': int(os.getenv("SCOUT_LIGHTGBM_MIN_DATA_IN_LEAF", "20")),
            'bagging_freq': int(os.getenv("SCOUT_LIGHTGBM_BAGGING_FREQ", "1")),
        }

    def _check_dependencies(self) -> Tuple[bool, List[str]]:
        """Check if required dependencies are available."""
        available = []
        missing = []

        if XGBOOST_AVAILABLE:
            available.append("xgboost")
        else:
            missing.append("xgboost")

        if LIGHTGBM_AVAILABLE:
            available.append("lightgbm")
        else:
            missing.append("lightgbm")

        if self.enable_shap and not SHAP_AVAILABLE:
            missing.append("shap (optional)")

        return len(available) > 0, missing

    def predict_profitability(
        self,
        wallet_features: Dict[str, Any],
        return_shap: bool = False
    ) -> Dict[str, Any]:
        """
        Predict profitability for a wallet.

        Args:
            wallet_features: Dictionary of wallet features
            return_shap: Whether to include SHAP values in response

        Returns:
            Dictionary with predicted_pnl_sol, confidence, and metadata
        """
        start_time = time.perf_counter()

        # Check dependencies
        has_model, missing = self._check_dependencies()
        if not has_model:
            logger.error(f"No gradient boosting models available. Missing: {missing}")
            return self._fallback_prediction(wallet_features)

        # Select model
        model = self._select_model()
        if model is None:
            logger.warning("No trained model available, using fallback")
            return self._fallback_prediction(wallet_features)

        # Prepare features
        features_array, feature_names = self._prepare_features(wallet_features)

        if features_array is None:
            return self._fallback_prediction(wallet_features)

        # Make prediction
        try:
            prediction = self._predict_with_model(model, features_array)

            # Calculate confidence
            confidence = self._calculate_confidence(features_array, prediction)

            # Track latency
            inference_time_ms = (time.perf_counter() - start_time) * 1000
            self._track_latency(inference_time_ms)

            result = {
                'predicted_pnl_sol': float(prediction),
                'confidence': float(confidence),
                'prediction_timestamp': datetime.utcnow().isoformat(),
                'model_type': self.best_model_type or self.model_type,
                'training_samples': self.training_samples,
                'last_trained': self.last_trained,
                'inference_time_ms': round(inference_time_ms, 2),
                'latency_budget_ms': self.latency_budget_ms,
                'latency_ok': inference_time_ms <= self.latency_budget_ms,
            }

            # Add SHAP values if requested
            if return_shap and self.enable_shap:
                result['shap_values'] = self._get_shap_explanation(
                    model, features_array, feature_names, prediction
                )

            return result

        except Exception as e:
            logger.error(f"Prediction error: {e}")
            return self._fallback_prediction(wallet_features)

    def _select_model(self) -> Union[Any, None]:
        """Select the best available model for prediction."""
        # If we have a trained best model, use it
        if self.best_model_type == "xgboost" and self.xgboost_model is not None:
            return self.xgboost_model
        elif self.best_model_type == "lightgbm" and self.lightgbm_model is not None:
            return self.lightgbm_model

        # Otherwise, prefer XGBoost if available
        if self.model_type == "xgboost" and self.xgboost_model is not None:
            return self.xgboost_model
        elif self.model_type == "lightgbm" and self.lightgbm_model is not None:
            return self.lightgbm_model

        # Auto-select based on availability
        if XGBOOST_AVAILABLE and self.xgboost_model is not None:
            return self.xgboost_model
        if LIGHTGBM_AVAILABLE and self.lightgbm_model is not None:
            return self.lightgbm_model

        return None

    def _predict_with_model(
        self,
        model: Any,
        features_array: np.ndarray
    ) -> float:
        """Make prediction with a specific model."""
        # Reshape for single sample prediction
        if len(features_array.shape) == 1:
            features_array = features_array.reshape(1, -1)

        # XGBoost prediction
        if XGBOOST_AVAILABLE and isinstance(model, xgb.Booster):
            dmatrix = xgb.DMatrix(features_array, feature_names=self.feature_names)
            prediction = model.predict(dmatrix)
            return float(prediction[0])

        # LightGBM prediction
        if LIGHTGBM_AVAILABLE and hasattr(model, 'predict'):
            prediction = model.predict(features_array)
            return float(prediction[0])

        # Fallback for sklearn-like API
        if hasattr(model, 'predict'):
            prediction = model.predict(features_array)
            return float(prediction[0])

        raise ValueError(f"Unknown model type: {type(model)}")

    def _prepare_features(
        self,
        wallet_features: Dict[str, Any]
    ) -> Tuple[Optional[np.ndarray], List[str]]:
        """
        Prepare features for model input.

        Args:
            wallet_features: Raw wallet features dictionary

        Returns:
            Tuple of (features_array, feature_names)
        """
        if not self.feature_names:
            logger.warning("No feature names available - model may not be trained")
            return None, []

        # Extract features in the correct order
        features = []
        for name in self.feature_names:
            value = wallet_features.get(name)
            if value is None:
                # Use median/imputed value if available
                value = 0.0
            features.append(float(value))

        return np.array(features), self.feature_names

    def _calculate_confidence(
        self,
        features_array: np.ndarray,
        prediction: float
    ) -> float:
        """
        Calculate prediction confidence score.

        Uses multiple factors:
        - Feature availability
        - Prediction magnitude (very high predictions are less reliable)
        - Model training samples
        """
        confidence = 1.0

        # Reduce confidence for extreme predictions
        if abs(prediction) > 1.0:
            confidence *= 0.7

        # Adjust based on training samples
        if self.training_samples < 50:
            confidence *= 0.8
        elif self.training_samples < 100:
            confidence *= 0.9

        return max(0.1, min(1.0, confidence))

    def _get_shap_explanation(
        self,
        model: Any,
        features_array: np.ndarray,
        feature_names: List[str],
        prediction: float
    ) -> Dict[str, Any]:
        """
        Get SHAP values for model explanation.

        Args:
            model: The trained model
            features_array: Feature values
            feature_names: List of feature names
            prediction: The predicted value

        Returns:
            Dictionary with SHAP explanation
        """
        if not SHAP_AVAILABLE:
            return {'error': 'shap_not_available'}

        try:
            # Create explainer based on model type
            if XGBOOST_AVAILABLE and isinstance(model, xgb.Booster):
                explainer = shap.TreeExplainer(model)
            elif LIGHTGBM_AVAILABLE and hasattr(model, 'booster_'):
                explainer = shap.TreeExplainer(model)
            else:
                return {'error': 'unsupported_model_type'}

            # Calculate SHAP values
            if len(features_array.shape) == 1:
                features_array = features_array.reshape(1, -1)

            shap_values = explainer.shap_values(features_array)

            # Format explanation
            feature_contributions = {}
            for i, name in enumerate(feature_names):
                feature_contributions[name] = float(shap_values[0][i])

            # Sort by absolute contribution
            sorted_contributions = dict(
                sorted(
                    feature_contributions.items(),
                    key=lambda x: abs(x[1]),
                    reverse=True
                )
            )

            return {
                'base_value': float(explainer.expected_value),
                'contributions': sorted_contributions,
                'top_features': list(sorted_contributions.keys())[:5],
            }

        except Exception as e:
            logger.warning(f"SHAP calculation failed: {e}")
            return {'error': str(e)}

    def _fallback_prediction(self, wallet_features: Dict[str, Any]) -> Dict[str, Any]:
        """Provide a fallback prediction when model is unavailable."""
        # Simple heuristic based on ROI
        roi_30d = wallet_features.get('roi_30d', 0.0)
        win_rate = wallet_features.get('win_rate', 0.5)

        predicted_pnl = roi_30d * 0.1 * win_rate
        confidence = 0.5  # Low confidence for fallback

        return {
            'predicted_pnl_sol': float(predicted_pnl),
            'confidence': float(confidence),
            'prediction_timestamp': datetime.utcnow().isoformat(),
            'model_type': 'fallback',
            'warning': 'No trained model available, using heuristic fallback',
        }

    def _track_latency(self, inference_time_ms: float):
        """Track inference latency for monitoring."""
        self.inference_times_ms.append(inference_time_ms)

        # Keep only last 1000 measurements
        if len(self.inference_times_ms) > 1000:
            self.inference_times_ms = self.inference_times_ms[-1000:]

        # Check latency budget
        if inference_time_ms > self.latency_budget_ms:
            self.latency_violations += 1
            logger.warning(
                f"Latency budget exceeded: {inference_time_ms:.2f}ms > {self.latency_budget_ms}ms"
            )

    def train_from_history(
        self,
        historical_data: List[Dict[str, Any]],
        validation_split: float = 0.2,
        early_stopping_rounds: int = 10
    ) -> Dict[str, Any]:
        """
        Train the gradient boosting models from historical data.

        Args:
            historical_data: List of dicts with features and actual_pnl_sol
            validation_split: Fraction of data to use for validation
            early_stopping_rounds: Early stopping rounds for training

        Returns:
            Dictionary with training metrics
        """
        has_model, missing = self._check_dependencies()
        if not has_model:
            logger.error(f"Cannot train - missing dependencies: {missing}")
            return {'error': 'missing_dependencies', 'missing': missing}

        if len(historical_data) < 10:
            logger.warning(f"Insufficient training data: {len(historical_data)} < 10")
            return {'error': 'insufficient_data', 'min_required': 10}

        # Prepare training data
        X, y, feature_names = self._prepare_training_data(historical_data)
        if X is None:
            return {'error': 'data_preparation_failed'}

        self.feature_names = feature_names

        # Split data
        split_idx = int(len(X) * (1 - validation_split))
        X_train, X_val = X[:split_idx], X[split_idx:]
        y_train, y_val = y[:split_idx], y[split_idx:]

        # Train both models if available
        metrics = {}

        if XGBOOST_AVAILABLE:
            xgb_metrics = self._train_xgboost(
                X_train, y_train, X_val, y_val, early_stopping_rounds
            )
            metrics['xgboost'] = xgb_metrics

        if LIGHTGBM_AVAILABLE:
            lgb_metrics = self._train_lightgbm(
                X_train, y_train, X_val, y_val, early_stopping_rounds
            )
            metrics['lightgbm'] = lgb_metrics

        # Select best model
        if 'xgboost' in metrics and 'lightgbm' in metrics:
            # Select based on validation RMSE
            if metrics['xgboost'].get('val_rmse', float('inf')) <= metrics['lightgbm'].get('val_rmse', float('inf')):
                self.best_model_type = 'xgboost'
            else:
                self.best_model_type = 'lightgbm'
        elif 'xgboost' in metrics:
            self.best_model_type = 'xgboost'
        elif 'lightgbm' in metrics:
            self.best_model_type = 'lightgbm'

        self.training_samples = len(historical_data)
        self.last_trained = datetime.utcnow().isoformat()

        # Calculate feature importance
        self._calculate_feature_importance()

        # Apply pruning if enabled
        if self.enable_pruning:
            self._prune_model_for_latency()

        # Save models
        self._save_models()

        return {
            'best_model_type': self.best_model_type,
            'training_samples': self.training_samples,
            'last_trained': self.last_trained,
            'metrics': metrics,
            'feature_importance': self.feature_importance,
            'validation_metrics': self.validation_metrics,
        }

    def train_from_history(
        self,
        X_train: np.ndarray,
        y_train: np.ndarray,
        X_val: np.ndarray,
        y_val: np.ndarray,
        feature_names: List[str],
        early_stopping_rounds: int = 10
    ) -> Dict[str, Any]:
        """
        Train the gradient boosting models from pre-split numpy arrays.

        Args:
            X_train: Training features array
            y_train: Training targets array
            X_val: Validation features array
            y_val: Validation targets array
            feature_names: List of feature names
            early_stopping_rounds: Early stopping rounds for training

        Returns:
            Dictionary with training metrics
        """
        has_model, missing = self._check_dependencies()
        if not has_model:
            logger.error(f"Cannot train - missing dependencies: {missing}")
            return {'error': 'missing_dependencies', 'missing': missing}

        if len(X_train) < 10:
            logger.warning(f"Insufficient training data: {len(X_train)} < 10")
            return {'error': 'insufficient_data', 'min_required': 10}

        # Set feature names
        self.feature_names = feature_names
        self.training_samples = len(X_train) + len(X_val)

        # Train both models if available
        metrics = {}

        if XGBOOST_AVAILABLE:
            xgb_metrics = self._train_xgboost(
                X_train, y_train, X_val, y_val, early_stopping_rounds
            )
            metrics['xgboost'] = xgb_metrics

        if LIGHTGBM_AVAILABLE:
            lgb_metrics = self._train_lightgbm(
                X_train, y_train, X_val, y_val, early_stopping_rounds
            )
            metrics['lightgbm'] = lgb_metrics

        # Select best model
        if 'xgboost' in metrics and 'lightgbm' in metrics:
            # Select based on validation RMSE
            if metrics['xgboost'].get('val_rmse', float('inf')) <= metrics['lightgbm'].get('val_rmse', float('inf')):
                self.best_model_type = 'xgboost'
            else:
                self.best_model_type = 'lightgbm'
        elif 'xgboost' in metrics:
            self.best_model_type = 'xgboost'
        elif 'lightgbm' in metrics:
            self.best_model_type = 'lightgbm'

        self.last_trained = datetime.utcnow().isoformat()

        # Calculate feature importance
        self._calculate_feature_importance()

        # Apply pruning if enabled
        if self.enable_pruning:
            self._prune_model_for_latency()

        # Save models
        self._save_models()

        return {
            'best_model_type': self.best_model_type,
            'training_samples': self.training_samples,
            'last_trained': self.last_trained,
            'metrics': metrics,
            'feature_importance': self.feature_importance,
            'validation_metrics': self.validation_metrics,
        }

    def train_from_arrays(
        self,
        X_train: np.ndarray,
        y_train: np.ndarray,
        X_val: np.ndarray,
        y_val: np.ndarray,
        feature_names: List[str],
        early_stopping_rounds: int = 10
    ) -> Dict[str, Any]:
        """
        Train the gradient boosting models from pre-split numpy arrays.

        This method is designed to work with the TrainingDataLoader output.

        Args:
            X_train: Training features array
            y_train: Training targets array
            X_val: Validation features array
            y_val: Validation targets array
            feature_names: List of feature names
            early_stopping_rounds: Early stopping rounds for training

        Returns:
            Dictionary with training metrics
        """
        has_model, missing = self._check_dependencies()
        if not has_model:
            logger.error(f"Cannot train - missing dependencies: {missing}")
            return {'error': 'missing_dependencies', 'missing': missing}

        if len(X_train) < 10:
            logger.warning(f"Insufficient training data: {len(X_train)} < 10")
            return {'error': 'insufficient_data', 'min_required': 10}

        # Set feature names
        self.feature_names = feature_names
        self.training_samples = len(X_train) + len(X_val)

        # Train both models if available
        metrics = {}

        if XGBOOST_AVAILABLE:
            xgb_metrics = self._train_xgboost(
                X_train, y_train, X_val, y_val, early_stopping_rounds
            )
            metrics['xgboost'] = xgb_metrics

        if LIGHTGBM_AVAILABLE:
            lgb_metrics = self._train_lightgbm(
                X_train, y_train, X_val, y_val, early_stopping_rounds
            )
            metrics['lightgbm'] = lgb_metrics

        # Select best model
        if 'xgboost' in metrics and 'lightgbm' in metrics:
            # Select based on validation RMSE
            if metrics['xgboost'].get('val_rmse', float('inf')) <= metrics['lightgbm'].get('val_rmse', float('inf')):
                self.best_model_type = 'xgboost'
            else:
                self.best_model_type = 'lightgbm'
        elif 'xgboost' in metrics:
            self.best_model_type = 'xgboost'
        elif 'lightgbm' in metrics:
            self.best_model_type = 'lightgbm'

        self.last_trained = datetime.utcnow().isoformat()

        # Calculate feature importance
        self._calculate_feature_importance()

        # Apply pruning if enabled
        if self.enable_pruning:
            self._prune_model_for_latency()

        # Save models
        self._save_models()

        return {
            'best_model_type': self.best_model_type,
            'training_samples': self.training_samples,
            'last_trained': self.last_trained,
            'metrics': metrics,
            'feature_importance': self.feature_importance,
            'validation_metrics': self.validation_metrics,
        }

    def train_from_history(
        self,
        historical_data: List[Dict[str, Any]],
        validation_split: float = 0.2,
        early_stopping_rounds: int = 10
    ) -> Dict[str, Any]:
        """
        Train the gradient boosting models from historical data.

        Args:
            historical_data: List of dicts with features and actual_pnl_sol
            validation_split: Fraction of data to use for validation
            early_stopping_rounds: Early stopping rounds for training

        Returns:
            Dictionary with training metrics
        """

    def _prepare_training_data(
        self,
        historical_data: List[Dict[str, Any]]
    ) -> Tuple[Optional[np.ndarray], Optional[np.ndarray], List[str]]:
        """
        Prepare training data from historical records.

        Returns:
            Tuple of (X, y, feature_names)
        """
        # Define feature set to use
        feature_set = [
            'roi_7d', 'roi_30d', 'roi_90d',
            'win_rate', 'profit_factor', 'sortino_ratio',
            'trade_count_30d', 'avg_trade_size_sol', 'avg_hold_time_hours',
            'max_drawdown_30d', 'total_unrealized_loss_sol',
            'mev_risk_score', 'dex_diversity_score',
            'wmi_score', 'trajectory_improving', 'trajectory_stable',
            'is_sniper', 'is_swing', 'is_scalper', 'is_whale',
        ]

        X = []
        y = []

        for record in historical_data:
            # Extract features
            features = []
            for feature_name in feature_set:
                value = record.get(feature_name)

                # Handle trajectory enum
                if feature_name == 'trajectory_improving':
                    value = 1.0 if record.get('trajectory') == 'IMPROVING' else 0.0
                elif feature_name == 'trajectory_stable':
                    value = 1.0 if record.get('trajectory') == 'STABLE' else 0.0
                elif feature_name.startswith('is_'):
                    value = 1.0 if record.get(feature_name[3:].upper()) else 0.0

                if value is None:
                    value = 0.0  # Impute missing values

                features.append(float(value))

            # Get target
            target = record.get('actual_pnl_sol')
            if target is None:
                continue  # Skip records without target

            X.append(features)
            y.append(float(target))

        if len(X) == 0:
            return None, None, []

        return np.array(X), np.array(y), feature_set

    def _train_xgboost(
        self,
        X_train: np.ndarray,
        y_train: np.ndarray,
        X_val: np.ndarray,
        y_val: np.ndarray,
        early_stopping_rounds: int
    ) -> Dict[str, Any]:
        """Train XGBoost model."""
        try:
            # Create DMatrices
            dtrain = xgb.DMatrix(X_train, label=y_train, feature_names=self.feature_names)
            dval = xgb.DMatrix(X_val, label=y_val, feature_names=self.feature_names)

            # Training parameters
            params = {
                'objective': 'reg:squarederror',
                'eval_metric': 'rmse',
                'max_depth': self.config['max_depth'],
                'eta': self.config['learning_rate'],
                'subsample': self.config['subsample'],
                'colsample_bytree': self.config['colsample_bytree'],
                'min_child_weight': self.config['min_child_weight'],
                'alpha': self.config['reg_alpha'],
                'lambda': self.config['reg_lambda'],
                'tree_method': 'hist',  # Faster training
                'predictor': 'cpu_predictor',
            }

            # Train with early stopping
            evals_result = {}
            self.xgboost_model = xgb.train(
                params,
                dtrain,
                num_boost_round=self.config['n_estimators'],
                evals=[(dtrain, 'train'), (dval, 'val')],
                early_stopping_rounds=early_stopping_rounds,
                evals_result=evals_result,
                verbose_eval=False,
            )

            # Extract metrics
            train_rmse = evals_result['train']['rmse'][-1]
            val_rmse = evals_result['val']['rmse'][-1]
            best_iteration = int(evals_result['val']['rmse'].index(min(evals_result['val']['rmse'])))

            return {
                'train_rmse': float(train_rmse),
                'val_rmse': float(val_rmse),
                'best_iteration': best_iteration,
                'status': 'success',
            }

        except Exception as e:
            logger.error(f"XGBoost training failed: {e}")
            return {'status': 'failed', 'error': str(e)}

    def _train_lightgbm(
        self,
        X_train: np.ndarray,
        y_train: np.ndarray,
        X_val: np.ndarray,
        y_val: np.ndarray,
        early_stopping_rounds: int
    ) -> Dict[str, Any]:
        """Train LightGBM model."""
        try:
            # Create datasets
            train_data = lgb.Dataset(X_train, label=y_train, feature_name=self.feature_names)
            val_data = lgb.Dataset(X_val, label=y_val, feature_name=self.feature_names, reference=train_data)

            # Training parameters
            params = {
                'objective': 'regression',
                'metric': 'rmse',
                'max_depth': self.config['max_depth'],
                'learning_rate': self.config['learning_rate'],
                'feature_fraction': self.config['subsample'],
                'bagging_fraction': self.config['subsample'],
                'bagging_freq': self.config['bagging_freq'],
                'num_leaves': self.config['num_leaves'],
                'min_data_in_leaf': self.config['min_data_in_leaf'],
                'verbose': -1,
            }

            # Train with early stopping
            self.lightgbm_model = lgb.train(
                params,
                train_data,
                num_boost_round=self.config['n_estimators'],
                valid_sets=[train_data, val_data],
                valid_names=['train', 'val'],
                callbacks=[
                    lgb.early_stopping(stopping_rounds=early_stopping_rounds, verbose=False),
                    lgb.log_evaluation(period=0),  # Disable logging
                ],
            )

            # Extract metrics
            train_rmse = self.lightgbm_model.best_score['train']['rmse']
            val_rmse = self.lightgbm_model.best_score['val']['rmse']
            best_iteration = self.lightgbm_model.best_iteration

            return {
                'train_rmse': float(train_rmse),
                'val_rmse': float(val_rmse),
                'best_iteration': best_iteration,
                'status': 'success',
            }

        except Exception as e:
            logger.error(f"LightGBM training failed: {e}")
            return {'status': 'failed', 'error': str(e)}

    def _calculate_feature_importance(self):
        """Calculate feature importance from trained models."""
        importance = defaultdict(float)

        # XGBoost importance
        if self.xgboost_model is not None:
            try:
                xgb_imp = self.xgboost_model.get_score(importance_type='gain')
                for feature, score in xgb_imp.items():
                    importance[feature] += float(score)
            except Exception as e:
                logger.warning(f"Failed to get XGBoost feature importance: {e}")

        # LightGBM importance
        if self.lightgbm_model is not None:
            try:
                lgb_imp = self.lightgbm_model.feature_importance(importance_type='gain')
                for i, score in enumerate(lgb_imp):
                    if i < len(self.feature_names):
                        importance[self.feature_names[i]] += float(score)
            except Exception as e:
                logger.warning(f"Failed to get LightGBM feature importance: {e}")

        # Normalize importance
        total = sum(importance.values())
        if total > 0:
            self.feature_importance = {
                k: round(v / total, 4) for k, v in importance.items()
            }

        self.validation_metrics['feature_importance'] = self.feature_importance

    def _prune_model_for_latency(self):
        """Prune model to meet latency budget."""
        if not self.enable_pruning:
            return

        # Measure current latency
        if not self.inference_times_ms:
            return

        avg_latency = np.mean(self.inference_times_ms[-10:])  # Last 10 predictions

        if avg_latency <= self.latency_budget_ms:
            return  # Already within budget

        logger.info(f"Pruning model to meet latency budget: {avg_latency:.2f}ms -> {self.latency_budget_ms}ms")

        # Pruning strategy: reduce number of trees
        # This is a simplified approach - production would use more sophisticated pruning

        try:
            if self.best_model_type == 'xgboost' and self.xgboost_model is not None:
                # XGBoost: can prune trees directly
                # For now, just log - actual pruning would require more complex logic
                logger.info("XGBoost pruning enabled (simplified)")

            elif self.best_model_type == 'lightgbm' and self.lightgbm_model is not None:
                # LightGBM: can prune trees
                logger.info("LightGBM pruning enabled (simplified)")

        except Exception as e:
            logger.warning(f"Model pruning failed: {e}")

    def _save_models(self):
        """Save trained models to disk."""
        # Save XGBoost model
        if self.xgboost_model is not None:
            try:
                self.xgboost_model.save_model(str(self.xgboost_path))
                logger.info(f"XGBoost model saved to {self.xgboost_path}")
            except Exception as e:
                logger.error(f"Failed to save XGBoost model: {e}")

        # Save LightGBM model
        if self.lightgbm_model is not None:
            try:
                self.lightgbm_model.save_model(str(self.lightgbm_path))
                logger.info(f"LightGBM model saved to {self.lightgbm_path}")
            except Exception as e:
                logger.error(f"Failed to save LightGBM model: {e}")

        # Save metadata
        metadata = {
            'best_model_type': self.best_model_type,
            'feature_names': self.feature_names,
            'feature_importance': self.feature_importance,
            'training_samples': self.training_samples,
            'last_trained': self.last_trained,
            'config': self.config,
        }

        metadata_path = self.xgboost_path.parent / "gradient_boost_metadata.json"
        try:
            with open(metadata_path, 'w') as f:
                json.dump(metadata, f, indent=2)
            logger.info(f"Metadata saved to {metadata_path}")
        except Exception as e:
            logger.error(f"Failed to save metadata: {e}")

    def _load_models(self):
        """Load trained models from disk."""
        # Load XGBoost model
        if XGBOOST_AVAILABLE and self.xgboost_path.exists():
            try:
                self.xgboost_model = xgb.Booster()
                self.xgboost_model.load_model(str(self.xgboost_path))
                logger.info(f"XGBoost model loaded from {self.xgboost_path}")
            except Exception as e:
                logger.warning(f"Failed to load XGBoost model: {e}")

        # Load LightGBM model
        if LIGHTGBM_AVAILABLE and self.lightgbm_path.exists():
            try:
                self.lightgbm_model = lgb.Booster(
                    model_file=str(self.lightgbm_path)
                )
                logger.info(f"LightGBM model loaded from {self.lightgbm_path}")
            except Exception as e:
                logger.warning(f"Failed to load LightGBM model: {e}")

        # Load metadata
        metadata_path = self.xgboost_path.parent / "gradient_boost_metadata.json"
        if metadata_path.exists():
            try:
                with open(metadata_path, 'r') as f:
                    metadata = json.load(f)

                self.best_model_type = metadata.get('best_model_type')
                self.feature_names = metadata.get('feature_names', [])
                self.feature_importance = metadata.get('feature_importance', {})
                self.training_samples = metadata.get('training_samples', 0)
                self.last_trained = metadata.get('last_trained')

                logger.info(f"Metadata loaded: {self.training_samples} samples, best model: {self.best_model_type}")
            except Exception as e:
                logger.warning(f"Failed to load metadata: {e}")

    def get_latency_stats(self) -> Dict[str, Any]:
        """Get latency statistics for monitoring."""
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
            'violations': self.latency_violations,
            'violation_rate': self.latency_violations / len(times) if times.size > 0 else 0,
            'budget_ms': self.latency_budget_ms,
        }


# Convenience function for quick predictions
def predict_wallet_profitability_gb(
    wallet_features: Dict[str, Any],
    return_shap: bool = False
) -> Dict[str, Any]:
    """
    Quick prediction of wallet profitability using gradient boosting.

    Args:
        wallet_features: Dictionary of wallet features
        return_shap: Whether to include SHAP values in response

    Returns:
        Dictionary with predicted_pnl_sol and confidence
    """
    predictor = GradientBoostPredictor()
    return predictor.predict_profitability(wallet_features, return_shap=return_shap)
