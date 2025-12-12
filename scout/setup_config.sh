#!/bin/bash
# Scout Configuration Setup Script

set -e

echo "=========================================="
echo "Scout Configuration Setup"
echo "=========================================="
echo ""

# Check if .env already exists
if [ -f ".env" ]; then
    echo "⚠️  .env file already exists!"
    read -p "Do you want to overwrite it? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "Keeping existing .env file"
        exit 0
    fi
fi

# Create .env from example if it exists, otherwise create new
if [ -f ".env.example" ]; then
    cp .env.example .env
    echo "✓ Created .env from .env.example"
else
    # Create basic .env file
    cat > .env << 'EOF'
# Scout Configuration - Environment Variables
# Fill in your API keys below

# ============================================================================
# API Keys (Required for production)
# ============================================================================

# Birdeye API Key (recommended for best historical liquidity coverage)
# Get your key at: https://birdeye.so/
BIRDEYE_API_KEY=

# Helius API Key (for wallet transaction data)
# Get your key at: https://www.helius.dev/
HELIUS_API_KEY=

# DexScreener API Key (optional, public API doesn't require key)
DEXSCREENER_API_KEY=

# ============================================================================
# Liquidity Provider Configuration
# ============================================================================

# Liquidity mode: 'real' (default) or 'simulated' (for testing/dev)
SCOUT_LIQUIDITY_MODE=real

# Cache TTL for liquidity data (seconds)
SCOUT_LIQUIDITY_CACHE_TTL_SECONDS=60

# Allow fallback to current liquidity when historical unavailable
SCOUT_LIQUIDITY_ALLOW_FALLBACK=true

# ============================================================================
# WQS Thresholds (Rescaled 0-100 range)
# ============================================================================

# Minimum WQS score for ACTIVE status (default: 60.0)
SCOUT_MIN_WQS_ACTIVE=60.0

# Minimum WQS score for CANDIDATE status (default: 30.0)
SCOUT_MIN_WQS_CANDIDATE=30.0

# ============================================================================
# Backtest Configuration
# ============================================================================

# Minimum realized closes (SELLs with PnL) required for promotion
SCOUT_MIN_CLOSES_REQUIRED=10

# Minimum closes in walk-forward holdout window
SCOUT_WALK_FORWARD_MIN_TRADES=5

# Minimum liquidity thresholds (USD)
SCOUT_MIN_LIQUIDITY_SHIELD=10000.0
SCOUT_MIN_LIQUIDITY_SPEAR=5000.0

# Priority fee cost per trade (SOL)
SCOUT_PRIORITY_FEE_SOL=0.00005

# Jito tip cost per trade (SOL)
SCOUT_JITO_TIP_SOL=0.0001

# ============================================================================
# Wallet Discovery & Analysis
# ============================================================================

# Wallet discovery lookback window (hours, default: 168 = 7 days)
SCOUT_DISCOVERY_HOURS=168

# Maximum wallets to analyze per run
SCOUT_MAX_WALLETS=50

# Maximum transactions to fetch per wallet
SCOUT_WALLET_TX_LIMIT=500

# Maximum pagination pages per wallet transaction fetch
SCOUT_WALLET_TX_MAX_PAGES=20

# ============================================================================
# Database Configuration
# ============================================================================

# Path to main Chimera database (for historical liquidity storage)
CHIMERA_DB_PATH=../data/chimera.db

# ============================================================================
# RPC Configuration
# ============================================================================

# Primary RPC URL (Helius or other provider)
CHIMERA_RPC__PRIMARY_URL=

# Alternative: Solana RPC URL
SOLANA_RPC_URL=
EOF
    echo "✓ Created new .env file"
fi

echo ""
echo "=========================================="
echo "Next Steps:"
echo "=========================================="
echo ""
echo "1. Edit .env and add your API keys:"
echo "   - BIRDEYE_API_KEY (recommended)"
echo "   - HELIUS_API_KEY (required)"
echo ""
echo "2. Review other configuration options in .env"
echo ""
echo "3. Run Scout:"
echo "   python main.py --dry-run  # Test first"
echo "   python main.py             # Production run"
echo ""
echo "For detailed configuration documentation, see README_CONFIG.md"
echo ""
