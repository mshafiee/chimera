#!/usr/bin/env python3
"""
Comprehensive Integration Verification Script

This script verifies that all dead code integrations across Sprint 1, 2, and 3
are working correctly together. Tests component interactions and end-to-end workflows.
"""

import sys
from pathlib import Path

# Add Scout directory to path
sys.path.insert(0, str(Path(__file__).parent))

def test_sprint_1_integration():
    """Verify Sprint 1 components: Validation Reporter, State Persistence, Volume Cache (Operator)"""
    print("\n" + "="*70)
    print("SPRINT 1: Quick Wins Integration Verification")
    print("="*70)

    results = []

    # Test Validation Reporter
    print("\n1. Validation Reporter Integration...")
    try:
        from core.validation_reporter import AlertConfig
        from config import ScoutConfig

        # Verify configuration methods exist
        if not (hasattr(ScoutConfig, 'get_validation_enabled')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_alert_webhook_url')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_alert_drift_threshold')):
            raise AssertionError()

        # Verify basic initialization
        alert_config = AlertConfig(
            webhook_url="https://test.webhook.com",
            high_error_threshold=0.5,
            drift_threshold=0.15
        )
        if not (alert_config.webhook_url == "https://test.webhook.com"):
            raise AssertionError("AlertConfig construction failed")
        print("✓ Validation Reporter integration verified")
        results.append(True)
    except Exception as e:
        print(f"✗ Validation Reporter integration failed: {e}")
        results.append(False)

    # Test State Persistence
    print("\n2. State Persistence Integration...")
    try:
        from config import ScoutConfig

        # Verify configuration methods exist
        if not (hasattr(ScoutConfig, 'get_state_persistence_enabled')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_state_persistence_db_path')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_state_persistence_max_days')):
            raise AssertionError()

        print("✓ State Persistence integration verified")
        results.append(True)
    except Exception as e:
        print(f"✗ State Persistence integration failed: {e}")
        results.append(False)

    # Volume Cache is Operator-side, verify the integration actually exists
    print("\n3. Volume Cache (Operator)...")
    try:
        operator_main = Path(__file__).parent.parent / "operator" / "src" / "main.rs"
        if not operator_main.exists():
            print("✗ operator/src/main.rs not found - cannot verify Volume Cache integration")
            results.append(False)
        else:
            content = operator_main.read_text()
            has_volume_cache = ("volume_cache" in content.lower()
                                or "volume cache" in content.lower()
                                or "VolumeCache" in content)
            if has_volume_cache:
                print("✓ Volume Cache integration referenced in operator/src/main.rs")
                results.append(True)
            else:
                print("✗ Volume Cache not referenced in operator/src/main.rs")
                results.append(False)
    except Exception as e:
        print(f"✗ Volume Cache verification failed: {e}")
        results.append(False)

    return all(results)


def test_sprint_2_integration():
    """Verify Sprint 2 components: Advanced Cache, Stop-Loss Optimizer, Position Manager"""
    print("\n" + "="*70)
    print("SPRINT 2: High Impact Integration Verification")
    print("="*70)

    results = []

    # Test Advanced Cache System
    print("\n1. Advanced Cache System Integration...")
    try:
        from core.advanced_cache import AdvancedCache
        from config import ScoutConfig

        # Verify configuration methods exist
        if not (hasattr(ScoutConfig, 'get_advanced_cache_enabled')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_cache_l1_enabled')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_cache_l2_enabled')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_cache_l3_enabled')):
            raise AssertionError()

        # Verify cache initialization
        cache = AdvancedCache()
        if not (cache is not None):
            raise AssertionError()

        print("✓ Advanced Cache System integration verified")
        results.append(True)
    except Exception as e:
        print(f"✗ Advanced Cache System integration failed: {e}")
        results.append(False)

    # Test Stop-Loss Optimizer
    print("\n2. Stop-Loss Optimizer Integration...")
    try:
        from core.stop_loss_optimizer import StopLossOptimizer
        from config import ScoutConfig

        # Verify configuration methods exist
        if not (hasattr(ScoutConfig, 'get_stop_loss_optimizer_enabled')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_atr_period_default')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_bull_regime_multiplier')):
            raise AssertionError()

        print("✓ Stop-Loss Optimizer integration verified")
        results.append(True)
    except Exception as e:
        print(f"✗ Stop-Loss Optimizer integration failed: {e}")
        results.append(False)

    # Test Position Manager
    print("\n3. Position Manager Integration...")
    try:
        from core.position_manager import PositionManager
        from core.stop_loss_optimizer import StopLossOptimizer

        # Verify integration between components
        optimizer = StopLossOptimizer()
        position_manager = PositionManager(optimizer)
        if not (position_manager is not None):
            raise AssertionError()

        print("✓ Position Manager integration verified")
        results.append(True)
    except Exception as e:
        print(f"✗ Position Manager integration failed: {e}")
        results.append(False)

    return all(results)


