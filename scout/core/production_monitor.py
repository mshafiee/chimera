"""
Production Monitoring and Alerting System

This module implements comprehensive monitoring and alerting for production readiness:
- Health checks and system monitoring
- Performance metrics tracking
- Anomaly detection
- Alert management
- Incident response automation
- Production readiness validation

Features:
- Real-time health monitoring
- Performance tracking
- Resource usage monitoring
- Automated alerting
- Incident response
- Production readiness checks
"""

import os
import json
import time
import psutil
import logging
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Tuple, Any, Callable
from dataclasses import dataclass, asdict
from enum import Enum
import threading

from .db import get_connection

logger = logging.getLogger(__name__)


class HealthStatus(Enum):
    """Health status levels."""

    HEALTHY = "healthy"
    DEGRADED = "degraded"
    CRITICAL = "critical"
    DOWN = "down"


class AlertSeverity(Enum):
    """Alert severity levels."""

    INFO = "info"
    WARNING = "warning"
    ERROR = "error"
    CRITICAL = "critical"


class MonitorType(Enum):
    """Types of monitors."""

    API_HEALTH = "api_health"
    RESOURCE_USAGE = "resource_usage"
    PERFORMANCE = "performance"
    DATA_QUALITY = "data_quality"
    BUSINESS_LOGIC = "business_logic"


@dataclass
class HealthCheck:
    """Health check result."""

    name: str
    status: HealthStatus
    message: str
    timestamp: float
    details: Dict[str, Any]
    response_time_ms: Optional[float] = None

    def __post_init__(self):
        if self.timestamp == 0:
            self.timestamp = time.time()

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary."""
        return {
            'name': self.name,
            'status': self.status.value,
            'message': self.message,
            'timestamp': self.timestamp,
            'details': self.details,
            'response_time_ms': self.response_time_ms,
        }


@dataclass
class Alert:
    """Alert notification."""

    id: str
    severity: AlertSeverity
    title: str
    message: str
    timestamp: float
    source: str
    resolved: bool = False
    resolution_timestamp: Optional[float] = None
    details: Dict[str, Any] = None

    def __post_init__(self):
        if self.timestamp == 0:
            self.timestamp = time.time()
        if self.details is None:
            self.details = {}

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary."""
        return {
            'id': self.id,
            'severity': self.severity.value,
            'title': self.title,
            'message': self.message,
            'timestamp': self.timestamp,
            'source': self.source,
            'resolved': self.resolved,
            'resolution_timestamp': self.resolution_timestamp,
            'details': self.details,
        }


@dataclass
class PerformanceMetrics:
    """Performance metrics snapshot."""

    timestamp: float
    cpu_percent: float
    memory_percent: float
    memory_used_mb: float
    disk_usage_percent: float
    active_threads: int
    open_files: int
    network_connections: int

    # Application-specific metrics
    wallets_analyzed: int = 0
    requests_made: int = 0
    cache_hit_rate: float = 0.0
    avg_response_time_ms: float = 0.0

    def __post_init__(self):
        if self.timestamp == 0:
            self.timestamp = time.time()

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary."""
        return asdict(self)


@dataclass
class GrowthMetrics:
    """
    Growth tracking metrics (Phase 6).

    Tracks progress toward $200 → $1000 target with velocity and projections.
    """

    timestamp: float
    current_capital: float
    target_capital: float
    starting_capital: float

    # ROI metrics
    roi_daily: float = 0.0
    roi_weekly: float = 0.0
    roi_monthly: float = 0.0

    # Growth velocity
    growth_rate_daily: float = 0.0  # Daily growth rate (%)
    growth_rate_weekly: float = 0.0  # Weekly growth rate (%)
    growth_rate_monthly: float = 0.0  # Monthly growth rate (%)

    # Target projections
    days_to_target: Optional[float] = None  # Estimated days to reach target
    date_to_target: Optional[str] = None  # Estimated date to reach target

    # Capital efficiency
    capital_efficiency: float = 0.0  # ROI per day per dollar
    compounding_effect: float = 0.0  # Bonus from compound growth

    # Wallet performance
    high_wqs_wallets: int = 0  # Count of WQS >= 70 wallets
    avg_wallet_wqs: float = 0.0  # Average WQS across tracked wallets

    # Credit efficiency (Helius)
    credits_used: int = 0
    credits_remaining: int = 0
    credits_roi: float = 0.0  # Return per credit spent

    def __post_init__(self):
        if self.timestamp == 0:
            self.timestamp = time.time()

    @property
    def progress_percentage(self) -> float:
        """Progress toward target as percentage."""
        if self.target_capital <= self.starting_capital:
            return 100.0
        progress = ((self.current_capital - self.starting_capital) /
                   (self.target_capital - self.starting_capital)) * 100
        return max(0.0, min(progress, 100.0))

    @property
    def capital_multiplier(self) -> float:
        """Current capital multiplier from starting point."""
        if self.starting_capital <= 0:
            return 1.0
        return self.current_capital / self.starting_capital

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary."""
        data = asdict(self)
        data['progress_percentage'] = self.progress_percentage
        data['capital_multiplier'] = self.capital_multiplier
        return data


