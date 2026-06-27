#!/usr/bin/env python3
"""
Test script for Stop-Loss Optimizer integration.

This script tests the stop-loss optimizer and position manager integration:
1. Stop-Loss Optimizer functionality
2. Position Manager operations
3. ATR calculation
4. Market regime adjustments
5. Stop-loss trigger detection
"""

import sys
import os
from pathlib import Path

# Add Scout directory to path
sys.path.insert(0, str(Path(__file__).parent))

def test_stop_loss_optimizer():
    """Test Stop-Loss Optimizer basic functionality."""
    print("Testing Stop-Loss Optimizer...")

    try:
        from core.stop_loss_optimizer import StopLossOptimizer, MarketRegime, StopLossConfig

        # Create optimizer with default config
        config = StopLossConfig()
        optimizer = StopLossOptimizer(config)
        print("✓ Stop-Loss Optimizer created successfully")

        # Test ATR calculation
        test_prices = [100.0, 102.0, 98.0, 105.0, 103.0, 107.0, 101.0,
                      106.0, 104.0, 108.0, 102.0, 109.0, 105.0, 110.0, 108.0]
        atr = optimizer.calculate_atr(test_prices, period=14)
        print(f"✓ ATR calculated: {atr:.2f}")
        assert atr > 0, "ATR should be positive"

        # Test ATR-based stop calculation
        stop_order = optimizer.calculate_atr_stop(
            entry_price=100.0,
            atr_value=atr,
            regime=MarketRegime.NEUTRAL,
            growth_stage="mid"
        )
        print(f"✓ Stop calculated: ${stop_order.stop_price:.2f}")
        assert stop_order.stop_price < 100.0, "Stop should be below entry for long"
        assert stop_order.stop_type.value == "atr", "Stop type should be ATR"

        # Test different market regimes
        bull_stop = optimizer.calculate_atr_stop(100.0, atr, MarketRegime.BULL, "mid")
        bear_stop = optimizer.calculate_atr_stop(100.0, atr, MarketRegime.BEAR, "mid")
        volatile_stop = optimizer.calculate_atr_stop(100.0, atr, MarketRegime.VOLATILE, "mid")

        print(f"✓ Bull market stop: ${bull_stop.stop_price:.2f}")
        print(f"✓ Bear market stop: ${bear_stop.stop_price:.2f}")
        print(f"✓ Volatile market stop: ${volatile_stop.stop_price:.2f}")

        # Verify regime adjustments
        assert bear_stop.stop_price > bull_stop.stop_price, "Bear stops should be tighter"
        assert volatile_stop.stop_price < bear_stop.stop_price, "Volatile stops should be widest"

        return True
    except Exception as e:
        print(f"✗ Stop-Loss Optimizer test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_position_manager():
    """Test Position Manager functionality."""
    print("\nTesting Position Manager...")

    try:
        from core.position_manager import PositionManager, Position, PositionSide, PositionStatus
        from core.stop_loss_optimizer import StopLossOptimizer, StopLossConfig

        # Create stop-loss optimizer
        config = StopLossConfig()
        optimizer = StopLossOptimizer(config)

        # Create position manager
        position_manager = PositionManager(optimizer)
        print("✓ Position Manager created successfully")

        # Create a test position
        position = position_manager.create_position(
            position_id="test_position_1",
            wallet_address="test_wallet",
            token_address="test_token",
            token_symbol="TEST",
            entry_price=100.0,
            position_size_sol=10.0,
            position_value_usd=1000.0,
            side=PositionSide.LONG,
            strategy="SHIELD",
            wqs_score=75.0
        )
        print(f"✓ Position created: {position.position_id}")
        print(f"  Entry: ${position.entry_price:.2f}")
        print(f"  Stop-loss: ${position.stop_loss_price:.2f}")
        print(f"  Stop type: {position.stop_type}")

        # Test price update
        updated_position = position_manager.update_position_price("test_position_1", 105.0)
        assert updated_position is not None, "Position should be found"
        assert updated_position.current_price == 105.0, "Price should be updated"
        assert updated_position.unrealized_pnl > 0, "Position should be profitable"
        print(f"✓ Position updated: ${updated_position.current_price:.2f}, PnL: ${updated_position.unrealized_pnl:.2f}")

        # Test stop-loss trigger
        # Update price below stop-loss to trigger exit
        trigger_position = position_manager.update_position_price("test_position_1", position.stop_loss_price - 1.0)
        assert trigger_position.status == PositionStatus.EXITING, "Position should be exiting"
        print(f"✓ Stop-loss triggered at ${trigger_position.current_price:.2f}")

        # Test position close
        closed_position = position_manager.close_position("test_position_1", 95.0, "Stop-loss exit")
        assert closed_position.status == PositionStatus.CLOSED, "Position should be closed"
        print(f"✓ Position closed: PnL ${closed_position.realized_pnl:.2f}")

        # Test position summary
        summary = position_manager.get_summary()
        print(f"✓ Position summary: {summary['total_positions']} total, {summary['active_positions']} active")

        return True
    except Exception as e:
        print(f"✗ Position Manager test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_market_regime_detector():
    """Test Market Regime Detector functionality."""
    print("\nTesting Market Regime Detector...")

    try:
        from core.market_regime_detector import MarketRegimeDetector, MarketRegime

        detector = MarketRegimeDetector()
        print("✓ Market Regime Detector created")

        # Test regime detection (with mock data)
        mock_market_data = {
            "price_changes": [2.5, 1.8, -0.5, 3.2, 1.1, -1.2, 2.8],
            "volatility_index": 18.5,
            "volume_trend": "increasing",
            "market_sentiment": "bullish"
        }

        classification = detector.detect_regime(mock_market_data)
        print(f"✓ Market regime detected: {classification.regime.value}")
        assert classification.regime in [MarketRegime.BULL, MarketRegime.BEAR, MarketRegime.NEUTRAL, MarketRegime.VOLATILE]

        return True
    except Exception as e:
        print(f"✗ Market Regime Detector test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def test_config_import():
    """Test Scout config with stop-loss options."""
    print("\nTesting Scout config import...")

    try:
        from config import ScoutConfig

        # Test stop-loss configuration methods
        stop_loss_enabled = ScoutConfig.get_stop_loss_enabled()
        print(f"✓ Stop-loss enabled: {stop_loss_enabled}")

        atr_period = ScoutConfig.get_atr_period()
        print(f"✓ ATR period: {atr_period}")

        bull_multiplier = ScoutConfig.get_bull_multiplier()
        print(f"✓ Bull multiplier: {bull_multiplier}")

        min_risk_reward = ScoutConfig.get_min_risk_reward()
        print(f"✓ Min risk/reward: {min_risk_reward}")

        return True
    except Exception as e:
        print(f"✗ Scout config test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


def main():
    """Run all stop-loss integration tests."""
    print("=" * 70)
    print("Stop-Loss Optimizer Integration Tests")
    print("=" * 70)

    tests = [
        test_stop_loss_optimizer,
        test_position_manager,
        test_market_regime_detector,
        test_config_import,
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