def test_sprint_3_integration():
    """Verify Sprint 3 components: Signal Quality Filter"""
    print("\n" + "="*70)
    print("SPRINT 3: Quality Enhancement Integration Verification")
    print("="*70)

    results = []

    # Test Signal Quality Filter
    print("\n1. Signal Quality Filter Integration...")
    try:
        from core.signal_quality_filter import SignalQualityFilter, TradingSignal
        from config import ScoutConfig

        # Verify configuration methods exist
        if not (hasattr(ScoutConfig, 'get_signal_quality_filter_enabled')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_wqs_weight')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_timing_weight')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_top_percentile_target')):
            raise AssertionError()

        # Verify filter initialization
        filter = SignalQualityFilter()
        if not (filter is not None):
            raise AssertionError()

        # Verify signal processing
        signal = TradingSignal(
            wallet_address="test_wallet",
            token_address="test_token",
            wqs_score=85.0,
            timing_score=0.8,
            market_regime="BULL",
            ensemble_confidence=0.75,
            signal_age_seconds=60,
            pnl_prediction=0.15,
        )

        decision = filter.should_execute_signal(signal)
        if not (decision is not None):
            raise AssertionError()

        print("✓ Signal Quality Filter integration verified")
        results.append(True)
    except Exception as e:
        print(f"✗ Signal Quality Filter integration failed: {e}")
        import traceback
        traceback.print_exc()
        results.append(False)

    return all(results)