class ProductionMonitor:
    """
    Production monitoring and alerting system.

    Features:
    - Real-time health monitoring
    - Resource usage tracking
    - Performance metrics
    - Alert management
    - Incident response
    """

    def __init__(self):
        """Initialize the production monitor."""
        # Health checks registry
        self._health_checks: Dict[str, Callable] = {}

        # Alert management
        self._alerts: List[Alert] = []
        self._alert_handlers: Dict[AlertSeverity, List[Callable]] = {}

        # Metrics storage
        self._metrics_history: List[PerformanceMetrics] = []
        self._max_metrics_history = 1000

        # Monitoring configuration
        self._check_interval = int(os.getenv("SCOUT_MONITOR_INTERVAL", "60"))  # 60 seconds
        self._alert_cooldown = int(os.getenv("SCOUT_ALERT_COOLDOWN", "300"))  # 5 minutes
        self._last_alert_time: Dict[str, float] = {}

        # Thresholds
        self._thresholds = {
            'cpu_critical': 90.0,
            'cpu_warning': 75.0,
            'memory_critical': 90.0,
            'memory_warning': 75.0,
            'disk_critical': 90.0,
            'disk_warning': 80.0,
        }

        # Monitoring state
        self._monitoring_active = False
        self._monitor_thread: Optional[threading.Thread] = None

        # Database for alerts storage
        self._db_path = os.getenv("SCOUT_MONITOR_DB_PATH", "/tmp/scout_monitoring.db")
        self._init_database()

        logger.info("Production Monitor initialized")

    def _init_database(self):
        """Initialize monitoring database."""
        try:
            os.makedirs(os.path.dirname(self._db_path), exist_ok=True)

            conn = get_connection(self._db_path)
            cursor = conn.cursor()

            # Create alerts table
            cursor.execute("""
                CREATE TABLE IF NOT EXISTS alerts (
                    id TEXT PRIMARY KEY,
                    severity TEXT,
                    title TEXT,
                    message TEXT,
                    timestamp REAL,
                    source TEXT,
                    resolved INTEGER,
                    resolution_timestamp REAL,
                    details TEXT
                )
            """)

            # Create metrics table
            cursor.execute("""
                CREATE TABLE IF NOT EXISTS metrics (
                    timestamp REAL PRIMARY KEY,
                    cpu_percent REAL,
                    memory_percent REAL,
                    memory_used_mb REAL,
                    disk_usage_percent REAL,
                    active_threads INTEGER,
                    open_files INTEGER,
                    network_connections INTEGER,
                    wallets_analyzed INTEGER,
                    requests_made INTEGER,
                    cache_hit_rate REAL,
                    avg_response_time_ms REAL
                )
            """)

            # Create health_checks table
            cursor.execute("""
                CREATE TABLE IF NOT EXISTS health_checks (
                    name TEXT,
                    status TEXT,
                    message TEXT,
                    timestamp REAL,
                    details TEXT,
                    response_time_ms REAL,
                    PRIMARY KEY (name, timestamp)
                )
            """)

            conn.commit()
            conn.close()

            logger.debug(f"Monitoring database initialized: {self._db_path}")
        except Exception as e:
            logger.warning(f"Failed to initialize monitoring database: {e}")

    def register_health_check(self, name: str, check_func: Callable):
        """
        Register a health check function.

        Args:
            name: Health check name
            check_func: Function that returns HealthCheck result
        """
        self._health_checks[name] = check_func
        logger.info(f"Registered health check: {name}")

    def register_alert_handler(self, severity: AlertSeverity, handler: Callable):
        """
        Register an alert handler.

        Args:
            severity: Alert severity to handle
            handler: Handler function
        """
        if severity not in self._alert_handlers:
            self._alert_handlers[severity] = []
        self._alert_handlers[severity].append(handler)
        logger.info(f"Registered alert handler for {severity.value}")

    def run_health_checks(self) -> List[HealthCheck]:
        """Run all registered health checks."""
        results = []

        for name, check_func in self._health_checks.items():
            try:
                start_time = time.time()
                result = check_func()
                response_time = (time.time() - start_time) * 1000

                # Ensure result is a HealthCheck
                if not isinstance(result, HealthCheck):
                    result = HealthCheck(
                        name=name,
                        status=HealthStatus.HEALTHY,
                        message="Check completed",
                        timestamp=time.time(),
                        details={},
                        response_time_ms=response_time
                    )
                else:
                    result.response_time_ms = response_time

                results.append(result)

                # Store in database
                self._store_health_check(result)

            except Exception as e:
                logger.error(f"Health check failed: {name} - {e}")
                results.append(HealthCheck(
                    name=name,
                    status=HealthStatus.DOWN,
                    message=f"Check failed: {str(e)}",
                    timestamp=time.time(),
                    details={'error': str(e)},
                    response_time_ms=None
                ))

        return results

    def collect_metrics(self) -> PerformanceMetrics:
        """Collect current performance metrics."""
        try:
            # System metrics
            cpu_percent = psutil.cpu_percent(interval=1)
            memory = psutil.virtual_memory()
            memory_percent = memory.percent
            memory_used_mb = memory.used / (1024 * 1024)

            disk = psutil.disk_usage('/')
            disk_usage_percent = disk.percent

            # Process metrics
            process = psutil.Process()
            active_threads = process.num_threads()
            open_files = len(process.open_files())

            try:
                network_connections = len(process.connections())
            except psutil.AccessDenied:
                network_connections = 0

            metrics = PerformanceMetrics(
                timestamp=time.time(),
                cpu_percent=cpu_percent,
                memory_percent=memory_percent,
                memory_used_mb=memory_used_mb,
                disk_usage_percent=disk_usage_percent,
                active_threads=active_threads,
                open_files=open_files,
                network_connections=network_connections,
            )

            # Store in history
            self._metrics_history.append(metrics)
            if len(self._metrics_history) > self._max_metrics_history:
                self._metrics_history.pop(0)

            # Store in database
            self._store_metrics(metrics)

            return metrics

        except Exception as e:
            logger.error(f"Failed to collect metrics: {e}")
            return PerformanceMetrics(
                timestamp=time.time(),
                cpu_percent=0,
                memory_percent=0,
                memory_used_mb=0,
                disk_usage_percent=0,
                active_threads=0,
                open_files=0,
                network_connections=0,
            )

    def _store_health_check(self, check: HealthCheck):
        """Store health check result in database."""
        try:
            conn = get_connection(self._db_path)
            cursor = conn.cursor()

            cursor.execute("""
                INSERT OR REPLACE INTO health_checks
                (name, status, message, timestamp, details, response_time_ms)
                VALUES (?, ?, ?, ?, ?, ?)
            """, (
                check.name,
                check.status.value,
                check.message,
                check.timestamp,
                json.dumps(check.details),
                check.response_time_ms
            ))

            conn.commit()
            conn.close()
        except Exception as e:
            logger.debug(f"Failed to store health check: {e}")

    def _store_metrics(self, metrics: PerformanceMetrics):
        """Store metrics in database."""
        try:
            conn = get_connection(self._db_path)
            cursor = conn.cursor()

            cursor.execute("""
                INSERT INTO metrics VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """, (
                metrics.timestamp,
                metrics.cpu_percent,
                metrics.memory_percent,
                metrics.memory_used_mb,
                metrics.disk_usage_percent,
                metrics.active_threads,
                metrics.open_files,
                metrics.network_connections,
                metrics.wallets_analyzed,
                metrics.requests_made,
                metrics.cache_hit_rate,
                metrics.avg_response_time_ms,
            ))

            conn.commit()
            conn.close()
        except Exception as e:
            logger.debug(f"Failed to store metrics: {e}")

    def create_alert(self, severity: AlertSeverity, title: str, message: str,
                    source: str = "scout", details: Dict[str, Any] = None) -> Alert:
        """
        Create an alert.

        Args:
            severity: Alert severity
            title: Alert title
            message: Alert message
            source: Alert source
            details: Additional details

        Returns:
            Created alert
        """
        alert_id = f"{source}_{int(time.time())}_{severity.value}"

        alert = Alert(
            id=alert_id,
            severity=severity,
            title=title,
            message=message,
            timestamp=time.time(),
            source=source,
            details=details or {}
        )

        self._alerts.append(alert)

        # Store in database
        self._store_alert(alert)

        # Trigger handlers
        self._trigger_alert_handlers(alert)

        logger.warning(f"Alert created: [{severity.value.upper()}] {title}")

        return alert

    def _store_alert(self, alert: Alert):
        """Store alert in database."""
        try:
            conn = get_connection(self._db_path)
            cursor = conn.cursor()

            cursor.execute("""
                INSERT OR REPLACE INTO alerts
                (id, severity, title, message, timestamp, source, resolved, resolution_timestamp, details)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            """, (
                alert.id,
                alert.severity.value,
                alert.title,
                alert.message,
                alert.timestamp,
                alert.source,
                int(alert.resolved),
                alert.resolution_timestamp,
                json.dumps(alert.details)
            ))

            conn.commit()
            conn.close()
        except Exception as e:
            logger.debug(f"Failed to store alert: {e}")

    def _trigger_alert_handlers(self, alert: Alert):
        """Trigger alert handlers for severity."""
        handlers = self._alert_handlers.get(alert.severity, [])

        for handler in handlers:
            try:
                handler(alert)
            except Exception as e:
                logger.error(f"Alert handler failed: {e}")

    def check_thresholds(self, metrics: PerformanceMetrics) -> List[Alert]:
        """Check metrics against thresholds and create alerts."""
        alerts = []

        # CPU checks
        if metrics.cpu_percent >= self._thresholds['cpu_critical']:
            if self._can_alert('cpu_critical'):
                alerts.append(self.create_alert(
                    severity=AlertSeverity.CRITICAL,
                    title="CPU Critical",
                    message=f"CPU usage at {metrics.cpu_percent:.1f}%",
                    source="system_monitor",
                    details={'cpu_percent': metrics.cpu_percent}
                ))
        elif metrics.cpu_percent >= self._thresholds['cpu_warning']:
            if self._can_alert('cpu_warning'):
                alerts.append(self.create_alert(
                    severity=AlertSeverity.WARNING,
                    title="CPU High",
                    message=f"CPU usage at {metrics.cpu_percent:.1f}%",
                    source="system_monitor",
                    details={'cpu_percent': metrics.cpu_percent}
                ))

        # Memory checks
        if metrics.memory_percent >= self._thresholds['memory_critical']:
            if self._can_alert('memory_critical'):
                alerts.append(self.create_alert(
                    severity=AlertSeverity.CRITICAL,
                    title="Memory Critical",
                    message=f"Memory usage at {metrics.memory_percent:.1f}%",
                    source="system_monitor",
                    details={'memory_percent': metrics.memory_percent}
                ))
        elif metrics.memory_percent >= self._thresholds['memory_warning']:
            if self._can_alert('memory_warning'):
                alerts.append(self.create_alert(
                    severity=AlertSeverity.WARNING,
                    title="Memory High",
                    message=f"Memory usage at {metrics.memory_percent:.1f}%",
                    source="system_monitor",
                    details={'memory_percent': metrics.memory_percent}
                ))

        # Disk checks
        if metrics.disk_usage_percent >= self._thresholds['disk_critical']:
            if self._can_alert('disk_critical'):
                alerts.append(self.create_alert(
                    severity=AlertSeverity.CRITICAL,
                    title="Disk Critical",
                    message=f"Disk usage at {metrics.disk_usage_percent:.1f}%",
                    source="system_monitor",
                    details={'disk_usage_percent': metrics.disk_usage_percent}
                ))
        elif metrics.disk_usage_percent >= self._thresholds['disk_warning']:
            if self._can_alert('disk_warning'):
                alerts.append(self.create_alert(
                    severity=AlertSeverity.WARNING,
                    title="Disk High",
                    message=f"Disk usage at {metrics.disk_usage_percent:.1f}%",
                    source="system_monitor",
                    details={'disk_usage_percent': metrics.disk_usage_percent}
                ))

        return alerts

    def _can_alert(self, alert_key: str) -> bool:
        """Check if alert can be sent (cooldown check)."""
        last_alert = self._last_alert_time.get(alert_key, 0)
        return (time.time() - last_alert) >= self._alert_cooldown

    def _monitoring_loop(self):
        """Main monitoring loop."""
        while self._monitoring_active:
            try:
                # Run health checks
                self.run_health_checks()

                # Collect metrics
                metrics = self.collect_metrics()

                # Check thresholds
                self.check_thresholds(metrics)

                # Sleep for next iteration
                time.sleep(self._check_interval)

            except Exception as e:
                logger.error(f"Monitoring loop error: {e}")
                time.sleep(self._check_interval)

    def start_monitoring(self):
        """Start monitoring in background thread."""
        if self._monitoring_active:
            logger.warning("Monitoring already active")
            return

        self._monitoring_active = True
        self._monitor_thread = threading.Thread(target=self._monitoring_loop, daemon=True)
        self._monitor_thread.start()

        logger.info(f"Production monitoring started (interval: {self._check_interval}s)")

    def stop_monitoring(self):
        """Stop monitoring."""
        self._monitoring_active = False

        if self._monitor_thread:
            self._monitor_thread.join(timeout=5)

        logger.info("Production monitoring stopped")

    def get_health_status(self) -> Dict[str, Any]:
        """Get current health status."""
        health_results = self.run_health_checks()
        metrics = self.collect_metrics()

        # Determine overall status
        critical_count = sum(1 for r in health_results if r.status == HealthStatus.CRITICAL)
        down_count = sum(1 for r in health_results if r.status == HealthStatus.DOWN)

        if critical_count > 0 or down_count > 0:
            overall_status = HealthStatus.CRITICAL
        elif any(r.status == HealthStatus.DEGRADED for r in health_results):
            overall_status = HealthStatus.DEGRADED
        else:
            overall_status = HealthStatus.HEALTHY

        return {
            'overall_status': overall_status.value,
            'timestamp': time.time(),
            'health_checks': [r.to_dict() for r in health_results],
            'metrics': metrics.to_dict(),
            'active_alerts': len([a for a in self._alerts if not a.resolved]),
        }

    def get_recent_alerts(self, limit: int = 50) -> List[Alert]:
        """Get recent alerts."""
        # Get from database
        conn = None
        try:
            conn = get_connection(self._db_path)
            cursor = conn.cursor()

            cursor.execute("""
                SELECT id, severity, title, message, timestamp, source, resolved, resolution_timestamp, details
                FROM alerts
                ORDER BY timestamp DESC
                LIMIT %s
            """, (limit,))

            alerts = []
            for row in cursor.fetchall():
                alert = Alert(
                    id=row[0],
                    severity=AlertSeverity(row[1]),
                    title=row[2],
                    message=row[3],
                    timestamp=row[4],
                    source=row[5],
                    resolved=bool(row[6]),
                    resolution_timestamp=row[7],
                    details=json.loads(row[8]) if row[8] else {}
                )
                alerts.append(alert)

            return alerts

        except Exception as e:
            logger.error(f"Failed to get recent alerts: {e}")
            return []
        finally:
            if conn:
                conn.close()

    def print_status_report(self):
        """Print comprehensive status report."""
        status = self.get_health_status()

        print("\n" + "="*70)
        print("PRODUCTION MONITOR - STATUS REPORT")
        print("="*70)

        print(f"\nOverall Status: {status['overall_status'].upper()}")
        print(f"Timestamp: {datetime.fromtimestamp(status['timestamp']).strftime('%Y-%m-%d %H:%M:%S')}")

        # Health checks
        print(f"\nHealth Checks ({len(status['health_checks'])} checks):")
        for check in status['health_checks']:
            status_symbol = "✓" if check['status'] == 'healthy' else "✗"
            response_time = f" ({check['response_time_ms']:.1f}ms)" if check['response_time_ms'] else ""
            print(f"  {status_symbol} {check['name']}: {check['status'].upper()}{response_time}")
            if check['status'] != 'healthy':
                print(f"      Message: {check['message']}")

        # Metrics
        metrics = status['metrics']
        print("\nSystem Metrics:")
        print(f"  CPU: {metrics['cpu_percent']:.1f}%")
        print(f"  Memory: {metrics['memory_percent']:.1f}% ({metrics['memory_used_mb']:.1f} MB)")
        print(f"  Disk: {metrics['disk_usage_percent']:.1f}%")
        print(f"  Threads: {metrics['active_threads']}")
        print(f"  Open files: {metrics['open_files']}")

        # Active alerts
        print(f"\nActive Alerts: {status['active_alerts']}")
        recent_alerts = self.get_recent_alerts(10)
        if recent_alerts:
            print("Recent alerts:")
            for alert in recent_alerts[:5]:
                status_symbol = "!" if not alert.resolved else "✓"
                print(f"  {status_symbol} [{alert.severity.value.upper()}] {alert.title}")
                print(f"      {alert.message}")

        print("="*70 + "\n")

    def validate_production_readiness(self) -> Tuple[bool, List[str]]:
        """
        Validate production readiness.

        Returns:
            Tuple of (is_ready, list_of_issues)
        """
        issues = []

        # Check health status
        status = self.get_health_status()
        if status['overall_status'] != HealthStatus.HEALTHY.value:
            issues.append(f"System health is {status['overall_status']}")

        # Check for critical alerts
        critical_alerts = [a for a in self.get_recent_alerts(100)
                           if a.severity == AlertSeverity.CRITICAL and not a.resolved]
        if critical_alerts:
            issues.append(f"{len(critical_alerts)} unresolved critical alerts")

        # Check resource usage
        metrics = status['metrics']
        if metrics['cpu_percent'] > 80:
            issues.append(f"High CPU usage: {metrics['cpu_percent']:.1f}%")
        if metrics['memory_percent'] > 80:
            issues.append(f"High memory usage: {metrics['memory_percent']:.1f}%")
        if metrics['disk_usage_percent'] > 85:
            issues.append(f"High disk usage: {metrics['disk_usage_percent']:.1f}%")

        return (len(issues) == 0, issues)

    def shutdown(self):
        """Shutdown monitoring system."""
        self.stop_monitoring()
        logger.info("Production Monitor shut down")


