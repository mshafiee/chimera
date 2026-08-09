"""
Coverage tests for core/production_monitor.py.

Covers ProductionMonitor lifecycle (health checks, alert handlers, monitoring
loop, alert storage/retrieval), GrowthTracker error paths and math branches,
and the module singletons.
"""

import time

import psutil
import pytest

from core.production_monitor import (
    Alert,
    AlertSeverity,
    GrowthMetrics,
    GrowthTracker,
    HealthCheck,
    HealthStatus,
    PerformanceMetrics,
    ProductionMonitor,
    get_production_monitor,
)


@pytest.fixture(autouse=True)
def _reset_singletons():
    import core.production_monitor as pm
    prev_monitor = pm._monitor
    prev_growth = pm._growth_tracker
    pm._monitor = None
    pm._growth_tracker = None
    yield
    pm._monitor = prev_monitor
    pm._growth_tracker = prev_growth


@pytest.fixture
def monitor(fake_db_layer, tmp_path):
    m = ProductionMonitor()
    m._db_path = str(tmp_path / "monitor.db")
    m._sustained_samples = 1
    m._alert_cooldown = 0
    return m


def _metrics(cpu=10.0, mem=10.0, disk=10.0):
    return PerformanceMetrics(
        timestamp=0.0, cpu_percent=cpu, memory_percent=mem,
        memory_used_mb=512.0, disk_usage_percent=disk,
        active_threads=5, open_files=10, network_connections=3,
    )


# --- Dataclass branches ---

def test_health_check_defaults():
    hc = HealthCheck(name="n", status=HealthStatus.HEALTHY, message="m",
                     timestamp=0, details={})
    assert hc.timestamp > 0
    d = hc.to_dict()
    assert d["status"] == "healthy"
    assert d["response_time_ms"] is None


def test_alert_defaults():
    a = Alert(id="1", severity=AlertSeverity.WARNING, title="t", message="m",
              timestamp=0, source="s")
    assert a.timestamp > 0
    assert a.details == {}
    d = a.to_dict()
    assert d["severity"] == "warning"
    assert d["resolved"] is False


def test_growth_metrics_defaults_and_properties():
    gm = GrowthMetrics(timestamp=0, current_capital=500.0, target_capital=1000.0,
                       starting_capital=200.0)
    assert gm.timestamp > 0
    assert gm.progress_percentage > 0
    assert gm.capital_multiplier == 2.5
    gm2 = GrowthMetrics(timestamp=1, current_capital=100.0, target_capital=50.0,
                        starting_capital=200.0)
    assert gm2.progress_percentage == 100.0
    gm3 = GrowthMetrics(timestamp=1, current_capital=100.0, target_capital=1000.0,
                        starting_capital=0.0)
    assert gm3.capital_multiplier == 1.0
    d = gm.to_dict()
    assert "progress_percentage" in d
    assert "capital_multiplier" in d


# --- ProductionMonitor ---

def test_init_database_creates_alerts_table(monitor, fake_db_layer):
    cur = fake_db_layer.cursor()
    cur.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='alerts'")
    assert cur.fetchone() is not None


def test_init_database_failure_swallowed(fake_db_layer, monkeypatch):
    monkeypatch.setitem(
        ProductionMonitor._init_database.__globals__, "get_connection",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("db down")),
    )
    m = ProductionMonitor()
    assert m._health_checks == {}


def test_register_health_check_and_handler(monitor):
    monitor.register_health_check("check1", lambda: None)
    assert "check1" in monitor._health_checks
    handler = lambda a: None  # noqa: E731
    monitor.register_alert_handler(AlertSeverity.ERROR, handler)
    monitor.register_alert_handler(AlertSeverity.ERROR, handler)
    assert len(monitor._alert_handlers[AlertSeverity.ERROR]) == 2


def test_run_health_checks_wraps_non_healthcheck(monitor):
    monitor.register_health_check("plain", lambda: {"ok": True})
    results = monitor.run_health_checks()
    assert len(results) == 1
    assert results[0].status == HealthStatus.HEALTHY
    assert results[0].response_time_ms is not None


def test_run_health_checks_returns_healthcheck_with_rt(monitor):
    monitor.register_health_check(
        "hc", lambda: HealthCheck(name="hc", status=HealthStatus.DEGRADED,
                                  message="slow", timestamp=time.time(), details={})
    )
    results = monitor.run_health_checks()
    assert results[0].response_time_ms is not None


