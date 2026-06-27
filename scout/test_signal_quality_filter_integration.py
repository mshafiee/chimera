#!/usr/bin/env python3
"""
Test script for Signal Quality Filter integration.

This script tests the signal quality filter integration:
1. Signal Quality Filter imports and initialization
2. Configuration integration
3. Multi-factor quality scoring
4. Top-percentile filtering
5. Dynamic threshold adjustment
6. Statistics and state persistence
"""

import sys
import os
from pathlib import Path
import tempfile
import time

# Add Scout directory to path
sys.path.insert(0, str(Path(__file__).parent))

def test_signal_quality_filter_import():
    """Test Signal Quality Filter imports."""
    print("Testing Signal Quality Filter imports...")

    try:
        from core.signal_quality_filter import (
            SignalQualityFilter, FilterConfig, TradingSignal,
            QualityScore, SignalQuality, ExecuteDecision
        )
        print("✓ Signal Quality Filter imports successful")
        return True
    except Exception as e:
        print(f"✗ Signal Quality Filter import failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_signal_quality_filter_initialization():
    """Test Signal Quality Filter initialization."""
    print("\nTesting Signal Quality Filter initialization...")

    try:
        from core.signal_quality_filter import SignalQualityFilter, FilterConfig

        # Test basic initialization
        filter = SignalQualityFilter()
        print("✓ Signal Quality Filter initialized with defaults")

        # Test with custom config
        config = FilterConfig(
            WQS_WEIGHT=0.35,  # Higher weight for WQS
            TIMING_WEIGHT=0.25,
            REGIME_WEIGHT=0.20,
            ENSEMBLE_WEIGHT=0.15,
            FRESHNESS_WEIGHT=0.05,
            TOP_PERCENTILE_TARGET=15.0,  # Top 15% only
        )
        filter_custom = SignalQualityFilter(config=config)
        print("✓ Signal Quality Filter initialized with custom config")

        return True
    except Exception as e:
        print(f"✗ Signal Quality Filter initialization failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_trading_signal_creation():
    """Test TradingSignal creation and evaluation."""
    print("\nTesting TradingSignal creation...")

    try:
        from core.signal_quality_filter import TradingSignal

        # Create a high-quality signal
        high_quality_signal = TradingSignal(
            wallet_address="high_quality_wallet",
            token_address="token_xyz",
            wqs_score=85.0,
            timing_score=0.8,
            market_regime="BULL",
            ensemble_confidence=0.75,
            signal_age_seconds=60,
            pnl_prediction=0.15,
        )
        print("✓ High-quality trading signal created")

        # Create a low-quality signal
        low_quality_signal = TradingSignal(
            wallet_address="low_quality_wallet",
            token_address="token_abc",
            wqs_score=35.0,
            timing_score=0.4,
            market_regime="BEAR",
            ensemble_confidence=0.55,
            signal_age_seconds=240,
            pnl_prediction=0.05,
        )
        print("✓ Low-quality trading signal created")

        return True
    except Exception as e:
        print(f"✗ TradingSignal creation failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_quality_scoring():
    """Test multi-factor quality scoring."""
    print("\nTesting quality scoring...")

    try:
        from core.signal_quality_filter import SignalQualityFilter, TradingSignal

        filter = SignalQualityFilter()

        # Create test signal
        signal = TradingSignal(
            wallet_address="test_wallet",
            token_address="test_token",
            wqs_score=75.0,
            timing_score=0.7,
            market_regime="BULL",
            ensemble_confidence=0.8,
            signal_age_seconds=90,
            pnl_prediction=0.12,
        )

        # Calculate percentile
        percentile = filter.calculate_signal_percentile(signal)
        print(f"✓ Signal percentile: {percentile:.1f}")

        assert 0 <= percentile <= 100, "Percentile should be between 0-100"

        return True
    except Exception as e:
        print(f"✗ Quality scoring failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_execution_decision():
    """Test execution decision making."""
    print("\nTesting execution decision making...")

    try:
        from core.signal_quality_filter import SignalQualityFilter, TradingSignal, ExecuteDecision

        filter = SignalQualityFilter()

        # Create high-quality signal (should execute)
        high_quality = TradingSignal(
            wallet_address="high_wallet",
            token_address="high_token",
            wqs_score=90.0,
            timing_score=0.9,
            market_regime="BULL",
            ensemble_confidence=0.85,
            signal_age_seconds=45,
            pnl_prediction=0.20,
        )

        decision = filter.should_execute_signal(high_quality)
        print(f"✓ High-quality signal decision: {decision.decision.value}")
        print(f"  Overall score: {decision.overall_score:.3f}")
        print(f"  Percentile: {decision.percentile:.1f}")

        # Create low-quality signal (should skip)
        low_quality = TradingSignal(
            wallet_address="low_wallet",
            token_address="low_token",
            wqs_score=30.0,
            timing_score=0.3,
            market_regime="BEAR",
            ensemble_confidence=0.45,
            signal_age_seconds=300,
            pnl_prediction=0.02,
        )

        decision_low = filter.should_execute_signal(low_quality)
        print(f"✓ Low-quality signal decision: {decision_low.decision.value}")
        print(f"  Overall score: {decision_low.overall_score:.3f}")
        print(f"  Percentile: {decision_low.percentile:.1f}")

        # Verify decisions make sense
        assert decision.decision in [ExecuteDecision.EXECUTE, ExecuteDecision.DELAY]
        assert decision_low.decision in [ExecuteDecision.SKIP, ExecuteDecision.HOLD]

        return True
    except Exception as e:
        print(f"✗ Execution decision test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_dynamic_threshold_adjustment():
    """Test dynamic threshold adjustment based on performance."""
    print("\nTesting dynamic threshold adjustment...")

    try:
        from core.signal_quality_filter import SignalQualityFilter

        filter = SignalQualityFilter()

        # Simulate good performance (should tighten threshold)
        good_performance = [0.15, 0.12, 0.18, 0.14, 0.16] * 3  # 15 good results
        filter.update_threshold_based_on_performance(good_performance)

        threshold_after_good = filter.get_top_percentile_threshold()
        print(f"✓ Threshold after good performance: top {threshold_after_good:.1f}%")

        # Simulate poor performance (should relax threshold)
        poor_performance = [0.02, -0.05, 0.01, -0.03, 0.04] * 3  # 15 poor results
        filter.update_threshold_based_on_performance(poor_performance)

        threshold_after_poor = filter.get_top_percentile_threshold()
        print(f"✓ Threshold after poor performance: top {threshold_after_poor:.1f}%")

        # Verify threshold moved in expected direction
        assert threshold_after_poor >= threshold_after_good, "Threshold should relax after poor performance"

        return True
    except Exception as e:
        print(f"✗ Dynamic threshold adjustment failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_filter_statistics():
    """Test filter statistics and reporting."""
    print("\nTesting filter statistics...")

    try:
        from core.signal_quality_filter import SignalQualityFilter, TradingSignal

        filter = SignalQualityFilter()

        # Create some test signals
        for i in range(10):
            signal = TradingSignal(
                wallet_address=f"wallet_{i}",
                token_address=f"token_{i}",
                wqs_score=50.0 + i * 5,  # 50 to 95
                timing_score=0.5 + i * 0.05,
                market_regime="BULL",
                ensemble_confidence=0.6 + i * 0.03,
                signal_age_seconds=60,
                pnl_prediction=0.1,
            )

            decision = filter.should_execute_signal(signal)
            # Simulate execution results
            pnl = 0.15 if i >= 7 else 0.02  # Better performance for higher WQS
            filter.record_execution_result(signal, pnl)

        # Get statistics
        stats = filter.get_filter_stats()
        print(f"✓ Filter statistics:")
        print(f"  Total signals: {stats['total_signals']}")
        print(f"  Executed count: {stats['executed_count']}")
        print(f"  Skipped count: {stats['skipped_count']}")
        print(f"  Execution rate: {stats['execution_rate']:.1%}")
        print(f"  Current threshold: top {stats['current_threshold']:.1f}%")
        print(f"  Recent win rate: {stats['recent_win_rate']:.1%}")
        print(f"  Avg quality score: {stats['avg_quality_score']:.3f}")

        # Get quality distribution
        distribution = filter.get_quality_distribution()
        print(f"✓ Quality distribution: {distribution}")

        return True
    except Exception as e:
        print(f"✗ Filter statistics test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_state_persistence():
    """Test state persistence and loading."""
    print("\nTesting state persistence...")

    try:
        from core.signal_quality_filter import SignalQualityFilter, TradingSignal
        import tempfile
        import json

        # Use temporary state file
        with tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False) as tmp:
            state_file = tmp.name

        try:
            # Create filter with custom state file
            filter = SignalQualityFilter()
            filter._state_file = state_file

            # Add some signals
            for i in range(5):
                signal = TradingSignal(
                    wallet_address=f"wallet_{i}",
                    token_address=f"token_{i}",
                    wqs_score=60.0 + i * 10,
                    timing_score=0.6,
                    market_regime="BULL",
                    ensemble_confidence=0.7,
                    signal_age_seconds=60,
                    pnl_prediction=0.1,
                )
                decision = filter.should_execute_signal(signal)

            # Save state
            filter.save_state()
            print("✓ State saved")

            # Create new filter and load state
            new_filter = SignalQualityFilter()
            new_filter._state_file = state_file
            new_filter._load_state()
            print("✓ State loaded")

            # Verify statistics persist
            original_stats = filter.get_filter_stats()
            loaded_stats = new_filter.get_filter_stats()

            assert original_stats['total_signals'] == loaded_stats['total_signals']
            print("✓ Statistics persisted correctly")

            return True
        finally:
            # Clean up state file
            if os.path.exists(state_file):
                os.unlink(state_file)

    except Exception as e:
        print(f"✗ State persistence test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_config_integration():
    """Test Scout config integration."""
    print("\nTesting Scout config integration...")

    try:
        from config import ScoutConfig

        # Test signal quality filter configuration methods
        filter_enabled = ScoutConfig.get_signal_quality_filter_enabled()
        print(f"✓ Signal quality filter enabled: {filter_enabled}")

        wqs_weight = ScoutConfig.get_wqs_weight()
        print(f"✓ WQS weight: {wqs_weight}")

        timing_weight = ScoutConfig.get_timing_weight()
        print(f"✓ Timing weight: {timing_weight}")

        regime_weight = ScoutConfig.get_regime_weight()
        print(f"✓ Regime weight: {regime_weight}")

        ensemble_weight = ScoutConfig.get_ensemble_weight()
        print(f"✓ Ensemble weight: {ensemble_weight}")

        freshness_weight = ScoutConfig.get_freshness_weight()
        print(f"✓ Freshness weight: {freshness_weight}")

        top_percentile = ScoutConfig.get_top_percentile_target()
        print(f"✓ Top percentile target: {top_percentile}%")

        min_threshold = ScoutConfig.get_min_percentile_threshold()
        print(f"✓ Min percentile threshold: {min_threshold}%")

        max_threshold = ScoutConfig.get_max_percentile_threshold()
        print(f"✓ Max percentile threshold: {max_threshold}%")

        adaptive_threshold = ScoutConfig.get_signal_quality_adaptive_threshold()
        print(f"✓ Adaptive threshold: {adaptive_threshold}")

        return True
    except Exception as e:
        print(f"✗ Scout config integration failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_quality_levels():
    """Test quality level classification."""
    print("\nTesting quality level classification...")

    try:
        from core.signal_quality_filter import SignalQualityFilter, TradingSignal, SignalQuality

        filter = SignalQualityFilter()

        # Test different quality levels
        quality_levels = []

        for wqs in [95, 85, 75, 65, 55, 45, 35]:
            signal = TradingSignal(
                wallet_address="test_wallet",
                token_address="test_token",
                wqs_score=float(wqs),
                timing_score=0.7,
                market_regime="BULL",
                ensemble_confidence=0.75,
                signal_age_seconds=60,
                pnl_prediction=0.1,
            )

            decision = filter.should_execute_signal(signal)
            quality_levels.append((wqs, decision.quality_level))

        print("✓ Quality level classification:")
        for wqs, level in quality_levels:
            print(f"  WQS {wqs}: {level.value}")

        return True
    except Exception as e:
        print(f"✗ Quality level classification failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def main():
    """Run all signal quality filter integration tests."""
    print("=" * 70)
    print("Signal Quality Filter Integration Tests")
    print("=" * 70)

    tests = [
        test_signal_quality_filter_import,
        test_signal_quality_filter_initialization,
        test_trading_signal_creation,
        test_quality_scoring,
        test_execution_decision,
        test_dynamic_threshold_adjustment,
        test_filter_statistics,
        test_state_persistence,
        test_config_integration,
        test_quality_levels,
    ]

    results = []
    for test in tests:
        try:
            result = test()
            results.append(result)
        except Exception as e:
            print(f"\n✗ Test failed with exception: {e}")
            import traceback
            traceback.print_exc()
            results.append(False)

    print("\n" + "=" * 70)
    print(f"Test Results: {sum(results)}/{len(results)} passed")
    print("=" * 70)

    return all(results)


if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)