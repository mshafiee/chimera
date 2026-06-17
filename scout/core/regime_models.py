"""
Regime-Specific Models for Scout

Implements separate models for different market regimes.
This module provides:
- Automatic market regime classification (BULL/BEAR/VOLATILE)
- Separate models for each regime
- Dynamic model switching based on regime detection
- Regime-aware prediction routing

Usage:
    regime_models = RegimeSpecificModels()
    regime_models.train_regime_models(historical_data_with_regime_labels)
    prediction = regime_models.predict(features, current_regime)
"""

import json
import logging
import os
from datetime import datetime, timedelta
from pathlib import Path
from typing import Dict, List, Optional, Tuple, Any, Union
from enum import Enum
import numpy as np

logger = logging.getLogger(__name__)


class MarketRegime(Enum):
    """Market regime types."""
    BULL = "bull"          # Uptrend, low volatility
    BEAR = "bear"          # Downtrend, high volatility
    VOLATILE = "volatile"  # Sideways, high volatility
    STABLE = "stable"      # Sideways, low volatility
    UNKNOWN = "unknown"


class RegimeClassifier:
    """
    Classifies market regime based on price and volatility.

    Uses statistical features to classify the current market regime.
    """

    def __init__(
        self,
        window: int = 30,
        trend_threshold: float = 0.02,  # 2% for trend
        volatility_threshold: float = 0.05,  # 5% for high volatility
    ):
        """
        Initialize regime classifier.

        Args:
            window: Lookback window for regime detection
            trend_threshold: Threshold for trend detection
            volatility_threshold: Threshold for volatility classification
        """
        self.window = window
        self.trend_threshold = trend_threshold
        self.volatility_threshold = volatility_threshold

        # Price history for regime detection
        self.price_history = []

    def classify(self, current_price: float, price_history: Optional[List[float]] = None) -> MarketRegime:
        """
        Classify the current market regime.

        Args:
            current_price: Current SOL price
            price_history: Optional historical prices (uses internal buffer if None)

        Returns:
            MarketRegime enum value
        """
        # Add to history
        self.price_history.append(current_price)

        # Keep only recent history
        if len(self.price_history) > self.window * 2:
            self.price_history = self.price_history[-self.window * 2:]

        # Use provided history if available
        if price_history:
            prices = price_history[-self.window:]
        else:
            prices = self.price_history[-self.window:]

        if len(prices) < 5:
            return MarketRegime.UNKNOWN

        prices_array = np.array(prices)

        # Calculate trend (linear regression slope)
        x = np.arange(len(prices_array))
        z = np.polyfit(x, prices_array, 1)
        slope = z[0] / (np.mean(prices_array) + 1e-8)  # Normalized slope

        # Calculate volatility
        returns = np.diff(prices_array) / (prices_array[:-1] + 1e-8)
        volatility = np.std(returns)

        # Classify regime
        if abs(slope) > self.trend_threshold:
            if slope > 0:
                if volatility > self.volatility_threshold:
                    return MarketRegime.VOLATILE  # Bull but volatile
                else:
                    return MarketRegime.BULL
            else:
                return MarketRegime.BEAR
        else:
            if volatility > self.volatility_threshold:
                return MarketRegime.VOLATILE
            else:
                return MarketRegime.STABLE

    def get_regime_features(self, price_history: List[float]) -> Dict[str, float]:
        """
        Extract regime-related features from price history.

        Args:
            price_history: Historical prices

        Returns:
            Dictionary of regime features
        """
        if len(price_history) < 5:
            return {}

        prices_array = np.array(price_history)
        returns = np.diff(prices_array) / (prices_array[:-1] + 1e-8)

        features = {}

        # Trend features
        x = np.arange(len(prices_array))
        slope, _ = np.polyfit(x, prices_array, 1)
        features['price_trend'] = float(slope / (np.mean(prices_array) + 1e-8))

        # Volatility features
        features['volatility'] = float(np.std(returns))
        features['downside_volatility'] = float(
            np.std([r for r in returns if r < 0])
        ) if any(r < 0 for r in returns) else 0.0

        # Momentum
        if len(returns) >= 5:
            features['momentum_5'] = float(np.mean(returns[-5:]))
        if len(returns) >= 10:
            features['momentum_10'] = float(np.mean(returns[-10:]))

        # Range
        features['price_range'] = float(
            (np.max(prices_array) - np.min(prices_array)) / (np.mean(prices_array) + 1e-8)
        )

        return features


