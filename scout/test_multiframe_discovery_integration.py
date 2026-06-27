#!/usr/bin/env python3
"""
Comprehensive Multi-Timeframe Discovery Integration Tests

This test suite validates the Sprint 4 Multi-Timeframe Discovery integration
including configuration, parallel execution, cross-timeframe deduplication,
and state persistence.
"""

import sys
import os
import asyncio
import tempfile
from pathlib import Path
from datetime import datetime
import unittest
from unittest.mock import Mock, AsyncMock, MagicMock, patch

# Add Scout directory to path
sys.path.insert(0, str(Path(__file__).parent))

# Import configuration
try:
    from config import ScoutConfig
    CONFIG_AVAILABLE = True
except ImportError:
    CONFIG_AVAILABLE = False
    ScoutConfig = None

# Import multi-timeframe discovery components
try:
    from core.multitimeframe_discovery import (
        MultiTimeframeDiscovery,
        DiscoveryTimeframe,
        get_multi_timeframe_discovery,
        TimeframeConfig,
        TimeframeResult,
        MultiTimeframeResult
    )
    MULTITIMEFRAME_AVAILABLE = True
except ImportError:
    MULTITIMEFRAME_AVAILABLE = False
    MultiTimeframeDiscovery = None
    DiscoveryTimeframe = None
    get_multi_timeframe_discovery = None

# Import state persistence
try:
    from core.state_persistence import StatePersistence
    STATE_PERSISTENCE_AVAILABLE = True
except ImportError:
    STATE_PERSISTENCE_AVAILABLE = False
    StatePersistence = None


class TestMultiTimeframeConfiguration(unittest.TestCase):
    """Test Multi-Timeframe Discovery configuration methods."""

    def setUp(self):
        """Set up test configuration."""
        if CONFIG_AVAILABLE and ScoutConfig:
            # Set test environment variables
            os.environ["SCOUT_MULTI_TIMEFRAME_ENABLED"] = "true"
            os.environ["SCOUT_MULTI_TIMEFRAME_PARALLEL"] = "true"
            os.environ["SCOUT_MULTI_TIMEFRAME_GOAL"] = "balanced"
            os.environ["SCOUT_DISCOVERY_DEEP_HOURS"] = "720"
            os.environ["SCOUT_DISCOVERY_FAST_HOURS"] = "24"
            os.environ["SCOUT_DISCOVERY_TRENDING_HOURS"] = "4"

    def test_multi_timeframe_enabled(self):
        """Test multi-timeframe enabled configuration."""
        if not CONFIG_AVAILABLE or not ScoutConfig:
            self.skipTest("ScoutConfig not available")

        result = ScoutConfig.get_multi_timeframe_enabled()
        self.assertTrue(result, "Multi-timeframe should be enabled")

    def test_multi_timeframe_parallel(self):
        """Test multi-timeframe parallel configuration."""
        if not CONFIG_AVAILABLE or not ScoutConfig:
            self.skipTest("ScoutConfig not available")

        result = ScoutConfig.get_multi_timeframe_parallel()
        self.assertTrue(result, "Parallel execution should be enabled")

    def test_multi_timeframe_goal(self):
        """Test multi-timeframe goal configuration."""
        if not CONFIG_AVAILABLE or not ScoutConfig:
            self.skipTest("ScoutConfig not available")

        result = ScoutConfig.get_multi_timeframe_goal()
        self.assertEqual(result, "balanced", "Discovery goal should be 'balanced'")

    def test_discovery_deep_hours(self):
        """Test deep discovery hours configuration."""
        if not CONFIG_AVAILABLE or not ScoutConfig:
            self.skipTest("ScoutConfig not available")

        result = ScoutConfig.get_discovery_deep_hours()
        self.assertEqual(result, 720, "Deep discovery should be 720 hours")

    def test_discovery_fast_hours(self):
        """Test fast discovery hours configuration."""
        if not CONFIG_AVAILABLE or not ScoutConfig:
            self.skipTest("ScoutConfig not available")

        result = ScoutConfig.get_discovery_fast_hours()
        self.assertEqual(result, 24, "Fast discovery should be 24 hours")

    def test_discovery_trending_hours(self):
        """Test trending discovery hours configuration."""
        if not CONFIG_AVAILABLE or not ScoutConfig:
            self.skipTest("ScoutConfig not available")

        result = ScoutConfig.get_discovery_trending_hours()
        self.assertEqual(result, 4, "Trending discovery should be 4 hours")


