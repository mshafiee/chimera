#!/usr/bin/env python3
"""
Test script for State Persistence integration.

This script tests the state persistence integration:
1. State Persistence imports and initialization
2. Configuration integration
3. Credit history operations
4. Wallet performance persistence
5. ROI metrics tracking
6. Database operations and maintenance
"""

import sys
import os
from pathlib import Path
import tempfile

# Add Scout directory to path
sys.path.insert(0, str(Path(__file__).parent))

def test_state_persistence_import():
    """Test State Persistence imports."""
    print("Testing State Persistence imports...")

    try:
        from core.state_persistence import (
            StatePersistence, PersistenceConfig, CreditHistory,
            WalletPerformance, ROIMetrics, BudgetCategory
        )
        print("✓ State Persistence imports successful")
        return True
    except Exception as e:
        print(f"✗ State Persistence import failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_state_persistence_initialization():
    """Test State Persistence initialization."""
    print("\nTesting State Persistence initialization...")

    try:
        from core.state_persistence import StatePersistence, PersistenceConfig

        # Use temporary database for testing
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
            test_db = tmp.name

        try:
            # Test basic initialization
            config = PersistenceConfig(db_path=test_db)
            persistence = StatePersistence(config=config)
            print("✓ State Persistence initialized with custom config")

            # Test database stats
            stats = persistence.get_database_stats()
            total_records = (stats['credit_history_records'] +
                           stats['wallet_performance_records'] +
                           stats['roi_metrics_records'])
            print(f"✓ Database stats: {total_records} total records")
            print(f"  Database size: {stats['database_size_mb']:.2f} MB")

            return True
        finally:
            # Clean up test database
            if os.path.exists(test_db):
                os.unlink(test_db)

    except Exception as e:
        print(f"✗ State Persistence initialization failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_credit_history_operations():
    """Test credit history save/load operations."""
    print("\nTesting credit history operations...")

    try:
        from core.state_persistence import StatePersistence, PersistenceConfig, CreditHistory, BudgetCategory
        import time

        # Use temporary database
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
            test_db = tmp.name

        try:
            persistence = StatePersistence(PersistenceConfig(db_path=test_db))

            # Create test credit history
            history = CreditHistory(
                date="2025-06-27",
                total_credits=1000,
                credits_by_category={
                    BudgetCategory.DISCOVERY.value: 300,
                    BudgetCategory.ANALYSIS.value: 250,
                    BudgetCategory.VALIDATION.value: 200,
                    BudgetCategory.ENRICHMENT.value: 150,
                    BudgetCategory.MONITORING.value: 100,
                },
                credits_remaining=500,
                day_of_month=27,
                timestamp=time.time()
            )

            # Save credit history
            persistence.save_credit_history(history)
            print("✓ Credit history saved")

            # Load credit history
            loaded_history = persistence.load_credit_history(days=1)
            assert len(loaded_history) > 0, "Should have loaded credit history"
            assert loaded_history[0].total_credits == 1000
            print("✓ Credit history loaded successfully")

            # Test credit summary
            summary = persistence.get_credit_summary(days=1)
            assert summary['period_days'] > 0
            assert summary['total_credits'] > 0
            print(f"✓ Credit summary: {summary['total_credits']} credits over {summary['period_days']} days")

            return True
        finally:
            if os.path.exists(test_db):
                os.unlink(test_db)

    except Exception as e:
        print(f"✗ Credit history operations failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_wallet_performance_persistence():
    """Test wallet performance persistence."""
    print("\nTesting wallet performance persistence...")

    try:
        from core.state_persistence import StatePersistence, PersistenceConfig, WalletPerformance
        import time

        # Use temporary database
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
            test_db = tmp.name

        try:
            persistence = StatePersistence(PersistenceConfig(db_path=test_db))

            # Create test wallet performance
            performance = WalletPerformance(
                wallet_address="test_wallet_123",
                wqs_score=75.0,
                total_trades=100,
                winning_trades=65,
                total_pnl=500.0,
                avg_pnl=5.0,
                win_rate=0.65,
                roi_score=2.5,
                first_seen=time.time() - 86400,  # 1 day ago
                last_updated=time.time()
            )

            # Save wallet performance
            persistence.save_wallet_performance(performance)
            print("✓ Wallet performance saved")

            # Load wallet performance
            loaded_performance = persistence.load_wallet_performance(wallet_address="test_wallet_123")
            assert "test_wallet_123" in loaded_performance
            assert loaded_performance["test_wallet_123"].wqs_score == 75.0
            print("✓ Wallet performance loaded successfully")

            # Load all wallet performance
            all_performance = persistence.load_wallet_performance()
            assert len(all_performance) > 0
            print(f"✓ All wallet performance loaded: {len(all_performance)} wallets")

            return True
        finally:
            if os.path.exists(test_db):
                os.unlink(test_db)

    except Exception as e:
        print(f"✗ Wallet performance persistence failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_roi_metrics_persistence():
    """Test ROI metrics persistence."""
    print("\nTesting ROI metrics persistence...")

    try:
        from core.state_persistence import StatePersistence, PersistenceConfig, ROIMetrics
        import time

        # Use temporary database
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
            test_db = tmp.name

        try:
            persistence = StatePersistence(PersistenceConfig(db_path=test_db))

            # Create test ROI metrics
            metrics = ROIMetrics(
                category="discovery",
                credits_consumed=500,
                value_generated=1250.0,
                roi_score=2.5,
                operations_count=50,
                period_start=time.time() - 86400,
                period_end=time.time()
            )

            # Save ROI metrics
            persistence.save_roi_metrics(metrics)
            print("✓ ROI metrics saved")

            # Load ROI metrics
            loaded_metrics = persistence.load_roi_metrics(category="discovery")
            assert len(loaded_metrics) > 0
            assert loaded_metrics[0].roi_score == 2.5
            print("✓ ROI metrics loaded successfully")

            # Load all ROI metrics
            all_metrics = persistence.load_roi_metrics()
            assert len(all_metrics) > 0
            print(f"✓ All ROI metrics loaded: {len(all_metrics)} categories")

            return True
        finally:
            if os.path.exists(test_db):
                os.unlink(test_db)

    except Exception as e:
        print(f"✗ ROI metrics persistence failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_database_maintenance():
    """Test database maintenance operations."""
    print("\nTesting database maintenance operations...")

    try:
        from core.state_persistence import StatePersistence, PersistenceConfig, CreditHistory
        import time

        # Use temporary database
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
            test_db = tmp.name

        try:
            persistence = StatePersistence(PersistenceConfig(db_path=test_db, max_history_days=1))

            # Add some test data
            old_history = CreditHistory(
                date="2025-06-01",
                total_credits=100,
                credits_by_category={"discovery": 50, "analysis": 50},
                credits_remaining=100,
                day_of_month=1,
                timestamp=time.time() - 86400 * 30  # 30 days ago
            )
            persistence.save_credit_history(old_history)

            # Cleanup old history
            removed_count = persistence.cleanup_old_history()
            print(f"✓ Cleanup removed {removed_count} old records")

            # Vacuum database
            persistence.vacuum_database()
            print("✓ Database vacuumed")

            # Backup database
            backup_path = persistence.backup_database()
            print(f"✓ Database backed up to: {backup_path}")

            # Clean up backup
            if os.path.exists(backup_path):
                os.unlink(backup_path)

            return True
        finally:
            if os.path.exists(test_db):
                os.unlink(test_db)

    except Exception as e:
        print(f"✗ Database maintenance operations failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_config_integration():
    """Test Scout config integration."""
    print("\nTesting Scout config integration...")

    try:
        from config import ScoutConfig

        # Test state persistence configuration methods
        persistence_enabled = ScoutConfig.get_state_persistence_enabled()
        print(f"✓ State persistence enabled: {persistence_enabled}")

        db_path = ScoutConfig.get_state_persistence_db_path()
        print(f"✓ Database path: {db_path}")

        max_days = ScoutConfig.get_state_persistence_max_days()
        print(f"✓ Max history days: {max_days}")

        backup_enabled = ScoutConfig.get_state_persistence_backup_enabled()
        print(f"✓ Backup enabled: {backup_enabled}")

        backup_interval = ScoutConfig.get_state_persistence_backup_interval()
        print(f"✓ Backup interval: {backup_interval} hours")

        vacuum_interval = ScoutConfig.get_state_persistence_vacuum_interval()
        print(f"✓ Vacuum interval: {vacuum_interval} days")

        credit_history_enabled = ScoutConfig.get_state_persistence_credit_history_enabled()
        print(f"✓ Credit history enabled: {credit_history_enabled}")

        wallet_performance_enabled = ScoutConfig.get_state_persistence_wallet_performance_enabled()
        print(f"✓ Wallet performance enabled: {wallet_performance_enabled}")

        roi_metrics_enabled = ScoutConfig.get_state_persistence_roi_metrics_enabled()
        print(f"✓ ROI metrics enabled: {roi_metrics_enabled}")

        return True
    except Exception as e:
        print(f"✗ Scout config integration failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def main():
    """Run all state persistence integration tests."""
    print("=" * 70)
    print("State Persistence Integration Tests")
    print("=" * 70)

    tests = [
        test_state_persistence_import,
        test_state_persistence_initialization,
        test_credit_history_operations,
        test_wallet_performance_persistence,
        test_roi_metrics_persistence,
        test_database_maintenance,
        test_config_integration,
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