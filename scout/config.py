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
    def get_discovery_timeout_seconds() -> int:
        """Timeout per discovery strategy (deep/fast/trending)."""
        return int(os.getenv("SCOUT_DISCOVERY_TIMEOUT_SECONDS", "300"))

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

    @staticmethod
    def get_discovery_limit_per_token() -> int:
        """Get maximum transactions to query per token during active-token discovery."""
        return int(os.getenv("SCOUT_DISCOVERY_LIMIT_PER_TOKEN", "200"))

    @staticmethod
    def get_discovery_block_limit() -> int:
        """Get maximum transactions to scan during recent-blocks discovery."""
        return int(os.getenv("SCOUT_DISCOVERY_BLOCK_LIMIT", "500"))

    @staticmethod
    def get_discovery_program_limit() -> int:
        """Get maximum accounts to scan during DEX-program discovery."""
        return int(os.getenv("SCOUT_DISCOVERY_PROGRAM_LIMIT", "500"))

    @staticmethod
    def get_discovery_seed_limit_per_wallet() -> int:
        """Get maximum transactions to fetch per seed wallet during seed-wallet discovery."""
        return int(os.getenv("SCOUT_DISCOVERY_SEED_LIMIT", "50"))

    @staticmethod
    def get_discovery_fallback_threshold_pct() -> float:
        """Fraction of max_wallets below which fallback strategies are triggered (0.0-1.0)."""
        return float(os.getenv("SCOUT_DISCOVERY_FALLBACK_THRESHOLD", "0.5"))

    @staticmethod
    def get_balance_batch_size() -> int:
        """Get batch size for batch RPC balance-check calls."""
        return int(os.getenv("SCOUT_BALANCE_BATCH_SIZE", "20"))

    @staticmethod
    def get_activity_validation_concurrency() -> int:
        """Get max concurrent activity validation checks."""
        return int(os.getenv("SCOUT_ACTIVITY_VALIDATION_CONCURRENCY", "20"))

    @staticmethod
    def get_discovery_cache_ttl() -> int:
        """Get TTL for discovery result cache (seconds)."""
        return int(os.getenv("SCOUT_DISCOVERY_CACHE_TTL", "3600"))

    @staticmethod
    def get_max_api_calls_per_run() -> int:
        """Get maximum API calls allowed per discovery run."""
        return int(os.getenv("SCOUT_MAX_API_CALLS_PER_RUN", "500"))

    @staticmethod
    def get_balance_fail_mode() -> str:
        """Get balance validation fail mode: 'open' (include all on error) or 'closed' (exclude batch)."""
        return os.getenv("SCOUT_BALANCE_FAIL_MODE", "open").lower()

    @staticmethod
    def get_dedup_ttl() -> int:
        """Get TTL for persistent wallet deduplication set (seconds)."""
        return int(os.getenv("SCOUT_DEDUP_TTL", str(6 * 3600)))

    # ========================================================================
    # Optimization Configuration
    # ========================================================================

    @staticmethod
    def get_optimization_enabled() -> bool:
        """Get whether optimization systems are enabled."""
        return os.getenv("SCOUT_OPTIMIZATION_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_credit_tracking_enabled() -> bool:
        """Get whether Helius credit tracking is enabled."""
        return os.getenv("SCOUT_CREDIT_TRACKING_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_wqs_boost_enabled() -> bool:
        """Get whether WQS growth boost via profitability prediction is enabled."""
        return os.getenv("SCOUT_WQS_BOOST_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_production_monitoring_enabled() -> bool:
        """Get whether production monitoring is enabled."""
        return os.getenv("SCOUT_PRODUCTION_MONITORING_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_growth_optimized() -> bool:
        """Get whether growth optimization is enabled ($200 → $1000)."""
        return os.getenv("SCOUT_GROWTH_OPTIMIZED", "true").lower() == "true"

    @staticmethod
    def get_current_capital() -> float:
        """Get current capital for growth optimization."""
        return float(os.getenv("SCOUT_CURRENT_CAPITAL", "200.0"))

    @staticmethod
    def get_target_capital() -> float:
        """Get target capital for growth optimization."""
        return float(os.getenv("SCOUT_TARGET_CAPITAL", "1000.0"))

    @staticmethod
    def get_monthly_credits() -> int:
        """Get Helius monthly credit budget."""
        return int(os.getenv("SCOUT_MONTHLY_CREDITS", "10000000"))

    @staticmethod
    def get_max_requests_per_second() -> int:
        """Get Helius rate limit (requests per second)."""
        return int(os.getenv("SCOUT_MAX_REQUESTS_PER_SECOND", "50"))

    @staticmethod
    def get_target_rps() -> int:
        """Get target requests per second for adaptive rate limiting (safe operating target)."""
        return int(os.getenv("SCOUT_TARGET_RPS", "25"))  # Reduced from 45 to 25 for developer tier stability
    
    @staticmethod
    def get_rate_limit_adaptive() -> bool:
        """Get whether adaptive rate limiting is enabled."""
        return os.getenv("SCOUT_RATE_LIMIT_ADAPTIVE", "true").lower() == "true"
    
    @staticmethod
    def get_rate_limit_min_delay_ms() -> int:
        """Get minimum delay between requests in milliseconds."""
        return int(os.getenv("SCOUT_RATE_LIMIT_MIN_DELAY_MS", "30"))  # Increased from 15 to 30ms
    
    @staticmethod
    def get_rate_limit_max_delay_ms() -> int:
        """Get maximum delay between requests in milliseconds."""
        return int(os.getenv("SCOUT_RATE_LIMIT_MAX_DELAY_MS", "200"))  # Increased from 100 to 200ms

    @staticmethod
    def get_discovery_concurrency() -> int:
        """Get maximum concurrent requests during wallet discovery."""
        return int(os.getenv("SCOUT_DISCOVERY_CONCURRENCY", "30"))  # Reduced from 50 to 30 for better rate limit handling

    @staticmethod
    def get_circuit_breaker_threshold() -> int:
        """Get circuit breaker failure threshold."""
        return int(os.getenv("SCOUT_CIRCUIT_BREAKER_THRESHOLD", "5"))

    @staticmethod
    def get_circuit_breaker_reset_seconds() -> int:
        """Get circuit breaker reset time in seconds."""
        return int(os.getenv("SCOUT_CIRCUIT_BREAKER_RESET_SECONDS", "60"))

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
    # ML Model Configuration (Phase 1-5)
    # ========================================================================

    @staticmethod
    def get_ml_enabled() -> bool:
        """Get whether ML enhancements are enabled."""
        return os.getenv("SCOUT_ML_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_ensemble_enabled() -> bool:
        """Get whether ensemble methods (Phase 1) are enabled."""
        return os.getenv("SCOUT_ENSEMBLE_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_xgboost_enabled() -> bool:
        """Get whether XGBoost model is enabled."""
        return os.getenv("SCOUT_XGBOOST_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_lightgbm_enabled() -> bool:
        """Get whether LightGBM model is enabled."""
        return os.getenv("SCOUT_LIGHTGBM_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_meta_learner_enabled() -> bool:
        """Get whether meta-learner stacking is enabled."""
        return os.getenv("SCOUT_META_LEARNER_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_time_series_features_enabled() -> bool:
        """Get whether time-series features (Phase 2) are enabled."""
        return os.getenv("SCOUT_TIME_SERIES_FEATURES_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_market_context_features_enabled() -> bool:
        """Get whether market context features are enabled."""
        return os.getenv("SCOUT_MARKET_CONTEXT_FEATURES_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_network_features_enabled() -> bool:
        """Get whether network analysis features are enabled."""
        return os.getenv("SCOUT_NETWORK_FEATURES_ENABLED", "false").lower() == "true"

    @staticmethod
    def get_advanced_risk_features_enabled() -> bool:
        """Get whether advanced risk features are enabled."""
        return os.getenv("SCOUT_ADVANCED_RISK_FEATURES_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_online_learning_enabled() -> bool:
        """Get whether online learning (Phase 3) is enabled."""
        return os.getenv("SCOUT_ONLINE_LEARNING_ENABLED", "false").lower() == "true"

    @staticmethod
    def get_regime_models_enabled() -> bool:
        """Get whether regime-specific models are enabled."""
        return os.getenv("SCOUT_REGIME_MODELS_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_torch_enabled() -> bool:
        """Get whether PyTorch models (Phase 5) are enabled."""
        return os.getenv("SCOUT_TORCH_ENABLED", "false").lower() == "true"

    # ========================================================================
    # ML Model Paths
    # ========================================================================

    @staticmethod
    def get_model_dir() -> str:
        """Get directory for storing trained models."""
        env_dir = os.getenv("SCOUT_MODEL_DIR")
        if env_dir:
            return env_dir
        # Resolve relative to this config.py file
        return str(Path(__file__).resolve().parent.parent / "models")

    @staticmethod
    def get_xgboost_model_path() -> str:
        """Get path to XGBoost model file."""
        return os.path.join(ScoutConfig.get_model_dir(), "xgboost_model.json")

    @staticmethod
    def get_lightgbm_model_path() -> str:
        """Get path to LightGBM model file."""
        return os.path.join(ScoutConfig.get_model_dir(), "lightgbm_model.txt")

    @staticmethod
    def get_meta_learner_model_path() -> str:
        """Get path to meta-learner model file."""
        return os.path.join(ScoutConfig.get_model_dir(), "meta_learner.pkl")

    @staticmethod
    def get_regime_model_path(regime: str) -> str:
        """Get path to regime-specific model file."""
        return os.path.join(ScoutConfig.get_model_dir(), f"regime_{regime.lower()}_model.json")

    # ========================================================================
    # ML Latency Configuration (Critical: <50ms Budget)
    # ========================================================================

    @staticmethod
    def get_ml_latency_budget_ms() -> int:
        """Get ML inference latency budget in milliseconds."""
        return int(os.getenv("SCOUT_ML_LATENCY_BUDGET_MS", "50"))

    @staticmethod
    def get_ml_latency_warn_threshold_ms() -> int:
        """Get warning threshold for ML latency (90% of budget)."""
        return int(ScoutConfig.get_ml_latency_budget_ms() * 0.9)

    @staticmethod
    def get_ml_latency_critical_ms() -> int:
        """Get critical threshold for ML latency (at budget)."""
        return ScoutConfig.get_ml_latency_budget_ms()

    @staticmethod
    def get_model_pruning_enabled() -> bool:
        """Get whether model pruning for latency optimization is enabled."""
        return os.getenv("SCOUT_MODEL_PRUNING_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_model_quantization_enabled() -> bool:
        """Get whether model quantization (float16) is enabled."""
        return os.getenv("SCOUT_MODEL_QUANTIZATION_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_batch_inference_enabled() -> bool:
        """Get whether batch inference is enabled."""
        return os.getenv("SCOUT_BATCH_INFERENCE_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_batch_inference_size() -> int:
        """Get batch size for batch inference."""
        return int(os.getenv("SCOUT_BATCH_INFERENCE_SIZE", "32"))

    @staticmethod
    def get_feature_cache_enabled() -> bool:
        """Get whether feature caching is enabled."""
        return os.getenv("SCOUT_FEATURE_CACHE_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_feature_cache_ttl_seconds() -> int:
        """Get feature cache TTL in seconds."""
        return int(os.getenv("SCOUT_FEATURE_CACHE_TTL_SECONDS", "3600"))

    # ========================================================================
    # ML Training Configuration
    # ========================================================================

    @staticmethod
    def get_auto_retrain_enabled() -> bool:
        """Get whether automatic model retraining is enabled."""
        return os.getenv("SCOUT_AUTO_RETRAIN_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_retrain_interval_hours() -> int:
        """Get interval for automatic retraining in hours."""
        return int(os.getenv("SCOUT_RETRAIN_INTERVAL_HOURS", "24"))

    @staticmethod
    def get_min_samples_for_retrain() -> int:
        """Get minimum samples required for retraining."""
        return int(os.getenv("SCOUT_MIN_SAMPLES_FOR_RETRAIN", "100"))

    @staticmethod
    def get_concept_drift_threshold() -> float:
        """Get threshold for concept drift detection (0.0-1.0)."""
        return float(os.getenv("SCOUT_CONCEPT_DRIFT_THRESHOLD", "0.1"))

    @staticmethod
    def get_shap_enabled() -> bool:
        """Get whether SHAP explainability is enabled."""
        return os.getenv("SCOUT_SHAP_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_hyperopt_enabled() -> bool:
        """Get whether hyperparameter optimization is enabled."""
        return os.getenv("SCOUT_HYPEROPT_ENABLED", "false").lower() == "true"

    @staticmethod
    def get_hyperopt_trials() -> int:
        """Get number of hyperparameter optimization trials."""
        return int(os.getenv("SCOUT_HYPEROPT_TRIALS", "50"))

    @staticmethod
    def get_ab_testing_enabled() -> bool:
        """Get whether A/B testing for models is enabled."""
        return os.getenv("SCOUT_AB_TESTING_ENABLED", "false").lower() == "true"

    @staticmethod
    def get_ab_test_traffic_split() -> float:
        """Get traffic split for A/B testing (0.0-1.0, e.g., 0.1 = 10% to model B)."""
        return float(os.getenv("SCOUT_AB_TEST_TRAFFIC_SPLIT", "0.1"))

    # ========================================================================
    # ML Monitoring Configuration
    # ========================================================================

    @staticmethod
    def get_ml_monitoring_enabled() -> bool:
        """Get whether ML model monitoring is enabled."""
        return os.getenv("SCOUT_ML_MONITORING_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_prediction_tracking_enabled() -> bool:
        """Get whether prediction accuracy tracking is enabled."""
        return os.getenv("SCOUT_PREDICTION_TRACKING_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_prediction_logging_enabled() -> bool:
        """Get whether prediction logging to database is enabled."""
        return os.getenv("SCOUT_PREDICTION_LOGGING_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_validation_enabled() -> bool:
        """Get whether model validation is enabled."""
        return os.getenv("SCOUT_VALIDATION_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_validation_time_window_days() -> int:
        """Get primary time window for validation in days."""
        return int(os.getenv("SCOUT_VALIDATION_TIME_WINDOW_DAYS", "7"))

    @staticmethod
    def get_alert_webhook_url() -> Optional[str]:
        """Get webhook URL for validation alerts (Discord/Slack)."""
        return os.getenv("SCOUT_ALERT_WEBHOOK_URL")

    @staticmethod
    def get_alert_high_error_threshold() -> float:
        """Get high error rate threshold for alerts (SOL)."""
        return float(os.getenv("SCOUT_ALERT_HIGH_ERROR_THRESHOLD", "0.5"))

    @staticmethod
    def get_alert_drift_threshold() -> float:
        """Get drift threshold for alerts (0.0-1.0)."""
        return float(os.getenv("SCOUT_ALERT_DRIFT_THRESHOLD", "0.15"))

    @staticmethod
    def get_alert_low_accuracy_threshold() -> float:
        """Get low direction accuracy threshold for alerts (0.0-1.0)."""
        return float(os.getenv("SCOUT_ALERT_LOW_ACCURACY_THRESHOLD", "0.5"))

    @staticmethod
    def get_alert_dir() -> str:
        """Get directory for storing alert files."""
        return os.getenv("SCOUT_ALERT_DIR", "data/alerts")

    @staticmethod
    def get_drift_detection_enabled() -> bool:
        """Get whether model drift detection is enabled."""
        return os.getenv("SCOUT_DRIFT_DETECTION_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_feature_drift_threshold() -> float:
        """Get threshold for feature distribution drift (KL divergence)."""
        return float(os.getenv("SCOUT_FEATURE_DRIFT_THRESHOLD", "0.2"))

    @staticmethod
    def get_model_registry_enabled() -> bool:
        """Get whether model registry (versioning) is enabled."""
        return os.getenv("SCOUT_MODEL_REGISTRY_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_mlflow_tracking_enabled() -> bool:
        """Get whether MLflow experiment tracking is enabled."""
        return os.getenv("SCOUT_MLFLOW_TRACKING_ENABLED", "false").lower() == "true"

    @staticmethod
    def get_mlflow_tracking_uri() -> str:
        """Get MLflow tracking URI."""
        return os.getenv("SCOUT_MLFLOW_TRACKING_URI", "mlruns")

    # ========================================================================
    # ML Model Hyperparameters (XGBoost/LightGBM defaults)
    # ========================================================================

    @staticmethod
    def get_xgboost_n_estimators() -> int:
        """Get XGBoost number of estimators."""
        return int(os.getenv("SCOUT_XGBOOST_N_ESTIMATORS", "100"))

    @staticmethod
    def get_xgboost_max_depth() -> int:
        """Get XGBoost maximum tree depth."""
        return int(os.getenv("SCOUT_XGBOOST_MAX_DEPTH", "6"))

    @staticmethod
    def get_xgboost_learning_rate() -> float:
        """Get XGBoost learning rate."""
        return float(os.getenv("SCOUT_XGBOOST_LEARNING_RATE", "0.1"))

    @staticmethod
    def get_xgboost_subsample() -> float:
        """Get XGBoost subsample ratio."""
        return float(os.getenv("SCOUT_XGBOOST_SUBSAMPLE", "0.8"))

    @staticmethod
    def get_lightgbm_n_estimators() -> int:
        """Get LightGBM number of estimators."""
        return int(os.getenv("SCOUT_LIGHTGBM_N_ESTIMATORS", "100"))

    @staticmethod
    def get_lightgbm_max_depth() -> int:
        """Get LightGBM maximum tree depth."""
        return int(os.getenv("SCOUT_LIGHTGBM_MAX_DEPTH", "6"))

    @staticmethod
    def get_lightgbm_learning_rate() -> float:
        """Get LightGBM learning rate."""
        return float(os.getenv("SCOUT_LIGHTGBM_LEARNING_RATE", "0.1"))

    @staticmethod
    def get_lightgbm_num_leaves() -> int:
        """Get LightGBM number of leaves."""
        return int(os.getenv("SCOUT_LIGHTGBM_NUM_LEAVES", "31"))

    # ========================================================================
    # ML Feature Selection Configuration
    # ========================================================================

    @staticmethod
    def get_feature_selection_enabled() -> bool:
        """Get whether automatic feature selection is enabled."""
        return os.getenv("SCOUT_FEATURE_SELECTION_ENABLED", "true").lower() == "true"

    @staticmethod
    def get_max_features() -> int:
        """Get maximum number of features to use."""
        return int(os.getenv("SCOUT_MAX_FEATURES", "50"))

    @staticmethod
    def get_feature_importance_threshold() -> float:
        """Get threshold for feature importance (0.0-1.0)."""
        return float(os.getenv("SCOUT_FEATURE_IMPORTANCE_THRESHOLD", "0.01"))

    # ========================================================================
    # Token Safety Fail Mode
    # ========================================================================

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
    def validate_production_config(strict: bool = True) -> None:
        """
        Validate production configuration and raise an exception if critical settings are missing.

        This is a stricter version of validate_config() that actually fails instead of just warning.
        Should be called at application startup to fail fast if required configuration is missing.

        Args:
            strict: If True, raises exceptions for missing critical config. If False, just logs warnings.

        Raises:
            RuntimeError: If critical production configuration is missing
        """
        import logging
        logger = logging.getLogger(__name__)

        errors = []
        warnings = []

        # Critical API keys for production
        if not os.getenv("HELIUS_API_KEY"):
            error = "HELIUS_API_KEY is required for wallet discovery in production"
            if strict:
                errors.append(error)
            else:
                warnings.append(error)

        # At least one liquidity source is required in production (unless in simulated mode)
        mode = ScoutConfig.get_liquidity_mode()
        if mode == "real":
            if not os.getenv("BIRDEYE_API_KEY"):
                error = "BIRDEYE_API_KEY is required for historical liquidity data in production"
                if strict:
                    errors.append(error)
                else:
                    warnings.append(error)

            # Warn if strict mode is off in production
            strict_mode = os.getenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "true").lower() == "true"
            if not strict_mode:
                msg = "SCOUT_STRICT_HISTORICAL_LIQUIDITY is OFF - backtests may use current liquidity for old trades (SURVIVORSHIP BIAS RISK)"
                warnings.append(msg)

        # Database path validation
        db_path = os.getenv("CHIMERA_DB_PATH", "data/chimera.db")
        db_dir = Path(db_path).parent
        if not db_dir.exists() and strict:
            # Try to create it
            try:
                db_dir.mkdir(parents=True, exist_ok=True)
            except Exception as e:
                errors.append(f"Cannot create database directory {db_dir}: {e}")

        # Log warnings first
        for warning in warnings:
            logger.warning(f"Config validation warning: {warning}")

        # Raise error if any critical errors
        if errors:
            error_msg = "Production configuration validation failed:\n  - " + "\n  - ".join(errors)
            raise RuntimeError(error_msg)

        if strict:
            logger.info("Production configuration validation passed")

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

        # Optimization Configuration
        print("\nOptimization Settings:")
        print(f"  Optimization Enabled: {ScoutConfig.get_optimization_enabled()}")
        print(f"  Credit Tracking: {ScoutConfig.get_credit_tracking_enabled()}")
        print(f"  WQS Boost: {ScoutConfig.get_wqs_boost_enabled()}")
        print(f"  Production Monitoring: {ScoutConfig.get_production_monitoring_enabled()}")
        print(f"  Growth Optimized: {ScoutConfig.get_growth_optimized()}")
        print(f"  Current Capital: ${ScoutConfig.get_current_capital():,.0f}")
        print(f"  Target Capital: ${ScoutConfig.get_target_capital():,.0f}")
        print(f"  Monthly Credits: {ScoutConfig.get_monthly_credits():,}")
        print("\nML Model Settings:")
        print(f"  ML Enabled: {ScoutConfig.get_ml_enabled()}")
        print(f"  Ensemble Methods: {ScoutConfig.get_ensemble_enabled()}")
        print(f"  XGBoost: {ScoutConfig.get_xgboost_enabled()}")
        print(f"  LightGBM: {ScoutConfig.get_lightgbm_enabled()}")
        print(f"  Meta-Learner: {ScoutConfig.get_meta_learner_enabled()}")
        print(f"  Time-Series Features: {ScoutConfig.get_time_series_features_enabled()}")
        print(f"  Market Context Features: {ScoutConfig.get_market_context_features_enabled()}")
        print(f"  Advanced Risk Features: {ScoutConfig.get_advanced_risk_features_enabled()}")
        print(f"  Regime-Specific Models: {ScoutConfig.get_regime_models_enabled()}")
        print(f"  Online Learning: {ScoutConfig.get_online_learning_enabled()}")
        print(f"  PyTorch Models: {ScoutConfig.get_torch_enabled()}")
        print(f"  Latency Budget: {ScoutConfig.get_ml_latency_budget_ms()}ms")
        print(f"  Model Pruning: {ScoutConfig.get_model_pruning_enabled()}")
        print(f"  Quantization: {ScoutConfig.get_model_quantization_enabled()}")
        print(f"  Batch Inference: {ScoutConfig.get_batch_inference_enabled()} (size: {ScoutConfig.get_batch_inference_size()})")
        print(f"  Feature Cache: {ScoutConfig.get_feature_cache_enabled()} (TTL: {ScoutConfig.get_feature_cache_ttl_seconds()}s)")
        print(f"  Auto-Retrain: {ScoutConfig.get_auto_retrain_enabled()} (interval: {ScoutConfig.get_retrain_interval_hours()}h)")
        print(f"  SHAP Explainability: {ScoutConfig.get_shap_enabled()}")
        print(f"  Hyperopt: {ScoutConfig.get_hyperopt_enabled()}")
        print(f"  A/B Testing: {ScoutConfig.get_ab_testing_enabled()}")
        print("\nRate Limiting:")
        print(f"  Max Requests/sec: {ScoutConfig.get_max_requests_per_second()}")
        print(f"  Target RPS: {ScoutConfig.get_target_rps()}")
        print(f"  Adaptive Rate Limiting: {ScoutConfig.get_rate_limit_adaptive()}")
        if ScoutConfig.get_rate_limit_adaptive():
            print(f"  Min Delay: {ScoutConfig.get_rate_limit_min_delay_ms()}ms")
            print(f"  Max Delay: {ScoutConfig.get_rate_limit_max_delay_ms()}ms")
            print(f"  Discovery Concurrency: {ScoutConfig.get_discovery_concurrency()}")
        print(f"  Circuit Breaker Threshold: {ScoutConfig.get_circuit_breaker_threshold()}")
        print("=" * 70)

        is_valid, warnings = ScoutConfig.validate_config()
        if warnings:
            print("\nConfiguration Warnings:")
            for warning in warnings:
                print(f"  ⚠️  {warning}")
        else:
            print("\n✓ Configuration looks good!")


