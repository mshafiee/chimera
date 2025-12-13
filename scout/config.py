"""
Scout Configuration Module

Centralized configuration management for Scout module.
Loads from environment variables with sensible defaults.
"""

import os
from typing import Optional
from pathlib import Path
from urllib.parse import urlparse, parse_qs



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
            if rpc_url:
                try:
                    parsed = urlparse(rpc_url)
                    query_params = parse_qs(parsed.query)
                    # parse_qs returns a list, e.g., {'api-key': ['xyz']}
                    if 'api-key' in query_params:
                        key = query_params['api-key'][0]
                except Exception:
                    pass # Fallback to None if parsing fails
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
    def get_dex_program_ids() -> list[str]:
        """Get list of DEX program IDs to monitor."""
        default_ids = [
            "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4",  # Jupiter
            "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",  # Raydium
            "9W959DqEETiGZocYWCQPaJ6sBmUzgfxXfqGeTEdp3aQP",  # Orca
            "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",  # Whirlpool
        ]
        
        env_val = os.getenv("SCOUT_DEX_PROGRAM_IDS")
        if env_val:
            return [x.strip() for x in env_val.split(",") if x.strip()]
        return default_ids
    
    # ========================================================================
    # RugCheck Security Configuration
    # ========================================================================
    
    @staticmethod
    def get_rugcheck_enabled() -> bool:
        """Get whether RugCheck integration is enabled."""
        return os.getenv("RUGCHECK_ENABLED", "true").lower() == "true"
    
    @staticmethod
    def get_rugcheck_api_key() -> Optional[str]:
        """Get RugCheck API key from environment (optional, uses public API if not set)."""
        return os.getenv("RUGCHECK_API_KEY")
    
    @staticmethod
    def get_rugcheck_fail_mode() -> str:
        """Get RugCheck fail mode: 'open' (allow if API fails) or 'closed' (reject if API fails)."""
        return os.getenv("RUGCHECK_FAIL_MODE", "closed").lower()
    
    # ========================================================================
    # Redis Configuration
    # ========================================================================
    
    @staticmethod
    def get_redis_enabled() -> bool:
        """Get whether Redis caching is enabled."""
        return os.getenv("REDIS_ENABLED", "false").lower() == "true"
    
    @staticmethod
    def get_redis_url() -> str:
        """Get Redis connection URL."""
        return os.getenv("REDIS_URL", "redis://localhost:6379")

    @staticmethod
    def validate_config() -> tuple[bool, list[str]]:
        """
        Validate the current configuration.
        
        Returns:
            Tuple of (is_valid, list_of_warnings)
        """
        warnings = []
        is_valid = True
        
        # Check API keys
        if not os.getenv("HELIUS_API_KEY"):
            warnings.append("HELIUS_API_KEY is not set. Discovery will use sample data.")
        
        if not os.getenv("BIRDEYE_API_KEY"):
            warnings.append("BIRDEYE_API_KEY is not set. Historical liquidity data will be limited.")

        # Strict Liquidity Check
        mode = ScoutConfig.get_liquidity_mode()
        if mode == "real":
            strict_mode = os.getenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "false").lower() == "true"
            allow_fallback = os.getenv("SCOUT_LIQUIDITY_ALLOW_FALLBACK", "true").lower() == "true"
            
            if not strict_mode and allow_fallback:
                warnings.append("WARNING: Strict Historical Liquidity is OFF. Backtests may use current liquidity for old trades (Survivorship Bias).")
                warnings.append("Recommended for Production: Set SCOUT_STRICT_HISTORICAL_LIQUIDITY=true")
        elif mode == "simulated":
            warnings.append("WARNING: Running in simulated liquidity mode - results are non-deterministic!")
            warnings.append("Set SCOUT_LIQUIDITY_MODE=real and provide BIRDEYE_API_KEY for production use")
        
            
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


