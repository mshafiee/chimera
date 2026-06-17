"""
Model Explainability for Scout

Provides SHAP-based explanations for ML model predictions.
This module provides:
- SHAP values for individual predictions
- Counterfactual explanations ("what if" scenarios)
- Feature importance tracking over time
- Model-agnostic explanations

Usage:
    explainer = ModelExplainer(model, feature_names)
    explanation = explainer.explain_prediction(features)
    shap_values = explainer.get_shap_values(features)
"""

import json
import logging
import os
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Tuple, Any, Union
from collections import defaultdict
import numpy as np

logger = logging.getLogger(__name__)

# Try to import SHAP
try:
    import shap
    SHAP_AVAILABLE = True
except ImportError:
    SHAP_AVAILABLE = False
    logger.warning("SHAP not available - install with: pip install shap")

# Try to import sklearn for counterfactuals
try:
    from sklearn.preprocessing import StandardScaler
    SKLEARN_AVAILABLE = True
except ImportError:
    SKLEARN_AVAILABLE = False


class ModelExplainer:
    """
    Explain ML model predictions using SHAP values.

    Features:
    - SHAP value calculation for tree models and general models
    - Feature importance tracking
    - Counterfactual explanations
    - Time-based importance tracking
    """

    def __init__(
        self,
        model: Any,
        feature_names: List[str],
        model_type: str = "auto",
        background_samples: int = 100,
        explanation_cache_size: int = 1000
    ):
        """
        Initialize the model explainer.

        Args:
            model: Trained model to explain
            feature_names: List of feature names
            model_type: Type of model ("tree", "linear", "deep", "auto")
            background_samples: Number of background samples for SHAP
            explanation_cache_size: Size of explanation cache
        """
        self.model = model
        self.feature_names = feature_names
        self.background_samples = background_samples

        # SHAP explainer
        self.explainer = None
        self.background_data = None

        # Feature importance tracking
        self.feature_importance_history = defaultdict(list)
        self.importance_window_size = 100

        # Explanation cache
        self.explanation_cache = {}
        self.cache_size = explanation_cache_size

        # Initialize SHAP if available
        if SHAP_AVAILABLE:
            self._initialize_shap(model_type)

    def _initialize_shap(self, model_type: str):
        """Initialize SHAP explainer based on model type."""
        try:
            # Detect model type if auto
            if model_type == "auto":
                model_type = self._detect_model_type()

            # Create appropriate explainer
            if model_type == "tree":
                # TreeExplainer for XGBoost, LightGBM, etc.
                try:
                    self.explainer = shap.TreeExplainer(self.model)
                    logger.info("Using TreeExplainer")
                except Exception as e:
                    logger.warning(f"TreeExplainer failed: {e}, falling back to KernelExplainer")
                    self.explainer = shap.KernelExplainer(self._predict_fn, np.zeros((1, len(self.feature_names))))

            elif model_type == "linear":
                # LinearExplainer for linear models
                self.explainer = shap.LinearExplainer(self.model, np.zeros((1, len(self.feature_names))))
                logger.info("Using LinearExplainer")

            elif model_type == "deep":
                # DeepExplainer for neural networks
                if hasattr(self.model, 'predict_proba'):
                    self.explainer = shap.DeepExplainer(self.model, np.zeros((10, len(self.feature_names))))
                    logger.info("Using DeepExplainer")
                else:
                    logger.warning("DeepExplainer requires model with predict_proba, using KernelExplainer")
                    self.explainer = shap.KernelExplainer(self._predict_fn, np.zeros((1, len(self.feature_names))))

            else:
                # Default to KernelExplainer (model-agnostic)
                self.explainer = shap.KernelExplainer(self._predict_fn, np.zeros((self.background_samples, len(self.feature_names))))
                logger.info("Using KernelExplainer")

        except Exception as e:
            logger.error(f"SHAP initialization failed: {e}")
            self.explainer = None

    def _detect_model_type(self) -> str:
        """Detect model type from model instance."""
        # Check for tree models
        try:
            import xgboost as xgb
            if isinstance(self.model, xgb.Booster):
                return "tree"
        except ImportError:
            pass

        try:
            import lightgbm as lgb
            if hasattr(self.model, 'booster_'):
                return "tree"
        except ImportError:
            pass

        # Check for sklearn models
        if hasattr(self.model, 'coef_'):
            return "linear"

        # Default to kernel
        return "kernel"

    def _predict_fn(self, X: np.ndarray) -> np.ndarray:
        """Prediction function for KernelExplainer."""
        if hasattr(self.model, 'predict'):
            return self.model.predict(X)
        elif hasattr(self.model, 'predict_proba'):
            return self.model.predict_proba(X)[:, 1]
        else:
            raise ValueError(f"Model {type(self.model)} has no predict method")

    def explain_prediction(
        self,
        features: Dict[str, Any],
        return_plot_data: bool = False
    ) -> Dict[str, Any]:
        """
        Explain a single prediction using SHAP values.

        Args:
            features: Feature dictionary
            return_plot_data: Whether to return data for plotting

        Returns:
            Dictionary with explanation data
        """
        if not SHAP_AVAILABLE or self.explainer is None:
            return {
                'error': 'shap_not_available',
                'message': 'SHAP not installed or explainer initialization failed'
            }

        try:
            # Prepare features
            feature_array = self._prepare_features(features)

            if feature_array is None:
                return {'error': 'feature_preparation_failed'}

            # Calculate SHAP values
            shap_values = self.explainer.shap_values(feature_array)

            # Handle multi-output (use first class)
            if isinstance(shap_values, list):
                shap_values = shap_values[0]

            # Ensure shape is correct
            if len(shap_values.shape) == 2:
                shap_values = shap_values[0]

            # Get base value
            if hasattr(self.explainer, 'expected_value'):
                base_value = self.explainer.expected_value
                if isinstance(base_value, np.ndarray):
                    base_value = base_value[0] if len(base_value) > 0 else base_value
            else:
                base_value = 0.0

            # Create explanation
            explanation = {
                'prediction': float(self._predict_fn(feature_array.reshape(1, -1))[0]),
                'base_value': float(base_value),
                'shap_values': {},
                'feature_importance': {},
                'top_positive_features': [],
                'top_negative_features': [],
            }

            # Process SHAP values
            feature_contributions = {}
            for i, name in enumerate(self.feature_names):
                if i < len(shap_values):
                    contribution = float(shap_values[i])
                    feature_contributions[name] = contribution
                    explanation['shap_values'][name] = contribution

            # Sort by absolute contribution
            sorted_features = sorted(
                feature_contributions.items(),
                key=lambda x: abs(x[1]),
                reverse=True
            )

            # Top features
            explanation['top_positive_features'] = [
                {'name': name, 'value': float(value)}
                for name, value in sorted_features
                if value > 0
            ][:5]

            explanation['top_negative_features'] = [
                {'name': name, 'value': float(value)}
                for name, value in sorted_features
                if value < 0
            ][:5]

            # Feature importance (absolute SHAP values)
            for name, value in feature_contributions.items():
                explanation['feature_importance'][name] = abs(value)

            # Track importance over time
            self._track_importance(feature_contributions)

            # Add plot data if requested
            if return_plot_data:
                explanation['plot_data'] = {
                    'features': self.feature_names,
                    'shap_values': [float(shap_values[i]) if i < len(shap_values) else 0.0 for i in range(len(self.feature_names))],
                    'feature_values': [float(feature_array[i]) if i < feature_array.shape[0] else 0.0 for i in range(len(self.feature_names))],
                }

            return explanation

        except Exception as e:
            logger.error(f"Prediction explanation failed: {e}")
            return {'error': str(e)}

    def _prepare_features(self, features: Dict[str, Any]) -> Optional[np.ndarray]:
        """Prepare feature array from dictionary."""
        try:
            feature_array = []
            for name in self.feature_names:
                value = features.get(name, 0.0)
                if value is None:
                    value = 0.0
                feature_array.append(float(value))

            return np.array(feature_array)
        except Exception as e:
            logger.error(f"Feature preparation failed: {e}")
            return None

    def _track_importance(self, feature_contributions: Dict[str, float]):
        """Track feature importance over time."""
        # Calculate absolute importance
        abs_importance = {name: abs(value) for name, value in feature_contributions.items()}

        # Normalize
        total = sum(abs_importance.values())
        if total > 0:
            normalized = {name: value / total for name, value in abs_importance.items()}
        else:
            normalized = {name: 0.0 for name in abs_importance.keys()}

        # Add to history
        for name, value in normalized.items():
            self.feature_importance_history[name].append((datetime.utcnow().isoformat(), value))

            # Keep only recent history
            if len(self.feature_importance_history[name]) > self.importance_window_size:
                self.feature_importance_history[name] = self.feature_importance_history[name][-self.importance_window_size:]

    def get_feature_importance_trend(
        self,
        feature_name: str,
        window: int = 20
    ) -> Dict[str, Any]:
        """
        Get trend of feature importance over time.

        Args:
            feature_name: Feature to analyze
            window: Number of recent points to consider

        Returns:
            Dictionary with trend data
        """
        if feature_name not in self.feature_importance_history:
            return {'error': 'feature_not_found'}

        history = self.feature_importance_history[feature_name]

        if len(history) < 2:
            return {'error': 'insufficient_data'}

        # Get recent window
        recent = history[-window:] if len(history) > window else history

        timestamps = [t[0] for t in recent]
        values = [t[1] for t in recent]

        # Calculate trend
        if len(values) >= 2:
            recent_avg = np.mean(values[-max(1, len(values) // 5):])
            early_avg = np.mean(values[:max(1, len(values) // 5)])
            trend = recent_avg - early_avg

            return {
                'feature_name': feature_name,
                'current_importance': float(values[-1]),
                'avg_importance': float(np.mean(values)),
                'trend': float(trend),
                'increasing': bool(trend > 0),
                'volatility': float(np.std(values)),
                'sample_count': len(values),
                'timestamps': timestamps,
                'values': values,
            }

        return {'error': 'insufficient_data'}

    def get_global_importance(self, method: str = 'mean') -> Dict[str, float]:
        """
        Get global feature importance across all explanations.

        Args:
            method: Aggregation method ('mean', 'max', 'last')

        Returns:
            Dictionary of feature importance scores
        """
        global_importance = {}

        for feature_name, history in self.feature_importance_history.items():
            if not history:
                continue

            values = [v for _, v in history]

            if method == 'mean':
                importance = np.mean(values)
            elif method == 'max':
                importance = np.max(values)
            elif method == 'last':
                importance = values[-1]
            else:
                importance = np.mean(values)

            global_importance[feature_name] = float(importance)

        return global_importance

    def generate_counterfactual(
        self,
        features: Dict[str, Any],
        target_change: float = 0.1,
        max_features: int = 3
    ) -> Dict[str, Any]:
        """
        Generate counterfactual explanations.

        Answers: "What if this feature were different?"

        Args:
            features: Original feature dictionary
            target_change: Desired change in prediction (e.g., 0.1 = +0.1 SOL)
            max_features: Maximum number of features to modify

        Returns:
            Dictionary with counterfactual scenarios
        """
        if not SHAP_AVAILABLE or self.explainer is None:
            return {'error': 'shap_not_available'}

        try:
            # Get current prediction
            feature_array = self._prepare_features(features)
            if feature_array is None:
                return {'error': 'feature_preparation_failed'}

            current_pred = float(self._predict_fn(feature_array.reshape(1, -1))[0])

            # Get SHAP values
            explanation = self.explain_prediction(features)
            if 'error' in explanation:
                return explanation

            # Generate counterfactuals
            counterfactuals = []

            # Sort features by SHAP value magnitude
            sorted_features = sorted(
                explanation['shap_values'].items(),
                key=lambda x: abs(x[1]),
                reverse=True
            )[:max_features * 2]

            for i, (feat_name, shap_val) in enumerate(sorted_features[:max_features]):
                # Determine direction of change
                if (target_change > 0 and shap_val > 0) or (target_change < 0 and shap_val < 0):
                    # Same direction - amplify
                    direction = 1 if target_change > 0 else -1
                    magnitude = abs(target_change) / max(abs(shap_val), 0.01)
                else:
                    # Opposite direction - reverse
                    direction = -1 if target_change > 0 else 1
                    magnitude = abs(target_change) / max(abs(shap_val), 0.01)

                # Get feature stats for reasonable modification
                feat_idx = self.feature_names.index(feat_name)
                original_value = feature_array[feat_idx]

                # Suggested change (scaled by feature std if available)
                suggested_value = original_value + (direction * magnitude * abs(original_value + 0.1))

                # Create counterfactual
                cf_features = features.copy()
                cf_features[feat_name] = suggested_value

                cf_array = self._prepare_features(cf_features)
                cf_pred = float(self._predict_fn(cf_array.reshape(1, -1))[0])

                counterfactuals.append({
                    'feature': feat_name,
                    'original_value': float(original_value),
                    'suggested_value': float(suggested_value),
                    'change_percent': float((suggested_value - original_value) / (abs(original_value) + 1e-8) * 100),
                    'original_prediction': current_pred,
                    'counterfactual_prediction': cf_pred,
                    'prediction_change': float(cf_pred - current_pred),
                    'target_achieved': abs(cf_pred - current_pred - target_change) < abs(target_change * 0.5),
                })

            return {
                'original_prediction': current_pred,
                'target_change': target_change,
                'counterfactuals': counterfactuals,
            }

        except Exception as e:
            logger.error(f"Counterfactual generation failed: {e}")
            return {'error': str(e)}

    def save_explanations(self, filepath: Optional[str] = None):
        """Save explanation history to file."""
        if filepath is None:
            filepath = os.getenv("SCOUT_EXPLANATION_PATH", "../data/explanations.json")

        try:
            # Prepare data
            data = {
                'feature_importance_history': {
                    name: values for name, values in self.feature_importance_history.items()
                },
                'global_importance': self.get_global_importance(),
                'last_updated': datetime.utcnow().isoformat(),
            }

            # Save
            Path(filepath).parent.mkdir(parents=True, exist_ok=True)
            with open(filepath, 'w') as f:
                json.dump(data, f, indent=2)

            logger.info(f"Explanations saved to {filepath}")

        except Exception as e:
            logger.error(f"Failed to save explanations: {e}")

    def load_explanations(self, filepath: Optional[str] = None):
        """Load explanation history from file."""
        if filepath is None:
            filepath = os.getenv("SCOUT_EXPLANATION_PATH", "../data/explanations.json")

        try:
            if not Path(filepath).exists():
                return

            with open(filepath, 'r') as f:
                data = json.load(f)

            # Load history
            self.feature_importance_history = defaultdict(list)
            for name, values in data.get('feature_importance_history', {}).items():
                self.feature_importance_history[name] = [
                    (t, float(v)) for t, v in values
                ]

            logger.info(f"Explanations loaded from {filepath}")

        except Exception as e:
            logger.warning(f"Failed to load explanations: {e}")


# Convenience function
def explain_prediction(
    model: Any,
    features: Dict[str, Any],
    feature_names: List[str]
) -> Dict[str, Any]:
    """
    Quick explanation of a prediction.

    Args:
        model: Trained model
        features: Feature dictionary
        feature_names: List of feature names

    Returns:
        Dictionary with explanation
    """
    explainer = ModelExplainer(model, feature_names)
    return explainer.explain_prediction(features)