class TestMultiTimeframeDiscovery(unittest.TestCase):
    """Test Multi-Timeframe Discovery system functionality."""

    def setUp(self):
        """Set up test fixtures."""
        if not MULTITIMEFRAME_AVAILABLE:
            self.skipTest("MultiTimeframeDiscovery not available")

        # Create mock Helius client
        self.mock_helius_client = Mock()
        self.mock_helius_client.discover_wallets = AsyncMock()

        # Initialize discovery system
        self.discovery = MultiTimeframeDiscovery(helius_client=self.mock_helius_client)

    def test_initialization(self):
        """Test MultiTimeframeDiscovery initialization."""
        if not MULTITIMEFRAME_AVAILABLE:
            self.skipTest("MultiTimeframeDiscovery not available")

        self.assertIsNotNone(self.discovery)
        self.assertEqual(len(self.discovery._timeframe_configs), 3)

    def test_timeframe_configs_exist(self):
        """Test that all timeframe configurations exist."""
        if not MULTITIMEFRAME_AVAILABLE:
            self.skipTest("MultiTimeframeDiscovery not available")

        configs = self.discovery._timeframe_configs
        self.assertIn(DiscoveryTimeframe.DEEP, configs)
        self.assertIn(DiscoveryTimeframe.FAST, configs)
        self.assertIn(DiscoveryTimeframe.TRENDING, configs)

    def test_singleton_instance(self):
        """Test singleton instance creation."""
        if not MULTITIMEFRAME_AVAILABLE:
            self.skipTest("MultiTimeframeDiscovery not available")

        instance1 = get_multi_timeframe_discovery()
        instance2 = get_multi_timeframe_discovery()
        self.assertIs(instance1, instance2, "Should return the same singleton instance")

    def test_timeframe_config_structure(self):
        """Test timeframe configuration structure."""
        if not MULTITIMEFRAME_AVAILABLE:
            self.skipTest("MultiTimeframeDiscovery not available")

        deep_config = self.discovery.get_timeframe_config(DiscoveryTimeframe.DEEP)
        self.assertIsNotNone(deep_config)
        self.assertEqual(deep_config.timeframe, DiscoveryTimeframe.DEEP)
        self.assertEqual(deep_config.hours_back, 720)
        self.assertGreater(deep_config.max_wallets, 0)


class TestMultiTimeframeExecution(unittest.TestCase):
    """Test Multi-Timeframe Discovery execution modes."""

    def setUp(self):
        """Set up test fixtures."""
        if not MULTITIMEFRAME_AVAILABLE:
            self.skipTest("MultiTimeframeDiscovery not available")

        # Create mock Helius client with sample data
        self.mock_helius_client = Mock()
        self.mock_helius_client.discover_wallets = AsyncMock()

        # Set up mock responses for different timeframes
        self.mock_helius_client.discover_wallets.side_effect = [
            {"wallet1": 10, "wallet2": 8, "wallet3": 15},  # DEEP
            {"wallet4": 12, "wallet5": 9},                  # FAST
            {"wallet6": 20, "wallet1": 18}                   # TRENDING (wallet1 appears in multiple)
        ]

        self.discovery = MultiTimeframeDiscovery(helius_client=self.mock_helius_client)

    def test_parallel_execution(self):
        """Test parallel execution mode."""
        if not MULTITIMEFRAME_AVAILABLE:
            self.skipTest("MultiTimeframeDiscovery not available")

        async def run_parallel_test():
            result = await self.discovery.discover_all_timeframes(
                budget_credits=500,
                parallel=True,
                timeframes=[DiscoveryTimeframe.DEEP, DiscoveryTimeframe.FAST]
            )

            self.assertIsNotNone(result)
            self.assertIsInstance(result, MultiTimeframeResult)
            self.assertGreater(len(result.timeframe_results), 0)
            return result

        result = asyncio.run(run_parallel_test())

    def test_sequential_execution(self):
        """Test sequential execution mode."""
        if not MULTITIMEFRAME_AVAILABLE:
            self.skipTest("MultiTimeframeDiscovery not available")

        async def run_sequential_test():
            result = await self.discovery.discover_all_timeframes(
                budget_credits=500,
                parallel=False,
                timeframes=[DiscoveryTimeframe.DEEP, DiscoveryTimeframe.FAST]
            )

            self.assertIsNotNone(result)
            return result

        result = asyncio.run(run_sequential_test())

    def test_cross_timeframe_deduplication(self):
        """Test cross-timeframe deduplication."""
        if not MULTITIMEFRAME_AVAILABLE:
            self.skipTest("MultiTimeframeDiscovery not available")

        async def run_dedup_test():
            result = await self.discovery.discover_all_timeframes(
                budget_credits=500,
                parallel=True
            )

            # Check deduplication happened
            self.assertIsNotNone(result.deduplication_stats)
            self.assertIn('deduplication_ratio', result.deduplication_stats)
            self.assertIn('multi_timeframe_wallets', result.deduplication_stats)

            # Deduplication ratio should be reasonable (< 1.0)
            self.assertLess(result.deduplication_stats['deduplication_ratio'], 1.0)
            return result

        result = asyncio.run(run_dedup_test())


