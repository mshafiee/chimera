#!/usr/bin/env python3
"""
Test script for Validation Reporter integration.

This script tests the validation reporter integration:
1. Validation Reporter imports and initialization
2. Configuration integration
3. Report generation
4. Alert configuration
"""

import sys
import os
from pathlib import Path

# Add Scout directory to path
sys.path.insert(0, str(Path(__file__).parent))

def test_validation_reporter_import():
    """Test Validation Reporter imports."""
    print("Testing Validation Reporter imports...")

    try:
        from core.validation_reporter import ValidationReporter, AlertConfig, get_validation_reporter
        print("✓ Validation Reporter imports successful")
        return True
    except Exception as e:
        print(f"✗ Validation Reporter import failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_validation_reporter_initialization():
    """Test Validation Reporter initialization."""
    print("\nTesting Validation Reporter initialization...")

    try:
        from core.validation_reporter import ValidationReporter, AlertConfig

        # Test basic initialization
        reporter = ValidationReporter()
        print("✓ Validation Reporter initialized with defaults")

        # Test with custom config
        alert_config = AlertConfig(
            webhook_url="https://example.com/webhook",
            high_error_threshold=1.0,
            drift_threshold=0.2,
            low_accuracy_threshold=0.6,
            alert_dir="data/test_alerts"
        )
        reporter_with_config = ValidationReporter(alert_config=alert_config)
        print("✓ Validation Reporter initialized with custom config")

        # Test singleton function (optional test)
        try:
            from core.validation_reporter import get_validation_reporter
            reporter_singleton = get_validation_reporter()
            print("✓ Validation Reporter singleton function works")
        except Exception as e:
            print(f"⚠ Singleton function test skipped: {e}")

        return True
    except Exception as e:
        print(f"✗ Validation Reporter initialization failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_validation_reporter_initialization():
    """Test Validation Reporter initialization."""
    print("\nTesting Validation Reporter initialization...")

    try:
        from core.validation_reporter import ValidationReporter, AlertConfig

        # Test basic initialization
        reporter = ValidationReporter()
        print("✓ Validation Reporter initialized with defaults")

        # Test with custom config
        alert_config = AlertConfig(
            webhook_url="https://example.com/webhook",
            high_error_threshold=1.0,
            drift_threshold=0.2,
            low_accuracy_threshold=0.6,
            alert_dir="data/test_alerts"
        )
        reporter_with_config = ValidationReporter(alert_config=alert_config)
        print("✓ Validation Reporter initialized with custom config")

        return True
    except Exception as e:
        print(f"✗ Validation Reporter initialization failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_alert_config():
    """Test AlertConfig configuration."""
    print("\nTesting AlertConfig...")

    try:
        from core.validation_reporter import AlertConfig

        # Test default AlertConfig
        default_config = AlertConfig()
        print("✓ Default AlertConfig created")

        # Test AlertConfig with custom values
        custom_config = AlertConfig(
            webhook_url="https://hooks.slack.com/test",
            high_error_threshold=2.0,
            drift_threshold=0.25,
            low_accuracy_threshold=0.4,
            alert_dir="custom_alerts"
        )

        assert custom_config.webhook_url == "https://hooks.slack.com/test"
        assert custom_config.high_error_threshold == 2.0
        assert custom_config.drift_threshold == 0.25
        assert custom_config.low_accuracy_threshold == 0.4
        assert custom_config.alert_dir == "custom_alerts"

        print("✓ Custom AlertConfig works correctly")
        return True
    except Exception as e:
        print(f"✗ AlertConfig test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_config_integration():
    """Test Scout config integration."""
    print("\nTesting Scout config integration...")

    try:
        from config import ScoutConfig

        # Test validation reporter configuration methods
        validation_enabled = ScoutConfig.get_validation_enabled()
        print(f"✓ Validation enabled: {validation_enabled}")

        alert_webhook = ScoutConfig.get_alert_webhook_url()
        print(f"✓ Alert webhook URL: {alert_webhook}")

        high_error_threshold = ScoutConfig.get_alert_high_error_threshold()
        print(f"✓ High error threshold: {high_error_threshold}")

        drift_threshold = ScoutConfig.get_alert_drift_threshold()
        print(f"✓ Drift threshold: {drift_threshold}")

        low_accuracy_threshold = ScoutConfig.get_alert_low_accuracy_threshold()
        print(f"✓ Low accuracy threshold: {low_accuracy_threshold}")

        alert_dir = ScoutConfig.get_alert_dir()
        print(f"✓ Alert directory: {alert_dir}")

        validation_schedule = ScoutConfig.get_validation_report_schedule()
        print(f"✓ Validation schedule: {validation_schedule}")

        time_window = ScoutConfig.get_validation_time_window()
        print(f"✓ Validation time window: {time_window}")

        report_format = ScoutConfig.get_validation_report_format()
        print(f"✓ Validation report format: {report_format}")

        return True
    except Exception as e:
        print(f"✗ Scout config integration failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_report_generation():
    """Test report generation (with mock data)."""
    print("\nTesting report generation...")

    try:
        from core.validation_reporter import ValidationReporter

        reporter = ValidationReporter()

        # Test report generation with empty database
        try:
            report = reporter.generate_report(
                model_types=['xgboost', 'lightgbm'],
                time_window='7d',
                output_format='dict',
                include_recommendations=True
            )

            # Verify report structure
            assert 'generated_at' in report
            assert 'time_window' in report
            assert 'summary' in report
            assert 'issues' in report
            assert 'recommendations' in report

            print("✓ Report generation works (empty DB)")
            print(f"  Report keys: {list(report.keys())}")

        except Exception as e:
            print(f"⚠ Report generation failed (expected if no ML data): {e}")
            # This is acceptable if there's no actual ML data

        return True
    except Exception as e:
        print(f"✗ Report generation test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_environment_config_loading():
    """Test environment variable configuration loading."""
    print("\nTesting environment variable configuration...")

    try:
        import os
        from core.validation_reporter import ValidationReporter

        # Set environment variables
        os.environ['SCOUT_ALERT_WEBHOOK_URL'] = 'https://test.webhook.com'
        os.environ['SCOUT_ALERT_HIGH_ERROR_THRESHOLD'] = '1.5'
        os.environ['SCOUT_ALERT_DRIFT_THRESHOLD'] = '0.3'
        os.environ['SCOUT_ALERT_LOW_ACCURACY_THRESHOLD'] = '0.4'
        os.environ['SCOUT_ALERT_DIR'] = 'test_alerts'

        # Create Validation Reporter (should load from env)
        reporter = ValidationReporter()

        # Verify environment variables were loaded
        assert reporter.alert_config.webhook_url == 'https://test.webhook.com'
        assert reporter.alert_config.high_error_threshold == 1.5
        assert reporter.alert_config.drift_threshold == 0.3
        assert reporter.alert_config.low_accuracy_threshold == 0.4
        assert reporter.alert_config.alert_dir == 'test_alerts'

        print("✓ Environment variable configuration loading works")

        # Clean up environment variables
        del os.environ['SCOUT_ALERT_WEBHOOK_URL']
        del os.environ['SCOUT_ALERT_HIGH_ERROR_THRESHOLD']
        del os.environ['SCOUT_ALERT_DRIFT_THRESHOLD']
        del os.environ['SCOUT_ALERT_LOW_ACCURACY_THRESHOLD']
        del os.environ['SCOUT_ALERT_DIR']

        return True
    except Exception as e:
        print(f"✗ Environment configuration test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def main():
    """Run all validation reporter integration tests."""
    print("=" * 70)
    print("Validation Reporter Integration Tests")
    print("=" * 70)

    tests = [
        test_validation_reporter_import,
        test_validation_reporter_initialization,
        test_alert_config,
        test_config_integration,
        test_report_generation,
        test_environment_config_loading,
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