def test_run_health_checks_exception_marks_down(monitor):
    def boom():
        raise RuntimeError("check exploded")
    monitor.register_health_check("boom", boom)
    results = monitor.run_health_checks()
    assert results[0].status == HealthStatus.DOWN
    assert "check exploded" in results[0].message


def test_collect_metrics_access_denied(monitor, monkeypatch):
    monkeypatch.setattr(
        psutil.Process, "connections",
        lambda self: (_ for _ in ()).throw(psutil.AccessDenied()),
    )
    metrics = monitor.collect_metrics()
    assert metrics.network_connections == 0


def test_collect_metrics_history_capped(monitor, monkeypatch):
    monitor._max_metrics_history = 2
    for _ in range(5):
        monitor.collect_metrics()
    assert len(monitor._metrics_history) == 2


def test_collect_metrics_error_returns_zeroed(monitor, monkeypatch):
    def boom(*a, **k):
        raise RuntimeError("psutil broke")
    monkeypatch.setattr(psutil, "cpu_percent", boom)
    metrics = monitor.collect_metrics()
    assert metrics.cpu_percent == 0
    assert metrics.memory_percent == 0


def test_create_alert_and_store(monitor, fake_db_layer):
    alert = monitor.create_alert(AlertSeverity.ERROR, "Title", "Msg", source="test")
    assert alert.id.startswith("test_")
    assert alert in monitor._alerts
    cur = fake_db_layer.cursor()
    cur.execute("SELECT * FROM alerts WHERE id = ?", (alert.id,))
    row = cur.fetchone()
    assert row is not None
    assert row["severity"] == "error"


def test_store_alert_failure_logged(monitor, fake_db_layer, monkeypatch):
    monkeypatch.setitem(
        ProductionMonitor._store_alert.__globals__, "get_connection",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("db down")),
    )
    alert = monitor.create_alert(AlertSeverity.INFO, "T", "M", source="s")
    assert alert.id  # no exception


def test_trigger_alert_handlers_with_failure(monitor):
    calls = []

    def handler(alert):
        calls.append(alert.id)

    def bad_handler(alert):
        raise RuntimeError("handler failed")

    monitor.register_alert_handler(AlertSeverity.ERROR, handler)
    monitor.register_alert_handler(AlertSeverity.ERROR, bad_handler)
    monitor.create_alert(AlertSeverity.ERROR, "T", "M", source="s")
    assert len(calls) == 1


def test_can_alert_cooldown_blocks(monitor, fake_db_layer):
    monitor._alert_cooldown = 3600
    first = monitor.check_thresholds(_metrics(cpu=95.0))
    assert len(first) == 2
    second = monitor.check_thresholds(_metrics(cpu=95.0))
    assert second == []  # cooldown not elapsed


def test_check_thresholds_creates_and_resolves(monitor, fake_db_layer):
    alerts = monitor.check_thresholds(_metrics(cpu=95.0))
    assert len(alerts) == 2  # CPU Critical + CPU High
    # resolve via DB-backed _resolve_alerts
    monitor.check_thresholds(_metrics(cpu=10.0))
    unresolved = [a for a in monitor._alerts if not a.resolved]
    assert unresolved == []


def test_start_monitoring_twice_and_stop(monitor):
    monitor._check_interval = 0.01
    monitor.start_monitoring()
    assert monitor._monitoring_active is True
    monitor.start_monitoring()  # already active -> warning return
    monitor.stop_monitoring()
    assert monitor._monitoring_active is False
    monitor.stop_monitoring()  # no thread -> graceful
    # Directly cover the already-active return without a running thread
    monitor._monitoring_active = True
    monitor.start_monitoring()  # logs "already active", returns
    monitor._monitoring_active = False


def test_monitoring_loop_runs_and_handles_errors(monitor, monkeypatch):
    monitor._check_interval = 0.01
    calls = {"n": 0}

    def flaky_collect():
        calls["n"] += 1
        if calls["n"] == 1:
            raise RuntimeError("transient")
        return _metrics()

    monkeypatch.setattr(monitor, "collect_metrics", flaky_collect)
    monkeypatch.setattr(monitor, "run_health_checks", lambda: [])
    monitor.start_monitoring()
    time.sleep(0.06)
    monitor.stop_monitoring()
    assert calls["n"] >= 2


def test_get_health_status_critical(monitor):
    monitor.register_health_check(
        "crit", lambda: HealthCheck(name="crit", status=HealthStatus.CRITICAL,
                                    message="x", timestamp=time.time(), details={})
    )
    status = monitor.get_health_status()
    assert status["overall_status"] == "critical"
    assert status["active_alerts"] >= 0
    assert "metrics" in status


