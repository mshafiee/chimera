#!/usr/bin/env python3
"""
Example: Scout Optimization Integration

This example shows how to integrate the new optimization systems
into existing Scout code for maximum profitability with Helius Developer Plan constraints.

Key optimizations demonstrated:
1. Helius credit tracking and budgeting
2. Advanced caching to reduce API calls
3. ML-based profitability prediction
4. Growth-focused wallet selection
5. Production monitoring
"""

import sys
from pathlib import Path

# Add parent directory to path
sys.path.insert(0, str(Path(__file__).parent.parent))

from core.scout_optimizer import get_scout_optimizer


def example_optimized_analysis():
    """Example: Optimized wallet analysis using new systems."""
    print("="*70)
    print("EXAMPLE: Optimized Wallet Analysis with Scout Optimizations")
    print("="*70)

    # Initialize optimizer
    optimizer = get_scout_optimizer()
    if not optimizer.initialize():
        print("Failed to initialize optimizer")
        return

    # Start monitoring
    optimizer.start_monitoring()
    print("✓ Production monitoring started")

    # Example wallet metrics (normally from WalletAnalyzer)
    wallet_metrics = {
        'address': '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU',
        'roi_7d': 15.0,
        'roi_30d': 45.0,
        'win_rate': 0.72,
        'profit_factor': 2.1,
        'max_drawdown_30d': 8.5,
        'sortino_ratio': 1.8,
        'trade_count_30d': 127,
        'avg_trade_size_sol': 0.5,
        'uses_mev_protection': True,
        'uses_limit_orders': True,
        'dex_diversity_score': 3,
        'parse_rate': 0.95,
        'insider_probability': 0.1,
    }

    print("\n1. Caching Wallet Metrics")
    # Cache the metrics for future use
    optimizer.cache_wallet_metrics(wallet_metrics['address'], wallet_metrics)
    print(f"✓ Cached metrics for {wallet_metrics['address'][:8]}...")

    print("\n2. Checking Analysis Permissions")
    # Check if we can analyze this wallet given budget
    can_analyze, reason = optimizer.can_analyze_wallet(
        wallet_metrics['address'],
        wallet_wqs=65.0  # Assume WQS of 65
    )
    print(f"✓ Can analyze: {can_analyze} ({reason})")

    if can_analyze:
        print("\n3. Predicting Profitability")
        # Predict profitability using ML model
        prediction = optimizer.predict_profitability(wallet_metrics)
        print(f"✓ Expected return: {prediction.expected_return_pct:.1f}%")
        print(f"✓ Confidence: {prediction.confidence:.1f}")
        print(f"✓ Risk score: {prediction.risk_score:.1f}")
        print(f"✓ Profitability class: {prediction.profitability_class.value}")

        print("\n4. Optimizing Capital Allocation")
        # If we have multiple predictions, optimize allocation
        test_predictions = [
            ("wallet1", prediction),
            ("wallet2", prediction),  # Same prediction for demo
        ]
        allocation = optimizer.get_investment_allocation(test_predictions)
        print(f"✓ Optimized allocation: {allocation}")

    print("\n5. Production Health Check")
    # Check system health
    health_status = optimizer.check_production_health()
    print(f"✓ System status: {health_status['overall_status']}")
    print(f"✓ Active alerts: {health_status['active_alerts']}")

    print("\n6. Optimization Report")
    # Print comprehensive report
    optimizer.print_optimization_report()

    print("\n7. Optimization Suggestions")
    # Get optimization suggestions
    suggestions = optimizer.get_optimization_suggestions()
    if suggestions:
        for i, suggestion in enumerate(suggestions[:5], 1):
            print(f"  {i}. {suggestion}")

    # Cleanup
    optimizer.shutdown()
    print("\n✓ Optimizer shut down successfully")


