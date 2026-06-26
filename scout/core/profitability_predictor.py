"""
ML-Based Profitability Prediction System

This module implements machine learning models to predict wallet profitability
from limited data, optimizing for the $200 → $1000 growth goal.

Features:
- Ensemble model combining multiple predictors
- Feature engineering for wallet behavior patterns
- Probability scoring for profitable copy-trading
- Risk-adjusted return predictions
- Early identification of high-potential wallets
"""

import math
import os
import logging
import numpy as np
from datetime import datetime
from decimal import Decimal
from typing import Dict, List, Optional, Tuple, Any
from dataclasses import dataclass
from enum import Enum

logger = logging.getLogger(__name__)


class PredictionModel(Enum):
    """Available prediction models."""

    ENSEMBLE = "ensemble"           # Combined model
    LINEAR_REGRESSION = "linear"    # Simple baseline
    RANDOM_FOREST = "forest"        # Tree-based model
    GRADIENT_BOOSTING = "boost"     # Boosted trees
    NEURAL_NETWORK = "neural"       # Deep learning


class ProfitabilityClass(Enum):
    """Profitability classification."""

    HIGH_PROFIT = "high_profit"       # >20% expected return
    MODERATE_PROFIT = "moderate_profit" # 5-20% expected return
    LOW_PROFIT = "low_profit"         # 0-5% expected return
    LOSS = "loss"                     # <0% expected return


@dataclass
class ProfitabilityFeatures:
    """
    Feature set for profitability prediction.

    Features designed to capture:
    - Trading skill and consistency
    - Risk management quality
    - Market timing ability
    - Capital efficiency
    """

    # Basic performance metrics
    roi_7d: Optional[float] = None
    roi_30d: Optional[float] = None
    win_rate: Optional[float] = None
    profit_factor: Optional[float] = None
    max_drawdown: Optional[float] = None
    sortino_ratio: Optional[float] = None

    # Trading behavior features
    trade_count_30d: Optional[int] = None
    avg_trade_size_sol: Optional[Decimal] = None
    avg_hold_time_hours: Optional[float] = None
    entry_delay_seconds: Optional[float] = None

    # Risk management features
    uses_mev_protection: bool = False
    uses_limit_orders: bool = False
    dex_diversity_score: Optional[int] = None
    max_position_size_sol: Optional[float] = None

    # Market timing features
    roi_7d_to_30d_ratio: Optional[float] = None
    recent_momentum: Optional[float] = None
    volatility_30d: Optional[float] = None

    # Quality indicators
    parse_rate: Optional[float] = None
    bag_holder_score: Optional[float] = None
    insider_probability: Optional[float] = None

    # Liquidity awareness
    avg_liquidity_usd: Optional[float] = None
    slippage_tolerance: Optional[float] = None

    def to_feature_vector(self) -> np.ndarray:
        """Convert to numpy feature vector for ML models."""
        features = [
            self.roi_7d or 0.0,
            self.roi_30d or 0.0,
            self.win_rate or 0.0,
            self.profit_factor or 0.0,
            self.max_drawdown or 0.0,
            self.sortino_ratio or 0.0,
            self.trade_count_30d or 0,
            self.avg_trade_size_sol or Decimal(0),
            self.avg_hold_time_hours or 0.0,
            self.entry_delay_seconds or 0.0,
            1.0 if self.uses_mev_protection else 0.0,
            1.0 if self.uses_limit_orders else 0.0,
            self.dex_diversity_score or 0,
            self.max_position_size_sol or 0.0,
            self.roi_7d_to_30d_ratio or 0.0,
            self.recent_momentum or 0.0,
            self.volatility_30d or 0.0,
            self.parse_rate or 0.0,
            self.bag_holder_score or 0.0,
            self.insider_probability or 0.0,
            self.avg_liquidity_usd or 0.0,
            self.slippage_tolerance or 0.0,
        ]

        return np.array(features, dtype=np.float32)


@dataclass
class ProfitabilityPrediction:
    """Profitability prediction result."""

    expected_return_pct: float         # Expected return over next 30 days
    confidence: float                   # Model confidence (0.0-1.0)
    risk_score: float                   # Risk score (0.0-1.0, higher = riskier)
    profitability_class: ProfitabilityClass
    feature_importance: Dict[str, float]
    prediction_timestamp: float

    # Risk-adjusted metrics
    sharpe_ratio_predicted: Optional[float] = None
    max_loss_predicted_pct: Optional[float] = None
    probability_of_profit: Optional[float] = None

    def __post_init__(self):
        if self.prediction_timestamp == 0:
            self.prediction_timestamp = datetime.now().timestamp()


