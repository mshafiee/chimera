"""
EXPERIMENTAL — Not wired into the production Scout pipeline.

This module provides optional WQS enhancement using gradient boosting,
meta-learner stacking, and time-series features. It is imported by no
production code path and exists as a framework for future use.

To activate, wire this module into scout_optimizer.py or main.py and set
the appropriate env vars (see config.py).

ML Integration for Scout WQS

Integrates new ML components (gradient boosting, meta-learner, feature extractors)
with the existing WQS calculation workflow.

This module provides:
- Feature extraction integration
- ML prediction pipeline
- WQS enhancement with ML predictions
- Model selection and routing

Usage:
    enhanced_wqs = enhance_wqs_with_ml(wallet_metrics, strategy)
"""

import logging
from typing import Dict, Any, Optional, List

from scout.config import ScoutConfig
from scout.core.wqs import calculate_wqs_with_confidence, WqsResult

logger = logging.getLogger(__name__)

# Import new ML components (with fallbacks if not available)
try:
    from scout.core.archive.gradient_boost_predictor import GradientBoostPredictor
    GRADIENT_BOOST_AVAILABLE = True
except ImportError:
    GRADIENT_BOOST_AVAILABLE = False
    logger.warning("GradientBoostPredictor not available (archive)")

try:
    from scout.core.archive.meta_learner import MetaLearner
    META_LEARNER_AVAILABLE = True
except ImportError:
    META_LEARNER_AVAILABLE = False
    logger.warning("MetaLearner not available (archive)")

try:
    from scout.core.time_series_features import TimeSeriesFeatures
    TIME_SERIES_AVAILABLE = True
except ImportError:
    TIME_SERIES_AVAILABLE = False
    logger.warning("TimeSeriesFeatures not available")

try:
    from scout.core.market_context_features import MarketContextFeatures
    MARKET_CONTEXT_AVAILABLE = True
except ImportError:
    MARKET_CONTEXT_AVAILABLE = False
    logger.warning("MarketContextFeatures not available")

try:
    from scout.core.model_monitoring import get_monitor
    MONITORING_AVAILABLE = True
except ImportError:
    MONITORING_AVAILABLE = False
    logger.warning("ModelMonitor not available")

try:
    from scout.core.prediction_logger import get_prediction_logger
    PREDICTION_LOGGER_AVAILABLE = True
except ImportError:
    PREDICTION_LOGGER_AVAILABLE = False
    logger.warning("PredictionLogger not available")


