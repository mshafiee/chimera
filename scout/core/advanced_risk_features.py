"""
Advanced Risk Features for Scout

Extracts sophisticated risk metrics beyond basic drawdown.
This module provides:
- Conditional Value at Risk (CVaR)
- Maximum drawdown duration analysis
- Tail risk metrics (95th/99th percentile)
- Risk-adjusted performance ratios

Usage:
    extractor = AdvancedRiskFeatures()
    features = extractor.extract_features(trade_history)
"""

import logging
import numpy as np
from typing import Dict, List, Any

logger = logging.getLogger(__name__)

# Try to import scipy for advanced statistics
try:
    from scipy import stats
    SCIPY_AVAILABLE = True
except ImportError:
    SCIPY_AVAILABLE = False
    logger.warning("scipy not available - some risk features will be limited")


class AdvancedRiskFeatures:
    """
    Advanced risk metrics for wallet analysis.

    Features:
    - CVaR (Conditional Value at Risk)
    - Maximum drawdown duration
    - Tail risk metrics (95th/99th percentile)
    - Risk-adjusted return ratios
    - Downside deviation
    - Ulcer Index
    """

    def __init__(self, confidence_levels: List[float] = None):
        """
        Initialize advanced risk features.

        Args:
            confidence_levels: Confidence levels for VaR/CVaR (default: [0.95, 0.99])
        """
        self.confidence_levels = confidence_levels or [0.90, 0.95, 0.99]

    def extract_features(
        self,
        trade_history: List[Dict[str, Any]]
    ) -> Dict[str, Any]:
        """
        Extract advanced risk features from trade history.

        Args:
            trade_history: List of trade records with pnl_sol

        Returns:
            Dictionary of risk features
        """
        if not trade_history:
            return self._empty_features()

        features = {}

        try:
            # Extract PnL values
            pnl_values = []
            for trade in trade_history:
                pnl = trade.get('pnl_sol', trade.get('pnl', 0.0))
                if pnl is not None:
                    pnl_values.append(float(pnl))

            if len(pnl_values) < 5:
                return self._empty_features()

            pnl_array = np.array(pnl_values)

            # VaR and CVaR
            var_features = self._calculate_var_cvar(pnl_array)
            features.update(var_features)

            # Drawdown duration
            dd_features = self._calculate_drawdown_duration(pnl_array)
            features.update(dd_features)

            # Tail risk metrics
            tail_features = self._calculate_tail_risk(pnl_array)
            features.update(tail_features)

            # Risk-adjusted return ratios
            ratio_features = self._calculate_risk_ratios(pnl_array)
            features.update(ratio_features)

            # Downside risk
            downside_features = self._calculate_downside_risk(pnl_array)
            features.update(downside_features)

            # Ulcer Index
            ulcer_index = self._calculate_ulcer_index(pnl_array)
            features['ulcer_index'] = ulcer_index

            # Risk regime classification
            features['risk_regime'] = self._classify_risk_regime(features)

            features['extraction_success'] = True
            features['sample_count'] = len(pnl_array)

        except Exception as e:
            logger.error(f"Advanced risk feature extraction failed: {e}")
            return self._empty_features()

        return features

    def _empty_features(self) -> Dict[str, Any]:
        """Return empty features dict."""
        return {
            'extraction_success': False,
            'sample_count': 0,
        }

    def _calculate_var_cvar(
        self,
        pnl_array: np.ndarray
    ) -> Dict[str, float]:
        """Calculate Value at Risk and Conditional Value at Risk."""
        features = {}

        for confidence in self.confidence_levels:
            alpha = 1 - confidence

            # VaR (percentile)
            var = float(np.percentile(pnl_array, alpha * 100))
            features[f'var_{int(confidence * 100)}'] = var

            # CVaR (average of losses beyond VaR)
            losses = pnl_array[pnl_array <= var]
            if len(losses) > 0:
                cvar = float(np.mean(losses))
            else:
                cvar = var

            features[f'cvar_{int(confidence * 100)}'] = cvar

            # CVaR to VaR ratio
            if var != 0:
                features[f'cvar_var_ratio_{int(confidence * 100)}'] = cvar / var
            else:
                features[f'cvar_var_ratio_{int(confidence * 100)}'] = 1.0

        return features

    def _calculate_drawdown_duration(
        self,
        pnl_array: np.ndarray
    ) -> Dict[str, float]:
        """Calculate drawdown duration metrics."""
        features = {}

        # Calculate cumulative returns
        cumulative = np.cumsum(pnl_array)
        peak = cumulative[0]
        max_duration = 0
        current_duration = 0
        max_depth = 0

        for i, value in enumerate(cumulative):
            if value > peak:
                peak = value
                current_duration = 0
            else:
                current_duration += 1
                max_duration = max(max_duration, current_duration)

                # Track depth
                depth = (peak - value) / (abs(peak) + 1e-8)
                max_depth = max(max_depth, depth)

        features['max_drawdown_duration_trades'] = int(max_duration)
        features['max_drawdown_depth'] = float(max_depth)

        # Average drawdown duration
        drawdowns = []
        in_drawdown = False
        dd_start = 0

        for i, value in enumerate(cumulative):
            if value > peak:
                if in_drawdown:
                    drawdowns.append(i - dd_start)
                    in_drawdown = False
                peak = value
            elif not in_drawdown:
                in_drawdown = True
                dd_start = i

        if in_drawdown:
            drawdowns.append(len(cumulative) - dd_start)

        if drawdowns:
            features['avg_drawdown_duration_trades'] = float(np.mean(drawdowns))
        else:
            features['avg_drawdown_duration_trades'] = 0.0

        # Recovery time (time from max drawdown to new high)
        max_dd_idx = np.argmax(peak - cumulative)
        recovery_time = 0

        for i in range(max_dd_idx + 1, len(cumulative)):
            if cumulative[i] >= peak:
                recovery_time = i - max_dd_idx
                break

        features['recovery_time_trades'] = int(recovery_time)

        return features

    def _calculate_tail_risk(
        self,
        pnl_array: np.ndarray
    ) -> Dict[str, float]:
        """Calculate tail risk metrics."""
        features = {}

        if len(pnl_array) >= 10:
            # Percentiles
            features['percentile_loss_1'] = float(np.percentile(pnl_array, 1))
            features['percentile_loss_5'] = float(np.percentile(pnl_array, 5))
            features['percentile_loss_10'] = float(np.percentile(pnl_array, 10))
            features['percentile_gain_90'] = float(np.percentile(pnl_array, 90))
            features['percentile_gain_95'] = float(np.percentile(pnl_array, 95))
            features['percentile_gain_99'] = float(np.percentile(pnl_array, 99))

            # Tail ratio (worst loss to best gain ratio)
            worst_loss = abs(features['percentile_loss_1'])
            best_gain = features['percentile_gain_99']

            if best_gain > 0:
                features['tail_ratio'] = worst_loss / best_gain
            else:
                features['tail_ratio'] = float('inf') if worst_loss > 0 else 0.0

            # Skewness and kurtosis
            if SCIPY_AVAILABLE:
                features['return_skewness'] = float(stats.skew(pnl_array))
                features['return_kurtosis'] = float(stats.kurtosis(pnl_array))
            else:
                # Manual calculation
                mean = np.mean(pnl_array)
                std = np.std(pnl_array)
                if std > 0:
                    skew = np.mean(((pnl_array - mean) / std) ** 3)
                    kurt = np.mean(((pnl_array - mean) / std) ** 4) - 3
                    features['return_skewness'] = float(skew)
                    features['return_kurtosis'] = float(kurt)

        return features

    def _calculate_risk_ratios(
        self,
        pnl_array: np.ndarray
    ) -> Dict[str, float]:
        """Calculate risk-adjusted return ratios."""
        features = {}

        if len(pnl_array) < 2:
            return features

        returns = np.diff(pnl_array)

        # Total return
        total_return = pnl_array[-1] - pnl_array[0]

        # Volatility (standard deviation)
        volatility = np.std(returns)

        # Downside deviation
        negative_returns = returns[returns < 0]
        downside_deviation = np.std(negative_returns) if len(negative_returns) > 0 else 0.0

        # Sharpe Ratio (simplified, assuming risk-free rate = 0)
        if volatility > 0:
            features['sharpe_ratio'] = total_return / volatility
        else:
            features['sharpe_ratio'] = 0.0

        # Sortino Ratio
        if downside_deviation > 0:
            features['sortino_ratio'] = total_return / downside_deviation
        else:
            features['sortino_ratio'] = float('inf') if total_return > 0 else 0.0

        # Calmar Ratio (return / max drawdown)
        max_drawdown = self._calculate_max_drawdown(pnl_array)
        if max_drawdown != 0:
            features['calmar_ratio'] = total_return / abs(max_drawdown)
        else:
            features['calmar_ratio'] = float('inf') if total_return > 0 else 0.0

        # Sterling Ratio (return / average drawdown)
        avg_drawdown = self._calculate_avg_drawdown(pnl_array)
        if avg_drawdown != 0:
            features['sterling_ratio'] = total_return / avg_drawdown
        else:
            features['sterling_ratio'] = float('inf') if total_return > 0 else 0.0

        return features

    def _calculate_downside_risk(
        self,
        pnl_array: np.ndarray
    ) -> Dict[str, float]:
        """Calculate downside risk metrics."""
        features = {}

        if len(pnl_array) < 2:
            return features

        returns = np.diff(pnl_array)
        negative_returns = returns[returns < 0]

        if len(negative_returns) == 0:
            features['downside_deviation'] = 0.0
            features['downside_frequency'] = 0.0
            features['avg_downside'] = 0.0
            return features

        # Downside deviation
        features['downside_deviation'] = float(np.std(negative_returns))

        # Downside frequency
        features['downside_frequency'] = len(negative_returns) / len(returns)

        # Average downside
        features['avg_downside'] = float(np.mean(negative_returns))

        # Maximum downside
        features['max_downside'] = float(np.min(negative_returns))

        return features

    def _calculate_ulcer_index(
        self,
        pnl_array: np.ndarray
    ) -> float:
        """
        Calculate Ulcer Index.

        Ulcer Index measures the depth and duration of drawdowns.
        Lower values indicate less risk.
        """
        cumulative = np.cumsum(pnl_array)
        peak = cumulative[0]
        ulcer_values = []

        for value in cumulative:
            if value > peak:
                peak = value
            # Percentage drop from peak
            drop = (peak - value) / (abs(peak) + 1e-8) * 100
            ulcer_values.append(drop ** 2)

        if ulcer_values:
            ulcer_index = np.sqrt(np.mean(ulcer_values))
        else:
            ulcer_index = 0.0

        return float(ulcer_index)

    def _calculate_max_drawdown(
        self,
        pnl_array: np.ndarray
    ) -> float:
        """Calculate maximum drawdown."""
        cumulative = np.cumsum(pnl_array)
        peak = cumulative[0]
        max_dd = 0.0

        for value in cumulative:
            if value > peak:
                peak = value
            dd = (peak - value) / (abs(peak) + 1e-8)
            max_dd = max(max_dd, dd)

        return max_dd

    def _calculate_avg_drawdown(
        self,
        pnl_array: np.ndarray
    ) -> float:
        """Calculate average drawdown."""
        cumulative = np.cumsum(pnl_array)
        peak = cumulative[0]
        drawdowns = []

        for value in cumulative:
            if value > peak:
                peak = value
            dd = abs((peak - value) / (abs(peak) + 1e-8))
            drawdowns.append(dd)

        return np.mean(drawdowns) if drawdowns else 0.0

    def _classify_risk_regime(
        self,
        features: Dict[str, Any]
    ) -> str:
        """
        Classify the risk regime of the wallet.

        Returns:
            One of: "LOW", "MODERATE", "HIGH", "EXTREME"
        """
        # Get key risk indicators
        var_95 = features.get('var_95', 0)
        cvar_95 = features.get('cvar_95', 0)
        ulcer_index = features.get('ulcer_index', 0)
        max_dd_duration = features.get('max_drawdown_duration_trades', 0)

        # Calculate risk score
        risk_score = 0

        # VaR component
        if var_95 < -0.5:
            risk_score += 3
        elif var_95 < -0.2:
            risk_score += 2
        elif var_95 < -0.1:
            risk_score += 1

        # CVaR component
        if cvar_95 < -1.0:
            risk_score += 3
        elif cvar_95 < -0.5:
            risk_score += 2
        elif cvar_95 < -0.2:
            risk_score += 1

        # Ulcer Index component
        if ulcer_index > 20:
            risk_score += 3
        elif ulcer_index > 10:
            risk_score += 2
        elif ulcer_index > 5:
            risk_score += 1

        # Drawdown duration component
        if max_dd_duration > 20:
            risk_score += 2
        elif max_dd_duration > 10:
            risk_score += 1

        # Classify
        if risk_score >= 10:
            return "EXTREME"
        elif risk_score >= 6:
            return "HIGH"
        elif risk_score >= 3:
            return "MODERATE"
        else:
            return "LOW"


# Convenience function
def extract_advanced_risk_features(
    trade_history: List[Dict[str, Any]]
) -> Dict[str, Any]:
    """
    Quick extraction of advanced risk features.

    Args:
        trade_history: List of trade records

    Returns:
        Dictionary of risk features
    """
    extractor = AdvancedRiskFeatures()
    return extractor.extract_features(trade_history)
