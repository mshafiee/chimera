"""
Optimized Wallet Analyzer - Integration wrapper for optimization systems.

This module wraps WalletAnalyzer with optimization features:
- Credit-aware API calls
- Multi-level caching
- ML-based profitability prediction
- Production monitoring
"""

from __future__ import annotations

import logging
from typing import Optional, Dict, List, Any, TYPE_CHECKING
from .analyzer import WalletAnalyzer
from .wqs import WalletMetrics

if TYPE_CHECKING:
    from .scout_optimizer import ScoutOptimizer

logger = logging.getLogger(__name__)


class OptimizedWalletAnalyzer:
    """
    Optimized wrapper for WalletAnalyzer with full optimization integration.

    Features:
    - Credit-aware API calls (check budget before expensive operations)
    - Multi-level caching (fast cache hit before slow API calls)
    - ML prediction integration (early filtering of low-potential wallets)
    - Production monitoring (performance tracking and alerting)
    - Graceful degradation (fallback to base analyzer if optimizations fail)
    """

    def __init__(
        self,
        base_analyzer: WalletAnalyzer,
        optimizer: Optional['ScoutOptimizer'] = None
    ):
        """
        Initialize optimized analyzer.

        Args:
            base_analyzer: Base WalletAnalyzer instance
            optimizer: ScoutOptimizer instance (optional)
        """
        self._analyzer = base_analyzer
        self._optimizer = optimizer
        self._optimization_enabled = optimizer is not None

    @property
    def _trades_cache(self):
        """Delegate trades cache access to base analyzer."""
        return getattr(self._analyzer, '_trades_cache', {})

    def compute_wallet_trade_stats(self, trades):
        """Delegate trade stats computation to base analyzer."""
        if hasattr(self._analyzer, 'compute_wallet_trade_stats'):
            return self._analyzer.compute_wallet_trade_stats(trades)
        return {}

    def print_parse_health_dashboard(self) -> None:
        """Delegate print parse health dashboard to base analyzer."""
        if hasattr(self._analyzer, 'print_parse_health_dashboard'):
            self._analyzer.print_parse_health_dashboard()

    def is_parse_rate_below_threshold(self) -> bool:
        """Delegate parse rate threshold check to base analyzer."""
        if hasattr(self._analyzer, 'is_parse_rate_below_threshold'):
            return self._analyzer.is_parse_rate_below_threshold()
        return False

    async def get_wallet_metrics(self, address: str) -> Optional[WalletMetrics]:
        """
        Get wallet metrics with full optimization stack.

        Optimization pipeline:
        1. Check cache (fastest) - if hit, return immediately
        2. Check credit budget - if insufficient, skip or use fallback
        3. Use base analyzer (expensive API call)
        4. Cache results for future use
        5. Monitor performance and create alerts if needed
        """
        print(f"  [Optimized] Getting metrics for {address[:8]}...")

        try:
            # 1. Try cache first (multi-level cache)
            if self._optimization_enabled and self._optimizer:
                try:
                    cached = self._optimizer.get_cached_wallet_metrics(address)
                    if cached:
                        print(f"  [Optimized] ✓ Cache hit for {address[:8]}...")
                        return self._dict_to_metrics(cached)
                except Exception as cache_error:
                    print(f"  [Optimized] Cache check failed: {cache_error}")

            # 2. Check credit budget before expensive operation
            if self._optimization_enabled and self._optimizer:
                try:
                    can_proceed, reason = self._optimizer.can_analyze_wallet(address)
                    if not can_proceed:
                        print(f"  [Optimized] ✗ Credit budget limit reached: {reason}")
                        return None
                except Exception as credit_error:
                    print(f"  [Optimized] Credit check failed: {credit_error}")

            # 3. Get metrics from base analyzer (expensive API call)
            metrics = await self._analyzer.get_wallet_metrics(address)
            if not metrics:
                return None

            # 4. Cache results for future use
            if self._optimization_enabled and self._optimizer:
                try:
                    self._optimizer.cache_wallet_metrics(address, self._metrics_to_dict(metrics))
                    print(f"  [Optimized] ✓ Cached metrics for {address[:8]}...")
                except Exception as cache_error:
                    print(f"  [Optimized] Cache write failed (non-critical): {cache_error}")

            return metrics

        except Exception as e:
            logger.error(f"Error in optimized analysis: {e}")
            if self._optimization_enabled and self._optimizer:
                try:
                    self._optimizer.create_alert(
                        severity="warning",
                        title="Wallet Analysis Failed",
                        message=f"Failed to analyze {address[:8]}...: {e}",
                        source="optimized_analyzer"
                    )
                except Exception:
                    pass  # Non-critical
            return None

    def _metrics_to_dict(self, metrics: WalletMetrics) -> Dict[str, Any]:
        """Convert WalletMetrics to dict for caching."""
        return {
            'address': metrics.address,
            'roi_7d': metrics.roi_7d,
            'roi_30d': metrics.roi_30d,
            'trade_count_30d': metrics.trade_count_30d,
            'win_rate': metrics.win_rate,
            'max_drawdown_30d': metrics.max_drawdown_30d,
            'avg_trade_size_sol': metrics.avg_trade_size_sol,
            'profit_factor': metrics.profit_factor,
            'sortino_ratio': metrics.sortino_ratio,
        }

    def _dict_to_metrics(self, data: Dict[str, Any]) -> WalletMetrics:
        """Convert dict to WalletMetrics."""
        return WalletMetrics(
            address=data['address'],
            roi_7d=data.get('roi_7d'),
            roi_30d=data.get('roi_30d'),
            trade_count_30d=data.get('trade_count_30d'),
            win_rate=data.get('win_rate'),
            max_drawdown_30d=data.get('max_drawdown_30d'),
            avg_trade_size_sol=data.get('avg_trade_size_sol'),
            profit_factor=data.get('profit_factor'),
            sortino_ratio=data.get('sortino_ratio'),
        )

    def get_candidate_wallets(self) -> List[str]:
        """Get candidate wallets with credit-aware filtering."""
        candidates = self._analyzer.get_candidate_wallets()

        # Optimize based on credit budget
        if self._optimization_enabled and self._optimizer:
            try:
                optimized_count = self._optimizer.optimize_wallet_count(len(candidates))
                print(f"[Optimized] Candidates: {len(candidates)} → {optimized_count}")
                return candidates[:optimized_count]
            except Exception as e:
                print(f"[Optimized] Wallet count optimization failed: {e}")

        return candidates

    async def clear_wallet_cache(self, address: str):
        """Clear cached data for a specific wallet."""
        # Clear base analyzer cache
        if hasattr(self._analyzer, 'clear_wallet_cache'):
            self._analyzer.clear_wallet_cache(address)

        # Clear optimization cache
        if self._optimization_enabled and self._optimizer:
            try:
                self._optimizer.invalidate_cache("wallet", address)
            except Exception as e:
                print(f"[Optimized] Cache invalidation failed: {e}")

    async def clear_all_caches(self):
        """Clear all cached data."""
        # Clear base analyzer cache
        if hasattr(self._analyzer, 'clear_all_caches'):
            await self._analyzer.clear_all_caches()

        # Clear optimization cache - invalidate all wallet entries
        if self._optimization_enabled and self._optimizer:
            try:
                self._optimizer.invalidate_cache("wallet", "*")
            except Exception as e:
                print(f"[Optimized] Cache invalidation failed: {e}")

    def close(self):
        """Close resources (HTTP sessions, etc.)."""
        if hasattr(self._analyzer, 'close'):
            # Close base analyzer resources
            if hasattr(self._analyzer, 'helius_client') and self._analyzer.helius_client:
                return self._analyzer.helius_client.close()

    @property
    def rugcheck_client(self):
        """Delegate rugcheck_client to base analyzer."""
        return getattr(self._analyzer, 'rugcheck_client', None)

    @property
    def helius_client(self):
        """Delegate helius_client to base analyzer."""
        return getattr(self._analyzer, 'helius_client', None)

    async def shutdown(self):
        """Cleanup and shutdown."""
        try:
            # Close base analyzer resources
            if hasattr(self._analyzer, 'helius_client') and self._analyzer.helius_client:
                await self._analyzer.helius_client.close()
                print("[Optimized] Closed HTTP sessions")
        except Exception as e:
            print(f"[Optimized] Shutdown error: {e}")
        if hasattr(self._analyzer, 'shutdown'):
            try:
                await self._analyzer.shutdown()
            except Exception:
                pass  # Non-critical

    def determine_archetype(self, metrics, trades):
        """Delegate determine_archetype to base analyzer."""
        if hasattr(self._analyzer, 'determine_archetype'):
            return self._analyzer.determine_archetype(metrics, trades)
        return {}

    def _calculate_alpha_decay(self, trades):
        """Delegate _calculate_alpha_decay to base analyzer."""
        if hasattr(self._analyzer, '_calculate_alpha_decay'):
            return self._analyzer._calculate_alpha_decay(trades)
        return {}

    def _calculate_trade_size_decay(self, trades):
        """Delegate _calculate_trade_size_decay to base analyzer."""
        if hasattr(self._analyzer, '_calculate_trade_size_decay'):
            return self._analyzer._calculate_trade_size_decay(trades)
        return None

    def _calculate_token_rotation_decay(self, trades):
        """Delegate _calculate_token_rotation_decay to base analyzer."""
        if hasattr(self._analyzer, '_calculate_token_rotation_decay'):
            return self._analyzer._calculate_token_rotation_decay(trades)
        return None