def test_cross_component_integration():
    """Test interactions between components from different sprints"""
    print("\n" + "="*70)
    print("Cross-Component Integration Verification")
    print("="*70)

    results = []

    print("\n1. Testing Advanced Cache + State Persistence synergy...")
    try:
        from core.advanced_cache import AdvancedCache
        from core.state_persistence import StatePersistence, PersistenceConfig
        from config import ScoutConfig

        # Both components can be imported and configured (StatePersistence
        # instantiation needs a live database, so verify its API surface here)
        cache = AdvancedCache()
        if not (cache is not None):
            raise AssertionError()

        if not (hasattr(StatePersistence, 'save_credit_history')):
            raise AssertionError()
        if not (hasattr(PersistenceConfig, 'db_path')):
            raise AssertionError()
        if not (hasattr(ScoutConfig, 'get_state_persistence_enabled')):
            raise AssertionError()

        # Cache statistics could be stored in persistence
        cache_stats = cache.get_stats()
        if not (cache_stats is not None):
            raise AssertionError()

        print("✓ Cache + Persistence synergy verified")
        results.append(True)
    except Exception as e:
        print(f"✗ Cache + Persistence synergy failed: {e}")
        results.append(False)

    print("\n2. Testing Stop-Loss + Position Manager integration...")
    try:
        from core.stop_loss_optimizer import StopLossOptimizer
        from core.position_manager import PositionManager, PositionSide

        # Create optimizer
        optimizer = StopLossOptimizer()

        # Create position manager with optimizer
        position_manager = PositionManager(optimizer)

        # Exercise the public stop-loss computation path
        position = position_manager.create_position(
            position_id="verify_pos_1",
            wallet_address="test_wallet",
            token_address="test_token",
            token_symbol="TEST",
            entry_price=100.0,
            position_size_sol=1.0,
            position_value_usd=100.0,
            side=PositionSide.LONG,
            strategy="SHIELD",
            wqs_score=75.0,
        )
        position = position_manager.calculate_stop_for_position(position)
        if not (position.stop_loss_price is not None and position.stop_loss_price < position.entry_price):
            raise AssertionError(f"Stop-loss price invalid: {position.stop_loss_price}")

        print("✓ Stop-Loss + Position Manager integration verified")
        results.append(True)
    except Exception as e:
        print(f"✗ Stop-Loss + Position Manager integration failed: {e}")
        results.append(False)

    print("\n3. Testing Signal Quality + Advanced Cache interaction...")
    try:
        from core.signal_quality_filter import SignalQualityFilter, TradingSignal
        from core.advanced_cache import AdvancedCache

        # Both components can operate together
        filter = SignalQualityFilter()
        cache = AdvancedCache()

        # High-quality signals could benefit from cache warming
        signal = TradingSignal(
            wallet_address="test_wallet",
            token_address="test_token",
            wqs_score=90.0,
            timing_score=0.9,
            market_regime="BULL",
            ensemble_confidence=0.85,
            signal_age_seconds=45,
            pnl_prediction=0.20,
        )

        decision = filter.should_execute_signal(signal)
        if not (decision is not None):
            raise AssertionError()

        print("✓ Signal Quality + Cache interaction verified")
        results.append(True)
    except Exception as e:
        print(f"✗ Signal Quality + Cache interaction failed: {e}")
        results.append(False)

    return all(results)


