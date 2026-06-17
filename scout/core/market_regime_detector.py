"""
Market Regime Detection for Solana Trading Strategy Adjustment

This module implements market regime detection to classify current market conditions
and adjust trading strategies accordingly.

Regime Classification:
- BULL: Strong upward momentum (follow signals aggressively)
- BEAR: Downward momentum (reduce exposure, tighten stops)
- VOLATILE: High volatility (reduce position sizes, wider stops)
- NEUTRAL: Sideways (standard parameters)

Detection Features:
- SOL price momentum and trend analysis
- Volume pattern analysis
- Volatility measurement
- Market breadth calculation
- Network activity metrics
- Cross-chain correlation analysis

Features:
- Real-time regime classification
- Multi-feature ensemble detection
- Regime transition probability calculation
- Strategy recommendation based on regime
- Historical regime performance tracking
"""

import os
import time
import logging
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Tuple, Any
from dataclasses import dataclass, field
from enum import Enum
import threading
import json
from pathlib import Path

logger = logging.getLogger(__name__)


class MarketRegime(Enum):
    """Market regime types."""
    BULL = "bull"          # Strong upward momentum
    BEAR = "bear"          # Downward momentum
    VOLATILE = "volatile"  # High volatility
    NEUTRAL = "neutral"    # Sideways/ranging


class RegimeTransition(Enum):
    """Types of regime transitions."""
    NONE = "none"                    # No transition
    BULL_TO_BEAR = "bull_to_bear"    # Trend reversal
    BULL_TO_VOLATILE = "bull_to_volatile"
    BEAR_TO_BULL = "bear_to_bull"    # Recovery
    BEAR_TO_VOLATILE = "bear_to_volatile"
    VOLATILE_TO_BULL = "volatile_to_bull"
    VOLATILE_TO_BEAR = "volatile_to_bear"
    TO_NEUTRAL = "to_neutral"        # Uncertain conditions


@dataclass
class RegimeFeatures:
    """Features for regime classification."""
    sol_price_momentum: float        # SOL price change over lookback
    sol_volatility: float             # Price volatility
    volume_ratio: float              # Current vs average volume
    market_breadth: float             # % of assets in uptrend
    network_tps: float                # Network transactions per second
    correlation_strength: float       # Cross-chain correlation

    @classmethod
    def empty(cls) -> 'RegimeFeatures':
        """Create empty features."""
        return cls(
            sol_price_momentum=0.0,
            sol_volatility=0.0,
            volume_ratio=1.0,
            market_breadth=0.5,
            network_tps=0.0,
            correlation_strength=0.0,
        )


@dataclass
class RegimeClassification:
    """Market regime classification result."""
    regime: MarketRegime
    confidence: float                 # 0-1 confidence score
    probabilities: Dict[MarketRegime, float]  # Probability for each regime
    features: RegimeFeatures
    transition: RegimeTransition
    timestamp: float = field(default_factory=time.time)
    reason: str = ""


@dataclass
class RegimeConfig:
    """Configuration for regime detection."""

    # Detection parameters
    PRICE_LOOKBACK_SHORT: int = 24    # 1 day for short-term momentum
    PRICE_LOOKBACK_LONG: int = 168    # 1 week for long-term trend
    VOLATILITY_WINDOW: int = 24       # 1 day for volatility calculation

    # Regime thresholds
    BULL_MOMENTUM_THRESHOLD: float = 0.05    # 5% positive momentum
    BEAR_MOMENTUM_THRESHOLD: float = -0.05   # 5% negative momentum
    VOLATILITY_THRESHOLD: float = 0.10       # 10% daily volatility
    HIGH_VOLATILITY_THRESHOLD: float = 0.15   # 15% volatility

    # Feature weights for classification
    WEIGHT_MOMENTUM: float = 0.35
    WEIGHT_VOLATILITY: float = 0.25
    WEIGHT_VOLUME: float = 0.15
    WEIGHT_BREADTH: float = 0.15
    WEIGHT_TPS: float = 0.10

    # Transition confidence threshold
    TRANSITION_CONFIDENCE: float = 0.7  # 70% confidence for regime change


