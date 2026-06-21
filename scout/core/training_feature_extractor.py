"""
Training Feature Extractor for Scout ML Models

Enriches wallet features with all available feature types:
- Time-series features (RSI, MACD, Bollinger Bands, etc.)
- Market context features (beta, DEX preferences, time patterns)
- Network features (centrality, co-holding patterns)
- Advanced risk features (CVaR, drawdown duration, tail risk)

Usage:
    extractor = TrainingFeatureExtractor()
    features = extractor.extract_all_features(wallet_address, trade_history)
"""

import logging
from typing import Dict, List, Optional, Tuple, Any
import numpy as np

logger = logging.getLogger(__name__)


class TrainingFeatureExtractor:
    """
    Enriches wallet features with all feature types for training.

    This class combines multiple feature extractors to create
    comprehensive feature vectors for ML training.
    """

    def __init__(self):
        """Initialize the training feature extractor."""
        # Import feature extractors
        try:
            from scout.core.time_series_features import TimeSeriesFeatures
            self.time_series_extractor = TimeSeriesFeatures()
        except ImportError:
            logger.warning("TimeSeriesFeatures not available")
            self.time_series_extractor = None

        try:
            from scout.core.market_context_features import MarketContextFeatures
            self.market_extractor = MarketContextFeatures()
        except ImportError:
            logger.warning("MarketContextFeatures not available")
            self.market_extractor = None

        try:
            from scout.core.network_features import NetworkFeatures
            self.network_extractor = NetworkFeatures()
        except ImportError:
            logger.warning("NetworkFeatures not available")
            self.network_extractor = None

        try:
            from scout.core.advanced_risk_features import AdvancedRiskFeatures
            self.risk_extractor = AdvancedRiskFeatures()
        except ImportError:
            logger.warning("AdvancedRiskFeatures not available")
            self.risk_extractor = None

    def extract_all_features(
        self,
        wallet_address: str,
        wallet_metrics: Optional[Dict[str, Any]] = None,
        trade_history: Optional[List[Dict[str, Any]]] = None,
        transaction_graph: Optional[Dict[str, Any]] = None,
        token_holdings: Optional[Dict[str, float]] = None,
        sol_price_history: Optional[List[float]] = None,
        known_successful_wallets: Optional[List[str]] = None
    ) -> Dict[str, Any]:
        """
        Extract and combine all feature types.

        Args:
            wallet_address: Wallet address to analyze
            wallet_metrics: Base wallet metrics (from database)
            trade_history: List of trade records for time-series analysis
            transaction_graph: Transaction graph for network analysis
            token_holdings: Current token holdings
            sol_price_history: SOL price history for market context
            known_successful_wallets: List of known successful wallet addresses

        Returns:
            Dictionary combining all feature types
        """
        features = {}

        # Start with base wallet metrics
        if wallet_metrics:
            features.update(self._extract_base_metrics(wallet_metrics))

        # Time-series features
        if self.time_series_extractor and trade_history:
            try:
                ts_features = self.time_series_extractor.extract_features(trade_history)
                features.update(ts_features)
            except Exception as e:
                logger.warning(f"Time-series feature extraction failed: {e}")

        # Market context features
        if self.market_extractor and sol_price_history:
            try:
                market_features = self.market_extractor.extract_features(
                    wallet_address,
                    sol_price_history,
                    trade_history
                )
                features.update(market_features)
            except Exception as e:
                logger.warning(f"Market context feature extraction failed: {e}")

        # Network features
        if self.network_extractor:
            try:
                network_features = self.network_extractor.extract_features(
                    wallet_address,
                    transaction_graph,
                    token_holdings,
                    known_successful_wallets
                )
                features.update(network_features)
            except Exception as e:
                logger.warning(f"Network feature extraction failed: {e}")

        # Advanced risk features
        if self.risk_extractor and trade_history:
            try:
                risk_features = self.risk_extractor.extract_features(trade_history)
                features.update(risk_features)
            except Exception as e:
                logger.warning(f"Advanced risk feature extraction failed: {e}")

        return features

    def _extract_base_metrics(self, wallet_metrics: Dict[str, Any]) -> Dict[str, Any]:
        """
        Extract base metrics from wallet data.

        Args:
            wallet_metrics: Wallet metrics dictionary

        Returns:
            Dictionary of base features
        """
        features = {}

        # Core metrics
        metric_fields = [
            'wqs_score', 'roi_7d', 'roi_30d', 'trade_count_30d',
            'win_rate', 'max_drawdown_30d', 'avg_trade_size_sol',
            'profit_factor', 'sortino_ratio', 'avg_entry_delay_seconds',
            'realized_pnl_30d_sol'
        ]

        for field in metric_fields:
            value = wallet_metrics.get(field)
            if value is not None and not np.isnan(value):
                features[field] = float(value)

        # Categorical features (one-hot encode)
        status = wallet_metrics.get('status')
        if status:
            features['status_ACTIVE'] = 1.0 if status == 'ACTIVE' else 0.0
            features['status_CANDIDATE'] = 1.0 if status == 'CANDIDATE' else 0.0
            features['status_REJECTED'] = 1.0 if status == 'REJECTED' else 0.0

        archetype = wallet_metrics.get('archetype')
        if archetype:
            archetypes = ['SNIPER', 'SWING', 'SCALPER', 'INSIDER', 'WHALE', 'UNKNOWN']
            for arch in archetypes:
                features[f'archetype_{arch}'] = 1.0 if archetype == arch else 0.0

        return features

    def extract_for_dataset(
        self,
        wallet_data: List[Dict[str, Any]],
        feature_names: Optional[List[str]] = None
    ) -> Tuple[np.ndarray, List[str]]:
        """
        Extract features for a dataset of wallets.

        Args:
            wallet_data: List of wallet data dictionaries
            feature_names: Optional list of feature names to use

        Returns:
            Tuple of (feature_matrix, feature_names)
        """
        if not wallet_data:
            return np.array([]), []

        # Extract features for each wallet
        all_features = []
        for wallet in wallet_data:
            features = self.extract_all_features(
                wallet_address=wallet.get('address', ''),
                wallet_metrics=wallet,
                trade_history=wallet.get('trade_history'),
                sol_price_history=wallet.get('sol_price_history')
            )
            all_features.append(features)

        # Get unified feature names
        if feature_names is None:
            feature_names = self._get_unified_feature_names(all_features)

        # Build feature matrix
        X = []
        for features in all_features:
            row = [features.get(name, 0.0) for name in feature_names]
            X.append(row)

        return np.array(X), feature_names

    def _get_unified_feature_names(self, feature_list: List[Dict[str, Any]]) -> List[str]:
        """
        Get unified list of feature names from all feature dictionaries.

        Args:
            feature_list: List of feature dictionaries

        Returns:
            Sorted list of unique feature names
        """
        all_names = set()
        for features in feature_list:
            all_names.update(features.keys())

        return sorted(all_names)

    def get_feature_importance(
        self,
        model: Any,
        feature_names: List[str]
    ) -> Dict[str, float]:
        """
        Get feature importance from trained model.

        Args:
            model: Trained model with feature_importances_ attribute
            feature_names: List of feature names

        Returns:
            Dictionary mapping feature names to importance scores
        """
        try:
            if hasattr(model, 'feature_importances_'):
                importances = model.feature_importances_
                return dict(zip(feature_names, importances))
            elif hasattr(model, 'get_booster'):
                # XGBoost
                booster = model.get_booster()
                importance_dict = booster.get_score(importance_type='weight')
                # Normalize to feature_names
                normalized = {}
                for i, name in enumerate(feature_names):
                    key = f'f{i}'
                    normalized[name] = float(importance_dict.get(key, 0.0))
                return normalized
            else:
                logger.warning("Model does not have feature importance attribute")
                return {}
        except Exception as e:
            logger.error(f"Failed to get feature importance: {e}")
            return {}