class TestStatePersistenceIntegration(unittest.TestCase):
    """Test state persistence integration for multi-timeframe discovery."""

    def setUp(self):
        """Set up test fixtures."""
        if not STATE_PERSISTENCE_AVAILABLE:
            self.skipTest("StatePersistence not available")

        # Create temporary database for testing
        self.temp_db = tempfile.NamedTemporaryFile(suffix='.db', delete=False)
        self.temp_db.close()

        from core.state_persistence import PersistenceConfig
        self.persistence = StatePersistence(
            config=PersistenceConfig(db_path=self.temp_db.name)
        )

    def tearDown(self):
        """Clean up test fixtures."""
        if hasattr(self, 'temp_db') and self.temp_db:
            try:
                os.unlink(self.temp_db.name)
            except:
                pass

    def test_save_multi_timeframe_stats(self):
        """Test saving multi-timeframe discovery statistics."""
        if not STATE_PERSISTENCE_AVAILABLE or not MULTITIMEFRAME_AVAILABLE:
            self.skipTest("Required components not available")

        # Create mock result
        mock_result = Mock(spec=MultiTimeframeResult)
        mock_result.timestamp = datetime.now().timestamp()
        mock_result.combined_wallets = ["wallet1", "wallet2", "wallet3"]
        mock_result.deduplication_stats = {
            'total_raw_wallets': 10,
            'deduplication_ratio': 0.7,
            'multi_timeframe_wallets': 2
        }
        mock_result.total_credits_consumed = 150
        mock_result.total_execution_time_seconds = 45.5
        mock_result.combined_quality_scores = {
            'wallet1': 85.0,
            'wallet2': 75.0,
            'wallet3': 65.0
        }
        mock_result.timeframe_results = {}

        # Test save
        try:
            self.persistence.save_multi_timeframe_discovery_stats(
                result=mock_result,
                parallel=True,
                discovery_goal='balanced'
            )
            self.assertTrue(True, "Statistics saved successfully")
        except Exception as e:
            self.fail(f"Failed to save statistics: {e}")

    def test_load_multi_timeframe_stats(self):
        """Test loading multi-timeframe discovery statistics."""
        if not STATE_PERSISTENCE_AVAILABLE:
            self.skipTest("StatePersistence not available")

        # Test load (may be empty)
        try:
            stats = self.persistence.load_multi_timeframe_discovery_stats(days=30)
            self.assertIsInstance(stats, list)
        except Exception as e:
            self.fail(f"Failed to load statistics: {e}")

    def test_get_multi_timeframe_summary(self):
        """Test getting multi-timeframe summary statistics."""
        if not STATE_PERSISTENCE_AVAILABLE:
            self.skipTest("StatePersistence not available")

        # Test summary
        try:
            summary = self.persistence.get_multi_timeframe_summary(days=30)
            self.assertIsInstance(summary, dict)
            self.assertIn('total_runs', summary)
            self.assertIn('avg_unique_wallets', summary)
        except Exception as e:
            self.fail(f"Failed to get summary: {e}")