class MarketRegimeDetector:
    """
    Market regime detector for Solana trading strategy adjustment.

    Implements:
    - Real-time regime classification
    - Multi-feature ensemble detection
    - Regime transition tracking
    - Strategy recommendation based on regime
    """

    def __init__(self, config: Optional[RegimeConfig] = None):
        """Initialize the regime detector."""
        self._config = config or RegimeConfig()
        self._current_regime = MarketRegime.NEUTRAL
        self._regime_history: List[Tuple[float, MarketRegime]] = []
        self._lock = threading.Lock()

        # Price history for momentum/volatility calculation
        self._price_history: List[Tuple[float, float]] = []  # (timestamp, price)

        # Volume history
        self._volume_history: List[Tuple[float, float]] = []

        logger.info("Market Regime Detector initialized")
        logger.info(f"  Current regime: {self._current_regime.value}")

    def update_price(self, price: float, timestamp: Optional[float] = None):
        """Update SOL price history."""
        if timestamp is None:
            timestamp = time.time()

        self._price_history.append((timestamp, price))

        # Keep only recent history (1 week)
        cutoff = timestamp - (self._config.PRICE_LOOKBACK_LONG * 3600)
        self._price_history = [(t, p) for t, p in self._price_history if t > cutoff]

    def update_volume(self, volume: float, timestamp: Optional[float] = None):
        """Update volume history."""
        if timestamp is None:
            timestamp = time.time()

        self._volume_history.append((timestamp, volume))

        # Keep only recent history (1 day)
        cutoff = timestamp - (24 * 3600)
        self._volume_history = [(t, v) for t, v in self._volume_history if t > cutoff]

    def calculate_momentum(self, lookback_hours: int = 24) -> float:
        """Calculate SOL price momentum over lookback period."""
        if not self._price_history:
            return 0.0

        now = time.time()
        cutoff = now - (lookback_hours * 3600)

        recent_prices = [(t, p) for t, p in self._price_history if t > cutoff]
        if len(recent_prices) < 2:
            return 0.0

        # Get oldest and newest prices
        oldest = recent_prices[0][1]
        newest = recent_prices[-1][1]

        if oldest <= 0:
            return 0.0

        return (newest - oldest) / oldest

    def calculate_volatility(self) -> float:
        """Calculate price volatility (daily)."""
        if not self._price_history:
            return 0.0

        now = time.time()
        cutoff = now - (self._config.VOLATILITY_WINDOW * 3600)
        recent_prices = [(t, p) for t, p in self._price_history if t > cutoff]

        if len(recent_prices) < 2:
            return 0.0

        # Calculate returns
        returns = []
        for i in range(1, len(recent_prices)):
            if recent_prices[i-1][1] > 0:
                ret = (recent_prices[i][1] - recent_prices[i-1][1]) / recent_prices[i-1][1]
                returns.append(ret)

        if not returns:
            return 0.0

        # Calculate standard deviation
        avg_return = sum(returns) / len(returns)
        variance = sum((r - avg_return) ** 2 for r in returns) / len(returns)

        return (variance ** 0.5) * (24 ** 0.5)  # Annualize to daily

    def calculate_volume_ratio(self) -> float:
        """Calculate current volume vs average volume ratio."""
        if not self._volume_history:
            return 1.0

        recent_volume = self._volume_history[-1][1]
        avg_volume = sum(v for _, v in self._volume_history) / len(self._volume_history)

        if avg_volume <= 0:
            return 1.0

        return recent_volume / avg_volume

    def extract_features(self) -> RegimeFeatures:
        """Extract features for regime classification."""
        return RegimeFeatures(
            sol_price_momentum=self.calculate_momentum(self._config.PRICE_LOOKBACK_SHORT),
            sol_volatility=self.calculate_volatility(),
            volume_ratio=self.calculate_volume_ratio(),
            market_breadth=0.5,  # Placeholder - would need external data
            network_tps=0.0,     # Placeholder - would need RPC data
            correlation_strength=0.0,  # Placeholder - would need cross-chain data
        )

    def classify_regime(self, features: RegimeFeatures) -> RegimeClassification:
        """
        Classify market regime based on features.

        Args:
            features: RegimeFeatures for classification

        Returns:
            RegimeClassification with regime and confidence
        """
        # Calculate scores for each regime
        scores = {
            MarketRegime.BULL: 0.0,
            MarketRegime.BEAR: 0.0,
            MarketRegime.VOLATILE: 0.0,
            MarketRegime.NEUTRAL: 0.0,
        }

        # Momentum score
        if features.sol_price_momentum > self._config.BULL_MOMENTUM_THRESHOLD:
            scores[MarketRegime.BULL] += self._config.WEIGHT_MOMENTUM
            scores[MarketRegime.NEUTRAL] += self._config.WEIGHT_MOMENTUM * 0.5
        elif features.sol_price_momentum < self._config.BEAR_MOMENTUM_THRESHOLD:
            scores[MarketRegime.BEAR] += self._config.WEIGHT_MOMENTUM
            scores[MarketRegime.NEUTRAL] += self._config.WEIGHT_MOMENTUM * 0.5
        else:
            scores[MarketRegime.NEUTRAL] += self._config.WEIGHT_MOMENTUM

        # Volatility score
        if features.sol_volatility > self._config.HIGH_VOLATILITY_THRESHOLD:
            scores[MarketRegime.VOLATILE] += self._config.WEIGHT_VOLATILITY
        elif features.sol_volatility < self._config.VOLATILITY_THRESHOLD:
            # Low volatility - add to directional scores
            if features.sol_price_momentum > 0:
                scores[MarketRegime.BULL] += self._config.WEIGHT_VOLATILITY * 0.5
            else:
                scores[MarketRegime.BEAR] += self._config.WEIGHT_VOLATILITY * 0.5
        else:
            scores[MarketRegime.NEUTRAL] += self._config.WEIGHT_VOLATILITY

        # Volume score
        if features.volume_ratio > 1.5:
            scores[MarketRegime.VOLATILE] += self._config.WEIGHT_VOLUME
        elif features.volume_ratio < 0.5:
            scores[MarketRegime.BEAR] += self._config.WEIGHT_VOLUME
        else:
            scores[MarketRegime.NEUTRAL] += self._config.WEIGHT_VOLUME

        # Normalize scores
        total_score = sum(scores.values())
        if total_score > 0:
            probabilities = {regime: score / total_score for regime, score in scores.items()}
        else:
            probabilities = {regime: 0.25 for regime in MarketRegime}

        # Determine winning regime
        winning_regime = max(probabilities.keys(), key=lambda k: probabilities[k])
        confidence = probabilities[winning_regime]

        # Calculate transition
        transition = self._calculate_transition(winning_regime, confidence)

        # Determine reason
        reason = self._generate_reason(winning_regime, features, confidence)

        return RegimeClassification(
            regime=winning_regime,
            confidence=confidence,
            probabilities=probabilities,
            features=features,
            transition=transition,
            reason=reason,
        )

    def _calculate_transition(self, new_regime: MarketRegime,
                              confidence: float) -> RegimeTransition:
        """Calculate regime transition type."""
        if confidence < self._config.TRANSITION_CONFIDENCE:
            return RegimeTransition.NONE

        if self._current_regime == new_regime:
            return RegimeTransition.NONE

        transitions = {
            MarketRegime.BULL: {
                MarketRegime.BEAR: RegimeTransition.BULL_TO_BEAR,
                MarketRegime.VOLATILE: RegimeTransition.BULL_TO_VOLATILE,
                MarketRegime.NEUTRAL: RegimeTransition.TO_NEUTRAL,
            },
            MarketRegime.BEAR: {
                MarketRegime.BULL: RegimeTransition.BEAR_TO_BULL,
                MarketRegime.VOLATILE: RegimeTransition.BEAR_TO_VOLATILE,
                MarketRegime.NEUTRAL: RegimeTransition.TO_NEUTRAL,
            },
            MarketRegime.VOLATILE: {
                MarketRegime.BULL: RegimeTransition.VOLATILE_TO_BULL,
                MarketRegime.BEAR: RegimeTransition.VOLATILE_TO_BEAR,
                MarketRegime.NEUTRAL: RegimeTransition.TO_NEUTRAL,
            },
            MarketRegime.NEUTRAL: {
                MarketRegime.BULL: RegimeTransition.TO_NEUTRAL,
                MarketRegime.BEAR: RegimeTransition.TO_NEUTRAL,
                MarketRegime.VOLATILE: RegimeTransition.TO_NEUTRAL,
            },
        }

        return transitions.get(self._current_regime, {}).get(new_regime, RegimeTransition.NONE)

    def _generate_reason(self, regime: MarketRegime, features: RegimeFeatures,
                        confidence: float) -> str:
        """Generate human-readable reason for classification."""
        parts = []

        if features.sol_price_momentum > self._config.BULL_MOMENTUM_THRESHOLD:
            parts.append("Strong upward momentum")
        elif features.sol_price_momentum < self._config.BEAR_MOMENTUM_THRESHOLD:
            parts.append("Downward momentum")
        else:
            parts.append("Sideways price action")

        if features.sol_volatility > self._config.HIGH_VOLATILITY_THRESHOLD:
            parts.append("High volatility")
        elif features.sol_volatility < self._config.VOLATILITY_THRESHOLD:
            parts.append("Low volatility")

        if features.volume_ratio > 1.5:
            parts.append("Elevated volume")
        elif features.volume_ratio < 0.5:
            parts.append("Low volume")

        return ", ".join(parts) if parts else "Neutral market conditions"

    def detect_regime(self, sol_price: Optional[float] = None,
                     volume: Optional[float] = None) -> RegimeClassification:
        """
        Detect current market regime.

        Args:
            sol_price: Current SOL price (optional)
            volume: Current volume (optional)

        Returns:
            RegimeClassification with detected regime
        """
        with self._lock:
            # Update price/volume if provided
            if sol_price:
                self.update_price(sol_price)
            if volume:
                self.update_volume(volume)

            # Extract features
            features = self.extract_features()

            # Classify regime
            classification = self.classify_regime(features)

            # Update current regime if confidence is high enough
            if classification.confidence >= self._config.TRANSITION_CONFIDENCE:
                old_regime = self._current_regime
                self._current_regime = classification.regime

                if old_regime != self._current_regime:
                    logger.info(f"Regime changed: {old_regime.value} → {self._current_regime.value}")
                    logger.info(f"  Confidence: {classification.confidence*100:.0f}%")
                    logger.info(f"  Reason: {classification.reason}")

            # Record in history
            self._regime_history.append((classification.timestamp, classification.regime))

            # Keep only recent history (7 days)
            cutoff = time.time() - (7 * 24 * 3600)
            self._regime_history = [(t, r) for t, r in self._regime_history if t > cutoff]

            return classification

    def get_current_regime(self) -> MarketRegime:
        """Get current market regime."""
        with self._lock:
            return self._current_regime

    def get_regime_summary(self, classification: RegimeClassification) -> Dict[str, Any]:
        """Get summary of regime classification."""
        return {
            "regime": classification.regime.value,
            "confidence": classification.confidence * 100,
            "probabilities": {r.value: p * 100 for r, p in classification.probabilities.items()},
            "transition": classification.transition.value,
            "reason": classification.reason,
            "features": {
                "momentum": classification.features.sol_price_momentum * 100,
                "volatility": classification.features.sol_volatility * 100,
                "volume_ratio": classification.features.volume_ratio,
            }
        }

    def print_regime_report(self, classification: RegimeClassification):
        """Print comprehensive regime report."""
        summary = self.get_regime_summary(classification)

        print("\n" + "="*70)
        print("MARKET REGIME DETECTION REPORT")
        print("="*70)

        print(f"\nDetected Regime: {summary['regime'].upper()}")
        print(f"Confidence: {summary['confidence']:.0f}%")
        print(f"Transition: {summary['transition']}")
        print(f"Reason: {summary['reason']}")

        print(f"\nProbabilities:")
        for regime, prob in summary['probabilities'].items():
            print(f"  {regime.capitalize()}: {prob:.0f}%")

        print(f"\nFeatures:")
        print(f"  SOL Momentum: {summary['features']['momentum']:+.1f}%")
        print(f"  Volatility: {summary['features']['volatility']:.1f}%")
        print(f"  Volume Ratio: {summary['features']['volume_ratio']:.2f}x")

        print("="*70 + "\n")


# Global singleton instance
_detector: Optional[MarketRegimeDetector] = None
_detector_lock = threading.Lock()


def get_market_regime_detector() -> MarketRegimeDetector:
    """Get the global market regime detector singleton."""
    global _detector

    with _detector_lock:
        if _detector is None:
            _detector = MarketRegimeDetector()

    return _detector


if __name__ == "__main__":
    # Test the market regime detector
    detector = get_market_regime_detector()

    # Simulate some price data
    print("Testing market regime detection:")

    # Bull market scenario
    for i in range(10):
        price = 100.0 + (i * 2.0)  # Rising price
        detector.update_price(price)

    classification = detector.detect_regime(sol_price=120.0)
    detector.print_regime_report(classification)

    # Volatile scenario
    detector = MarketRegimeDetector()
    for i in range(10):
        price = 100.0 + (i * 5.0 if i % 2 == 0 else -i * 5.0)  # Volatile
        detector.update_price(price)

    classification = detector.detect_regime(sol_price=100.0)
    detector.print_regime_report(classification)