def test_get_health_status_degraded_and_healthy(monitor):
    monitor.register_health_check(
        "deg", lambda: HealthCheck(name="deg", status=HealthStatus.DEGRADED,
                                   message="x", timestamp=time.time(), details={})
    )
    assert monitor.get_health_status()["overall_status"] == "degraded"
    monitor2 = ProductionMonitor()
    assert monitor2.get_health_status()["overall_status"] == "healthy"


def test_get_recent_alerts_from_db_and_memory(monitor, fake_db_layer):
    monitor.create_alert(AlertSeverity.ERROR, "FromMemory", "m", source="s1")
    alerts = monitor.get_recent_alerts(limit=10)
    assert any(a.title == "FromMemory" for a in alerts)
    assert len(alerts) >= 1


def test_get_recent_alerts_merges_in_memory(monitor, fake_db_layer, monkeypatch):
    monkeypatch.setattr(monitor, "_store_alert", lambda alert: None)  # skip DB write
    monitor.create_alert(AlertSeverity.WARNING, "MemOnly", "m", source="s1")
    alerts = monitor.get_recent_alerts(limit=10)
    assert any(a.title == "MemOnly" for a in alerts)


def test_get_recent_alerts_error_returns_memory(monitor, fake_db_layer, monkeypatch):
    monitor.create_alert(AlertSeverity.WARNING, "MemOnly", "m", source="s1")
    monkeypatch.setitem(
        ProductionMonitor.get_recent_alerts.__globals__, "get_connection",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("db down")),
    )
    alerts = monitor.get_recent_alerts(limit=10)
    assert alerts and alerts[0].title == "MemOnly"


def test_print_status_report(monitor, capsys, fake_db_layer):
    monitor.register_health_check(
        "ok", lambda: HealthCheck(name="ok", status=HealthStatus.HEALTHY,
                                  message="fine", timestamp=time.time(), details={})
    )
    monitor.register_health_check(
        "bad", lambda: HealthCheck(name="bad", status=HealthStatus.DEGRADED,
                                   message="disk full", timestamp=time.time(), details={})
    )
    monitor.create_alert(AlertSeverity.ERROR, "Disk High", "disk full", source="system")
    monitor.print_status_report()
    out = capsys.readouterr().out
    assert "PRODUCTION MONITOR - STATUS REPORT" in out
    assert "Disk High" in out


def test_validate_production_readiness_issues(monitor, fake_db_layer, monkeypatch):
    monkeypatch.setattr(monitor, "collect_metrics",
                        lambda: _metrics(cpu=95.0, mem=95.0, disk=95.0))
    monitor.register_health_check(
        "crit", lambda: HealthCheck(name="crit", status=HealthStatus.CRITICAL,
                                    message="x", timestamp=time.time(), details={})
    )
    monitor.check_thresholds(_metrics(cpu=95.0))
    ready, issues = monitor.validate_production_readiness()
    assert ready is False
    text = "\n".join(issues)
    assert "System health is critical" in text
    assert "critical alerts" in text
    assert "CPU" in text
    assert "memory" in text
    assert "disk" in text


def test_validate_production_readiness_clean(monitor, fake_db_layer, monkeypatch):
    monkeypatch.setattr(monitor, "collect_metrics", lambda: _metrics())
    ready, issues = monitor.validate_production_readiness()
    assert ready is True or "critical alerts" in "\n".join(issues)


def test_shutdown(monitor):
    monitor.shutdown()
    assert monitor._monitoring_active is False


def test_get_production_monitor_singleton(fake_db_layer):
    m1 = get_production_monitor()
    m2 = get_production_monitor()
    assert m1 is m2


# --- GrowthTracker ---

@pytest.fixture
def growth_tracker(fake_db_layer, tmp_path):
    return _growth_tracker(fake_db_layer, tmp_path)


def _growth_tracker(fake_db_layer, tmp_path):
    return GrowthTracker(
        starting_capital=200.0, target_capital=1000.0,
        db_path=str(tmp_path / "growth.db"),
    )


def test_growth_tracker_init_database_error(fake_db_layer, monkeypatch, tmp_path):
    monkeypatch.setitem(
        GrowthTracker._init_database.__globals__, "get_connection",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("db down")),
    )
    gt = GrowthTracker(db_path=str(tmp_path / "g.db"))
    assert gt.current_capital == 200.0


