"""
Scout Configuration Module

Centralized configuration management for Scout module.
Loads from environment variables with sensible defaults.
"""

import os
from typing import Optional
from pathlib import Path


class ScoutConfig:
    """Centralized Scout configuration."""
    
    # ========================================================================
    # API Keys
    # ========================================================================
    
    @staticmethod
    def get_birdeye_api_key() -> Optional[str]:
        """Get Birdeye API key from environment."""
        return os.getenv("BIRDEYE_API_KEY")
    
    @staticmethod
    def get_helius_api_key() -> Optional[str]:
        """Get Helius API key from environment or RPC URL."""
        key = os.getenv("HELIUS_API_KEY")
        if not key:
            # Try to extract from RPC URL
            rpc_url = os.getenv("CHIMERA_RPC__PRIMARY_URL") or os.getenv("SOLANA_RPC_URL", "")
            if "api-key=" in rpc_url:
                key = rpc_url.split("api-key=")[1].split("&")[0].split("?")[0]
        return key
    
    @staticmethod
    def get_dexscreener_api_key() -> Optional[str]:
        """Get DexScreener API key from environment (optional)."""
        return os.getenv("DEXSCREENER_API_KEY")
    
    # ========================================================================
    # Liquidity Provider Configuration
    # ========================================================================
    
    @staticmethod
    def get_liquidity_mode() -> str:
        """Get liquidity provider mode: 'real' or 'simulated'."""
        return os.getenv("SCOUT_LIQUIDITY_MODE", "real").lower()
    
    @staticmethod
    def get_liquidity_cache_ttl() -> int:
        """Get liquidity cache TTL in seconds."""
        return int(os.getenv("SCOUT_LIQUIDITY_CACHE_TTL_SECONDS", "60"))
    
    @staticmethod
    def get_liquidity_allow_fallback() -> bool:
        """Get whether to allow fallback to current liquidity when historical unavailable."""
        return os.getenv("SCOUT_LIQUIDITY_ALLOW_FALLBACK", "true").lower() == "true"
    
    # ========================================================================
    # WQS Thresholds (Rescaled 0-100 range)
    # ========================================================================
    
    @staticmethod
    def get_min_wqs_active() -> float:
        """Get minimum WQS score for ACTIVE status."""
        return float(os.getenv("SCOUT_MIN_WQS_ACTIVE", "60.0"))
    
    @staticmethod
    def get_min_wqs_candidate() -> float:
        """Get minimum WQS score for CANDIDATE status."""
        return float(os.getenv("SCOUT_MIN_WQS_CANDIDATE", "30.0"))
    
    # ========================================================================
    # Backtest Configuration
    # ========================================================================
    
    @staticmethod
    def get_min_closes_required() -> int:
        """Get minimum realized closes required for promotion."""
        return int(os.getenv("SCOUT_MIN_CLOSES_REQUIRED", "10"))
    
    @staticmethod
    def get_walk_forward_min_trades() -> int:
        """Get minimum closes in walk-forward holdout window."""
        return int(os.getenv("SCOUT_WALK_FORWARD_MIN_TRADES", "5"))
    
    @staticmethod
    def get_min_liquidity_shield() -> float:
        """Get minimum liquidity (USD) for Shield strategy."""
        return float(os.getenv("SCOUT_MIN_LIQUIDITY_SHIELD", "10000.0"))
    
    @staticmethod
    def get_min_liquidity_spear() -> float:
        """Get minimum liquidity (USD) for Spear strategy."""
        return float(os.getenv("SCOUT_MIN_LIQUIDITY_SPEAR", "5000.0"))
    
    @staticmethod
    def get_priority_fee_sol() -> float:
        """Get priority fee cost per trade (SOL)."""
        return float(os.getenv("SCOUT_PRIORITY_FEE_SOL", "0.00005"))
    
    @staticmethod
    def get_jito_tip_sol() -> float:
        """Get Jito tip cost per trade (SOL)."""
        return float(os.getenv("SCOUT_JITO_TIP_SOL", "0.0001"))
    
    # ========================================================================
    # Wallet Discovery & Analysis
    # ========================================================================
    
    @staticmethod
    def get_discovery_hours() -> int:
        """Get wallet discovery lookback window in hours."""
        return int(os.getenv("SCOUT_DISCOVERY_HOURS", "168"))
    
    @staticmethod
    def get_max_wallets() -> int:
        """Get maximum wallets to analyze per run."""
        return int(os.getenv("SCOUT_MAX_WALLETS", "50"))
    
    @staticmethod
    def get_wallet_tx_limit() -> int:
        """Get maximum transactions to fetch per wallet."""
        return int(os.getenv("SCOUT_WALLET_TX_LIMIT", "500"))
    
    @staticmethod
    def get_wallet_tx_max_pages() -> int:
        """Get maximum pagination pages per wallet transaction fetch."""
        return int(os.getenv("SCOUT_WALLET_TX_MAX_PAGES", "20"))
    
    # ========================================================================
    # Database Configuration
    # ========================================================================
    
    @staticmethod
    def get_db_path() -> str:
        """Get path to main Chimera database."""
        return os.getenv("CHIMERA_DB_PATH", "../data/chimera.db")
    
    # ========================================================================
    # Configuration Validation
    # ========================================================================
    
    @staticmethod
    def validate_config() -> tuple[bool, list[str]]:
        """
        Validate configuration and return (is_valid, warnings).
        
        Returns:
            Tuple of (is_valid, list of warning messages)
        """
        warnings = []
        is_valid = True
        
        # Check liquidity mode
        mode = ScoutConfig.get_liquidity_mode()
        if mode == "simulated":
            warnings.append("WARNING: Running in simulated liquidity mode - results are non-deterministic!")
            warnings.append("Set SCOUT_LIQUIDITY_MODE=real and provide BIRDEYE_API_KEY for production use")
        
        # Check API keys for real mode
        if mode == "real":
            if not ScoutConfig.get_birdeye_api_key():
                warnings.append("WARNING: BIRDEYE_API_KEY not set - Birdeye source will be unavailable")
                warnings.append("Historical liquidity coverage will be limited")
            
            if not ScoutConfig.get_helius_api_key():
                warnings.append("WARNING: HELIUS_API_KEY not set - wallet transaction fetching may fail")
        
        # Check database path
        db_path = ScoutConfig.get_db_path()
        db_dir = Path(db_path).parent
        if not db_dir.exists():
            warnings.append(f"WARNING: Database directory does not exist: {db_dir}")
            warnings.append("It will be created automatically on first run")
        
        return is_valid, warnings
    
    @staticmethod
    def print_config_summary():
        """Print a summary of current configuration."""
        print("=" * 70)
        print("Scout Configuration Summary")
        print("=" * 70)
        print(f"Liquidity Mode: {ScoutConfig.get_liquidity_mode()}")
        print(f"Birdeye API Key: {'Set' if ScoutConfig.get_birdeye_api_key() else 'Not set'}")
        print(f"Helius API Key: {'Set' if ScoutConfig.get_helius_api_key() else 'Not set'}")
        print(f"Min WQS Active: {ScoutConfig.get_min_wqs_active()}")
        print(f"Min WQS Candidate: {ScoutConfig.get_min_wqs_candidate()}")
        print(f"Min Closes Required: {ScoutConfig.get_min_closes_required()}")
        print(f"Min Liquidity Shield: ${ScoutConfig.get_min_liquidity_shield():,.0f}")
        print(f"Min Liquidity Spear: ${ScoutConfig.get_min_liquidity_spear():,.0f}")
        print(f"Database Path: {ScoutConfig.get_db_path()}")
        print("=" * 70)
        
        is_valid, warnings = ScoutConfig.validate_config()
        if warnings:
            print("\nConfiguration Warnings:")
            for warning in warnings:
                print(f"  ⚠️  {warning}")
        else:
            print("\n✓ Configuration looks good!")
