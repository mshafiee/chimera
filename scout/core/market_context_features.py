"""
Market Context Features for Scout

Extracts features related to market conditions and wallet behavior in different contexts.
This module provides:
- Beta calculation (correlation with SOL price movements)
- Market cap tier preferences
- DEX preference analysis (Raydium vs Orca)
- Time-of-day and day-of-week performance patterns

Usage:
    extractor = MarketContextFeatures()
    features = extractor.extract_features(wallet_trades, sol_price_history)
"""

import logging
import numpy as np
from typing import Dict, List, Optional, Tuple, Any
from datetime import datetime, timedelta
from collections import defaultdict

logger = logging.getLogger(__name__)


class MarketContextFeatures:
    """
    Extract market context features from wallet trading behavior.

    Features:
    - Beta with SOL price
    - Market cap tier preference
    - DEX preference (Raydium vs Orca)
    - Time-of-day patterns
    - Day-of-week patterns
    - Volume profile analysis
    """

    # Market cap tiers (in USD)
    MARKET_CAP_TIERS = {
        'nano': (0, 100_000),
        'micro': (100_000, 1_000_000),
        'small': (1_000_000, 10_000_000),
        'mid': (10_000_000, 100_000_000),
        'large': (100_000_000, float('inf')),
    }

    # DEX program IDs
    DEX_PROGRAMS = {
        'jupiter': 'JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4',
        'raydium': '675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8',
        'orca': '9W959DqEETiGZocYWCQPaJ6sBmUzgfxXfqGeTEdp3aQP',
        'whirlpool': 'whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc',
    }

    def __init__(self):
        """Initialize the market context feature extractor."""
        self.dex_preferences = defaultdict(int)
        self.time_preferences = defaultdict(int)
        self.day_preferences = defaultdict(int)

    def extract_features(
        self,
        trades: List[Dict[str, Any]],
        sol_price_history: Optional[List[Dict[str, Any]]] = None
    ) -> Dict[str, Any]:
        """
        Extract market context features from wallet trades.

        Args:
            trades: List of trade records
            sol_price_history: Optional SOL price history for beta calculation

        Returns:
            Dictionary of market context features
        """
        if not trades:
            return self._empty_features()

        features = {}

        try:
            # Beta calculation
            if sol_price_history:
                features.update(self._calculate_beta_features(trades, sol_price_history))

            # Market cap tier preference
            features.update(self._extract_market_cap_features(trades))

            # DEX preference
            features.update(self._extract_dex_features(trades))

            # Time patterns
            features.update(self._extract_time_features(trades))

            # Volume profile
            features.update(self._extract_volume_features(trades))

            # Market regime behavior
            features.update(self._extract_regime_features(trades))

            features['extraction_success'] = True
            features['trade_count'] = len(trades)

        except Exception as e:
            logger.error(f"Market context feature extraction failed: {e}")
            return self._empty_features()

        return features

    def _empty_features(self) -> Dict[str, Any]:
        """Return empty features dict."""
        return {
            'extraction_success': False,
            'trade_count': 0,
        }

    def _calculate_beta_features(
        self,
        trades: List[Dict[str, Any]],
        sol_price_history: List[Dict[str, Any]]
    ) -> Dict[str, float]:
        """Calculate beta (correlation with SOL price movements)."""
        features = {}

        try:
            # Align trade timestamps with SOL price points
            trade_returns = []
            sol_returns = []

            # Sort trades by timestamp
            sorted_trades = sorted(
                trades,
                key=lambda x: x.get('timestamp', '')
            )

            # Create price lookup
            sol_prices = {
                datetime.fromisoformat(p['timestamp']): p['price']
                for p in sol_price_history
                if 'timestamp' in p and 'price' in p
            }

            for i, trade in enumerate(sorted_trades):
                trade_time = datetime.fromisoformat(trade.get('timestamp', datetime.utcnow().isoformat()))

                # Get trade return
                trade_pnl = trade.get('pnl_sol', trade.get('pnl', 0.0))

                # Find closest SOL price
                closest_time = min(
                    sol_prices.keys(),
                    key=lambda t: abs((t - trade_time).total_seconds()),
                    default=None
                )

                if closest_time and i > 0:
                    prev_trade = sorted_trades[i - 1]
                    prev_time = datetime.fromisoformat(prev_trade.get('timestamp', trade_time))

                    # Get SOL return
                    sol_price_now = sol_prices.get(closest_time, 0)
                    sol_price_prev = sol_prices.get(closest_time - timedelta(hours=1), sol_price_now)

                    if sol_price_prev > 0:
                        sol_return = (sol_price_now - sol_price_prev) / sol_price_prev
                        sol_returns.append(sol_return)

                        # Normalize trade return
                        if i > 0:
                            prev_pnl = prev_trade.get('pnl_sol', prev_trade.get('pnl', 0.0))
                            trade_return = (trade_pnl - prev_pnl) / (abs(prev_pnl) + 1e-8)
                            trade_returns.append(trade_return)

            if len(trade_returns) >= 3 and len(sol_returns) >= 3:
                min_len = min(len(trade_returns), len(sol_returns))
                trade_returns = np.array(trade_returns[:min_len])
                sol_returns = np.array(sol_returns[:min_len])

                # Calculate covariance and variance
                covariance = np.cov(trade_returns, sol_returns)[0, 1]
                sol_variance = np.var(sol_returns)

                if sol_variance > 0:
                    beta = covariance / sol_variance
                    features['sol_beta'] = float(beta)
                    features['high_beta'] = float(abs(beta) > 1.0)
                    features['beta_direction'] = float(beta > 0)
                else:
                    features['sol_beta'] = 0.0
                    features['high_beta'] = 0.0
                    features['beta_direction'] = 0.5

                # Correlation
                correlation = np.corrcoef(trade_returns, sol_returns)[0, 1]
                features['sol_correlation'] = float(correlation) if not np.isnan(correlation) else 0.0

        except Exception as e:
            logger.warning(f"Beta calculation failed: {e}")

        return features

    def _extract_market_cap_features(
        self,
        trades: List[Dict[str, Any]]
    ) -> Dict[str, float]:
        """Extract market cap tier preference features."""
        features = {}

        tier_counts = defaultdict(int)
        tier_pnl = defaultdict(float)

        for trade in trades:
            market_cap = trade.get('token_market_cap', trade.get('market_cap', 0))

            # Determine tier
            tier = None
            for tier_name, (min_mc, max_mc) in self.MARKET_CAP_TIERS.items():
                if min_mc <= market_cap < max_mc:
                    tier = tier_name
                    break

            if tier:
                tier_counts[tier] += 1
                tier_pnl[tier] += trade.get('pnl_sol', trade.get('pnl', 0.0))

        total_trades = len(tier_counts)
        if total_trades == 0:
            return features

        # Tier preferences (fraction of trades in each tier)
        for tier in self.MARKET_CAP_TIERS.keys():
            count = tier_counts.get(tier, 0)
            features[f'mc_tier_{tier}_pct'] = count / total_trades

        # Performance by tier
        for tier in self.MARKET_CAP_TIERS.keys():
            count = tier_counts.get(tier, 0)
            if count > 0:
                features[f'mc_tier_{tier}_avg_pnl'] = tier_pnl[tier] / count
            else:
                features[f'mc_tier_{tier}_avg_pnl'] = 0.0

        # Dominant tier
        dominant_tier = max(tier_counts.keys(), key=lambda k: tier_counts[k]) if tier_counts else None
        features['dominant_mc_tier'] = float(
            list(self.MARKET_CAP_TIERS.keys()).index(dominant_tier) if dominant_tier else 2
        )  # Default to mid-tier

        # Micro-cap preference (risky behavior indicator)
        micro_cap_pct = tier_counts.get('micro', 0) / total_trades
        nano_cap_pct = tier_counts.get('nano', 0) / total_trades
        features['micro_cap_preference'] = float(micro_cap_pct + nano_cap_pct)

        return features

    def _extract_dex_features(
        self,
        trades: List[Dict[str, Any]]
    ) -> Dict[str, float]:
        """Extract DEX preference features."""
        features = {}

        dex_counts = defaultdict(int)
        dex_pnl = defaultdict(float)

        for trade in trades:
            dex_program = trade.get('dex_program', '')

            # Map program ID to DEX name
            dex_name = None
            for name, program_id in self.DEX_PROGRAMS.items():
                if dex_program == program_id or program_id in dex_program:
                    dex_name = name
                    break

            if not dex_name:
                dex_name = 'other'

            dex_counts[dex_name] += 1
            dex_pnl[dex_name] += trade.get('pnl_sol', trade.get('pnl', 0.0))

        total_trades = len(trades)
        if total_trades == 0:
            return features

        # DEX preferences
        for dex in ['jupiter', 'raydium', 'orca', 'whirlpool', 'other']:
            count = dex_counts.get(dex, 0)
            features[f'dex_{dex}_pct'] = count / total_trades

        # Performance by DEX
        for dex in ['jupiter', 'raydium', 'orca', 'whirlpool', 'other']:
            count = dex_counts.get(dex, 0)
            if count > 0:
                features[f'dex_{dex}_avg_pnl'] = dex_pnl[dex] / count
            else:
                features[f'dex_{dex}_avg_pnl'] = 0.0

        # DEX diversity (entropy)
        if total_trades > 0:
            proportions = [count / total_trades for count in dex_counts.values()]
            entropy = -sum(p * np.log(p + 1e-8) for p in proportions if p > 0)
            max_entropy = np.log(len(dex_counts))
            features['dex_diversity'] = float(entropy / (max_entropy + 1e-8))
        else:
            features['dex_diversity'] = 0.0

        # Dominant DEX
        if dex_counts:
            dominant_dex = max(dex_counts.keys(), key=lambda k: dex_counts[k])
            features['dominant_dex'] = float(
                ['jupiter', 'raydium', 'orca', 'whirlpool', 'other'].index(dominant_dex)
                if dominant_dex in ['jupiter', 'raydium', 'orca', 'whirlpool', 'other']
                else 4
            )
        else:
            features['dominant_dex'] = 0.0

        return features

    def _extract_time_features(
        self,
        trades: List[Dict[str, Any]]
    ) -> Dict[str, float]:
        """Extract time-of-day and day-of-week pattern features."""
        features = {}

        hour_pnl = defaultdict(float)
        hour_count = defaultdict(int)
        day_pnl = defaultdict(float)
        day_count = defaultdict(int)

        for trade in trades:
            try:
                timestamp_str = trade.get('timestamp', '')
                if isinstance(timestamp_str, str):
                    trade_time = datetime.fromisoformat(timestamp_str)
                else:
                    trade_time = timestamp_str

                hour = trade_time.hour
                day = trade_time.weekday()  # 0=Monday, 6=Sunday

                pnl = trade.get('pnl_sol', trade.get('pnl', 0.0))

                hour_pnl[hour] += pnl
                hour_count[hour] += 1

                day_pnl[day] += pnl
                day_count[day] += 1

            except Exception as e:
                logger.debug(f"Failed to parse timestamp: {e}")
                continue

        # Hour preferences (trading session)
        # Asia: 0-8 UTC, Europe: 8-16 UTC, US: 16-24 UTC
        asia_pnl = sum(hour_pnl[h] for h in range(0, 8))
        asia_count = sum(hour_count[h] for h in range(0, 8))

        europe_pnl = sum(hour_pnl[h] for h in range(8, 16))
        europe_count = sum(hour_count[h] for h in range(8, 16))

        us_pnl = sum(hour_pnl[h] for h in range(16, 24))
        us_count = sum(hour_count[h] for h in range(16, 24))

        total_count = sum(hour_count.values())
        if total_count > 0:
            features['asia_session_pct'] = asia_count / total_count
            features['europe_session_pct'] = europe_count / total_count
            features['us_session_pct'] = us_count / total_count

            if asia_count > 0:
                features['asia_session_avg_pnl'] = asia_pnl / asia_count
            else:
                features['asia_session_avg_pnl'] = 0.0

            if europe_count > 0:
                features['europe_session_avg_pnl'] = europe_pnl / europe_count
            else:
                features['europe_session_avg_pnl'] = 0.0

            if us_count > 0:
                features['us_session_avg_pnl'] = us_pnl / us_count
            else:
                features['us_session_avg_pnl'] = 0.0

        # Day-of-week preferences
        total_day_count = sum(day_count.values())
        if total_day_count > 0:
            for day in range(7):
                count = day_count.get(day, 0)
                features[f'day_{day}_pct'] = count / total_day_count

                if count > 0:
                    features[f'day_{day}_avg_pnl'] = day_pnl[day] / count
                else:
                    features[f'day_{day}_avg_pnl'] = 0.0

        # Weekend preference (Saturday=5, Sunday=6)
        weekend_count = sum(day_count.get(d, 0) for d in [5, 6])
        if total_day_count > 0:
            features['weekend_preference'] = weekend_count / total_day_count

        return features

    def _extract_volume_features(
        self,
        trades: List[Dict[str, Any]]
    ) -> Dict[str, float]:
        """Extract volume profile features."""
        features = {}

        if not trades:
            return features

        volumes = [
            trade.get('volume_usd', trade.get('volume', 0))
            for trade in trades
        ]
        volumes = [v for v in volumes if v > 0]

        if not volumes:
            return features

        # Volume statistics
        features['avg_volume'] = float(np.mean(volumes))
        features['median_volume'] = float(np.median(volumes))
        features['std_volume'] = float(np.std(volumes))

        # Volume tier preference
        # Small: < $1k, Medium: $1k-$10k, Large: $10k-$100k, Whale: >$100k
        tier_counts = {
            'small': sum(1 for v in volumes if v < 1_000),
            'medium': sum(1 for v in volumes if 1_000 <= v < 10_000),
            'large': sum(1 for v in volumes if 10_000 <= v < 100_000),
            'whale': sum(1 for v in volumes if v >= 100_000),
        }

        total_volume_trades = len(volumes)
        if total_volume_trades > 0:
            for tier, count in tier_counts.items():
                features[f'volume_tier_{tier}_pct'] = count / total_volume_trades

        return features

    def _extract_regime_features(
        self,
        trades: List[Dict[str, Any]]
    ) -> Dict[str, float]:
        """Extract features related to market regime behavior."""
        features = {}

        # This would require external market regime data
        # For now, infer from wallet's own performance patterns

        win_rates = []
        window_size = 10

        for i in range(window_size, len(trades)):
            window_trades = trades[i-window_size:i]
            wins = sum(1 for t in window_trades if t.get('pnl_sol', t.get('pnl', 0)) > 0)
            win_rates.append(wins / window_size)

        if win_rates:
            # Volatility in win rate
            features['win_rate_volatility'] = float(np.std(win_rates))

            # Trend in win rate
            if len(win_rates) >= 3:
                recent_avg = np.mean(win_rates[-3:])
                early_avg = np.mean(win_rates[:3])
                features['win_rate_trend'] = float(recent_avg - early_avg)
                features['win_rate_improving'] = float(features['win_rate_trend'] > 0)

        return features


# Convenience function
def extract_market_context_features(
    trades: List[Dict[str, Any]],
    sol_price_history: Optional[List[Dict[str, Any]]] = None
) -> Dict[str, Any]:
    """
    Quick extraction of market context features.

    Args:
        trades: List of trade records
        sol_price_history: Optional SOL price history

    Returns:
        Dictionary of market context features
    """
    extractor = MarketContextFeatures()
    return extractor.extract_features(trades, sol_price_history)