class MLWQSEnhancer:
    """
    Enhances WQS calculation with ML predictions and features.

    Features:
    - Time-series and market context feature extraction
    - Gradient boosting prediction
    - Meta-learner ensemble
    - WQS adjustment based on ML predictions
    - Monitoring integration
    """

    def __init__(self):
        """Initialize the ML WQS enhancer."""
        self.enabled = ScoutConfig.get_ml_enabled()

        # Feature extractors
        self.ts_extractor = None
        self.mc_extractor = None

        # Predictors
        self.gb_predictor = None
        self.meta_learner = None

        # Monitor
        self.monitor = None

        # Prediction Logger
        self.prediction_logger = None

        # Initialize components if enabled
        if self.enabled:
            self._initialize_components()

    def _initialize_components(self):
        """Initialize ML components."""
        # Feature extractors
        if TIME_SERIES_AVAILABLE and ScoutConfig.get_time_series_features_enabled():
            self.ts_extractor = TimeSeriesFeatures()
            logger.info("Time-series features initialized")

        if MARKET_CONTEXT_AVAILABLE and ScoutConfig.get_market_context_features_enabled():
            self.mc_extractor = MarketContextFeatures()
            logger.info("Market context features initialized")

        # Predictors
        if GRADIENT_BOOST_AVAILABLE and ScoutConfig.get_xgboost_enabled():
            self.gb_predictor = GradientBoostPredictor()
            logger.info("Gradient boost predictor initialized")

        if META_LEARNER_AVAILABLE and ScoutConfig.get_meta_learner_enabled():
            self.meta_learner = MetaLearner()
            logger.info("Meta-learner initialized")

        # Monitoring
        if MONITORING_AVAILABLE and ScoutConfig.get_ml_monitoring_enabled():
            self.monitor = get_monitor()
            logger.info("Model monitoring initialized")

        # Prediction Logger
        if PREDICTION_LOGGER_AVAILABLE and ScoutConfig.get_prediction_logging_enabled():
            self.prediction_logger = get_prediction_logger()
            logger.info("Prediction logger initialized")

    def enhance_wqs(
        self,
        wallet_metrics: Any,
        strategy: str = "SHIELD",
        trade_history: Optional[List[Dict[str, Any]]] = None,
        sol_price_history: Optional[List[Dict[str, Any]]] = None
    ) -> Dict[str, Any]:
        """
        Enhance WQS calculation with ML predictions.

        Args:
            wallet_metrics: WalletMetrics object with wallet data
            strategy: Trading strategy (SHIELD/SPEAR)
            trade_history: Optional trade history for feature extraction
            sol_price_history: Optional SOL price history for beta calculation

        Returns:
            Dictionary with enhanced WQS results
        """
        # Get base WQS
        wqs_result = calculate_wqs_with_confidence(wallet_metrics, strategy)

        if not self.enabled:
            return {
                'wqs_result': wqs_result,
                'ml_enhanced': False,
                'reason': 'ml_disabled',
            }

        result = {
            'wqs_result': wqs_result,
            'ml_enhanced': True,
            'ml_features': {},
            'ml_predictions': {},
            'enhancement_applied': False,
        }

        try:
            # Extract features
            features = self._extract_features(wallet_metrics, trade_history, sol_price_history)
            result['ml_features'] = features

            # Make predictions
            predictions = self._make_predictions(features, wallet_metrics)
            result['ml_predictions'] = predictions

            # Enhance WQS based on predictions
            enhancement = self._calculate_enhancement(predictions, wqs_result)
            result.update(enhancement)

            # Log to monitor if available
            if self.monitor and 'predicted_pnl_sol' in predictions:
                self.monitor.log_prediction(
                    wallet_id=getattr(wallet_metrics, 'wallet_id', 'unknown'),
                    predicted_pnl=predictions['predicted_pnl_sol'],
                    features=features,
                    model_type=predictions.get('model_type', 'ensemble'),
                    confidence=predictions.get('confidence', 0.5),
                    inference_time_ms=predictions.get('inference_time_ms', 0.0),
                )

            # Log to prediction logger for later validation
            if self.prediction_logger and 'predicted_pnl_sol' in predictions:
                self.prediction_logger.log_prediction(
                    wallet_address=getattr(wallet_metrics, 'wallet_id', 'unknown'),
                    predicted_pnl_sol=predictions['predicted_pnl_sol'],
                    model_type=predictions.get('model_type', 'ensemble'),
                    features=features,
                    confidence=predictions.get('confidence', 0.5),
                    strategy=strategy,
                    wqs_score=wqs_result.adjusted_score if hasattr(wqs_result, 'adjusted_score') else 0.0,
                    wqs_components=wqs_result.components if hasattr(wqs_result, 'components') else {},
                    predicted_class=predictions.get('predicted_class')
                )
                self.monitor.log_prediction(
                    wallet_id=getattr(wallet_metrics, 'wallet_id', 'unknown'),
                    predicted_pnl=predictions['predicted_pnl_sol'],
                    features=features,
                    model_type=predictions.get('model_type', 'ensemble'),
                    confidence=predictions.get('confidence', 0.5),
                    inference_time_ms=predictions.get('inference_time_ms', 0.0),
                )

        except Exception as e:
            logger.error(f"ML enhancement failed: {e}")
            result['error'] = str(e)
            result['ml_enhanced'] = False

        return result

    def _extract_features(
        self,
        wallet_metrics: Any,
        trade_history: Optional[List[Dict[str, Any]]],
        sol_price_history: Optional[List[Dict[str, Any]]]
    ) -> Dict[str, Any]:
        """Extract ML features from wallet metrics and history."""
        features = {}

        # Base metrics
        try:
            features.update({
                'roi_7d': float(wallet_metrics.roi_7d) if wallet_metrics.roi_7d else 0.0,
                'roi_30d': float(wallet_metrics.roi_30d) if wallet_metrics.roi_30d else 0.0,
                'roi_90d': float(wallet_metrics.roi_90d) if wallet_metrics.roi_90d else 0.0,
                'win_rate': float(wallet_metrics.win_rate) if wallet_metrics.win_rate else 0.0,
                'profit_factor': float(wallet_metrics.profit_factor) if wallet_metrics.profit_factor else 0.0,
                'sortino_ratio': float(wallet_metrics.sortino_ratio) if hasattr(wallet_metrics, 'sortino_ratio') and wallet_metrics.sortino_ratio else 0.0,
                'trade_count_30d': int(wallet_metrics.trade_count_30d) if wallet_metrics.trade_count_30d else 0,
                'avg_trade_size_sol': float(wallet_metrics.avg_trade_size_sol) if hasattr(wallet_metrics, 'avg_trade_size_sol') and wallet_metrics.avg_trade_size_sol else 0.0,
                'avg_hold_time_hours': float(wallet_metrics.avg_hold_time_hours) if hasattr(wallet_metrics, 'avg_hold_time_hours') and wallet_metrics.avg_hold_time_hours else 0.0,
                'max_drawdown_30d': float(wallet_metrics.max_drawdown_30d) if wallet_metrics.max_drawdown_30d else 0.0,
                'total_unrealized_loss_sol': float(wallet_metrics.total_unrealized_loss_sol) if hasattr(wallet_metrics, 'total_unrealized_loss_sol') and wallet_metrics.total_unrealized_loss_sol else 0.0,
            })
        except Exception as e:
            logger.warning(f"Failed to extract base features: {e}")

        # Time-series features
        if self.ts_extractor and trade_history:
            try:
                ts_features = self.ts_extractor.extract_from_wallet_trades(trade_history)
                features.update(ts_features)
            except Exception as e:
                logger.warning(f"Time-series feature extraction failed: {e}")

        # Market context features
        if self.mc_extractor and trade_history:
            try:
                mc_features = self.mc_extractor.extract_features(trade_history, sol_price_history)
                features.update(mc_features)
            except Exception as e:
                logger.warning(f"Market context feature extraction failed: {e}")

        # Archetype and trajectory
        try:
            if hasattr(wallet_metrics, 'archetype'):
                for arch in ['SNIPER', 'SWING', 'SCALPER', 'INSIDER', 'WHALE']:
                    features[f'is_{arch.lower()}'] = 1.0 if wallet_metrics.archetype == arch else 0.0

            if hasattr(wallet_metrics, 'trajectory'):
                features['trajectory_improving'] = 1.0 if wallet_metrics.trajectory == 'IMPROVING' else 0.0
                features['trajectory_stable'] = 1.0 if wallet_metrics.trajectory == 'STABLE' else 0.0
        except Exception as e:
            logger.warning(f"Failed to extract archetype/trajectory: {e}")

        return features

    def _make_predictions(
        self,
        features: Dict[str, Any],
        wallet_metrics: Any
    ) -> Dict[str, Any]:
        """Make predictions using available models."""
        predictions = {}

        # Gradient boost prediction
        if self.gb_predictor:
            try:
                gb_result = self.gb_predictor.predict_profitability(features)
                predictions['gradient_boost'] = gb_result
                predictions['predicted_pnl_sol'] = gb_result.get('predicted_pnl_sol', 0.0)
                predictions['model_type'] = gb_result.get('model_type', 'gradient_boost')
                predictions['confidence'] = gb_result.get('confidence', 0.5)
                predictions['inference_time_ms'] = gb_result.get('inference_time_ms', 0.0)
            except Exception as e:
                logger.warning(f"Gradient boost prediction failed: {e}")

        # Meta-learner prediction (if registered)
        if self.meta_learner and hasattr(self.meta_learner, 'base_models') and self.meta_learner.base_models:
            try:
                meta_result = self.meta_learner.predict_profitability(features)
                predictions['meta_learner'] = meta_result

                # Use meta-learner prediction if available
                if 'predicted_pnl_sol' in meta_result:
                    predictions['predicted_pnl_sol'] = meta_result['predicted_pnl_sol']
                    predictions['model_type'] = f"meta_{meta_result.get('combination_method', 'unknown')}"
                    predictions['confidence'] = meta_result.get('confidence', predictions.get('confidence', 0.5))
            except Exception as e:
                logger.warning(f"Meta-learner prediction failed: {e}")

        return predictions

    def _calculate_enhancement(
        self,
        predictions: Dict[str, Any],
        wqs_result: WqsResult
    ) -> Dict[str, Any]:
        """Calculate WQS enhancement based on ML predictions."""
        enhancement = {
            'enhancement_applied': False,
            'adjusted_wqs': wqs_result.adjusted_score,
            'wqs_boost': 0.0,
            'wqs_penalty': 0.0,
        }

        predicted_pnl = predictions.get('predicted_pnl_sol')
        confidence = predictions.get('confidence', 0.5)

        if predicted_pnl is None:
            return enhancement

        # Enhancement only applied if confidence is high enough
        if confidence < 0.6:
            enhancement['reason'] = 'low_confidence'
            return enhancement

        # Calculate boost/penalty based on predicted PnL
        # Positive prediction -> potential boost
        # Negative prediction -> potential penalty

        if predicted_pnl > 0.1:  # Strong positive prediction
            # Boost based on prediction magnitude and confidence
            boost = min(10.0, predicted_pnl * 5 * confidence)
            enhancement['wqs_boost'] = boost
            enhancement['adjusted_wqs'] = min(100.0, wqs_result.adjusted_score + boost)
            enhancement['enhancement_applied'] = True
            enhancement['reason'] = 'positive_ml_prediction'

        elif predicted_pnl < -0.05:  # Negative prediction
            # Penalty based on prediction magnitude
            penalty = min(15.0, abs(predicted_pnl) * 10 * confidence)
            enhancement['wqs_penalty'] = penalty
            enhancement['adjusted_wqs'] = max(0.0, wqs_result.adjusted_score - penalty)
            enhancement['enhancement_applied'] = True
            enhancement['reason'] = 'negative_ml_prediction'

        return enhancement


