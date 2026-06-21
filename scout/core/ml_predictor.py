"""
ML-based Profitability Prediction for Scout

Uses historical wallet features to predict profitability (PnL).
This module provides:
- Simple regression models for profitability prediction
- Feature importance tracking
- Continuous retraining support
- Prediction confidence scoring

Usage:
    predictor = ProfitabilityPredictor()
    prediction = predictor.predict_profitability(wallet_features)
"""

import logging
import os
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Tuple, Any
import pickle

logger = logging.getLogger(__name__)


class SimpleRegressionPredictor:
    """
    Simple linear regression predictor for wallet profitability.

    Uses weighted linear combination of features to predict PnL.
    Designed for interpretability and fast training without ML dependencies.
    """

    def __init__(self):
        """Initialize the predictor with default weights."""
        # Default feature weights (can be trained from historical data)
        self.feature_weights = {
            'roi_7d': 0.15,
            'roi_30d': 0.25,
            'win_rate': 0.20,
            'profit_factor': 0.15,
            'sortino_ratio': 0.10,
            'trade_count_30d': 0.05,
            'max_drawdown_30d': -0.10,  # Negative weight (lower is better)
        }
        self.feature_means = {}  # For normalization
        self.feature_stds = {}   # For normalization
        self.training_samples = 0
        self.last_trained = None

    def normalize_feature(self, feature_name: str, value: float) -> float:
        """Normalize a feature value using z-score normalization."""
        if feature_name not in self.feature_means:
            return value  # No normalization data available

        mean = self.feature_means[feature_name]
        std = self.feature_stds.get(feature_name, 1.0)
        if std == 0:
            return 0.0
        return (value - mean) / std

    def predict(self, features: Dict[str, Any]) -> Tuple[float, float]:
        """
        Predict profitability from wallet features.

        Args:
            features: Dictionary of wallet features

        Returns:
            Tuple of (predicted_pnl, confidence_score)
        """
        score = 0.0
        weight_sum = 0.0
        feature_count = 0

        for feature_name, weight in self.feature_weights.items():
            value = features.get(feature_name)
            if value is None:
                continue

            # Normalize the feature
            normalized_value = self.normalize_feature(feature_name, float(value))

            # Apply weight and accumulate
            score += normalized_value * weight
            weight_sum += abs(weight)
            feature_count += 1

        # Calculate confidence based on feature coverage
        confidence = feature_count / max(1, len(self.feature_weights))

        # Normalize score to get predicted PnL
        # Scale: assume 0.0 score = 0 PnL, ±1.0 = ±1 SOL
        if weight_sum > 0:
            normalized_score = score / weight_sum
        else:
            normalized_score = 0.0

        # Convert to PnL estimate (simple scaling)
        predicted_pnl = normalized_score * 0.5  # 0.5 SOL per unit score

        return predicted_pnl, confidence

    def train_from_history(self, historical_data: List[Dict[str, Any]]) -> Dict[str, float]:
        """
        Train the predictor from historical wallet performance data.

        Args:
            historical_data: List of dicts with features and actual_pnl

        Returns:
            Dictionary of training metrics
        """
        if len(historical_data) < 5:
            logger.warning(f"Insufficient training data: {len(historical_data)} < 5 samples")
            return {'error': 'insufficient_data'}

        # Calculate feature statistics for normalization
        feature_values = {name: [] for name in self.feature_weights.keys()}
        pnl_values = []

        for record in historical_data:
            pnl = record.get('actual_pnl_sol')
            if pnl is None:
                continue
            pnl_values.append(pnl)

            for feature_name in self.feature_weights.keys():
                value = record.get(feature_name)
                if value is not None:
                    feature_values[feature_name].append(float(value))

        # Calculate means and stds
        for feature_name, values in feature_values.items():
            if values:
                self.feature_means[feature_name] = sum(values) / len(values)
                if len(values) > 1:
                    variance = sum((v - self.feature_means[feature_name]) ** 2 for v in values) / len(values)
                    self.feature_stds[feature_name] = variance ** 0.5
                else:
                    self.feature_stds[feature_name] = 1.0

        # Simple correlation-based weight adjustment
        # (In a real implementation, this would use proper regression)
        for feature_name in self.feature_weights.keys():
            if feature_name in feature_values and len(feature_values[feature_name]) > 1:
                # Calculate correlation with PnL
                corr = self._calculate_correlation(
                    feature_values[feature_name],
                    pnl_values,
                    feature_name
                )
                # Adjust weight based on correlation (clamped)
                if corr is not None:
                    adjusted_weight = max(-0.3, min(0.3, corr * 0.3))
                    self.feature_weights[feature_name] = adjusted_weight

        self.training_samples = len(historical_data)
        self.last_trained = datetime.utcnow().isoformat()

        return {
            'training_samples': self.training_samples,
            'last_trained': self.last_trained,
            'feature_weights': self.feature_weights,
        }

    def _calculate_correlation(
        self,
        xs: List[float],
        ys: List[float],
        feature_name: str
    ) -> Optional[float]:
        """Calculate Pearson correlation coefficient."""
        if len(xs) != len(ys) or len(xs) < 2:
            return None

        n = len(xs)
        sum_x = sum(xs)
        sum_y = sum(ys)
        sum_xy = sum(x * y for x, y in zip(xs, ys))
        sum_x2 = sum(x * x for x in xs)
        sum_y2 = sum(y * y for y in ys)

        denominator = ((n * sum_x2 - sum_x ** 2) * (n * sum_y2 - sum_y ** 2)) ** 0.5
        if denominator == 0:
            return None

        correlation = (n * sum_xy - sum_x * sum_y) / denominator
        return correlation