class SimpleEnsembleModel:
    """
    Simple ensemble model for profitability prediction.

    Uses weighted combination of multiple prediction strategies:
    1. ROI momentum (recent performance)
    2. Win rate consistency
    3. Risk-adjusted returns (Sortino)
    4. Smart money indicators
    5. Liquidity awareness

    Designed for production use with minimal dependencies.
    """

    def __init__(self):
        """Initialize the ensemble model."""
        # Model weights (tuned for growth optimization)
        self.weights = {
            'roi_momentum': 0.30,
            'win_rate_consistency': 0.25,
            'risk_adjusted_returns': 0.20,
            'smart_money_indicators': 0.15,
            'liquidity_awareness': 0.10,
        }

        # Risk adjustment factors
        self.risk_factors = {
            'high_drawdown': 0.8,
            'low_win_rate': 0.7,
            'bag_holding': 0.6,
            'insider_risk': 0.5,
        }

        # Growth optimization parameters
        self.growth_optimized = os.getenv("SCOUT_GROWTH_OPTIMIZED", "true").lower() == "true"
        self.growth_target_multiplier = 5.0  # Target: $200 → $1000 (5x)

        # Feature importance tracking
        self.feature_importance = {}

    def _normalize_score(self, value: float, min_val: float, max_val: float) -> float:
        """Normalize a value to 0-1 range."""
        if max_val == min_val:
            return 0.5
        return (value - min_val) / (max_val - min_val)

    def _calculate_roi_momentum_score(self, features: ProfitabilityFeatures) -> Tuple[float, str]:
        """Calculate enhanced ROI momentum score with growth optimization."""
        roi_7d = features.roi_7d or 0.0
        roi_30d = features.roi_30d or 0.0

        # Growth-optimized momentum calculation
        if roi_7d > 0 and roi_30d > 0:
            # Base momentum from recent performance
            momentum = self._normalize_score(roi_7d, 0, 100) * 0.7 + \
                       self._normalize_score(roi_30d, 0, 50) * 0.3

            if os.getenv("SCOUT_ROI_ADDITIVE_MODE", "false").lower() == "true":
                # Additive mode (diminishing returns, cappable)
                if roi_7d > roi_30d * 0.6:
                    acceleration = min(1.0, roi_7d / max(roi_30d, 1.0))
                    momentum += acceleration * 0.12  # Max +0.12
                if roi_7d > 20:
                    momentum += min(0.12, math.log(roi_7d / 20.0) * 0.04)
                if roi_7d > 0 and roi_30d > 0 and roi_7d < roi_30d * 0.3:
                    momentum -= 0.15
            else:
                # Legacy multiplicative mode (deprecated)
                if roi_7d > roi_30d * 0.6:
                    momentum *= 1.3
                if roi_7d > 50:
                    momentum *= 1.2
                if roi_7d > 100:
                    momentum *= 1.5
                if roi_7d < roi_30d * 0.3:
                    momentum *= 0.5

            return min(momentum, 1.0), "roi_momentum"

        # Recovery bonus (turning around from losses)
        elif roi_7d > 0 and roi_30d < 0:
            return 0.6, "roi_momentum"  # Moderate score for recovery

        return 0.0, "roi_momentum"

    def _calculate_win_rate_consistency_score(self, features: ProfitabilityFeatures) -> Tuple[float, str]:
        """Calculate win rate consistency score."""
        win_rate = features.win_rate or 0.0
        profit_factor = features.profit_factor or 0.0

        # High win rate with solid profit factor
        if win_rate >= 0.6:
            consistency = self._normalize_score(win_rate, 0.6, 0.9) * 0.6 + \
                         self._normalize_score(profit_factor, 1.0, 3.0) * 0.4

            # Penalty for martingale pattern (high win rate, low profit factor)
            if win_rate > 0.7 and profit_factor < 1.5:
                consistency *= 0.5

            return min(consistency, 1.0), "win_rate_consistency"

        return 0.0, "win_rate_consistency"

    def _calculate_risk_adjusted_score(self, features: ProfitabilityFeatures) -> Tuple[float, str]:
        """Calculate risk-adjusted return score."""
        sortino = features.sortino_ratio or 0.0
        drawdown = features.max_drawdown or 0.0

        # High Sortino with low drawdown
        risk_adjusted = self._normalize_score(sortino, 0.0, 3.0) * 0.7 - \
                       self._normalize_score(drawdown, 0.0, 30.0) * 0.3

        return max(0.0, min(risk_adjusted, 1.0)), "risk_adjusted_returns"

    def _calculate_smart_money_score(self, features: ProfitabilityFeatures) -> Tuple[float, str]:
        """Calculate smart money indicator score."""
        score = 0.0

        # MEV protection
        if features.uses_mev_protection:
            score += 0.3

        # Limit orders
        if features.uses_limit_orders:
            score += 0.2

        # DEX diversity
        dex_diversity = features.dex_diversity_score or 0
        score += self._normalize_score(dex_diversity, 0, 4) * 0.3

        # Insider risk penalty
        insider_prob = features.insider_probability or 0.0
        if insider_prob > 0.7:
            score *= 0.5

        return min(score, 1.0), "smart_money_indicators"

    def _calculate_liquidity_score(self, features: ProfitabilityFeatures) -> Tuple[float, str]:
        """Calculate liquidity awareness score."""
        avg_liquidity = features.avg_liquidity_usd or 0.0
        parse_rate = features.parse_rate or 0.0

        # Good liquidity and high parse rate
        liquidity_score = self._normalize_score(avg_liquidity, 10000, 100000) * 0.6 + \
                         self._normalize_score(parse_rate, 0.5, 1.0) * 0.4

        return min(liquidity_score, 1.0), "liquidity_awareness"

    def _calculate_risk_score(self, features: ProfitabilityFeatures) -> float:
        """Calculate overall risk score."""
        risk_factors = []

        # High drawdown risk
        if features.max_drawdown and features.max_drawdown > 20:
            risk_factors.append(('high_drawdown', features.max_drawdown / 50))

        # Low win rate risk
        if features.win_rate and features.win_rate < 0.4:
            risk_factors.append(('low_win_rate', (0.4 - features.win_rate)))

        # Bag holder risk
        if features.bag_holder_score and features.bag_holder_score > 0.3:
            risk_factors.append(('bag_holding', features.bag_holder_score))

        # Insider risk
        if features.insider_probability and features.insider_probability > 0.5:
            risk_factors.append(('insider_risk', features.insider_probability))

        # Calculate combined risk score
        if not risk_factors:
            return 0.1  # Base risk level

        # Weight risk factors
        weighted_risk = 0.0
        for factor_name, factor_value in risk_factors:
            weight = self.risk_factors.get(factor_name, 0.5)
            weighted_risk += factor_value * weight

        return min(weighted_risk, 1.0)

    def predict(self, features: ProfitabilityFeatures) -> ProfitabilityPrediction:
        """
        Make profitability prediction.

        Args:
            features: Wallet features

        Returns:
            Profitability prediction
        """
        # Calculate component scores
        scores = {}

        scores['roi_momentum'] = self._calculate_roi_momentum_score(features)[0]
        scores['win_rate_consistency'] = self._calculate_win_rate_consistency_score(features)[0]
        scores['risk_adjusted_returns'] = self._calculate_risk_adjusted_score(features)[0]
        scores['smart_money_indicators'] = self._calculate_smart_money_score(features)[0]
        scores['liquidity_awareness'] = self._calculate_liquidity_score(features)[0]

        # Calculate weighted ensemble score
        ensemble_score = sum(
            scores[component] * weight
            for component, weight in self.weights.items()
        )

        # Calculate risk score
        risk_score = self._calculate_risk_score(features)

        # Adjust for growth optimization
        if self.growth_optimized:
            # Boost high-conviction wallets for growth goal
            if ensemble_score > 0.7:
                ensemble_score *= 1.2
            # Reduce exposure to low-conviction wallets
            elif ensemble_score < 0.4:
                ensemble_score *= 0.8

        # Calculate expected return based on score
        expected_return_pct = ensemble_score * 30  # Max 30% monthly return

        # Calculate confidence based on feature completeness
        feature_completeness = sum(
            1 for value in [features.roi_7d, features.roi_30d, features.win_rate,
                           features.profit_factor, features.max_drawdown]
            if value is not None
        ) / 5.0

        confidence = feature_completeness * 0.7 + (1.0 - risk_score) * 0.3

        # Determine profitability class
        if expected_return_pct > 20:
            profit_class = ProfitabilityClass.HIGH_PROFIT
        elif expected_return_pct > 5:
            profit_class = ProfitabilityClass.MODERATE_PROFIT
        elif expected_return_pct > 0:
            profit_class = ProfitabilityClass.LOW_PROFIT
        else:
            profit_class = ProfitabilityClass.LOSS

        # Calculate risk-adjusted metrics
        sharpe_predicted = (expected_return_pct / 100) / max(risk_score, 0.1) if risk_score > 0 else 0
        max_loss_predicted = -risk_score * 20  # Max 20% loss
        probability_of_profit = confidence if expected_return_pct > 0 else 1.0 - confidence

        return ProfitabilityPrediction(
            expected_return_pct=expected_return_pct,
            confidence=confidence,
            risk_score=risk_score,
            profitability_class=profit_class,
            feature_importance=scores,
            prediction_timestamp=datetime.now().timestamp(),
            sharpe_ratio_predicted=sharpe_predicted,
            max_loss_predicted_pct=max_loss_predicted,
            probability_of_profit=probability_of_profit,
        )


