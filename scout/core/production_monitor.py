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
import asyncio
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Tuple, Any, Callable
from dataclasses import dataclass, asdict
from enum import Enum
from pathlib import Path
import threading
import sqlite3

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

            conn = sqlite3.connect(self._db_path)
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
                        message=f"Check completed",
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
            conn = sqlite3.connect(self._db_path)
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
            conn = sqlite3.connect(self._db_path)
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
            conn = sqlite3.connect(self._db_path)
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
                health_results = self.run_health_checks()

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
        try:
            conn = sqlite3.connect(self._db_path)
            cursor = conn.cursor()

            cursor.execute("""
                SELECT id, severity, title, message, timestamp, source, resolved, resolution_timestamp, details
                FROM alerts
                ORDER BY timestamp DESC
                LIMIT ?
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

            conn.close()
            return alerts

        except Exception as e:
            logger.error(f"Failed to get recent alerts: {e}")
            return []

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
        print(f"\nSystem Metrics:")
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


# Global singleton instance
_monitor: Optional[ProductionMonitor] = None


def get_production_monitor() -> ProductionMonitor:
    """Get the global production monitor singleton."""
    global _monitor

    if _monitor is None:
        _monitor = ProductionMonitor()

    return _monitor


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

    monitor.shutdown()