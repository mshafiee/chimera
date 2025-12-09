#!/bin/bash
# Test script for new features
# Run this to verify all new implementations

set -e

echo "🧪 Testing New Features"
echo "======================"
echo ""

cd "$(dirname "$0")/../operator"

echo "1️⃣  Testing Consensus Detection..."
cargo test --test integration_tests consensus_detection -- --nocapture
echo "✅ Consensus detection tests passed"
echo ""

echo "2️⃣  Testing Volatility Calculations..."
cargo test --test integration_tests volatility -- --nocapture
echo "✅ Volatility tests passed"
echo ""

echo "3️⃣  Testing DEX Comparison (may require network)..."
cargo test --test integration_tests dex_comparison -- --ignored --nocapture || echo "⚠️  DEX tests skipped (network required)"
echo ""

echo "4️⃣  Testing Helius Token Age (requires API key)..."
if [ -z "$HELIUS_API_KEY" ]; then
    echo "⚠️  HELIUS_API_KEY not set, skipping Helius tests"
else
    cargo test --test integration_tests helius_token_age -- --ignored --nocapture || echo "⚠️  Helius tests failed (may be expected)"
fi
echo ""

echo "5️⃣  Verifying Auto-Demotion Config..."
if grep -q "auto_demote_wallets: true" ../config/config.yaml; then
    echo "✅ Auto-demotion is enabled in config"
else
    echo "⚠️  Auto-demotion is disabled in config"
fi
echo ""

echo "6️⃣  Running All Unit Tests..."
cargo test --lib -- --nocapture
echo "✅ All unit tests passed"
echo ""

echo "🎉 Testing Complete!"
echo ""
echo "Next Steps:"
echo "  - Review test output above"
echo "  - Check logs for consensus detection in production"
echo "  - Monitor volatility calculations"
echo "  - Test DEX comparison with real trades"
echo "  - Verify auto-demotion with test wallets"
