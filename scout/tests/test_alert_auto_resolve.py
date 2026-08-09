"""
Tests for alert auto-resolution in ProductionMonitor.

2026-08-09: alerts were created when a threshold breached but never resolved
when it recovered, so `validate_production_readiness` always reported the
accumulated historical count (e.g. "43 unresolved critical alerts" from a
single CPU spike on an otherwise healthy host).
"""

import os
import tempfile

from core.production_monitor import (
    PerformanceMetrics,
    ProductionMonitor,
)


def _metrics(cpu: float, mem: float = 30.0, disk: float = 30.0) -> PerformanceMetrics:
    return PerformanceMetrics(
        timestamp=0.0,
        cpu_percent=cpu,
        memory_percent=mem,
        memory_used_mb=512.0,
        disk_usage_percent=disk,
        active_threads=5,
        open_files=10,
        network_connections=3,
    )


def _fresh_monitor() -> ProductionMonitor:
    """Monitor with instant debounce/cooldown so tests are deterministic."""
    tmp = tempfile.TemporaryDirectory()
    monitor = ProductionMonitor()
    monitor._db_path = os.path.join(tmp.name, "alerts.db")
    monitor._sustained_samples = 1
    monitor._alert_cooldown = 0
    return monitor


def test_critical_alert_resolves_when_condition_clears():
    monitor = _fresh_monitor()
    # Breach CPU critical -> critical + warning alerts fire
    monitor.check_thresholds(_metrics(cpu=95.0))
    unresolved = [a for a in monitor._alerts if not a.resolved]
    assert {a.title for a in unresolved} == {"CPU Critical", "CPU High"}

    # Condition clears -> alerts auto-resolve on the next check
    monitor.check_thresholds(_metrics(cpu=10.0))
    unresolved = [a for a in monitor._alerts if not a.resolved]
    assert unresolved == [], "recovered condition must resolve the alerts"
    resolved = [a for a in monitor._alerts if a.title in ("CPU Critical", "CPU High")]
    assert resolved and all(a.resolved for a in resolved)


def test_alert_stays_unresolved_while_breached():
    monitor = _fresh_monitor()
    monitor.check_thresholds(_metrics(cpu=95.0))
    # Still breached -> stays unresolved (no false resolution while high)
    monitor.check_thresholds(_metrics(cpu=97.0))
    unresolved = [a for a in monitor._alerts if not a.resolved]
    assert unresolved and unresolved[0].title == "CPU Critical"


def test_warning_alert_resolves_too():
    monitor = _fresh_monitor()
    # Between warning and critical thresholds
    monitor.check_thresholds(_metrics(cpu=75.0))
    unresolved = [a for a in monitor._alerts if not a.resolved]
    assert unresolved and unresolved[0].title == "CPU High"

    monitor.check_thresholds(_metrics(cpu=10.0))
    unresolved = [a for a in monitor._alerts if not a.resolved]
    assert unresolved == []


def test_memory_and_disk_alert_resolution():
    monitor = _fresh_monitor()
    monitor.check_thresholds(_metrics(cpu=10.0, mem=92.0, disk=90.0))
    unresolved = [a for a in monitor._alerts if not a.resolved]
    assert {a.title for a in unresolved} == {
        "Memory Critical", "Memory High", "Disk Critical", "Disk High",
    }

    monitor.check_thresholds(_metrics(cpu=10.0, mem=30.0, disk=30.0))
    unresolved = [a for a in monitor._alerts if not a.resolved]
    assert unresolved == []


def test_readiness_report_is_clean_after_recovery():
    monitor = _fresh_monitor()
    monitor.check_thresholds(_metrics(cpu=95.0))
    monitor.check_thresholds(_metrics(cpu=10.0))
    _, issues = monitor.validate_production_readiness()
    # Other health checks (system DB etc.) may fail in the test env — the
    # alert-backlog issue must not be among them.
    assert all("critical alerts" not in i for i in issues)