# Global enhancer instance
_global_enhancer = None


def get_ml_enhancer() -> MLWQSEnhancer:
    """Get or create global ML enhancer instance."""
    global _global_enhancer
    if _global_enhancer is None:
        _global_enhancer = MLWQSEnhancer()
    return _global_enhancer


def enhance_wqs_with_ml(
    wallet_metrics: Any,
    strategy: str = "SHIELD",
    trade_history: Optional[List[Dict[str, Any]]] = None,
    sol_price_history: Optional[List[Dict[str, Any]]] = None
) -> Dict[str, Any]:
    """
    Convenience function to enhance WQS with ML predictions.

    Args:
        wallet_metrics: WalletMetrics object
        strategy: Trading strategy
        trade_history: Optional trade history
        sol_price_history: Optional SOL price history

    Returns:
        Dictionary with enhanced WQS results
    """
    enhancer = get_ml_enhancer()
    return enhancer.enhance_wqs(wallet_metrics, strategy, trade_history, sol_price_history)


def train_ml_models(
    historical_data: List[Dict[str, Any]],
    base_model_predictors: Optional[Dict[str, Any]] = None
) -> Dict[str, Any]:
    """
    Train all ML models from historical data.

    Args:
        historical_data: List of historical records
        base_model_predictors: Dict of base predictors for meta-learner

    Returns:
        Dictionary with training results
    """
    results = {}

    enhancer = get_ml_enhancer()

    # Train gradient boost model
    if enhancer.gb_predictor:
        try:
            gb_results = enhancer.gb_predictor.train_from_history(historical_data)
            results['gradient_boost'] = gb_results
        except Exception as e:
            results['gradient_boost'] = {'error': str(e)}

    # Train meta-learner
    if enhancer.meta_learner and base_model_predictors:
        try:
            # Register base models
            for name, predictor in base_model_predictors.items():
                enhancer.meta_learner.register_base_model(name, predictor)

            meta_results = enhancer.meta_learner.train_from_history(historical_data)
            results['meta_learner'] = meta_results
        except Exception as e:
            results['meta_learner'] = {'error': str(e)}

    return results
