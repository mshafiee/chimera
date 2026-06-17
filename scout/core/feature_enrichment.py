"""
Feature Enrichment for Scout Analyzer

Extends the analyzer's functionality with ML-based feature extraction.
This module provides:
- Time-series feature extraction from trade history
- Market context feature extraction
- Advanced risk metrics
- Feature caching for performance

Usage:
    enricher = FeatureEnricher()
    enriched_metrics = enricher.enrich_wallet_metrics(wallet_metrics, trade_history)
"""

import logging
import os
from typing import Dict, Any, Optional, List
from datetime import datetime
from functools import lru_cache

from scout.config import ScoutConfig

logger = logging.getLogger(__name__)

# Import feature extractors
try:
    from scout.core.time_series_features import TimeSeriesFeatures, extract_time_series_features
    TIME_SERIES_AVAILABLE = True
except ImportError:
    TIME_SERIES_AVAILABLE = False

try:
    from scout.core.market_context_features import MarketContextFeatures, extract_market_context_features
    MARKET_CONTEXT_AVAILABLE = True
except ImportError:
    MARKET_CONTEXT_AVAILABLE = False

try:
    from scout.core.advanced_risk_features import AdvancedRiskFeatures
    ADVANCED_RISK_AVAILABLE = True
except ImportError:
    ADVANCED_RISK_AVAILABLE = False