class GrowthTracker:
    """
    Growth tracking system for $200 → $1000 target (Phase 6).

    Features:
    - Growth velocity tracking (daily/weekly/monthly ROI)
    - Days to target estimation
    - Capital change alerts
    - Growth trajectory dashboard
    """

    def __init__(
        self,
        starting_capital: float = 200.0,
        target_capital: float = 1000.0,
        db_path: Optional[str] = None,
    ):
        """
        Initialize growth tracker.

        Args:
            starting_capital: Initial capital (default $200)
            target_capital: Target capital (default $1000)
            db_path: Path to growth tracking database
        """
        self.starting_capital = starting_capital
        self.target_capital = target_capital
        self.current_capital = starting_capital

        # Database for growth tracking
        self._db_path = db_path or os.getenv(
            "SCOUT_GROWTH_DB_PATH", "/tmp/scout_growth.db"
        )
        self._init_database()

        # Alert thresholds
        self._alert_thresholds = {
            'daily_roi_critical': -10.0,  # Alert if daily ROI < -10%
            'daily_roi_target': 5.0,  # Target daily ROI
            'weekly_roi_target': 25.0,  # Target weekly ROI
            'monthly_roi_target': 40.0,  # Target monthly ROI
        }

        # Load latest capital from database
        self._load_latest_state()

        logger.info(f"Growth Tracker initialized: ${starting_capital} → ${target_capital}")

    def _init_database(self):
        """Initialize growth tracking database."""
        try:
            os.makedirs(os.path.dirname(self._db_path), exist_ok=True)

            conn = get_connection(self._db_path)
            cursor = conn.cursor()

            # Create growth_history table
            cursor.execute("""
                CREATE TABLE IF NOT EXISTS growth_history (
                    timestamp REAL PRIMARY KEY,
                    current_capital REAL,
                    roi_daily REAL,
                    roi_weekly REAL,
                    roi_monthly REAL,
                    growth_rate_daily REAL,
                    growth_rate_weekly REAL,
                    growth_rate_monthly REAL,
                    days_to_target REAL,
                    high_wqs_wallets INTEGER,
                    avg_wallet_wqs REAL,
                    credits_used INTEGER,
                    credits_remaining INTEGER,
                    credits_roi REAL
                )
            """)

            # Create capital_events table for significant changes
            cursor.execute("""
                CREATE TABLE IF NOT EXISTS capital_events (
                    id TEXT PRIMARY KEY,
                    timestamp REAL,
                    event_type TEXT,
                    old_capital REAL,
                    new_capital REAL,
                    change_amount REAL,
                    change_percent REAL,
                    description TEXT,
                    metadata TEXT
                )
            """)

            # Create alerts table
            cursor.execute("""
                CREATE TABLE IF NOT EXISTS growth_alerts (
                    id TEXT PRIMARY KEY,
                    timestamp REAL,
                    alert_type TEXT,
                    severity TEXT,
                    message TEXT,
                    details TEXT
                )
            """)

            conn.commit()
            
            # Enable WAL mode for better concurrency
            cursor.execute("PRAGMA journal_mode=WAL")
            cursor.execute("PRAGMA busy_timeout=5000")
            conn.commit()
            conn.close()

            logger.debug(f"Growth database initialized: {self._db_path}")
        except Exception as e:
            logger.warning(f"Failed to initialize growth database: {e}")

    def _load_latest_state(self):
        """Load latest capital state from database."""
        try:
            conn = get_connection(self._db_path)
            cursor = conn.cursor()

            cursor.execute("""
                SELECT current_capital FROM growth_history
                ORDER BY timestamp DESC LIMIT 1
            """)

            row = cursor.fetchone()
            if row:
                self.current_capital = row[0]
                logger.info(f"Loaded current capital: ${self.current_capital:.2f}")

            conn.close()
        except Exception as e:
            logger.debug(f"Failed to load latest state: {e}")

    def record_capital(
        self,
        new_capital: float,
        event_type: str = "update",
        description: str = "",
        metadata: Dict[str, Any] = None,
    ) -> GrowthMetrics:
        """
        Record a capital change and calculate growth metrics.

        Args:
            new_capital: New capital amount
            event_type: Type of event (update, trade, deposit, withdrawal)
            description: Event description
            metadata: Additional event metadata

        Returns:
            GrowthMetrics snapshot
        """
        old_capital = self.current_capital
        change_amount = new_capital - old_capital
        change_percent = (change_amount / old_capital * 100) if old_capital > 0 else 0.0

        self.current_capital = new_capital

        # Calculate growth metrics
        metrics = self._calculate_growth_metrics()

        # Store in database
        self._store_growth_snapshot(metrics)

        # Record capital event if significant change
        if abs(change_percent) >= 1.0:  # Record changes >= 1%
            self._record_capital_event(
                event_type, old_capital, new_capital, description, metadata
            )

        logger.info(
            f"Capital recorded: ${old_capital:.2f} → ${new_capital:.2f} "
            f"({change_percent:+.2f}%)"
        )

        return metrics

    def _calculate_growth_metrics(self) -> GrowthMetrics:
        """Calculate current growth metrics from history."""
        now = time.time()
        day_ago = now - 86400
        week_ago = now - (7 * 86400)
        month_ago = now - (30 * 86400)

        try:
            conn = get_connection(self._db_path)
            cursor = conn.cursor()

            # Get capital at different time periods
            cursor.execute("""
                SELECT current_capital, timestamp FROM growth_history
                WHERE timestamp >= ? ORDER BY timestamp ASC LIMIT 1
            """, (month_ago,))
            month_row = cursor.fetchone()
            capital_month_ago = month_row[0] if month_row else self.starting_capital

            cursor.execute("""
                SELECT current_capital, timestamp FROM growth_history
                WHERE timestamp >= ? ORDER BY timestamp ASC LIMIT 1
            """, (week_ago,))
            week_row = cursor.fetchone()
            capital_week_ago = week_row[0] if week_row else self.starting_capital

            cursor.execute("""
                SELECT current_capital, timestamp FROM growth_history
                WHERE timestamp >= ? ORDER BY timestamp ASC LIMIT 1
            """, (day_ago,))
            day_row = cursor.fetchone()
            capital_day_ago = day_row[0] if day_row else self.starting_capital

            conn.close()

            # Calculate ROI
            roi_daily = ((self.current_capital - capital_day_ago) /
                       capital_day_ago * 100) if capital_day_ago > 0 else 0.0
            roi_weekly = ((self.current_capital - capital_week_ago) /
                        capital_week_ago * 100) if capital_week_ago > 0 else 0.0
            roi_monthly = ((self.current_capital - capital_month_ago) /
                         capital_month_ago * 100) if capital_month_ago > 0 else 0.0

            # Calculate growth rates (compound annual growth rate style)
            growth_rate_daily = roi_daily  # Daily rate
            growth_rate_weekly = roi_weekly / 7 if roi_weekly != 0 else 0.0  # Per day
            growth_rate_monthly = roi_monthly / 30 if roi_monthly != 0 else 0.0  # Per day

            # Estimate days to target
            days_to_target = self._estimate_days_to_target(growth_rate_daily)

            # Capital efficiency
            capital_efficiency = (roi_daily / max(0.01, self.current_capital))
            compounding_effect = roi_monthly - (roi_daily * 30)  # Bonus from compounding

            return GrowthMetrics(
                timestamp=now,
                current_capital=self.current_capital,
                target_capital=self.target_capital,
                starting_capital=self.starting_capital,
                roi_daily=roi_daily,
                roi_weekly=roi_weekly,
                roi_monthly=roi_monthly,
                growth_rate_daily=growth_rate_daily,
                growth_rate_weekly=growth_rate_weekly,
                growth_rate_monthly=growth_rate_monthly,
                days_to_target=days_to_target,
                date_to_target=self._calculate_target_date(days_to_target),
                capital_efficiency=capital_efficiency,
                compounding_effect=compounding_effect,
            )

        except Exception as e:
            logger.error(f"Failed to calculate growth metrics: {e}")
            return GrowthMetrics(
                timestamp=now,
                current_capital=self.current_capital,
                target_capital=self.target_capital,
                starting_capital=self.starting_capital,
            )

    def _estimate_days_to_target(self, daily_growth_rate: float) -> Optional[float]:
        """
        Estimate days to reach target capital.

        Uses compound growth formula: target = current * (1 + rate)^days

        Args:
            daily_growth_rate: Daily growth rate as decimal (e.g., 0.05 for 5%)

        Returns:
            Estimated days, or None if growth rate is non-positive
        """
        if daily_growth_rate <= 0.001:  # Less than 0.1% daily growth
            return None

        remaining_ratio = self.target_capital / self.current_capital

        # days = log(remaining_ratio) / log(1 + daily_rate)
        import math
        try:
            days = math.log(remaining_ratio) / math.log(1 + daily_growth_rate)
            return max(0, days)
        except (ValueError, ZeroDivisionError):
            return None

    def _calculate_target_date(self, days_to_target: Optional[float]) -> Optional[str]:
        """Calculate estimated date to reach target."""
        if days_to_target is None:
            return None

        target_date = datetime.now() + timedelta(days=days_to_target)
        return target_date.strftime("%Y-%m-%d")

    def _store_growth_snapshot(self, metrics: GrowthMetrics):
        """Store growth metrics snapshot in database."""
        conn = None
        try:
            conn = get_connection(self._db_path)
            cursor = conn.cursor()

            cursor.execute("""
                INSERT INTO growth_history VALUES
                (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """, (
                metrics.timestamp,
                metrics.current_capital,
                metrics.roi_daily,
                metrics.roi_weekly,
                metrics.roi_monthly,
                metrics.growth_rate_daily,
                metrics.growth_rate_weekly,
                metrics.growth_rate_monthly,
                metrics.days_to_target,
                metrics.high_wqs_wallets,
                metrics.avg_wallet_wqs,
                metrics.credits_used,
                metrics.credits_remaining,
                metrics.credits_roi,
            ))

            conn.commit()
            logger.debug(f"Successfully stored growth snapshot for capital={metrics.current_capital}")
        except Exception as e:
            logger.debug(f"Failed to store growth snapshot: {e}")
            logger.debug(f"Exception type: {type(e).__name__}")
        finally:
            if conn:
                conn.close()

    def _record_capital_event(
        self,
        event_type: str,
        old_capital: float,
        new_capital: float,
        description: str,
        metadata: Dict[str, Any] = None,
    ):
        """Record a significant capital event."""
        conn = None
        try:
            conn = get_connection(self._db_path)
            cursor = conn.cursor()

            event_id = f"{event_type}_{int(time.time())}_{time.time_ns()}"
            change_amount = new_capital - old_capital
            change_percent = (change_amount / old_capital * 100) if old_capital > 0 else 0.0

            cursor.execute("""
                INSERT INTO capital_events VALUES
                (?, ?, ?, ?, ?, ?, ?, ?, ?)
            """, (
                event_id,
                time.time(),
                event_type,
                old_capital,
                new_capital,
                change_amount,
                change_percent,
                description,
                json.dumps(metadata or {}),
            ))

            conn.commit()

            # Check for alert conditions
            self._check_capital_alerts(change_amount, change_percent)
        except Exception as e:
            logger.debug(f"Failed to record capital event: {e}")
        finally:
            if conn:
                conn.close()

    def _check_capital_alerts(self, change_amount: float, change_percent: float):
        """Check if capital change warrants an alert."""
        alerts = []

        # Significant loss alert (>10% drop)
        if change_percent < -10.0:
            alerts.append({
                'type': 'significant_loss',
                'severity': 'critical',
                'message': f"Capital dropped {change_percent:.2f}% (-${abs(change_amount):.2f})"
            })

        # Significant gain alert (>10% gain)
        elif change_percent > 10.0:
            alerts.append({
                'type': 'significant_gain',
                'severity': 'info',
                'message': f"Capital increased {change_percent:.2f}% (+${change_amount:.2f})"
            })

        # Target reached alert
        if self.current_capital >= self.target_capital:
            alerts.append({
                'type': 'target_reached',
                'severity': 'info',
                'message': f"Target capital reached! ${self.current_capital:.2f}"
            })

        # Store alerts
        for alert in alerts:
            self._store_alert(alert['type'], alert['severity'], alert['message'])

    def _store_alert(self, alert_type: str, severity: str, message: str):
        """Store growth alert."""
        conn = None
        try:
            conn = get_connection(self._db_path)
            cursor = conn.cursor()

            alert_id = f"{alert_type}_{int(time.time())}_{time.time_ns()}"

            cursor.execute("""
                INSERT INTO growth_alerts VALUES (?, ?, ?, ?, ?, ?)
            """, (
                alert_id,
                time.time(),
                alert_type,
                severity,
                message,
                "{}",
            ))

            conn.commit()

            logger.warning(f"[Growth Alert] {severity.upper()}: {message}")
        except Exception as e:
            logger.debug(f"Failed to store alert: {e}")
        finally:
            if conn:
                conn.close()

    def get_current_metrics(self) -> GrowthMetrics:
        """Get current growth metrics."""
        return self._calculate_growth_metrics()

    def get_growth_history(self, days: int = 30) -> List[GrowthMetrics]:
        """
        Get growth history for specified number of days.

        Args:
            days: Number of days of history to retrieve

        Returns:
            List of GrowthMetrics snapshots
        """
        try:
            conn = get_connection(self._db_path)
            cursor = conn.cursor()

            cutoff_time = time.time() - (days * 86400)

            cursor.execute("""
                SELECT * FROM growth_history
                WHERE timestamp >= ?
                ORDER BY timestamp ASC
            """, (cutoff_time,))

            history = []
            for row in cursor.fetchall():
                history.append(GrowthMetrics(
                    timestamp=row[0],
                    current_capital=row[1],
                    target_capital=self.target_capital,
                    starting_capital=self.starting_capital,
                    roi_daily=row[2],
                    roi_weekly=row[3],
                    roi_monthly=row[4],
                    growth_rate_daily=row[5],
                    growth_rate_weekly=row[6],
                    growth_rate_monthly=row[7],
                    days_to_target=row[8],
                    high_wqs_wallets=row[9],
                    avg_wallet_wqs=row[10],
                    credits_used=row[11],
                    credits_remaining=row[12],
                    credits_roi=row[13],
                ))

            conn.close()
            return history

        except Exception as e:
            logger.error(f"Failed to get growth history: {e}")
            return []

    def print_growth_dashboard(self):
        """Print comprehensive growth dashboard."""
        metrics = self.get_current_metrics()

        print("\n" + "="*70)
        print("GROWTH TRACKING - $200 → $1000 TARGET")
        print("="*70)

        print("\nCapital Status:")
        print(f"  Current:   ${metrics.current_capital:.2f}")
        print(f"  Target:    ${metrics.target_capital:.2f}")
        print(f"  Progress:  {metrics.progress_percentage:.1f}%")
        print(f"  Multiple:  {metrics.capital_multiplier:.2f}x")

        print("\nROI Performance:")
        print(f"  Daily:   {metrics.roi_daily:+.2f}%")
        print(f"  Weekly:  {metrics.roi_weekly:+.2f}%")
        print(f"  Monthly: {metrics.roi_monthly:+.2f}%")

        print("\nGrowth Velocity:")
        print(f"  Daily Rate:   {metrics.growth_rate_daily:.2f}%")
        print(f"  Weekly Rate:  {metrics.growth_rate_weekly:.2f}%/day")
        print(f"  Monthly Rate: {metrics.growth_rate_monthly:.2f}%/day")

        if metrics.days_to_target:
            print("\nTarget Projection:")
            print(f"  Days to Target:  {metrics.days_to_target:.1f}")
            if metrics.date_to_target:
                print(f"  Target Date:     {metrics.date_to_target}")
        else:
            print("\nTarget Projection: Insufficient growth rate")

        print("\nCapital Efficiency:")
        print(f"  Efficiency:      {metrics.capital_efficiency:.4f}% per $/day")
        print(f"  Compounding:     {metrics.compounding_effect:+.2f}%")

        print("="*70 + "\n")

    def get_growth_summary(self) -> Dict[str, Any]:
        """Get growth summary for API responses."""
        metrics = self.get_current_metrics()

        return {
            'current_capital': metrics.current_capital,
            'target_capital': metrics.target_capital,
            'starting_capital': metrics.starting_capital,
            'progress_percentage': metrics.progress_percentage,
            'capital_multiplier': metrics.capital_multiplier,
            'roi_daily': metrics.roi_daily,
            'roi_weekly': metrics.roi_weekly,
            'roi_monthly': metrics.roi_monthly,
            'days_to_target': metrics.days_to_target,
            'date_to_target': metrics.date_to_target,
        }


