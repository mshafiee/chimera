"""Coverage completion tests for core/circuit_breaker.py."""

import json
import time
from datetime import datetime

import pytest

from core.circuit_breaker import (
    CircuitBreaker,
    CircuitState,
    ProtectionLevel,
    get_circuit_breaker,
    reset_circuit_breaker,
)


@pytest.fixture
def state_file(tmp_path, monkeypatch):
    path = tmp_path / "cb_state.json"
    monkeypatch.setenv("SCOUT_CIRCUIT_BREAKER_STATE", str(path))
    return path


@pytest.fixture
def breaker(state_file):
    return CircuitBreaker()


class TestLoadSaveState:
    def test_load_missing_file(self, state_file):
        CircuitBreaker()
        assert not state_file.exists()

    def test_load_state_from_today(self, state_file):
        state_file.write_text(json.dumps({
            "current_level": ProtectionLevel.CAUTION.value,
            "triggered_at": time.time(),
            "last_state_change": time.time(),
            "shield_allocation": 1.0,
            "spear_allocation": 0.0,
            "current_capital": 150.0,
            "peak_capital": 200.0,
        }))
        cb = CircuitBreaker()
        assert cb._state.current_level == ProtectionLevel.CAUTION
        assert cb._state.shield_allocation == 1.0
        assert cb._config.CURRENT_CAPITAL == 150.0
        assert cb._config.PEAK_CAPITAL == 200.0

    def test_load_state_from_previous_day(self, state_file):
        old_ts = (datetime.now().timestamp() - 3 * 86400)
        state_file.write_text(json.dumps({
            "current_level": ProtectionLevel.EMERGENCY.value,
            "last_state_change": old_ts,
            "triggered_at": old_ts,
            "current_capital": 150.0,
            "peak_capital": 200.0,
        }))
        cb = CircuitBreaker()
        assert cb._state.current_level == ProtectionLevel.NORMAL

    def test_load_corrupt_state(self, state_file):
        state_file.write_text("{corrupt")
        cb = CircuitBreaker()
        assert cb._state.current_level == ProtectionLevel.NORMAL

    def test_save_state_writes_file(self, state_file, breaker):
        breaker.update_capital(190.0)
        assert state_file.exists()
        data = json.loads(state_file.read_text())
        assert data["current_capital"] == 190.0

    def test_save_state_handles_error(self, state_file, breaker, monkeypatch):
        monkeypatch.setattr("pathlib.Path.mkdir", lambda *a, **k: (_ for _ in ()).throw(OSError("no")))
        breaker.update_capital(190.0)


class TestVolatilityMultiplier:
    def test_too_few_samples(self, breaker):
        assert breaker.get_volatility_multiplier() == 1.0

    def test_high_volatility(self, breaker):
        breaker._volatility_samples = [0.05] * 10
        assert breaker.get_volatility_multiplier() == 1.5

    def test_medium_volatility(self, breaker):
        breaker._volatility_samples = [0.035] * 10
        assert breaker.get_volatility_multiplier() == 1.25

    def test_low_volatility(self, breaker):
        breaker._volatility_samples = [0.01] * 10
        assert breaker.get_volatility_multiplier() == 1.0


class TestDrawdown:
    def test_peak_zero_returns_zero(self, breaker):
        breaker._config.PEAK_CAPITAL = 0
        assert breaker.get_drawdown() == 0.0

    def test_normal_drawdown(self, breaker):
        breaker._config.PEAK_CAPITAL = 200.0
        breaker._config.CURRENT_CAPITAL = 180.0
        assert breaker.get_drawdown() == pytest.approx(0.1)