class FeatureEnricher:
    """
    Simplified feature enricher for production use.

    This is a lightweight version that enriches wallet metrics
    with essential features for WQS enhancement.
    """

    def __init__(self):
        """Initialize the feature enricher."""
        self.extractor = TrainingFeatureExtractor()

    def enrich_wallet_metrics(
        self,
        wallet_metrics: Dict[str, Any],
        trade_history: Optional[List[Dict[str, Any]]] = None,
        sol_price_history: Optional[List[float]] = None
    ) -> Dict[str, Any]:
        """
        Enrich wallet metrics with ML features.

        Args:
            wallet_metrics: Base wallet metrics
            trade_history: Optional trade history
            sol_price_history: Optional SOL price history

        Returns:
            Enriched wallet metrics
        """
        wallet_address = wallet_metrics.get('address', '')

        enriched = self.extractor.extract_all_features(
            wallet_address=wallet_address,
            wallet_metrics=wallet_metrics,
            trade_history=trade_history,
            sol_price_history=sol_price_history
        )

        # Merge with original metrics
        enriched.update(wallet_metrics)

        return enriched


# Convenience functions
def extract_training_features(
    wallet_data: List[Dict[str, Any]]
) -> Tuple[np.ndarray, List[str]]:
    """
    Convenience function to extract features for training.

    Args:
        wallet_data: List of wallet data dictionaries

    Returns:
        Tuple of (feature_matrix, feature_names)
    """
    extractor = TrainingFeatureExtractor()
    return extractor.extract_for_dataset(wallet_data)


def enrich_single_wallet(
    wallet_metrics: Dict[str, Any],
    trade_history: Optional[List[Dict[str, Any]]] = None
) -> Dict[str, Any]:
    """
    Convenience function to enrich a single wallet's metrics.

    Args:
        wallet_metrics: Base wallet metrics
        trade_history: Optional trade history

    Returns:
        Enriched wallet metrics
    """
    enricher = FeatureEnricher()
    return enricher.enrich_wallet_metrics(wallet_metrics, trade_history)