class ProfitabilityPredictor:
    """
    Main predictor class that handles training and prediction.

    Provides a simple interface for profitability prediction with
    confidence scoring and continuous retraining support.
    """

    def __init__(self, model_path: Optional[str] = None):
        """
        Initialize the predictor.

        Args:
            model_path: Path to save/load trained models
        """
        if model_path is None:
            model_path = os.getenv("SCOUT_ML_MODEL_PATH", "data/profitability_model.pkl")

        self.model_path = Path(model_path)
        self.model_path.parent.mkdir(parents=True, exist_ok=True)

        self.predictor = SimpleRegressionPredictor()
        self.feature_importance = {}

        # Try to load existing model
        self._load_model()

    def predict_profitability(
        self,
        wallet_features: Dict[str, Any]
    ) -> Dict[str, Any]:
        """
        Predict profitability for a wallet.

        Args:
            wallet_features: Dictionary of wallet features

        Returns:
            Dictionary with predicted_pnl, confidence, and metadata
        """
        predicted_pnl, confidence = self.predictor.predict(wallet_features)

        return {
            'predicted_pnl_sol': predicted_pnl,
            'confidence': confidence,
            'prediction_timestamp': datetime.utcnow().isoformat(),
            'training_samples': self.predictor.training_samples,
            'last_trained': self.predictor.last_trained,
        }

    def train_from_features(
        self,
        features_file: Optional[str] = None
    ) -> Dict[str, Any]:
        """
        Train the predictor from historical feature data.

        Args:
            features_file: Path to CSV file with historical features

        Returns:
            Dictionary with training metrics
        """
        if features_file is None:
            features_file = os.getenv("SCOUT_FEATURE_STORE_PATH", "data/features/wallet_features.csv")

        features_path = Path(features_file)
        if not features_path.exists():
            logger.warning(f"Features file not found: {features_file}")
            return {'error': 'file_not_found'}

        # Load and parse features
        historical_data = self._load_historical_features(features_path)

        if not historical_data:
            logger.warning("No valid historical data found for training")
            return {'error': 'no_data'}

        # Train the predictor
        metrics = self.predictor.train_from_history(historical_data)

        # Save the trained model
        self._save_model()

        return metrics

    def _load_historical_features(self, features_path: Path) -> List[Dict[str, Any]]:
        """Load historical features from CSV file."""
        import csv

        historical_data = []
        try:
            with open(features_path, 'r') as f:
                reader = csv.DictReader(f)
                for row in reader:
                    # Extract relevant features
                    record = {
                        'roi_7d': self._safe_float(row.get('roi_7d')),
                        'roi_30d': self._safe_float(row.get('roi_30d')),
                        'win_rate': self._safe_float(row.get('win_rate')),
                        'profit_factor': self._safe_float(row.get('profit_factor')),
                        'sortino_ratio': self._safe_float(row.get('sortino_ratio')),
                        'trade_count_30d': self._safe_int(row.get('trade_count_30d')),
                        'max_drawdown_30d': self._safe_float(row.get('max_drawdown_30d')),
                    }

                    # Look for actual PnL from database or other source
                    # For now, we'll use roi_30d as a proxy
                    if record.get('roi_30d') is not None:
                        # Convert ROI to SOL PnL (rough approximation)
                        # In production, this would come from actual copy-trade results
                        record['actual_pnl_sol'] = record['roi_30d'] * 0.1  # Rough scaling

                    historical_data.append(record)
        except Exception as e:
            logger.error(f"Error loading historical features: {e}")
            return []

        return historical_data

    def _safe_float(self, value: Any) -> Optional[float]:
        """Safely convert value to float."""
        if value is None or value == '':
            return None
        try:
            return float(value)
        except (ValueError, TypeError):
            return None

    def _safe_int(self, value: Any) -> Optional[int]:
        """Safely convert value to int."""
        if value is None or value == '':
            return None
        try:
            return int(float(value))
        except (ValueError, TypeError):
            return None

    def _save_model(self):
        """Save the trained model to disk."""
        try:
            model_data = {
                'feature_weights': self.predictor.feature_weights,
                'feature_means': self.predictor.feature_means,
                'feature_stds': self.predictor.feature_stds,
                'training_samples': self.predictor.training_samples,
                'last_trained': self.predictor.last_trained,
                'feature_importance': self.feature_importance,
            }
            with open(self.model_path, 'wb') as f:
                pickle.dump(model_data, f)
            logger.info(f"Model saved to {self.model_path}")
        except Exception as e:
            logger.error(f"Error saving model: {e}")

    def _load_model(self):
        """Load a trained model from disk."""
        if not self.model_path.exists():
            return

        try:
            with open(self.model_path, 'rb') as f:
                model_data = pickle.load(f)

            self.predictor.feature_weights = model_data.get('feature_weights', {})
            self.predictor.feature_means = model_data.get('feature_means', {})
            self.predictor.feature_stds = model_data.get('feature_stds', {})
            self.predictor.training_samples = model_data.get('training_samples', 0)
            self.predictor.last_trained = model_data.get('last_trained')
            self.feature_importance = model_data.get('feature_importance', {})

            logger.info(f"Model loaded from {self.model_path} ({self.predictor.training_samples} samples)")
        except Exception as e:
            logger.warning(f"Error loading model: {e}")


# Convenience function for quick predictions
def predict_wallet_profitability(wallet_features: Dict[str, Any]) -> Dict[str, Any]:
    """
    Quick prediction of wallet profitability.

    Args:
        wallet_features: Dictionary of wallet features

    Returns:
        Dictionary with predicted_pnl_sol and confidence
    """
    predictor = ProfitabilityPredictor()
    return predictor.predict_profitability(wallet_features)