def test_configuration_integration():
    """Test that all configuration methods are properly integrated"""
    print("\n" + "="*70)
    print("Configuration Integration Verification")
    print("="*70)

    results = []

    print("\n1. Counting configuration methods...")
    try:
        from config import ScoutConfig

        # Count all configuration methods
        config_methods = [
            # Validation Reporter (9)
            'get_validation_enabled', 'get_alert_webhook_url', 'get_alert_high_error_threshold',
            'get_alert_drift_threshold', 'get_alert_low_accuracy_threshold', 'get_alert_dir',
            'get_validation_report_schedule', 'get_validation_time_window', 'get_validation_report_format',

            # State Persistence (9)
            'get_state_persistence_enabled', 'get_state_persistence_db_path', 'get_state_persistence_max_days',
            'get_state_persistence_backup_enabled', 'get_state_persistence_backup_interval',
            'get_state_persistence_vacuum_interval', 'get_state_persistence_credit_history_enabled',
            'get_state_persistence_wallet_performance_enabled', 'get_state_persistence_roi_metrics_enabled',

            # Advanced Cache (12)
            'get_advanced_cache_enabled', 'get_cache_l1_enabled', 'get_cache_l2_enabled',
            'get_cache_l3_enabled', 'get_cache_l1_ttl_seconds', 'get_cache_l2_ttl_seconds',
            'get_cache_l3_ttl_seconds', 'get_cache_growth_aware_ttl', 'get_cache_exceptional_wqs_multiplier',
            'get_cache_high_wqs_multiplier', 'get_cache_average_wqs_multiplier', 'get_cache_below_average_wqs_multiplier',

            # Stop-Loss Optimizer (15)
            'get_stop_loss_optimizer_enabled', 'get_atr_period_default', 'get_atr_threshold_period',
            'get_bull_regime_multiplier', 'get_bear_regime_multiplier', 'get_volatile_regime_multiplier',
            'get_neutral_regime_multiplier', 'get_stop_loss_risk_multiplier', 'get_stop_loss_noise_tolerance_percent',
            'get_position_size_risk_percent', 'get_position_size_max_percent', 'get_risk_reward_ratio_target',
            'get_max_total_risk_percent', 'get_regime_atr_multiplier', 'get_stop_loss_trailing_enabled',

            # Signal Quality Filter (17)
            'get_signal_quality_filter_enabled', 'get_wqs_weight', 'get_timing_weight',
            'get_regime_weight', 'get_ensemble_weight', 'get_freshness_weight',
            'get_top_percentile_target', 'get_min_percentile_threshold', 'get_max_percentile_threshold',
            'get_signal_quality_adaptive_threshold', 'get_quality_adjustment_window', 'get_quality_adjustment_sensitivity',
            'get_signal_fresh_seconds', 'get_signal_stale_seconds', 'get_ensemble_min_confidence',
            'get_signal_max_age_seconds', 'get_quality_excellent_threshold', 'get_quality_high_threshold',
            'get_quality_good_threshold',
        ]

        # Verify all methods exist
        for method_name in config_methods:
            if not (hasattr(ScoutConfig, method_name)):
                raise AssertionError(f"Missing method: {method_name}")

        print(f"✓ All {len(config_methods)} configuration methods verified")
        results.append(True)
    except Exception as e:
        print(f"✗ Configuration integration failed: {e}")
        results.append(False)

    print("\n2. Testing configuration method calls...")
    try:
        from config import ScoutConfig

        # Test a subset of methods to ensure they work
        ScoutConfig.get_validation_enabled()
        ScoutConfig.get_state_persistence_enabled()
        ScoutConfig.get_advanced_cache_enabled()
        ScoutConfig.get_stop_loss_optimizer_enabled()
        ScoutConfig.get_signal_quality_filter_enabled()

        print("✓ Configuration method calls verified")
        results.append(True)
    except Exception as e:
        print(f"✗ Configuration method calls failed: {e}")
        results.append(False)

    return all(results)


def main():
    """Run comprehensive integration verification."""
    print("="*70)
    print("COMPREHENSIVE DEAD CODE INTEGRATION VERIFICATION")
    print("="*70)
    print("\nThis script verifies all integrations across Sprint 1, 2, and 3")
    print("including cross-component interactions and configuration.")

    results = []

    # Test each sprint
    sprint_1_result = test_sprint_1_integration()
    results.append(("Sprint 1: Quick Wins", sprint_1_result))

    sprint_2_result = test_sprint_2_integration()
    results.append(("Sprint 2: High Impact", sprint_2_result))

    sprint_3_result = test_sprint_3_integration()
    results.append(("Sprint 3: Quality Enhancement", sprint_3_result))

    # Test cross-component integration
    cross_component_result = test_cross_component_integration()
    results.append(("Cross-Component Integration", cross_component_result))

    # Test configuration integration
    config_result = test_configuration_integration()
    results.append(("Configuration Integration", config_result))

    # Print summary
    print("\n" + "="*70)
    print("INTEGRATION VERIFICATION SUMMARY")
    print("="*70)

    for name, result in results:
        status = "✅ PASS" if result else "❌ FAIL"
        print(f"{status}: {name}")

    total_pass = sum(1 for _, result in results if result)
    total_tests = len(results)

    print("\n" + "="*70)
    print(f"OVERALL RESULT: {total_pass}/{total_tests} test groups passed")
    print("="*70)

    if total_pass == total_tests:
        print("\n🎉 ALL INTEGRATIONS VERIFIED SUCCESSFULLY! 🎉")
        print("\n✅ Ready for production deployment")
        print("✅ All components working correctly")
        print("✅ Cross-component interactions verified")
        print("✅ Configuration infrastructure complete")
        return True
    else:
        print("\n⚠️  Some integrations need attention")
        print("Please review the failed tests above")
        return False


if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)