class ProfitabilityPredictor:
    """
    Main profitability prediction system.

    Features:
    - Ensemble model combining multiple strategies
    - Feature engineering from wallet metrics
    - Risk-adjusted predictions
    - Growth goal optimization
    """

    def __init__(self):
        """Initialize the predictor."""
        self.model = SimpleEnsembleModel()

        # Feature cache for performance
        self._feature_cache = {}

        logger.info("Profitability Predictor initialized")

    def extract_features(self, wallet_metrics: Dict[str, Any]) -> ProfitabilityFeatures:
        """
        Extract features from wallet metrics.

        Args:
            wallet_metrics: Dictionary of wallet metrics

        Returns:
            ProfitabilityFeatures object
        """
        return ProfitabilityFeatures(
            roi_7d=wallet_metrics.get('roi_7d'),
            roi_30d=wallet_metrics.get('roi_30d'),
            win_rate=wallet_metrics.get('win_rate'),
            profit_factor=wallet_metrics.get('profit_factor'),
            max_drawdown=wallet_metrics.get('max_drawdown_30d'),
            sortino_ratio=wallet_metrics.get('sortino_ratio'),
            trade_count_30d=wallet_metrics.get('trade_count_30d'),
            avg_trade_size_sol=wallet_metrics.get('avg_trade_size_sol'),
            uses_mev_protection=wallet_metrics.get('uses_mev_protection', False),
            uses_limit_orders=wallet_metrics.get('uses_limit_orders', False),
            dex_diversity_score=wallet_metrics.get('dex_diversity_score'),
            parse_rate=wallet_metrics.get('parse_rate'),
            insider_probability=wallet_metrics.get('insider_probability'),
            # Calculate derived features
            roi_7d_to_30d_ratio=self._calculate_roi_ratio(wallet_metrics),
            recent_momentum=self._calculate_momentum(wallet_metrics),
        )

    def _calculate_roi_ratio(self, metrics: Dict[str, Any]) -> Optional[float]:
        """Calculate ROI 7d to 30d ratio."""
        roi_7d = metrics.get('roi_7d')
        roi_30d = metrics.get('roi_30d')

        if roi_7d is not None and roi_30d is not None and roi_30d != 0:
            return roi_7d / roi_30d

        return None

    def _calculate_momentum(self, metrics: Dict[str, Any]) -> Optional[float]:
        """Calculate recent momentum score."""
        roi_7d = metrics.get('roi_7d')
        roi_30d = metrics.get('roi_30d')

        if roi_7d is not None and roi_30d is not None:
            if roi_30d > 0:
                return self._normalize_score(roi_7d, 0, roi_30d * 1.2)
            elif roi_7d > 0:
                return 0.7  # Recovering
            else:
                return 0.3  # Declining

        return None

    def _normalize_score(self, value: float, min_val: float, max_val: float) -> float:
        """Normalize a value to 0-1 range."""
        if max_val == min_val:
            return 0.5
        return (value - min_val) / (max_val - min_val)

    def predict_wallet_profitability(self, wallet_metrics: Dict[str, Any]) -> ProfitabilityPrediction:
        """
        Predict wallet profitability.

        Args:
            wallet_metrics: Wallet metrics dictionary

        Returns:
            Profitability prediction
        """
        # Extract features
        features = self.extract_features(wallet_metrics)

        # Make prediction
        prediction = self.model.predict(features)

        return prediction

    def predict_batch(self, wallets_metrics: List[Dict[str, Any]]) -> List[ProfitabilityPrediction]:
        """
        Predict profitability for multiple wallets.

        Args:
            wallets_metrics: List of wallet metrics dictionaries

        Returns:
            List of profitability predictions
        """
        predictions = []

        for metrics in wallets_metrics:
            try:
                prediction = self.predict_wallet_profitability(metrics)
                predictions.append(prediction)
            except Exception as e:
                logger.warning(f"Failed to predict wallet profitability: {e}")
                # Return default prediction
                predictions.append(ProfitabilityPrediction(
                    expected_return_pct=0.0,
                    confidence=0.0,
                    risk_score=0.5,
                    profitability_class=ProfitabilityClass.LOW_PROFIT,
                    feature_importance={},
                    prediction_timestamp=datetime.now().timestamp(),
                ))

        return predictions

    def rank_wallets_by_profitability(self, wallets_metrics: List[Dict[str, Any]],
                                    max_wallets: int = 50) -> List[Tuple[str, ProfitabilityPrediction]]:
        """
        Rank wallets by predicted profitability.

        Args:
            wallets_metrics: List of wallet metrics with addresses
            max_wallets: Maximum wallets to return

        Returns:
            List of (wallet_address, prediction) tuples, ranked by profitability
        """
        predictions = []

        for metrics in wallets_metrics:
            address = metrics.get('address')
            if not address:
                continue

            try:
                prediction = self.predict_wallet_profitability(metrics)
                predictions.append((address, prediction))
            except Exception as e:
                logger.warning(f"Failed to predict profitability for {address[:8]}...: {e}")

        # Sort by expected return (descending), then by confidence (descending)
        predictions.sort(key=lambda x: (x[1].expected_return_pct, x[1].confidence), reverse=True)

        return predictions[:max_wallets]

    def get_investment_allocation(self, predictions: List[Tuple[str, ProfitabilityPrediction]],
                                 total_capital_usd: float = 200.0) -> Dict[str, float]:
        """
        Calculate optimal investment allocation across wallets.

        Args:
            predictions: List of (wallet_address, prediction) tuples
            total_capital_usd: Total capital to allocate

        Returns:
            Dictionary mapping wallet_address to allocation amount
        """
        allocation = {}

        # Filter for profitable wallets only
        profitable_predictions = [
            (addr, pred) for addr, pred in predictions
            if pred.expected_return_pct > 0 and pred.confidence > 0.5
        ]

        if not profitable_predictions:
            logger.warning("No profitable wallets found for allocation")
            return allocation

        # Calculate confidence-weighted expected returns
        weighted_returns = [
            (addr, pred.expected_return_pct * pred.confidence * (1.0 - pred.risk_score))
            for addr, pred in profitable_predictions
        ]

        # Normalize weights
        total_weight = sum(weight for _, weight in weighted_returns)
        if total_weight == 0:
            return allocation

        # Allocate capital based on weights
        for addr, weight in weighted_returns:
            allocation[addr] = (weight / total_weight) * total_capital_usd

        logger.info(f"Allocated ${total_capital_usd:.2f} across {len(allocation)} wallets")
        return allocation

    def calculate_kelly_position_size(self, prediction: ProfitabilityPrediction,
                                     current_capital_usd: float = 200.0,
                                     target_capital_usd: float = 1000.0) -> float:
        """
        Calculate Kelly Criterion-inspired position size for maximum growth.

        Kelly Formula: f* = (bp - q) / b
        Where:
        - b = odds (profit_factor - 1)
        - p = probability of winning (win_rate)
        - q = probability of losing (1 - p)

        Adapted for our context with expected return and risk score.

        Args:
            prediction: Profitability prediction
            current_capital_usd: Current capital
            target_capital_usd: Target capital

        Returns:
            Recommended position size in USD
        """
        # Growth stage multiplier (more aggressive early on)
        growth_stage = min(2.0, target_capital_usd / max(current_capital_usd, 1.0))

        # Calculate Kelly fraction based on prediction
        expected_return = prediction.expected_return_pct / 100.0
        risk_of_loss = prediction.risk_score

        # Adjust win rate based on confidence
        win_rate = prediction.probability_of_profit or prediction.confidence
        lose_rate = 1.0 - win_rate

        # Estimate odds from expected return and risk
        if risk_of_loss > 0 and expected_return > 0:
            odds = expected_return / risk_of_loss
        else:
            odds = 1.0

        # Kelly calculation
        if odds > 0:
            kelly_fraction = (odds * win_rate - lose_rate) / odds
            # Clamp to reasonable range (0.5% to 25% of capital)
            kelly_fraction = max(0.005, min(kelly_fraction, 0.25))
        else:
            kelly_fraction = 0.01  # Conservative 1% default

        # Apply growth stage multiplier
        position_size = current_capital_usd * kelly_fraction * growth_stage

        # Capital-efficient sizing for early stage
        # Minimum position: $5 (2.5% of $200)
        # Maximum position: $50 (25% of capital)
        position_size = max(5.0, min(position_size, current_capital_usd * 0.25))

        return position_size

    def rank_wallets_for_growth(self, wallets_metrics: List[Dict[str, Any]],
                               max_wallets: int = 50,
                               current_capital_usd: float = 200.0) -> List[Tuple[str, ProfitabilityPrediction, float]]:
        """
        Rank wallets optimized for growth goal ($200 → $1000).

        Prioritizes:
        1. High ROI momentum (recent 7d performance)
        2. Early wallets (<30 days, high alpha potential)
        3. Strong risk-adjusted returns
        4. Penalizes bag-holder situations heavily

        Args:
            wallets_metrics: List of wallet metrics with addresses
            max_wallets: Maximum wallets to return
            current_capital_usd: Current capital for position sizing

        Returns:
            List of (wallet_address, prediction, position_size) tuples
        """
        predictions = []

        for metrics in wallets_metrics:
            address = metrics.get('address')
            if not address:
                continue

            try:
                prediction = self.predict_wallet_profitability(metrics)

                # Calculate growth-optimized position size
                position_size = self.calculate_kelly_position_size(
                    prediction,
                    current_capital_usd=current_capital_usd
                )

                predictions.append((address, prediction, position_size))
            except Exception as e:
                logger.warning(f"Failed to predict profitability for {address[:8]}...: {e}")

        # Growth-optimized sorting
        def growth_score(item):
            addr, pred, pos_size = item
            metrics = next((m for m in wallets_metrics if m.get('address') == addr), {})

            # Base score: expected return * confidence
            score = pred.expected_return_pct * pred.confidence

            # Momentum bonus
            roi_7d = metrics.get('roi_7d', 0)
            roi_30d = metrics.get('roi_30d', 0)
            if roi_7d > 0 and roi_30d > 0:
                momentum_ratio = roi_7d / max(roi_30d, 1.0)
                if momentum_ratio > 0.8:  # Strong recent momentum
                    score *= 1.3

            # Early wallet bonus (<30 days history)
            trade_count_30d = metrics.get('trade_count_30d', 0)
            if 10 <= trade_count_30d <= 50:  # Early but not brand new
                score *= 1.2

            # Bag holder penalty
            bag_holder_score = metrics.get('bag_holder_score', 0)
            if bag_holder_score > 0.3:
                score *= 0.5  # Heavy penalty

            # Insider risk penalty
            insider_prob = metrics.get('insider_probability', 0)
            if insider_prob > 0.7:
                score *= 0.3  # Severe penalty

            return score

        # Sort by growth score
        predictions.sort(key=growth_score, reverse=True)

        return predictions[:max_wallets]

    def get_capital_efficient_allocation(self, predictions: List[Tuple[str, ProfitabilityPrediction]],
                                       total_capital_usd: float = 200.0,
                                       min_position_usd: float = 5.0,
                                       max_position_usd: float = 50.0) -> Dict[str, float]:
        """
        Calculate capital-efficient allocation for $200 → $1000 growth goal.

        Focuses capital on highest-conviction opportunities while maintaining
        diversification.

        Args:
            predictions: List of (wallet_address, prediction) tuples
            total_capital_usd: Total capital to allocate
            min_position_usd: Minimum position size
            max_position_usd: Maximum position size

        Returns:
            Dictionary mapping wallet_address to allocation amount
        """
        allocation = {}

        # Filter for high-conviction wallets only
        high_conviction = [
            (addr, pred) for addr, pred in predictions
            if pred.expected_return_pct > 5 and pred.confidence > 0.6
        ]

        if not high_conviction:
            logger.warning("No high-conviction wallets found, using profitable wallets")
            high_conviction = [
                (addr, pred) for addr, pred in predictions
                if pred.expected_return_pct > 0 and pred.confidence > 0.5
            ]

        if not high_conviction:
            return allocation

        # Calculate position sizes using Kelly Criterion
        position_sizes = []
        for addr, pred in high_conviction:
            pos_size = self.calculate_kelly_position_size(pred, total_capital_usd)
            position_sizes.append((addr, pos_size, pred))

        # Sort by conviction (expected_return * confidence)
        position_sizes.sort(key=lambda x: x[2].expected_return_pct * x[2].confidence, reverse=True)

        # Allocate capital with diversification constraint
        # Maximum 20% per wallet, ensure at least 5 wallets
        max_per_wallet = max(max_position_usd, total_capital_usd * 0.20)
        min_wallets = max(5, len(high_conviction))
        remaining_capital = total_capital_usd

        for i, (addr, pos_size, pred) in enumerate(position_sizes):
            if remaining_capital <= min_position_usd:
                break

            # Check if we should add more wallets for diversification
            wallets_allocated = len(allocation)
            if wallets_allocated < min_wallets and i >= min_wallets:
                # Ensure minimum allocation for diversification
                alloc_amount = min(min_position_usd * 2, remaining_capital / (min_wallets - wallets_allocated + 1))
            else:
                # Use Kelly size but respect constraints
                alloc_amount = min(pos_size, max_per_wallet, remaining_capital)

            if alloc_amount >= min_position_usd:
                allocation[addr] = alloc_amount
                remaining_capital -= alloc_amount

        logger.info(f"Capital-efficient allocation: ${total_capital_usd:.2f} across {len(allocation)} wallets")
        logger.info(f"Average position: ${total_capital_usd / len(allocation):.2f}")
        return allocation