class TestCheckCircuitBreaker:
    def _set_capital(self, breaker, current, peak=200.0):
        breaker._config.PEAK_CAPITAL = peak
        breaker._config.CURRENT_CAPITAL = current
        breaker._config.VOLATILITY_MULTIPLIER = False

    def test_normal_operation(self, breaker):
        self._set_capital(breaker, 200.0)
        can_trade, level, reason = breaker.check_circuit_breaker()
        assert can_trade is True
        assert level == ProtectionLevel.NORMAL
        assert reason == "Normal operation"

    def test_warning_trigger(self, breaker):
        self._set_capital(breaker, 189.0)  # 5.5% drawdown
        can_trade, level, reason = breaker.check_circuit_breaker()
        assert can_trade is True
        assert level == ProtectionLevel.WARNING
        assert "Spear allocation reduced" in reason
        assert breaker._state.spear_allocation == 0.20
        assert breaker._state.trigger_count == 1

    def test_caution_trigger(self, breaker):
        self._set_capital(breaker, 179.0)  # 10.5% drawdown
        can_trade, level, reason = breaker.check_circuit_breaker()
        assert can_trade is True
        assert level == ProtectionLevel.CAUTION
        assert breaker._state.current_state == CircuitState.OPEN
        assert breaker._state.spear_allocation == 0.0

    def test_emergency_trigger(self, breaker):
        self._set_capital(breaker, 160.0)  # 20% drawdown
        can_trade, level, reason = breaker.check_circuit_breaker()
        assert can_trade is False
        assert level == ProtectionLevel.EMERGENCY
        assert breaker._state.current_state == CircuitState.OPEN
        assert breaker._state.spear_allocation == 0.0
        assert breaker._state.shield_allocation == 0.0

    def test_no_retrigger_on_same_level(self, breaker):
        self._set_capital(breaker, 160.0)
        breaker.check_circuit_breaker()
        count_after_first = breaker._state.trigger_count
        breaker.check_circuit_breaker()
        assert breaker._state.trigger_count == count_after_first

    def test_recovery_pending(self, breaker):
        self._set_capital(breaker, 160.0)
        breaker.check_circuit_breaker()
        self._set_capital(breaker, 200.0)
        can_trade, level, reason = breaker.check_circuit_breaker()
        assert can_trade is True
        assert level == ProtectionLevel.EMERGENCY
        assert "recovery pending" in reason

    def test_auto_reset_after_timeout(self, breaker):
        self._set_capital(breaker, 160.0)
        breaker.check_circuit_breaker()
        breaker._state.triggered_at = time.time() - 9999
        breaker._config.RESET_TIMEOUT_SECONDS = 60
        self._set_capital(breaker, 200.0)
        can_trade, level, reason = breaker.check_circuit_breaker()
        assert can_trade is True
        assert level == ProtectionLevel.NORMAL
        assert breaker._state.current_state == CircuitState.CLOSED
        assert breaker._state.spear_allocation == 0.40
        reset_events = [e for e in breaker._events if e.event_type == "reset"]
        assert len(reset_events) == 1

    def test_attempt_reset_no_triggered_at(self, breaker):
        breaker._state.triggered_at = None
        breaker._attempt_reset()
        assert breaker._state.current_level == ProtectionLevel.NORMAL


class TestApplyLevelAdjustments:
    def test_warning_floor(self, breaker):
        breaker._state.spear_allocation = 0.1
        breaker._apply_level_adjustments(ProtectionLevel.WARNING)
        assert breaker._state.spear_allocation == 0.20

    def test_caution(self, breaker):
        breaker._apply_level_adjustments(ProtectionLevel.CAUTION)
        assert breaker._state.spear_allocation == 0.0
        assert breaker._state.shield_allocation == 1.0

    def test_emergency(self, breaker):
        breaker._apply_level_adjustments(ProtectionLevel.EMERGENCY)
        assert breaker._state.spear_allocation == 0.0
        assert breaker._state.shield_allocation == 0.0

    def test_no_change_no_log(self, breaker):
        breaker._state.shield_allocation = 0.6
        breaker._state.spear_allocation = 0.4
        breaker._apply_level_adjustments(ProtectionLevel.NORMAL)


class TestUpdateCapital:
    def test_new_peak_tracked(self, breaker):
        breaker.update_capital(210.0)
        assert breaker._config.PEAK_CAPITAL == 210.0
        assert len(breaker._volatility_samples) == 1

    def test_no_volatility_when_old_zero(self, breaker):
        breaker._config.CURRENT_CAPITAL = 0.0
        breaker.update_capital(100.0)
        assert breaker._volatility_samples == []

    def test_volatility_samples_capped(self, breaker):
        breaker._volatility_samples = [0.01] * 100
        breaker.update_capital(201.0)
        assert len(breaker._volatility_samples) <= 100


class TestWalletBlacklist:
    def test_blacklisted_wallet_rejected(self, breaker):
        breaker.blacklist_wallet("wallet_abc123", "too many failures")
        can_trade, reason = breaker.can_trade_wallet("wallet_abc123")
        assert can_trade is False
        assert "blacklisted" in reason

    def test_blacklist_expired(self, breaker, monkeypatch):
        breaker.blacklist_wallet("wallet_abc123", "old failure")
        breaker._state.wallet_blacklist["wallet_abc123"] = time.time() - 10
        can_trade, reason = breaker.can_trade_wallet("wallet_abc123")
        assert can_trade is True
        assert "wallet_abc123" not in breaker._state.wallet_blacklist

    def test_circuit_open_blocks_wallet(self, breaker):
        breaker._config.PEAK_CAPITAL = 200.0
        breaker._config.CURRENT_CAPITAL = 150.0
        breaker._config.VOLATILITY_MULTIPLIER = False
        can_trade, reason = breaker.can_trade_wallet("wallet_xyz789")
        assert can_trade is False
        assert "Circuit breaker" in reason

    def test_wallet_ok(self, breaker):
        can_trade, reason = breaker.can_trade_wallet("wallet_ok_123")
        assert can_trade is True
        assert reason == "OK"

    def test_blacklist_event_recorded(self, breaker):
        breaker.blacklist_wallet("w1", "bad")
        assert breaker._events[-1].event_type == "wallet_blacklist"
        assert breaker._events[-1].metadata == {"wallet": "w1"}