class FeatureEnricher:
    """
    Enriches wallet metrics with ML-based features.

    Features:
    - Time-series momentum and trend indicators
    - Market context (beta, DEX preferences)
    - Advanced risk metrics (CVaR, drawdown duration)
    - Feature caching for performance
    """

    def __init__(self, cache_enabled: bool = True):
        """
        Initialize the feature enricher.

        Args:
            cache_enabled: Whether to enable feature caching
        """
        self.cache_enabled = cache_enabled and ScoutConfig.get_feature_cache_enabled()

        # Feature extractors
        self.ts_extractor = None
        self.mc_extractor = None
        self.risk_extractor = None

        # Initialize if enabled
        if ScoutConfig.get_time_series_features_enabled() and TIME_SERIES_AVAILABLE:
            self.ts_extractor = TimeSeriesFeatures()
            logger.info("Time-series features initialized")

        if ScoutConfig.get_market_context_features_enabled() and MARKET_CONTEXT_AVAILABLE:
            self.mc_extractor = MarketContextFeatures()
            logger.info("Market context features initialized")

        if ScoutConfig.get_advanced_risk_features_enabled() and ADVANCED_RISK_AVAILABLE:
            self.risk_extractor = AdvancedRiskFeatures()
            logger.info("Advanced risk features initialized")

    def enrich_wallet_metrics(
        self,
        wallet_metrics: Any,
        trade_history: Optional[List[Dict[str, Any]]] = None,
        sol_price_history: Optional[List[Dict[str, Any]]] = None,
        force_refresh: bool = False
    ) -> Dict[str, Any]:
        """
        Enrich wallet metrics with ML-based features.

        Args:
            wallet_metrics: WalletMetrics object or dict
            trade_history: Optional list of historical trades
            sol_price_history: Optional SOL price history for beta calculation
            force_refresh: Whether to bypass cache

        Returns:
            Dictionary with enriched features
        """
        enriched = {
            'enrichment_timestamp': datetime.utcnow().isoformat(),
            'enrichment_success': False,
        }

        # Extract wallet ID for caching
        wallet_id = None
        try:
            wallet_id = getattr(wallet_metrics, 'wallet_id', None)
            if wallet_id is None and isinstance(wallet_metrics, dict):
                wallet_id = wallet_metrics.get('wallet_id')
        except Exception:
            pass

        # Check cache
        if self.cache_enabled and wallet_id and not force_refresh:
            cached = self._get_cached_features(wallet_id)
            if cached:
                return cached

        try:
            # Convert wallet_metrics to dict if needed
            if hasattr(wallet_metrics, '__dict__'):
                metrics_dict = {
                    k: v for k, v in wallet_metrics.__dict__.items()
                    if not k.startswith('_')
                }
            elif isinstance(wallet_metrics, dict):
                metrics_dict = wallet_metrics
            else:
                metrics_dict = {}

            # Start with base metrics
            enriched.update(metrics_dict)
            enriched['base_metrics'] = True

            # Add time-series features
            if self.ts_extractor and trade_history:
                try:
                    ts_features = self._extract_time_series_features(trade_history)
                    enriched.update(ts_features)
                    enriched['time_series_enriched'] = True
                except Exception as e:
                    logger.warning(f"Time-series enrichment failed: {e}")
                    enriched['time_series_enriched'] = False

            # Add market context features
            if self.mc_extractor and trade_history:
                try:
                    mc_features = self._extract_market_context_features(
                        trade_history,
                        sol_price_history
                    )
                    enriched.update(mc_features)
                    enriched['market_context_enriched'] = True
                except Exception as e:
                    logger.warning(f"Market context enrichment failed: {e}")
                    enriched['market_context_enriched'] = False

            # Add advanced risk features
            if self.risk_extractor and trade_history:
                try:
                    risk_features = self._extract_advanced_risk_features(trade_history)
                    enriched.update(risk_features)
                    enriched['advanced_risk_enriched'] = True
                except Exception as e:
                    logger.warning(f"Advanced risk enrichment failed: {e}")
                    enriched['advanced_risk_enriched'] = False

            enriched['enrichment_success'] = True

            # Cache results
            if self.cache_enabled and wallet_id:
                self._cache_features(wallet_id, enriched)

        except Exception as e:
            logger.error(f"Feature enrichment failed: {e}")
            enriched['error'] = str(e)

        return enriched

    def _extract_time_series_features(
        self,
        trade_history: List[Dict[str, Any]]
    ) -> Dict[str, Any]:
        """Extract time-series features from trade history."""
        if not self.ts_extractor:
            return {}

        # Prepare performance history format
        performance_history = []
        for trade in trade_history:
            performance_history.append({
                'pnl_sol': trade.get('pnl_sol', trade.get('pnl', 0.0)),
                'roi': trade.get('roi', 0.0),
                'timestamp': trade.get('timestamp', datetime.utcnow().isoformat()),
            })

        return self.ts_extractor.extract_features(performance_history, feature_set="all")

    def _extract_market_context_features(
        self,
        trade_history: List[Dict[str, Any]],
        sol_price_history: Optional[List[Dict[str, Any]]]
    ) -> Dict[str, Any]:
        """Extract market context features from trade history."""
        if not self.mc_extractor:
            return {}

        return self.mc_extractor.extract_features(trade_history, sol_price_history)

    def _extract_advanced_risk_features(
        self,
        trade_history: List[Dict[str, Any]]
    ) -> Dict[str, Any]:
        """Extract advanced risk features from trade history."""
        if not self.risk_extractor:
            return {}

        return self.risk_extractor.extract_features(trade_history)

    def _cache_features(self, wallet_id: str, features: Dict[str, Any]):
        """Cache enriched features for a wallet."""
        try:
            cache_dir = Path(os.getenv("SCOUT_FEATURE_CACHE_DIR", "../cache/features"))
            cache_dir.mkdir(parents=True, exist_ok=True)

            cache_file = cache_dir / f"{wallet_id}.json"

            # Check TTL
            ttl = ScoutConfig.get_feature_cache_ttl_seconds()
            if cache_file.exists():
                # Check if cache is still valid
                import time
                age = time.time() - cache_file.stat().st_mtime
                if age > ttl:
                    cache_file.unlink()

            # Write to cache
            import json
            with open(cache_file, 'w') as f:
                json.dump(features, f, default=str)

        except Exception as e:
            logger.warning(f"Failed to cache features: {e}")

    def _get_cached_features(self, wallet_id: str) -> Optional[Dict[str, Any]]:
        """Get cached features for a wallet."""
        try:
            cache_dir = Path(os.getenv("SCOUT_FEATURE_CACHE_DIR", "../cache/features"))
            cache_file = cache_dir / f"{wallet_id}.json"

            if not cache_file.exists():
                return None

            # Check TTL
            ttl = ScoutConfig.get_feature_cache_ttl_seconds()
            import time
            age = time.time() - cache_file.stat().st_mtime
            if age > ttl:
                cache_file.unlink()
                return None

            # Load from cache
            import json
            with open(cache_file, 'r') as f:
                return json.load(f)

        except Exception as e:
            logger.warning(f"Failed to load cached features: {e}")
            return None

    def clear_cache(self, wallet_id: Optional[str] = None):
        """
        Clear feature cache.

        Args:
            wallet_id: Specific wallet to clear, or None for all
        """
        try:
            cache_dir = Path(os.getenv("SCOUT_FEATURE_CACHE_DIR", "../cache/features"))

            if wallet_id:
                cache_file = cache_dir / f"{wallet_id}.json"
                if cache_file.exists():
                    cache_file.unlink()
            else:
                # Clear all cache
                for cache_file in cache_dir.glob("*.json"):
                    cache_file.unlink()

        except Exception as e:
            logger.warning(f"Failed to clear cache: {e}")