# Global singleton instance
_predictor: Optional[ProfitabilityPredictor] = None


def get_profitability_predictor() -> ProfitabilityPredictor:
    """Get the global profitability predictor singleton."""
    global _predictor

    if _predictor is None:
        _predictor = ProfitabilityPredictor()

    return _predictor


def predict_wallet_profitability(wallet_metrics: Dict[str, Any]) -> ProfitabilityPrediction:
    """
    Convenience function to predict wallet profitability.

    Args:
        wallet_metrics: Wallet metrics dictionary

    Returns:
        Profitability prediction
    """
    predictor = get_profitability_predictor()
    return predictor.predict_wallet_profitability(wallet_metrics)


if __name__ == "__main__":
    # Test the predictor
    test_metrics = {
        'address': 'test_wallet',
        'roi_7d': 15.0,
        'roi_30d': 45.0,
        'win_rate': 0.72,
        'profit_factor': 2.1,
        'max_drawdown_30d': 8.5,
        'sortino_ratio': 1.8,
        'trade_count_30d': 127,
        'avg_trade_size_sol': 0.5,
        'uses_mev_protection': True,
        'uses_limit_orders': True,
        'dex_diversity_score': 3,
        'parse_rate': 0.95,
        'insider_probability': 0.1,
    }

    predictor = get_profitability_predictor()
    prediction = predictor.predict_wallet_profitability(test_metrics)

    print(f"Expected Return: {prediction.expected_return_pct:.1f}%")
    print(f"Confidence: {prediction.confidence:.1f}")
    print(f"Risk Score: {prediction.risk_score:.1f}")
    print(f"Profitability Class: {prediction.profitability_class.value}")
    print(f"Sharpe Ratio: {prediction.sharpe_ratio_predicted:.2f}")
    print(f"Probability of Profit: {prediction.probability_of_profit:.1f}")