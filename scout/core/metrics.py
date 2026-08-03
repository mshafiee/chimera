"""
Prometheus Metrics Export for Scout

Exports Scout-specific metrics for monitoring:
- WQS average and distribution
- Unrealized PnL tracking
- Archetype counts
- RugCheck rejections
"""

import os
import logging
from typing import List, Optional

logger = logging.getLogger(__name__)

# Try to import prometheus_client
try:
    from prometheus_client import Gauge, Histogram, Counter, start_http_server
    PROMETHEUS_AVAILABLE = True
except ImportError:
    PROMETHEUS_AVAILABLE = False
    Gauge = None
    Histogram = None
    Counter = None
    start_http_server = None


class ScoutMetrics:
    """
    Prometheus metrics exporter for Scout.
    
    Metrics exported:
    - scout_wqs_average: Average WQS of active roster (Gauge)
    - scout_wqs_distribution: WQS histogram (Histogram)
    - scout_unrealized_pnl_total: Total unrealized PnL of roster (Gauge)
    - scout_wallets_by_archetype: Count of wallets per archetype (Gauge with label)
    - scout_rugcheck_rejections: Count of tokens rejected by RugCheck (Counter)
    """
    
    def __init__(self, port: int = 8081):
        """
        Initialize metrics exporter.
        
        Args:
            port: Port to expose metrics on (default 8081)
        """
        self.port = port
        self.metrics_started = False
        
        if not PROMETHEUS_AVAILABLE:
            logger.warning("prometheus_client not available, metrics disabled")
            return
        
        # Define metrics
        self.wqs_average = Gauge(
            'scout_wqs_average',
            'Average WQS score of active roster',
        )
        
        # Live distribution of current roster WQS (GaugeVec per bucket — a
        # Histogram would accumulate counts forever and never reflect the
        # current state)
        self.wqs_distribution = Gauge(
            'scout_wqs_distribution',
            'Distribution of WQS scores (current roster)',
            ['bucket']
        )
        self._wqs_buckets = [0, 20, 40, 60, 70, 80, 90, 100]
        
        self.unrealized_pnl_total = Gauge(
            'scout_unrealized_pnl_total',
            'Total unrealized PnL (SOL) of active roster',
        )
        
        self.wallets_by_archetype = Gauge(
            'scout_wallets_by_archetype',
            'Count of wallets by archetype',
            ['archetype', 'status']
        )
        
        self.rugcheck_rejections = Counter(
            'scout_rugcheck_rejections_total',
            'Total number of tokens rejected by RugCheck',
        )
        
        self.wallets_analyzed = Counter(
            'scout_wallets_analyzed_total',
            'Total number of wallets analyzed',
        )
        
        self.analysis_duration = Histogram(
            'scout_analysis_duration_seconds',
            'Time taken to analyze wallets',
            buckets=[10, 30, 60, 120, 300, 600, 1800]
        )

        # WQS-PnL correlation metrics (Phase 4C)
        self.wqs_pnl_correlation = Gauge(
            'scout_wqs_pnl_correlation',
            'Pearson correlation between WQS at promotion and actual 30d copy PnL',
        )

        self.wallets_with_pnl_count = Gauge(
            'scout_wallets_with_pnl_count',
            'Number of promoted wallets with actual copy PnL data',
        )

        self.mean_copy_pnl_30d = Gauge(
            'scout_mean_copy_pnl_30d_sol',
            'Mean 30d copy PnL across all promoted wallets with data',
        )
    
    def start_server(self):
        """Start Prometheus metrics HTTP server."""
        if not PROMETHEUS_AVAILABLE:
            return
        
        if self.metrics_started:
            return
        
        try:
            start_http_server(self.port)
            self.metrics_started = True
            logger.info(f"Prometheus metrics server started on port {self.port}")
        except Exception as e:
            logger.warning(f"Failed to start Prometheus metrics server: {e}")
    
    def update_wqs_metrics(self, records: List):
        """
        Update WQS-related metrics from wallet records.
        
        Args:
            records: List of WalletRecord objects
        """
        if not PROMETHEUS_AVAILABLE:
            return
        
        if not records:
            # No roster: report "no data" instead of a stale average
            self.wqs_average.set(float('nan'))
            return
        
        # Calculate average WQS for active wallets
        active_wallets = [r for r in records if r.status == "ACTIVE" and r.wqs_score is not None]
        if active_wallets:
            avg_wqs = sum(r.wqs_score for r in active_wallets) / len(active_wallets)
            self.wqs_average.set(avg_wqs)
        else:
            self.wqs_average.set(float('nan'))
        
        # Record WQS distribution as a live snapshot: zero every bucket first,
        # then count the current roster
        for bucket in self._wqs_buckets:
            self.wqs_distribution.labels(bucket=str(bucket)).set(0)

        for record in records:
            if record.wqs_score is not None:
                bucket = self._wqs_buckets[-1]
                for b in self._wqs_buckets:
                    if record.wqs_score <= b:
                        bucket = b
                        break
                self.wqs_distribution.labels(bucket=str(bucket)).inc()
    
    def update_unrealized_pnl(self, total_unrealized_pnl_sol: float):
        """
        Update unrealized PnL metric.
        
        Args:
            total_unrealized_pnl_sol: Total unrealized PnL in SOL
        """
        if not PROMETHEUS_AVAILABLE:
            return
        
        self.unrealized_pnl_total.set(total_unrealized_pnl_sol)
    
    def update_archetype_counts(self, records: List):
        """
        Update archetype count metrics.
        
        Args:
            records: List of WalletRecord objects
        """
        if not PROMETHEUS_AVAILABLE:
            return
        
        # Reset all archetype gauges (UNKNOWN included so missing archetypes
        # are exported rather than silently dropped)
        archetypes = ["SNIPER", "SWING", "SCALPER", "INSIDER", "WHALE", "UNKNOWN"]
        statuses = ["ACTIVE", "CANDIDATE", "REJECTED"]
        
        for archetype in archetypes:
            for status in statuses:
                self.wallets_by_archetype.labels(archetype=archetype, status=status).set(0)
        
        # Count wallets by archetype and status (any other status is ignored
        # so its stale label series is not left behind)
        for record in records:
            archetype = record.archetype or "UNKNOWN"
            status = record.status
            if archetype in archetypes and status in statuses:
                self.wallets_by_archetype.labels(archetype=archetype, status=status).inc()
    
    def increment_rugcheck_rejections(self, count: int = 1):
        """
        Increment RugCheck rejection counter.
        
        Args:
            count: Number of rejections to add
        """
        if not PROMETHEUS_AVAILABLE:
            return
        
        self.rugcheck_rejections.inc(count)
    
    def increment_wallets_analyzed(self, count: int = 1):
        """
        Increment wallets analyzed counter.
        
        Args:
            count: Number of wallets analyzed
        """
        if not PROMETHEUS_AVAILABLE:
            return
        
        self.wallets_analyzed.inc(count)
    
    def record_analysis_duration(self, duration_seconds: float):
        """
        Record analysis duration.
        
        Args:
            duration_seconds: Duration in seconds
        """
        if not PROMETHEUS_AVAILABLE:
            return
        
        self.analysis_duration.observe(duration_seconds)
    
    def update_pnl_correlation_metrics(
        self,
        wallets_with_pnl: int,
        mean_pnl_30d: float,
        correlation_r: Optional[float] = None,
    ):
        """
        Update WQS-PnL correlation metrics.
        
        Args:
            wallets_with_pnl: Number of wallets with PnL data
            mean_pnl_30d: Mean 30d copy PnL in SOL
            correlation_r: Pearson correlation between WQS and actual PnL
        """
        if not PROMETHEUS_AVAILABLE:
            return
        
        self.wallets_with_pnl_count.set(wallets_with_pnl)
        self.mean_copy_pnl_30d.set(mean_pnl_30d)
        if correlation_r is not None:
            self.wqs_pnl_correlation.set(correlation_r)


# Global metrics instance
_metrics_instance: Optional[ScoutMetrics] = None


def get_metrics() -> Optional[ScoutMetrics]:
    """Get or create global metrics instance."""
    global _metrics_instance
    
    if _metrics_instance is None:
        try:
            port = int(os.getenv("SCOUT_METRICS_PORT", "8081"))
        except ValueError:
            # Non-numeric port config: fall back to the default instead of
            # crashing metric initialization
            logger.warning("Invalid SCOUT_METRICS_PORT value, using default 8081")
            port = 8081
        _metrics_instance = ScoutMetrics(port=port)
        
        # Auto-start if enabled
        if os.getenv("SCOUT_METRICS_ENABLED", "true").lower() == "true":
            _metrics_instance.start_server()
    
    return _metrics_instance