def example_batch_analysis():
    """Example: Optimized batch wallet analysis."""
    print("\n" + "="*70)
    print("EXAMPLE: Optimized Batch Analysis")
    print("="*70)

    optimizer = get_scout_optimizer()
    if not optimizer.initialize():
        return

    # Simulate wallet discovery results
    discovered_wallets = [f"wallet_{i}" for i in range(100)]

    print(f"\n1. Discovered {len(discovered_wallets)} wallets")

    print("\n2. Optimizing Analysis Count")
    # Optimize how many wallets we can actually analyze
    optimized_count = optimizer.optimize_wallet_count(len(discovered_wallets))
    print(f"✓ Can analyze {optimized_count} wallets (down from {len(discovered_wallets)})")

    print("\n3. Optimizing Discovery Depth")
    # Optimize discovery depth for budget
    original_depth = 168  # 1 week
    optimized_depth = optimizer.optimize_discovery_depth(original_depth)
    print(f"✓ Optimized discovery depth: {optimized_depth}h (down from {original_depth}h)")

    optimizer.shutdown()


def example_cache_usage():
    """Example: Using advanced caching system."""
    print("\n" + "="*70)
    print("EXAMPLE: Advanced Caching Usage")
    print("="*70)

    optimizer = get_scout_optimizer()
    if not optimizer.initialize():
        return

    print("\n1. Caching Token Metadata")
    # Cache token metadata
    token_address = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
    token_metadata = {
        'symbol': 'BONK',
        'decimals': 5,
        'liquidity': 5_000_000,
    }

    optimizer.set_cached_data("token", token_address, token_metadata, category="token_metadata")
    print(f"✓ Cached metadata for {token_metadata['symbol']}")

    print("\n2. Retrieving Cached Data")
    # Retrieve cached data
    cached = optimizer.get_cached_data("token", token_address, category="token_metadata")
    if cached:
        print(f"✓ Retrieved {cached['symbol']} metadata from cache")
    else:
        print("✗ Cache miss")

    print("\n3. Cache Invalidation")
    # Invalidate specific cache entry
    optimizer.invalidate_cache("token", token_address)
    print(f"✓ Invalidated cache for {token_address[:8]}...")

    optimizer.shutdown()


def example_production_monitoring():
    """Example: Production monitoring and alerting."""
    print("\n" + "="*70)
    print("EXAMPLE: Production Monitoring")
    print("="*70)

    optimizer = get_scout_optimizer()
    if not optimizer.initialize():
        return

    print("\n1. Checking Production Readiness")
    is_ready, issues = optimizer.is_production_ready()
    print(f"✓ Production ready: {is_ready}")
    if issues:
        print("  Issues found:")
        for issue in issues[:3]:
            print(f"    - {issue}")

    print("\n2. Creating Test Alert")
    # Create a test alert
    optimizer.create_alert(
        severity="warning",
        title="Test Alert",
        message="This is a test alert from Scout optimization system",
        source="example",
        details={"test": True}
    )
    print("✓ Test alert created")

    print("\n3. Health Status")
    # Get health status
    health = optimizer.check_production_health()
    print(f"✓ Overall status: {health['overall_status']}")
    print(f"✓ Health checks: {len(health['health_checks'])}")

    optimizer.shutdown()


def main():
    """Run all examples."""
    print("\n" + "="*70)
    print("SCOUT OPTIMIZATION INTEGRATION EXAMPLES")
    print("="*70)
    print("\nThese examples demonstrate how to use the new optimization systems")
    print("to maximize profitability while staying within Helius Developer Plan constraints.")
    print("\nGrowth Goal: $200 → $1,000 (5x)")
    print("Helius Plan: Developer (10M credits, 50 req/s)")
    print("="*70)

    try:
        # Run examples
        example_optimized_analysis()
        example_batch_analysis()
        example_cache_usage()
        example_production_monitoring()

        print("\n" + "="*70)
        print("ALL EXAMPLES COMPLETED SUCCESSFULLY")
        print("="*70)

        print("\nKey Takeaways:")
        print("1. Use ScoutOptimizer for unified optimization interface")
        print("2. Cache everything possible to reduce API calls")
        print("3. Use ML predictions for wallet selection")
        print("4. Monitor production health continuously")
        print("5. Optimize for growth goal ($200 → $1,000)")

        print("\nIntegration Steps:")
        print("1. Initialize ScoutOptimizer at startup")
        print("2. Use cache_wallet_metrics() after wallet analysis")
        print("3. Use predict_profitability() for wallet ranking")
        print("4. Use can_analyze_wallet() before expensive operations")
        print("5. Monitor health status in production")

    except Exception as e:
        print(f"\nError running examples: {e}")
        import traceback
        traceback.print_exc()


if __name__ == "__main__":
    main()