class AdvancedRiskFeatures:
    """
    Advanced risk metrics for wallet analysis.

    Features:
    - Conditional Value at Risk (CVaR)
    - Maximum drawdown duration
    - Tail risk metrics (95th/99th percentile)
    """

    def __init__(self):
        """Initialize advanced risk features."""
        pass

    def extract_features(
        self,
        trade_history: List[Dict[str, Any]]
    ) -> Dict[str, Any]:
        """
        Extract advanced risk features from trade history.

        Args:
            trade_history: List of trade records

        Returns:
            Dictionary of risk features
        """
        features = {}

        if not trade_history:
            return features

        # Extract PnL values
        pnl_values = []
        for trade in trade_history:
            pnl = trade.get('pnl_sol', trade.get('pnl', 0.0))
            if pnl is not None:
                pnl_values.append(float(pnl))

        if len(pnl_values) < 5:
            return features

        pnl_array = np.array(pnl_values)

        # CVaR (Conditional Value at Risk) at 95%
        try:
            var_95 = np.percentile(pnl_array, 5)  # 5th percentile (loss)
        except Exception:
            var_95 = 0.0

        # CVaR: average of losses beyond VaR
        losses = pnl_array[pnl_array < var_95]
        if len(losses) > 0:
            cvar_95 = float(np.mean(losses))
        else:
            cvar_95 = float(var_95)

        features['cvar_95'] = cvar_95
        features['var_95'] = float(var_95)

        # Tail risk metrics
        if len(pnl_array) >= 10:
            features['percentile_loss_1'] = float(np.percentile(pnl_array, 1))
            features['percentile_loss_5'] = float(np.percentile(pnl_array, 5))
            features['percentile_loss_10'] = float(np.percentile(pnl_array, 10))
            features['percentile_gain_90'] = float(np.percentile(pnl_array, 90))
            features['percentile_gain_95'] = float(np.percentile(pnl_array, 95))
            features['percentile_gain_99'] = float(np.percentile(pnl_array, 99))

        # Drawdown duration analysis
        dd_duration = self._calculate_max_drawdown_duration(pnl_values)
        features['max_drawdown_duration'] = dd_duration

        # Risk-adjusted return metrics
        if len(pnl_array) > 1:
            returns = np.diff(pnl_array)
            features['return_volatility'] = float(np.std(returns))
            features['downside_deviation'] = float(
                np.std([r for r in returns if r < 0])
            ) if any(r < 0 for r in returns) else 0.0

        return features

    def _calculate_max_drawdown_duration(
        self,
        pnl_values: List[float]
    ) -> int:
        """Calculate maximum drawdown duration in trades."""
        if not pnl_values:
            return 0

        peak = pnl_values[0]
        max_duration = 0
        current_duration = 0

        for value in pnl_values:
            if value > peak:
                peak = value
                current_duration = 0
            else:
                current_duration += 1
                max_duration = max(max_duration, current_duration)

        return max_duration


# Global enricher instance
_global_enricher = None


def get_feature_enricher() -> FeatureEnricher:
    """Get or create global feature enricher instance."""
    global _global_enricher
    if _global_enricher is None:
        _global_enricher = FeatureEnricher()
    return _global_enricher


def enrich_wallet_metrics(
    wallet_metrics: Any,
    trade_history: Optional[List[Dict[str, Any]]] = None,
    sol_price_history: Optional[List[Dict[str, Any]]] = None
) -> Dict[str, Any]:
    """
    Convenience function to enrich wallet metrics.

    Args:
        wallet_metrics: WalletMetrics object or dict
        trade_history: Optional trade history
        sol_price_history: Optional SOL price history

    Returns:
        Dictionary with enriched features
    """
    enricher = get_feature_enricher()
    return enricher.enrich_wallet_metrics(wallet_metrics, trade_history, sol_price_history)


# Import numpy
try:
    import numpy as np
    NUMPY_AVAILABLE = True
except ImportError:
    NUMPY_AVAILABLE = False

# Import Path
try:
    from pathlib import Path
    PATH_AVAILABLE = True
except ImportError:
    PATH_AVAILABLE = False
