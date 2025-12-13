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
from typing import List, Dict, Optional
from datetime import datetime

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
        
        self.wqs_distribution = Histogram(
            'scout_wqs_distribution',
            'Distribution of WQS scores',
            buckets=[0, 20, 40, 60, 70, 80, 90, 100]
        )
        
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
            return
        
        # Calculate average WQS for active wallets
        active_wallets = [r for r in records if r.status == "ACTIVE" and r.wqs_score is not None]
        if active_wallets:
            avg_wqs = sum(r.wqs_score for r in active_wallets) / len(active_wallets)
            self.wqs_average.set(avg_wqs)
        
        # Record WQS distribution
        for record in records:
            if record.wqs_score is not None:
                self.wqs_distribution.observe(record.wqs_score)
    
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
        
        # Reset all archetype gauges
        archetypes = ["SNIPER", "SWING", "SCALPER", "INSIDER", "WHALE"]
        statuses = ["ACTIVE", "CANDIDATE", "REJECTED"]
        
        for archetype in archetypes:
            for status in statuses:
                self.wallets_by_archetype.labels(archetype=archetype, status=status).set(0)
        
        # Count wallets by archetype and status
        for record in records:
            archetype = record.archetype or "UNKNOWN"
            status = record.status
            if archetype in archetypes:
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


# Global metrics instance
_metrics_instance: Optional[ScoutMetrics] = None


def get_metrics() -> Optional[ScoutMetrics]:
    """Get or create global metrics instance."""
    global _metrics_instance
    
    if _metrics_instance is None:
        port = int(os.getenv("SCOUT_METRICS_PORT", "8081"))
        _metrics_instance = ScoutMetrics(port=port)
        
        # Auto-start if enabled
        if os.getenv("SCOUT_METRICS_ENABLED", "false").lower() == "true":
            _metrics_instance.start_server()
    
    return _metrics_instance
