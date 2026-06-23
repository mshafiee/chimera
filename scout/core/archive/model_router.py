"""
EXPERIMENTAL — Not wired into the production Scout pipeline.

This module provides optional ML model routing (GradientBoost, MetaLearner,
Heuristic) with A/B testing support. It is imported by no production code
path and exists as a framework for future use.

To activate, wire this module into scout_optimizer.py or main.py and set
the appropriate env vars (see config.py).

Model Router for Scout

Provides intelligent model selection and routing for ML predictions.
This module provides:
- Dynamic model selection based on wallet characteristics
- A/B testing support
- Model fallback logic
- Performance-based routing

Usage:
    router = ModelRouter()
    prediction = router.predict(wallet_features, strategy)
"""

import logging
from typing import Dict, Any, Optional
from datetime import datetime
from enum import Enum

from scout.config import ScoutConfig

logger = logging.getLogger(__name__)


class ModelType(Enum):
    """Available model types."""
    GRADIENT_BOOST = "gradient_boost"
    META_LEARNER = "meta_learner"
    LINEAR = "linear"
    FALLBACK = "fallback"


class ModelRouter:
    """
    Routes prediction requests to appropriate models.

    Features:
    - Dynamic model selection based on wallet characteristics
    - A/B testing support
    - Automatic fallback on errors
    - Performance tracking
    """

    def __init__(self):
        """Initialize the model router."""
        self.enabled = ScoutConfig.get_ml_enabled()
        self.ab_testing_enabled = ScoutConfig.get_ab_testing_enabled()
        self.ab_test_split = ScoutConfig.get_ab_test_traffic_split()

        # Model references
        self.models = {}

        # Performance tracking
        self.model_performance = {}
        self.model_errors = {}

        # A/B testing state
        self.ab_test_group = {}  # wallet_id -> group (A or B)

        # Initialize models
        if self.enabled:
            self._initialize_models()

    def _initialize_models(self):
        """Initialize available models."""
        # Gradient boost model
        try:
            from scout.core.archive.gradient_boost_predictor import GradientBoostPredictor
            self.models[ModelType.GRADIENT_BOOST] = GradientBoostPredictor()
            logger.info("Gradient boost model initialized (archive)")
        except ImportError:
            logger.warning("GradientBoostPredictor not available")

        # Meta-learner
        try:
            from scout.core.archive.meta_learner import MetaLearner
            self.models[ModelType.META_LEARNER] = MetaLearner()
            logger.info("Meta-learner initialized (archive)")
        except ImportError:
            logger.warning("MetaLearner not available")

        # Linear model (existing)
        try:
            from scout.core.archive.ml_predictor import HeuristicProfitabilityPredictor
            self.models[ModelType.LINEAR] = HeuristicProfitabilityPredictor()
            logger.info("Linear model initialized")
        except ImportError:
            logger.warning("ProfitabilityPredictor not available")

    def predict(
        self,
        wallet_features: Dict[str, Any],
        strategy: str = "SHIELD",
        wallet_id: Optional[str] = None,
        force_model: Optional[ModelType] = None
    ) -> Dict[str, Any]:
        """
        Make a prediction using the best available model.

        Args:
            wallet_features: Feature dictionary
            strategy: Trading strategy (SHIELD/SPEAR)
            wallet_id: Optional wallet ID for A/B testing
            force_model: Force specific model to be used

        Returns:
            Dictionary with prediction results
        """
        if not self.enabled or not self.models:
            return self._fallback_prediction(wallet_features)

        # Determine which model to use
        model_type = force_model or self._select_model(wallet_features, wallet_id)

        # Get prediction from selected model
        try:
            model = self.models.get(model_type)
            if model is None:
                return self._fallback_prediction(wallet_features)

            if model_type == ModelType.GRADIENT_BOOST:
                prediction = model.predict_profitability(wallet_features)
            elif model_type == ModelType.META_LEARNER:
                prediction = model.predict_profitability(wallet_features)
            elif model_type == ModelType.LINEAR:
                prediction = model.predict_profitability(wallet_features)
            else:
                prediction = self._fallback_prediction(wallet_features)

            prediction['model_type'] = model_type.value
            prediction['router'] = 'ModelRouter'
            prediction['strategy'] = strategy

            # Track performance
            self._track_prediction(model_type, prediction)

            return prediction

        except Exception as e:
            logger.warning(f"Prediction with {model_type} failed: {e}")
            self._track_error(model_type, str(e))

            # Fallback to linear model
            if model_type != ModelType.LINEAR and ModelType.LINEAR in self.models:
                try:
                    linear_model = self.models[ModelType.LINEAR]
                    prediction = linear_model.predict_profitability(wallet_features)
                    prediction['model_type'] = ModelType.LINEAR.value
                    prediction['router'] = 'ModelRouter::fallback'
                    prediction['fallback_reason'] = str(e)
                    return prediction
                except Exception as e2:
                    logger.warning(f"Linear model fallback also failed: {e2}")

            return self._fallback_prediction(wallet_features)

    def _select_model(
        self,
        wallet_features: Dict[str, Any],
        wallet_id: Optional[str] = None
    ) -> ModelType:
        """
        Select the best model for this prediction.

        Args:
            wallet_features: Feature dictionary
            wallet_id: Optional wallet ID for A/B testing

        Returns:
            Selected model type
        """
        # A/B testing
        if self.ab_testing_enabled and wallet_id:
            return self._ab_test_select(wallet_id)

        # Check for meta-learner availability and performance
        if ModelType.META_LEARNER in self.models:
            # Use meta-learner if we have enough data
            meta_model = self.models[ModelType.META_LEARNER]
            if hasattr(meta_model, 'training_samples') and meta_model.training_samples >= 50:
                return ModelType.META_LEARNER

        # Default to gradient boost
        if ModelType.GRADIENT_BOOST in self.models:
            return ModelType.GRADIENT_BOOST

        # Fallback to linear
        if ModelType.LINEAR in self.models:
            return ModelType.LINEAR

        return ModelType.FALLBACK

    def _ab_test_select(self, wallet_id: str) -> ModelType:
        """Select model based on A/B test assignment."""
        # Get or assign group
        if wallet_id not in self.ab_test_group:
            # Assign group based on hash (consistent per wallet)
            hash_val = hash(wallet_id) % 100
            self.ab_test_group[wallet_id] = 'B' if hash_val < self.ab_test_split * 100 else 'A'

        group = self.ab_test_group[wallet_id]

        # Group A: Use gradient boost
        # Group B: Use meta-learner (or linear if meta unavailable)
        if group == 'A':
            if ModelType.GRADIENT_BOOST in self.models:
                return ModelType.GRADIENT_BOOST
            return ModelType.LINEAR
        else:
            if ModelType.META_LEARNER in self.models:
                return ModelType.META_LEARNER
            return ModelType.LINEAR

    def _fallback_prediction(self, wallet_features: Dict[str, Any]) -> Dict[str, Any]:
        """Generate a fallback prediction without ML models."""
        # Simple heuristic based on ROI
        roi_30d = wallet_features.get('roi_30d', 0.0)
        win_rate = wallet_features.get('win_rate', 0.5)
        profit_factor = wallet_features.get('profit_factor', 1.0)

        # Simple weighted prediction
        predicted_pnl = (roi_30d * 0.1 + win_rate * 0.05 + (profit_factor - 1.0) * 0.02)

        return {
            'predicted_pnl_sol': float(predicted_pnl),
            'confidence': 0.3,  # Low confidence for fallback
            'model_type': ModelType.FALLBACK.value,
            'router': 'ModelRouter::heuristic',
            'warning': 'No ML models available, using heuristic fallback',
        }

    def _track_prediction(self, model_type: ModelType, prediction: Dict[str, Any]):
        """Track prediction performance."""
        if model_type not in self.model_performance:
            self.model_performance[model_type] = {
                'count': 0,
                'total_confidence': 0.0,
                'total_latency': 0.0,
            }

        perf = self.model_performance[model_type]
        perf['count'] += 1
        perf['total_confidence'] += prediction.get('confidence', 0.5)
        perf['total_latency'] += prediction.get('inference_time_ms', 0.0)

    def _track_error(self, model_type: ModelType, error: str):
        """Track model errors."""
        if model_type not in self.model_errors:
            self.model_errors[model_type] = []

        self.model_errors[model_type].append({
            'error': error,
            'timestamp': datetime.utcnow().isoformat(),
        })

        # Keep only last 100 errors
        if len(self.model_errors[model_type]) > 100:
            self.model_errors[model_type] = self.model_errors[model_type][-100:]

    def get_performance_stats(self) -> Dict[str, Any]:
        """Get performance statistics for all models."""
        stats = {}

        for model_type, perf in self.model_performance.items():
            if perf['count'] > 0:
                stats[model_type.value] = {
                    'count': perf['count'],
                    'avg_confidence': perf['total_confidence'] / perf['count'],
                    'avg_latency_ms': perf['total_latency'] / perf['count'],
                    'error_count': len(self.model_errors.get(model_type, [])),
                }

        return stats

    def register_base_models(self, base_predictors: Dict[str, Any]):
        """
        Register base models for meta-learner.

        Args:
            base_predictors: Dictionary of name -> predictor
        """
        if ModelType.META_LEARNER in self.models:
            meta_learner = self.models[ModelType.META_LEARNER]
            for name, predictor in base_predictors.items():
                meta_learner.register_base_model(name, predictor)

            logger.info(f"Registered {len(base_predictors)} base models with meta-learner")


# Global router instance
_global_router = None


def get_model_router() -> ModelRouter:
    """Get or create global model router instance."""
    global _global_router
    if _global_router is None:
        _global_router = ModelRouter()
    return _global_router


def route_prediction(
    wallet_features: Dict[str, Any],
    strategy: str = "SHIELD",
    wallet_id: Optional[str] = None
) -> Dict[str, Any]:
    """
    Convenience function to route prediction to best model.

    Args:
        wallet_features: Feature dictionary
        strategy: Trading strategy
        wallet_id: Optional wallet ID

    Returns:
        Dictionary with prediction results
    """
    router = get_model_router()
    return router.predict(wallet_features, strategy, wallet_id)