# Global singleton instances
_monitor: Optional[ProductionMonitor] = None
_growth_tracker: Optional[GrowthTracker] = None


def get_production_monitor() -> ProductionMonitor:
    """Get the global production monitor singleton."""
    global _monitor

    if _monitor is None:
        _monitor = ProductionMonitor()

    return _monitor


def get_growth_tracker(
    starting_capital: float = 200.0,
    target_capital: float = 1000.0,
) -> GrowthTracker:
    """
    Get the global growth tracker singleton.

    Args:
        starting_capital: Initial capital (default $200)
        target_capital: Target capital (default $1000)

    Returns:
        GrowthTracker instance
    """
    global _growth_tracker

    if _growth_tracker is None:
        _growth_tracker = GrowthTracker(
            starting_capital=starting_capital,
            target_capital=target_capital,
        )

    return _growth_tracker


if __name__ == "__main__":
    # Test the monitor
    monitor = get_production_monitor()

    # Register some health checks
    def test_check():
        return HealthCheck(
            name="test_check",
            status=HealthStatus.HEALTHY,
            message="Test check passed",
            timestamp=time.time(),
            details={},
        )

    monitor.register_health_check("test", test_check)

    # Run status check
    monitor.print_status_report()

    # Validate production readiness
    is_ready, issues = monitor.validate_production_readiness()
    print(f"Production Ready: {is_ready}")
    if issues:
        print("Issues:")
        for issue in issues:
            print(f"  - {issue}")

    # Test growth tracker (Phase 6)
    print("\n" + "="*70)
    print("TESTING GROWTH TRACKER (Phase 6)")
    print("="*70)

    growth_tracker = get_growth_tracker(starting_capital=200.0, target_capital=1000.0)

    # Simulate growth over time
    print("\nSimulating capital growth:")
    capitals = [200.0, 225.0, 260.0, 310.0, 380.0, 480.0, 620.0, 810.0, 1000.0]
    for i, capital in enumerate(capitals, 1):
        print(f"  Day {i}: ${capital:.2f}")
        growth_tracker.record_capital(capital, event_type="simulation")

    # Print growth dashboard
    growth_tracker.print_growth_dashboard()

    # Get growth summary
    summary = growth_tracker.get_growth_summary()
    print("\nGrowth Summary:")
    for key, value in summary.items():
        print(f"  {key}: {value}")

    monitor.shutdown()