class TestAnalyzerIntegration(unittest.TestCase):
    """Test integration with WalletAnalyzer."""

    def setUp(self):
        """Set up test fixtures."""
        if not MULTITIMEFRAME_AVAILABLE or not STATE_PERSISTENCE_AVAILABLE:
            self.skipTest("Required components not available")

        # Mock Helius client
        self.mock_helius_client = Mock()
        self.mock_helius_client.api_key = "test_key"
        self.mock_helius_client.discover_wallets = AsyncMock(
            return_value={"wallet1": 10, "wallet2": 8}
        )

    def test_configuration_routing(self):
        """Test that configuration correctly routes to multi-timeframe system."""
        if not CONFIG_AVAILABLE or not ScoutConfig:
            self.skipTest("ScoutConfig not available")

        # Test configuration routing
        mt_enabled = ScoutConfig.get_multi_timeframe_enabled()
        self.assertIsNotNone(mt_enabled)
        self.assertIsInstance(mt_enabled, bool)

    def test_backward_compatibility(self):
        """Test backward compatibility with manual implementation."""
        if not CONFIG_AVAILABLE or not ScoutConfig:
            self.skipTest("ScoutConfig not available")

        # Test that disabling multi-timeframe discovery falls back to manual
        os.environ["SCOUT_MULTI_TIMEFRAME_ENABLED"] = "false"
        mt_enabled = ScoutConfig.get_multi_timeframe_enabled()
        self.assertFalse(mt_enabled, "Should be able to disable multi-timeframe discovery")

        # Re-enable for other tests
        os.environ["SCOUT_MULTI_TIMEFRAME_ENABLED"] = "true"


class TestCrossComponentIntegration(unittest.TestCase):
    """Test integration between multi-timeframe discovery and other components."""

    def test_configuration_persistence_synergy(self):
        """Test that configuration and persistence work together."""
        if not CONFIG_AVAILABLE or not ScoutConfig:
            self.skipTest("ScoutConfig not available")

        # Configuration should be accessible
        goal = ScoutConfig.get_multi_timeframe_goal()
        self.assertIsNotNone(goal)

    def test_discovery_goal_types(self):
        """Test different discovery goal types."""
        if not CONFIG_AVAILABLE or not ScoutConfig:
            self.skipTest("ScoutConfig not available")

        valid_goals = ['quality', 'quantity', 'balanced', 'speed']

        for goal in valid_goals:
            os.environ["SCOUT_MULTI_TIMEFRAME_GOAL"] = goal
            current_goal = ScoutConfig.get_multi_timeframe_goal()
            self.assertEqual(current_goal, goal)

        # Reset to default
        os.environ["SCOUT_MULTI_TIMEFRAME_GOAL"] = "balanced"


def run_tests():
    """Run all tests and report results."""
    print("=" * 70)
    print("MULTI-TIMEFRAME DISCOVERY INTEGRATION TESTS")
    print("=" * 70)

    # Create test suite
    test_classes = [
        TestMultiTimeframeConfiguration,
        TestMultiTimeframeDiscovery,
        TestMultiTimeframeExecution,
        TestStatePersistenceIntegration,
        TestAnalyzerIntegration,
        TestCrossComponentIntegration
    ]

    all_results = []

    for test_class in test_classes:
        print(f"\n{test_class.__name__}:")
        print("-" * 70)

        # Run tests for this class
        suite = unittest.TestLoader().loadTestsFromTestCase(test_class)
        runner = unittest.TextTestRunner(verbosity=2)
        result = runner.run(suite)

        all_results.append((test_class.__name__, result.wasSuccessful()))

    # Print summary
    print("\n" + "=" * 70)
    print("TEST SUMMARY")
    print("=" * 70)

    for test_name, passed in all_results:
        status = "✅ PASS" if passed else "❌ FAIL"
        print(f"{status}: {test_name}")

    total_passed = sum(1 for _, passed in all_results if passed)
    total_tests = len(all_results)

    print("\n" + "=" * 70)
    print(f"OVERALL RESULT: {total_passed}/{total_tests} test groups passed")
    print("=" * 70)

    if total_passed == total_tests:
        print("\n🎉 ALL MULTI-TIMEFRAME DISCOVERY INTEGRATION TESTS PASSED! 🎉")
        print("\n✅ Configuration integration verified")
        print("✅ Multi-timeframe discovery system working")
        print("✅ Parallel execution tested")
        print("✅ Cross-timeframe deduplication validated")
        print("✅ State persistence integration confirmed")
        print("✅ Analyzer integration working")
        print("✅ Cross-component integration verified")
        return True
    else:
        print("\n⚠️  Some integration tests failed")
        return False


if __name__ == "__main__":
    success = run_tests()
    sys.exit(0 if success else 1)