class RegimeSpecificModels:
    """
    Manages separate models for different market regimes.

    Features:
    - Separate model per regime
    - Automatic regime classification
    - Dynamic model switching
    - Regime-aware training
    """

    def __init__(
        self,
        model_dir: Optional[str] = None,
        regimes: Optional[List[MarketRegime]] = None
    ):
        """
        Initialize regime-specific models.

        Args:
            model_dir: Directory for storing regime models
            regimes: List of regimes to support (default: all)
        """
        if model_dir is None:
            model_dir = os.getenv("SCOUT_REGIME_MODEL_DIR", "../models/regimes")

        self.model_dir = Path(model_dir)
        self.model_dir.mkdir(parents=True, exist_ok=True)

        self.regimes = regimes or [
            MarketRegime.BULL,
            MarketRegime.BEAR,
            MarketRegime.VOLATILE,
            MarketRegime.STABLE,
        ]

        # Regime classifier
        self.classifier = RegimeClassifier()

        # Model storage (regime -> model)
        self.models = {}

        # Model metadata
        self.model_metadata = {}

        # Load existing models
        self._load_models()

    def _load_models(self):
        """Load existing regime models from disk."""
        for regime in self.regimes:
            model_file = self.model_dir / f"regime_{regime.value}_model.json"

            if model_file.exists():
                try:
                    # Load model based on type
                    # For now, store metadata and create placeholder
                    metadata_file = self.model_dir / f"regime_{regime.value}_metadata.json"

                    if metadata_file.exists():
                        with open(metadata_file, 'r') as f:
                            metadata = json.load(f)
                        self.model_metadata[regime.value] = metadata
                        logger.info(f"Loaded {regime.value} regime model metadata")
                except Exception as e:
                    logger.warning(f"Failed to load {regime.value} regime model: {e}")

    def train_regime_model(
        self,
        regime: MarketRegime,
        X_train: np.ndarray,
        y_train: np.ndarray,
        X_val: np.ndarray,
        y_val: np.ndarray,
        feature_names: List[str],
        model_type: str = "xgboost"
    ) -> Dict[str, Any]:
        """
        Train a model for a specific regime.

        Args:
            regime: Market regime for this model
            X_train: Training features
            y_train: Training labels
            X_val: Validation features
            y_val: Validation labels
            feature_names: Feature names
            model_type: Type of model to train

        Returns:
            Training metrics
        """
        try:
            if model_type == "xgboost":
                import xgboost as xgb

                dtrain = xgb.DMatrix(X_train, label=y_train, feature_names=feature_names)
                dval = xgb.DMatrix(X_val, label=y_val, feature_names=feature_names)

                params = {
                    'objective': 'reg:squarederror',
                    'max_depth': 6,
                    'eta': 0.1,
                }

                model = xgb.train(
                    params,
                    dtrain,
                    num_boost_round=100,
                    evals=[(dtrain, 'train'), (dval, 'val')],
                    early_stopping_rounds=20,
                    verbose_eval=False,
                )

                # Save model
                model_file = self.model_dir / f"regime_{regime.value}_model.json"
                model.save_model(str(model_file))

                # Get metrics
                train_rmse = model.eval(dtrain).split(':')[1].strip()
                val_rmse = model.eval(dval).split(':')[1].strip()

                self.models[regime] = model

                # Store metadata
                metadata = {
                    'regime': regime.value,
                    'model_type': model_type,
                    'train_rmse': float(train_rmse),
                    'val_rmse': float(val_rmse),
                    'training_samples': len(X_train),
                    'trained_at': datetime.utcnow().isoformat(),
                }

                metadata_file = self.model_dir / f"regime_{regime.value}_metadata.json"
                with open(metadata_file, 'w') as f:
                    json.dump(metadata, f, indent=2)

                self.model_metadata[regime.value] = metadata

                logger.info(f"Trained {regime.value} regime model")

                return metadata

            else:
                return {'error': 'unsupported_model_type'}

        except Exception as e:
            logger.error(f"Failed to train {regime.value} regime model: {e}")
            return {'error': str(e)}

    def predict(
        self,
        features: Dict[str, Any],
        current_price: float,
        price_history: Optional[List[float]] = None,
        force_regime: Optional[MarketRegime] = None
    ) -> Dict[str, Any]:
        """
        Make a prediction with regime-aware model selection.

        Args:
            features: Feature dictionary
            current_price: Current SOL price for regime classification
            price_history: Optional price history for regime classification
            force_regime: Optional forced regime (for testing)

        Returns:
            Dictionary with prediction and regime info
        """
        # Classify regime
        if force_regime:
            regime = force_regime
        else:
            regime = self.classifier.classify(current_price, price_history)

        # Get model for regime
        model = self.models.get(regime)

        if model is None:
            return {
                'error': 'no_model_for_regime',
                'regime': regime.value,
                'prediction': None,
            }

        try:
            # Prepare features
            feature_array = self._prepare_features(features)

            if feature_array is None:
                return {
                    'error': 'feature_preparation_failed',
                    'regime': regime.value,
                }

            # Make prediction
            import xgboost as xgb
            dmatrix = xgb.DMatrix(feature_array)
            prediction = model.predict(dmatrix)

            return {
                'prediction': float(prediction[0]),
                'regime': regime.value,
                'model_used': regime.value,
                'confidence': self._get_regime_confidence(regime, price_history),
            }

        except Exception as e:
            logger.error(f"Prediction failed for {regime.value} regime: {e}")
            return {
                'error': str(e),
                'regime': regime.value,
            }

    def _prepare_features(self, features: Dict[str, Any]) -> Optional[np.ndarray]:
        """Prepare feature array."""
        # Get feature names from first model's metadata
        if self.model_metadata:
            first_regime = next(iter(self.model_metadata.keys()))
            # Assume same features for all regimes
            feature_names = list(features.keys())
            X = [[float(features.get(name, 0.0)) for name in feature_names]]
            return np.array(X)
        else:
            # Use provided features as-is
            feature_names = list(features.keys())
            X = [[float(features.get(name, 0.0)) for name in feature_names]]
            return np.array(X)

    def _get_regime_confidence(
        self,
        regime: MarketRegime,
        price_history: Optional[List[float]]
    ) -> float:
        """Get confidence in regime classification."""
        if price_history is None or len(price_history) < 10:
            return 0.5

        # More history = higher confidence
        confidence = min(1.0, len(price_history) / 30.0)
        return float(confidence)

    def switch_regime(self, new_regime: MarketRegime) -> bool:
        """
        Manually switch to a different regime model.

        Args:
            new_regime: Regime to switch to

        Returns:
            True if successful
        """
        if new_regime in self.models:
            logger.info(f"Switched to {new_regime.value} regime model")
            return True
        else:
            logger.warning(f"No model available for {new_regime.value} regime")
            return False

    def get_available_regimes(self) -> List[str]:
        """Get list of regimes with trained models."""
        return [regime for regime in self.regimes if regime in self.models]

    def get_regime_metadata(self, regime: MarketRegime) -> Optional[Dict[str, Any]]:
        """Get metadata for a regime's model."""
        return self.model_metadata.get(regime.value)


# Global regime models instance
_global_regime_models = None


def get_regime_models() -> RegimeSpecificModels:
    """Get or create global regime models instance."""
    global _global_regime_models
    if _global_regime_models is None:
        _global_regime_models = RegimeSpecificModels()
    return _global_regime_models


def classify_regime(
    current_price: float,
    price_history: Optional[List[float]] = None
) -> MarketRegime:
    """
    Convenience function to classify market regime.

    Args:
        current_price: Current SOL price
        price_history: Optional historical prices

    Returns:
        MarketRegime enum value
    """
    classifier = RegimeClassifier()
    return classifier.classify(current_price, price_history)
