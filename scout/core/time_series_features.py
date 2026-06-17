"""
Time-Series Features for Scout

Extracts temporal features from wallet performance data.
This module provides:
- Momentum indicators (RSI, MACD, Bollinger Bands)
- Autocorrelation features for performance persistence
- Lagged features for trend analysis
- Cyclical pattern detection

Usage:
    extractor = TimeSeriesFeatures()
    features = extractor.extract_features(wallet_performance_history)
"""

import logging
import numpy as np
from typing import Dict, List, Optional, Tuple, Any, Union
from collections import deque
from datetime import datetime, timedelta

logger = logging.getLogger(__name__)

# Try to import scipy for advanced features
try:
    from scipy import signal, stats
    from scipy.fft import fft, fftfreq
    SCIPY_AVAILABLE = True
except ImportError:
    SCIPY_AVAILABLE = False
    logger.warning("scipy not available - some features will be limited")

# Try to import statsmodels
try:
    from statsmodels.tsa.stattools import acf
    STATSMODELS_AVAILABLE = True
except ImportError:
    STATSMODELS_AVAILABLE = False
    logger.warning("statsmodels not available - autocorrelation features will be limited")


class TimeSeriesFeatures:
    """
    Extract time-series features from wallet performance data.

    Features:
    - RSI (Relative Strength Index)
    - MACD (Moving Average Convergence Divergence)
    - Bollinger Bands
    - Autocorrelation
    - Lagged returns
    - Fourier transform for cyclical patterns
    """

    def __init__(
        self,
        min_samples: int = 5,
        max_samples: int = 100,
        default_window: int = 14
    ):
        """
        Initialize the time-series feature extractor.

        Args:
            min_samples: Minimum samples required for feature extraction
            max_samples: Maximum samples to use (memory limit)
            default_window: Default window for rolling calculations
        """
        self.min_samples = min_samples
        self.max_samples = max_samples
        self.default_window = default_window

    def extract_features(
        self,
        performance_history: List[Dict[str, Any]],
        feature_set: str = "all"
    ) -> Dict[str, Any]:
        """
        Extract time-series features from performance history.

        Args:
            performance_history: List of performance records with timestamps and pnl
            feature_set: Which features to extract ("all", "momentum", "trend", "cycles")

        Returns:
            Dictionary of extracted features
        """
        if len(performance_history) < self.min_samples:
            logger.warning(
                f"Insufficient history for time-series features: "
                f"{len(performance_history)} < {self.min_samples}"
            )
            return self._empty_features()

        # Sort by timestamp and extract PnL series
        sorted_history = sorted(
            performance_history,
            key=lambda x: x.get('timestamp', '')
        )

        pnl_series = np.array([
            float(x.get('pnl_sol', x.get('pnl', 0.0)))
            for x in sorted_history
        ])

        roi_series = np.array([
            float(x.get('roi', 0.0))
            for x in sorted_history
        ])

        timestamps = [
            datetime.fromisoformat(x.get('timestamp', datetime.utcnow().isoformat()))
            for x in sorted_history
        ]

        features = {}

        try:
            # Momentum indicators
            if feature_set in ["all", "momentum"]:
                features.update(self._extract_momentum_features(pnl_series, roi_series))

            # Trend features
            if feature_set in ["all", "trend"]:
                features.update(self._extract_trend_features(pnl_series, timestamps))

            # Autocorrelation features
            if feature_set in ["all", "trend"]:
                features.update(self._extract_autocorr_features(pnl_series))

            # Cyclical features
            if feature_set in ["all", "cycles"]:
                features.update(self._extract_cycle_features(pnl_series))

            # Lagged features
            if feature_set in ["all", "trend"]:
                features.update(self._extract_lagged_features(pnl_series))

            # Volatility features
            if feature_set in ["all", "volatility"]:
                features.update(self._extract_volatility_features(pnl_series))

            features['extraction_success'] = True
            features['sample_count'] = len(pnl_series)

        except Exception as e:
            logger.error(f"Time-series feature extraction failed: {e}")
            return self._empty_features()

        return features

    def _empty_features(self) -> Dict[str, Any]:
        """Return empty features dict with null values."""
        return {
            'extraction_success': False,
            'sample_count': 0,
        }

    def _extract_momentum_features(
        self,
        pnl_series: np.ndarray,
        roi_series: np.ndarray
    ) -> Dict[str, float]:
        """Extract momentum indicators (RSI, MACD, Bollinger Bands)."""
        features = {}

        # RSI (Relative Strength Index)
        rsi = self._calculate_rsi(roi_series, window=self.default_window)
        features['rsi'] = float(rsi) if not np.isnan(rsi) else 50.0
        features['rsi_overbought'] = float(rsi > 70) if not np.isnan(rsi) else 0.0
        features['rsi_oversold'] = float(rsi < 30) if not np.isnan(rsi) else 0.0

        # MACD (Moving Average Convergence Divergence)
        macd_line, macd_signal, macd_hist = self._calculate_macd(roi_series)
        features['macd_line'] = float(macd_line) if not np.isnan(macd_line) else 0.0
        features['macd_signal'] = float(macd_signal) if not np.isnan(macd_signal) else 0.0
        features['macd_histogram'] = float(macd_hist) if not np.isnan(macd_hist) else 0.0
        features['macd_bullish'] = float(macd_hist > 0) if not np.isnan(macd_hist) else 0.0

        # Bollinger Bands
        bb_upper, bb_middle, bb_lower, bb_width = self._calculate_bollinger_bands(
            roi_series, window=self.default_window
        )
        features['bb_upper'] = float(bb_upper) if not np.isnan(bb_upper) else 0.0
        features['bb_middle'] = float(bb_middle) if not np.isnan(bb_middle) else 0.0
        features['bb_lower'] = float(bb_lower) if not np.isnan(bb_lower) else 0.0
        features['bb_width'] = float(bb_width) if not np.isnan(bb_width) else 0.0
        features['bb_position'] = self._calculate_bb_position(
            roi_series[-1] if len(roi_series) > 0 else 0.0,
            bb_upper, bb_lower
        )

        # Momentum score
        features['momentum_score'] = self._calculate_momentum_score(roi_series)

        # Rate of change
        features['roc_3'] = self._calculate_roc(roi_series, 3)
        features['roc_7'] = self._calculate_roc(roi_series, 7)
        features['roc_14'] = self._calculate_roc(roi_series, 14)

        return features

    def _extract_trend_features(
        self,
        pnl_series: np.ndarray,
        timestamps: List[datetime]
    ) -> Dict[str, float]:
        """Extract trend and direction features."""
        features = {}

        if len(pnl_series) < 2:
            return features

        # Linear trend (slope)
        x = np.arange(len(pnl_series))
        slope, intercept = np.polyfit(x, pnl_series, 1)
        features['trend_slope'] = float(slope)
        features['trend_intercept'] = float(intercept)

        # Trend direction
        features['trend_up'] = float(slope > 0)
        features['trend_strength'] = float(abs(slope) / (np.std(pnl_series) + 1e-8))

        # Moving averages
        features['ma_3'] = float(np.mean(pnl_series[-3:])) if len(pnl_series) >= 3 else float(pnl_series[-1])
        features['ma_7'] = float(np.mean(pnl_series[-7:])) if len(pnl_series) >= 7 else features['ma_3']
        features['ma_14'] = float(np.mean(pnl_series[-14:])) if len(pnl_series) >= 14 else features['ma_7']

        # Price relative to moving averages
        current = pnl_series[-1]
        features['above_ma_3'] = float(current > features['ma_3']) if len(pnl_series) >= 3 else 0.5
        features['above_ma_7'] = float(current > features['ma_7']) if len(pnl_series) >= 7 else 0.5
        features['above_ma_14'] = float(current > features['ma_14']) if len(pnl_series) >= 14 else 0.5

        # Acceleration (second derivative)
        if len(pnl_series) >= 3:
            acceleration = pnl_series[-1] - 2 * pnl_series[-2] + pnl_series[-3]
            features['acceleration'] = float(acceleration)
            features['accelerating'] = float(acceleration > 0)

        return features

    def _extract_autocorr_features(
        self,
        pnl_series: np.ndarray
    ) -> Dict[str, float]:
        """Extract autocorrelation features for persistence analysis."""
        features = {}

        if len(pnl_series) < 3:
            return features

        try:
            # Calculate autocorrelation at different lags
            max_lag = min(5, len(pnl_series) // 2)

            if STATSMODELS_AVAILABLE:
                # Use statsmodels for efficient calculation
                autocorrs = acf(pnl_series, nlags=max_lag, fft=True)
            else:
                # Manual calculation
                autocorrs = [
                    self._calculate_autocorr(pnl_series, lag)
                    for lag in range(max_lag + 1)
                ]

            # Autocorrelation at different lags
            for lag in range(1, min(4, len(autocorrs))):
                features[f'autocorr_lag_{lag}'] = float(autocorrs[lag]) if lag < len(autocorrs) else 0.0

            # Persistence indicator (positive autocorrelation at lag 1)
            features['persistence'] = float(autocorrs[1] > 0) if len(autocorrs) > 1 else 0.0

            # Mean reversion indicator (negative autocorrelation)
            features['mean_reverting'] = float(autocorrs[1] < 0) if len(autocorrs) > 1 else 0.0

            # Autocorrelation decay
            if len(autocorrs) > 2:
                decay_rate = (autocorrs[1] - autocorrs[min(3, len(autocorrs)-1)]) / max(1e-8, abs(autocorrs[1]))
                features['autocorr_decay'] = float(decay_rate)

        except Exception as e:
            logger.warning(f"Autocorrelation calculation failed: {e}")

        return features

    def _extract_cycle_features(
        self,
        pnl_series: np.ndarray
    ) -> Dict[str, float]:
        """Extract cyclical pattern features using Fourier transform."""
        features = {}

        if len(pnl_series) < 4:
            return features

        try:
            if SCIPY_AVAILABLE:
                # Fourier transform
                fft_vals = fft(pnl_series)
                fft_freq = fftfreq(len(pnl_series))

                # Dominant frequency
                positive_freqs = fft_freq[:len(fft_freq)//2]
                positive_vals = np.abs(fft_vals[:len(fft_vals)//2])

                if len(positive_vals) > 0:
                    dominant_idx = np.argmax(positive_vals[1:]) + 1  # Skip DC component
                    features['dominant_frequency'] = float(positive_freqs[dominant_idx])
                    features['dominant_amplitude'] = float(positive_vals[dominant_idx])

                    # Cyclical strength (ratio of dominant to total power)
                    total_power = np.sum(positive_vals**2)
                    dominant_power = positive_vals[dominant_idx]**2
                    features['cyclical_strength'] = float(
                        dominant_power / (total_power + 1e-8)
                    )

        except Exception as e:
            logger.warning(f"Cycle feature extraction failed: {e}")

        return features

    def _extract_lagged_features(
        self,
        pnl_series: np.ndarray
    ) -> Dict[str, float]:
        """Extract lagged return features."""
        features = {}

        if len(pnl_series) < 2:
            return features

        # Lagged returns
        lags = [1, 2, 3, 5, 7]
        for lag in lags:
            if len(pnl_series) > lag:
                features[f'return_lag_{lag}'] = float(pnl_series[-1] - pnl_series[-(lag+1)])

        # Lagged relative returns
        if len(pnl_series) > 1:
            features['return_lag_1_pct'] = float(
                (pnl_series[-1] - pnl_series[-2]) / (abs(pnl_series[-2]) + 1e-8)
            )

        return features

    def _extract_volatility_features(
        self,
        pnl_series: np.ndarray
    ) -> Dict[str, float]:
        """Extract volatility and risk features."""
        features = {}

        if len(pnl_series) < 2:
            return features

        # Standard deviation
        features['volatility_std'] = float(np.std(pnl_series))

        # Rolling volatility
        window = min(self.default_window, len(pnl_series))
        if len(pnl_series) >= window:
            rolling_std = np.std(pnl_series[-window:])
            features['volatility_rolling'] = float(rolling_std)

        # Volatility regime
        if len(pnl_series) >= 10:
            first_half_std = np.std(pnl_series[:len(pnl_series)//2])
            second_half_std = np.std(pnl_series[len(pnl_series)//2:])
            features['volatility_increasing'] = float(second_half_std > first_half_std)
            features['volatility_ratio'] = float(
                second_half_std / (first_half_std + 1e-8)
            )

        return features

    def _calculate_rsi(
        self,
        series: np.ndarray,
        window: int = 14
    ) -> float:
        """Calculate RSI (Relative Strength Index)."""
        if len(series) < window + 1:
            return 50.0  # Neutral

        deltas = np.diff(series)

        gains = np.where(deltas > 0, deltas, 0)
        losses = np.where(deltas < 0, -deltas, 0)

        avg_gain = np.mean(gains[-window:])
        avg_loss = np.mean(losses[-window:])

        if avg_loss == 0:
            return 100.0

        rs = avg_gain / avg_loss
        rsi = 100 - (100 / (1 + rs))

        return rsi

    def _calculate_macd(
        self,
        series: np.ndarray,
        fast: int = 12,
        slow: int = 26,
        signal: int = 9
    ) -> Tuple[float, float, float]:
        """Calculate MACD (Moving Average Convergence Divergence)."""
        if len(series) < slow:
            return 0.0, 0.0, 0.0

        # Calculate EMAs
        def ema(data, period):
            alpha = 2 / (period + 1)
            ema_values = [data[0]]
            for value in data[1:]:
                ema_values.append(alpha * value + (1 - alpha) * ema_values[-1])
            return np.array(ema_values)

        ema_fast = ema(series, fast)
        ema_slow = ema(series, slow)

        macd_line = ema_fast[-1] - ema_slow[-1]

        # Signal line (EMA of MACD)
        if len(series) > slow + signal:
            # Need to recalculate full MACD line for signal
            macd_series = ema_fast - ema_slow
            ema_signal = ema(macd_series, signal)
            macd_signal = ema_signal[-1]
        else:
            macd_signal = macd_line

        macd_histogram = macd_line - macd_signal

        return macd_line, macd_signal, macd_histogram

    def _calculate_bollinger_bands(
        self,
        series: np.ndarray,
        window: int = 20,
        num_std: float = 2.0
    ) -> Tuple[float, float, float, float]:
        """Calculate Bollinger Bands."""
        if len(series) < window:
            current = series[-1] if len(series) > 0 else 0.0
            return current, current, current, 0.0

        rolling_mean = np.mean(series[-window:])
        rolling_std = np.std(series[-window:])

        upper = rolling_mean + num_std * rolling_std
        lower = rolling_mean - num_std * rolling_std
        width = upper - lower

        return upper, rolling_mean, lower, width

    def _calculate_bb_position(
        self,
        current: float,
        upper: float,
        lower: float
    ) -> float:
        """Calculate position within Bollinger Bands (0-1)."""
        if upper == lower:
            return 0.5

        return (current - lower) / (upper - lower)

    def _calculate_momentum_score(
        self,
        series: np.ndarray,
        window: int = 14
    ) -> float:
        """Calculate overall momentum score (-1 to 1)."""
        if len(series) < 2:
            return 0.0

        # Recent momentum
        if len(series) >= window:
            recent_change = series[-1] - np.mean(series[-window:])
        else:
            recent_change = series[-1] - series[0]

        # Normalize by volatility
        volatility = np.std(series) + 1e-8
        momentum = recent_change / volatility

        # Clamp to [-1, 1]
        return max(-1.0, min(1.0, momentum))

    def _calculate_roc(
        self,
        series: np.ndarray,
        period: int
    ) -> float:
        """Calculate Rate of Change."""
        if len(series) < period + 1:
            return 0.0

        current = series[-1]
        past = series[-(period + 1)]

        if past == 0:
            return 0.0

        return (current - past) / abs(past)

    def _calculate_autocorr(
        self,
        series: np.ndarray,
        lag: int
    ) -> float:
        """Calculate autocorrelation at specific lag."""
        if len(series) < lag + 1:
            return 0.0

        n = len(series)
        mean = np.mean(series)
        std = np.std(series)

        if std == 0:
            return 0.0

        # Calculate correlation
        sum1 = sum((series[i] - mean) * (series[i + lag] - mean) for i in range(n - lag))
        sum2 = sum((x - mean) ** 2 for x in series)

        if sum2 == 0:
            return 0.0

        return sum1 / sum2

    def extract_from_wallet_trades(
        self,
        trades: List[Dict[str, Any]],
        feature_set: str = "all"
    ) -> Dict[str, Any]:
        """
        Extract time-series features from wallet trade history.

        Args:
            trades: List of trade records with pnl and timestamp
            feature_set: Which features to extract

        Returns:
            Dictionary of time-series features
        """
        # Convert trades to performance history format
        performance_history = []

        for trade in trades:
            performance_history.append({
                'pnl_sol': trade.get('pnl_sol', trade.get('pnl', 0.0)),
                'roi': trade.get('roi', 0.0),
                'timestamp': trade.get('timestamp', datetime.utcnow().isoformat()),
            })

        return self.extract_features(performance_history, feature_set)


# Convenience function
def extract_time_series_features(
    performance_history: List[Dict[str, Any]],
    feature_set: str = "all"
) -> Dict[str, Any]:
    """
    Quick extraction of time-series features.

    Args:
        performance_history: List of performance records
        feature_set: Which features to extract

    Returns:
        Dictionary of extracted features
    """
    extractor = TimeSeriesFeatures()
    return extractor.extract_features(performance_history, feature_set)