def test_growth_tracker_load_latest_state_error(fake_db_layer, monkeypatch, tmp_path):
    monkeypatch.setitem(
        GrowthTracker._load_latest_state.__globals__, "get_connection",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("db down")),
    )
    gt = GrowthTracker(db_path=str(tmp_path / "g2.db"))
    assert gt.current_capital == 200.0


def test_growth_metrics_calculation_error(fake_db_layer, monkeypatch, tmp_path):
    gt = _growth_tracker(fake_db_layer, tmp_path)
    monkeypatch.setitem(
        GrowthTracker._calculate_growth_metrics.__globals__, "get_connection",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("db down")),
    )
    metrics = gt._calculate_growth_metrics()
    assert metrics.current_capital == 200.0
    assert metrics.days_to_target is None


def test_estimate_days_to_target_log_error():
    gt = GrowthTracker.__new__(GrowthTracker)
    gt.current_capital = -100.0
    gt.target_capital = 1000.0
    assert gt._estimate_days_to_target(5.0) is None  # log(negative) -> ValueError


def test_estimate_days_to_target_slow_growth_returns_none():
    gt = GrowthTracker.__new__(GrowthTracker)
    gt.current_capital = 500.0
    gt.target_capital = 1000.0
    assert gt._estimate_days_to_target(0.0) is None
    assert gt._estimate_days_to_target(5.0) is not None


def test_store_growth_snapshot_error(fake_db_layer, monkeypatch, tmp_path):
    gt = _growth_tracker(fake_db_layer, tmp_path)
    monkeypatch.setitem(
        GrowthTracker._store_growth_snapshot.__globals__, "get_connection",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("db down")),
    )
    gt._store_growth_snapshot(GrowthMetrics(
        timestamp=time.time(), current_capital=210.0, target_capital=1000.0,
        starting_capital=200.0,
    ))  # must not raise


def test_record_capital_event_error(fake_db_layer, monkeypatch, tmp_path):
    gt = _growth_tracker(fake_db_layer, tmp_path)
    monkeypatch.setitem(
        GrowthTracker._record_capital_event.__globals__, "get_connection",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("db down")),
    )
    gt._record_capital_event("update", 200.0, 300.0, "desc")  # must not raise


def test_store_growth_alert_error(fake_db_layer, monkeypatch, tmp_path):
    gt = _growth_tracker(fake_db_layer, tmp_path)
    monkeypatch.setitem(
        GrowthTracker._store_alert.__globals__, "get_connection",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("db down")),
    )
    gt._store_alert("type", "severity", "message")  # must not raise


def test_get_growth_history_error(fake_db_layer, monkeypatch, tmp_path):
    gt = _growth_tracker(fake_db_layer, tmp_path)
    monkeypatch.setitem(
        GrowthTracker.get_growth_history.__globals__, "get_connection",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("db down")),
    )
    assert gt.get_growth_history(days=30) == []


def test_print_growth_dashboard_insufficient_growth(fake_db_layer, tmp_path, capsys):
    gt = _growth_tracker(fake_db_layer, tmp_path)
    gt.print_growth_dashboard()
    out = capsys.readouterr().out
    assert "GROWTH TRACKING" in out
    assert "Insufficient growth rate" in out


def test_growth_record_capital_with_alerts(fake_db_layer, tmp_path):
    gt = _growth_tracker(fake_db_layer, tmp_path)
    # +100% jump -> significant gain + target reached (target = 1000)
    metrics = gt.record_capital(1200.0, event_type="trade", description="moon")
    assert metrics.current_capital == 1200.0
    cur = fake_db_layer.cursor()
    cur.execute("SELECT event_type FROM capital_events")
    events = [r["event_type"] for r in cur.fetchall()]
    assert "trade" in events
    cur.execute("SELECT alert_type FROM growth_alerts")
    alerts = [r["alert_type"] for r in cur.fetchall()]
    assert "significant_gain" in alerts
    assert "target_reached" in alerts
    # -50% drop -> significant loss alert
    gt.record_capital(500.0)
    cur.execute("SELECT alert_type FROM growth_alerts")
    alert_types = [r["alert_type"] for r in cur.fetchall()]
    assert "significant_loss" in alert_types


def test_growth_get_current_metrics_and_summary(fake_db_layer, tmp_path):
    gt = _growth_tracker(fake_db_layer, tmp_path)
    gt.record_capital(400.0)
    summary = gt.get_growth_summary()
    assert summary["current_capital"] == 400.0
    assert summary["progress_percentage"] == 25.0
    metrics = gt.get_current_metrics()
    assert metrics.current_capital == 400.0
