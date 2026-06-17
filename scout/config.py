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
    def get_helius_api_base_url() -> str:
        """Get Helius REST API base URL (e.g. https://api.helius.xyz/v0 or https://beta.helius-rpc.com/v0)."""
        return (os.getenv("SCOUT_HELIUS_API_BASE_URL") or "https://api.helius.xyz/v0").rstrip("/")
    
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
    
    @staticmethod
    def get_strict_historical_liquidity() -> bool:
        """
        Get whether to enforce strict historical liquidity (production recommended).
        
        When True, rejects backtests if historical liquidity data is unavailable,
        preventing "lucky" backtests based on current liquidity of mooned tokens.
        
        Default: True (production-safe)
        """
        # Default to True for production safety
        return os.getenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "true").lower() == "true"
    
    @staticmethod
    def get_historical_liquidity_grace_period_days() -> int:
        """
        Grace period (days) for historical liquidity fallback.
        
        When strict mode is enabled but historical liquidity is unavailable,
        trades within this many days of the current date can use current
        liquidity with a 30% haircut instead of being rejected outright.
        
        Default: 14 days
        """
        return int(os.getenv("SCOUT_HISTORICAL_LIQUIDITY_GRACE_PERIOD_DAYS", "14"))
    
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
        return float(os.getenv("SCOUT_MIN_WQS_CANDIDATE", "15.0"))

    # ========================================================================
    # Archetype-Aware WQS Thresholds
    # ========================================================================

    @staticmethod
    def get_min_wqs_whale() -> Optional[float]:
        """Get minimum WQS score for WHALE archetype (lower threshold for high-conviction trades)."""
        val = os.getenv("SCOUT_MIN_WQS_WHALE")
        return float(val) if val else 55.0

    @staticmethod
    def get_min_wqs_swing() -> Optional[float]:
        """Get minimum WQS score for SWING archetype (lower threshold for swing traders)."""
        val = os.getenv("SCOUT_MIN_WQS_SWING")
        return float(val) if val else 58.0

    @staticmethod
    def get_momentum_boost() -> float:
        """Get WQS momentum boost for IMPROVING trajectory."""
        return float(os.getenv("SCOUT_MOMENTUM_BOOST", "5.0"))
    
    # ========================================================================
    # Backtest Configuration
    # ========================================================================
    
    @staticmethod
    def get_min_closes_required() -> int:
        """Get minimum realized closes required for promotion."""
        return int(os.getenv("SCOUT_MIN_CLOSES_REQUIRED", "5"))
    
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
        return int(os.getenv("SCOUT_MAX_WALLETS", "250"))

    @staticmethod
    def get_max_wallets_tier1() -> int:
        """Get max wallets for Tier 1 (Shield candidates, deep analysis)."""
        return int(os.getenv("SCOUT_MAX_WALLETS_TIER1", "150"))

    @staticmethod
    def get_max_wallets_tier2() -> int:
        """Get max wallets for Tier 2 (Spear candidates, fast analysis)."""
        return int(os.getenv("SCOUT_MAX_WALLETS_TIER2", "100"))

    @staticmethod
    def get_discovery_deep_hours() -> int:
        """Deep scan lookback (established wallets with large samples)."""
        return int(os.getenv("SCOUT_DISCOVERY_DEEP_HOURS", "720"))

    @staticmethod
    def get_discovery_fast_hours() -> int:
        """Fast scan lookback (emerging wallets with recent activity)."""
        return int(os.getenv("SCOUT_DISCOVERY_FAST_HOURS", "24"))

    @staticmethod
    def get_discovery_trending_hours() -> int:
        """Trending scan lookback (wallets riding current narratives)."""
        return int(os.getenv("SCOUT_DISCOVERY_TRENDING_HOURS", "4"))
    
    @staticmethod
    def get_discovery_profitability_filter() -> bool:
        """
        Get whether to pre-screen discovered wallets for profitability
        before full analysis. When enabled, candidates are quick-checked
        for positive net SOL flow before expensive metrics computation.
        
        Default: True
        """
        return os.getenv("SCOUT_DISCOVERY_PROFITABILITY_FILTER", "true").lower() == "true"
    
    @staticmethod
    def get_wqs_recency_weight() -> bool:
        """
        Get whether to apply time-decayed weighting to WQS components.
        When enabled, recent performance (0-7d) is weighted more heavily
        than older performance (7-30d) in ROI and win rate scoring.
        
        Default: True
        """
        return os.getenv("SCOUT_WQS_RECENCY_WEIGHT", "true").lower() == "true"
    
    @staticmethod
    def get_archetype_diversity_min_pct() -> float:
        """
        Minimum fraction of ACTIVE slots allocated to each trader archetype
        (SCALPER, SWING, WHALE). Prevents a homogeneous roster.
        
        Default: 0.20 (20%)
        """
        return float(os.getenv("SCOUT_ARCHETYPE_DIVERSITY_MIN_PCT", "0.2"))
    
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

    @staticmethod
    def get_safety_fail_mode() -> str:
        """Get token safety fail mode: 'open' (assume safe on RPC error) or 'closed' (reject on error).

        When 'open', every RPC failure during token safety checks is logged but
        the token is assumed safe. When 'closed' (default), any RPC failure
        causes the token to be rejected.
        """
        return os.getenv("SCOUT_SAFETY_FAIL_MODE", "closed").lower()

    # ========================================================================
    # Redis Configuration
    # ========================================================================
    
    @staticmethod
    def get_redis_enabled() -> bool:
        """Get whether Redis caching is enabled."""
        return os.getenv("REDIS_ENABLED", "true").lower() == "true"
    
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

        # Strict Liquidity Check (default to true for production safety)
        mode = ScoutConfig.get_liquidity_mode()
        if mode == "real":
            strict_mode = os.getenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "true").lower() == "true"
            allow_fallback = os.getenv("SCOUT_LIQUIDITY_ALLOW_FALLBACK", "false").lower() == "true"

            if not strict_mode and allow_fallback:
                warnings.append("WARNING: Strict Historical Liquidity is OFF. Backtests may use current liquidity for old trades (Survivorship Bias).")
                warnings.append("Recommended for Production: Keep SCOUT_STRICT_HISTORICAL_LIQUIDITY=true")
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