class TestRecordTradeResult:
    def test_success_counts(self, breaker):
        breaker.record_trade_result(True, "wallet_1")
        assert breaker._state.daily_trades == 1
        assert breaker._state.daily_success == 1

    def test_failure_blacklists_after_max(self, breaker):
        breaker._config.MAX_WALLET_FAILURES = 3
        for _ in range(3):
            breaker.record_trade_result(False, "wallet_fail")
        assert "wallet_fail" in breaker._state.wallet_blacklist
        assert breaker._wallet_failures["wallet_fail"] == 0

    def test_failure_without_wallet(self, breaker):
        breaker.record_trade_result(False)
        assert breaker._state.daily_failures == 1

    def test_success_resets_failure_counter(self, breaker):
        breaker._config.MAX_WALLET_FAILURES = 3
        breaker.record_trade_result(False, "wallet_mix")
        breaker.record_trade_result(True, "wallet_mix")
        assert breaker._wallet_failures["wallet_mix"] == 0
        breaker.record_trade_result(False, "wallet_mix")
        assert breaker._wallet_failures["wallet_mix"] == 1


class TestAllocation:
    def test_get_current_allocation(self, breaker):
        assert breaker.get_current_allocation() == (0.60, 0.40)

    def test_aggressive_stages(self, breaker):
        assert breaker.get_aggressive_allocation(100) == (0.60, 0.40)
        assert breaker.get_aggressive_allocation(400) == (0.50, 0.50)
        assert breaker.get_aggressive_allocation(700) == (0.40, 0.60)
        assert breaker.get_aggressive_allocation(1000) == (0.30, 0.70)

    def test_adjust_for_growth_stage(self, breaker):
        breaker._config.CURRENT_CAPITAL = 900.0
        breaker.adjust_for_growth_stage()
        assert breaker.get_current_allocation() == (0.30, 0.70)

    def test_adjust_not_aggressive(self, breaker):
        breaker._config.AGGRESSIVE_MODE = False
        breaker.adjust_for_growth_stage()
        assert breaker.get_current_allocation() == (0.60, 0.40)

    def test_adjust_blocked_when_not_normal(self, breaker):
        breaker._state.current_level = ProtectionLevel.WARNING
        breaker.adjust_for_growth_stage()
        assert breaker.get_current_allocation() == (0.60, 0.40)


class TestStatusReport:
    def test_get_status_report(self, breaker):
        breaker.record_trade_result(True)
        breaker.record_trade_result(False)
        breaker.blacklist_wallet("w1", "r")
        report = breaker.get_status_report()
        assert report["current_level"] == "NORMAL"
        assert report["current_state"] == "closed"
        assert report["daily_trades"] == 2
        assert report["success_rate"] == 0.5
        assert report["blacklisted_wallets"] == 1
        assert report["events_today"] == 1

    def test_print_status_report(self, breaker, capsys):
        breaker._state.triggered_at = time.time() - 600
        breaker.print_status_report()
        out = capsys.readouterr().out
        assert "CIRCUIT BREAKER - STATUS REPORT" in out
        assert "Last triggered" in out

    def test_print_status_report_no_trigger(self, breaker, capsys):
        breaker.print_status_report()
        out = capsys.readouterr().out
        assert "Last triggered" not in out

    def test_reset_daily_counters(self, breaker):
        breaker.record_trade_result(True)
        breaker.reset_daily_counters()
        report = breaker.get_status_report()
        assert report["daily_trades"] == 0


class TestSingleton:
    def test_get_circuit_breaker_singleton(self, monkeypatch):
        import core.circuit_breaker as cb_module

        monkeypatch.setattr(cb_module, "_circuit_breaker", None)
        first = get_circuit_breaker()
        second = get_circuit_breaker()
        assert first is second
        monkeypatch.setattr(cb_module, "_circuit_breaker", None)

    def test_reset_circuit_breaker(self, monkeypatch):
        import core.circuit_breaker as cb_module

        monkeypatch.setattr(cb_module, "_circuit_breaker", CircuitBreaker())
        reset_circuit_breaker()
        assert cb_module._circuit_breaker is None

    def test_reset_when_none(self, monkeypatch):
        import core.circuit_breaker as cb_module

        monkeypatch.setattr(cb_module, "_circuit_breaker", None)
        reset_circuit_breaker()
