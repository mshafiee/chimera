//! Configuration management for Chimera Operator
//!
//! Loads configuration from YAML files and environment variables.
//! Environment variables override YAML values.

use config::{Config, ConfigError, Environment, File};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeMode {
    Devnet,
    Paper,
    #[default]
    Live,
}

/// Resolve trade mode from an optional explicit override, config value, and RPC URL.
///
/// Rules, in order:
/// 1. `explicit` is `Some` → return it (user chose explicitly via env).
/// 2. `config_mode` is non-default (not `Live`) → return it (set in YAML config).
/// 3. `rpc_url` contains `"devnet"` → `Devnet` (auto-detect with log).
/// 4. else `Live`.
pub fn resolve_trade_mode(
    explicit: Option<TradeMode>,
    config_mode: TradeMode,
    rpc_url: &str,
) -> TradeMode {
    if let Some(mode) = explicit {
        return mode;
    }
    if config_mode != TradeMode::Live {
        return config_mode;
    }
    if rpc_url.contains("devnet") {
        tracing::info!(rpc_url = %rpc_url, "Auto-detected devnet RPC URL → TradeMode::Devnet");
        return TradeMode::Devnet;
    }
    TradeMode::Live
}

impl std::fmt::Display for TradeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradeMode::Devnet => write!(f, "DEVNET"),
            TradeMode::Paper => write!(f, "PAPER"),
            TradeMode::Live => write!(f, "LIVE"),
        }
    }
}

/// Root configuration structure
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    /// Server configuration
    pub server: ServerConfig,
    /// RPC endpoint configuration
    pub rpc: RpcConfig,
    /// Database configuration
    pub database: DatabaseConfig,
    /// Security settings
    pub security: SecurityConfig,
    /// Circuit breaker thresholds
    pub circuit_breakers: CircuitBreakerConfig,
    /// Strategy allocation
    pub strategy: StrategyConfig,
    /// Jito tip configuration
    pub jito: JitoConfig,
    /// Trade mode: devnet, paper, or live
    #[serde(default)]
    pub trade_mode: TradeMode,
    /// Jupiter API configuration
    #[serde(default)]
    pub jupiter: JupiterConfig,
    /// Queue configuration
    pub queue: QueueConfig,
    /// Token safety configuration
    #[serde(default)]
    pub token_safety: TokenSafetyConfig,
    /// Notification configuration
    #[serde(default)]
    pub notifications: NotificationsConfig,
    /// Monitoring configuration
    #[serde(default)]
    pub monitoring: Option<MonitoringConfig>,
    /// Profit management configuration
    #[serde(default)]
    pub profit_management: ProfitManagementConfig,
    /// Position sizing configuration
    #[serde(default)]
    pub position_sizing: PositionSizingConfig,
    /// MEV protection configuration
    #[serde(default)]
    pub mev_protection: MevProtectionConfig,
    /// Degradation and reliability monitoring configuration
    #[serde(default)]
    pub degradation: DegradationConfig,
}

/// HTTP server configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Host to bind to
    #[serde(default = "default_host")]
    pub host: String,
    /// Port to listen on
    #[serde(default = "default_port")]
    pub port: u16,
    /// Number of worker threads for the Tokio runtime (0 = auto-detect)
    #[serde(default = "default_worker_threads")]
    pub worker_threads: usize,
    /// Request timeout in milliseconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_ms: u64,
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_worker_threads() -> usize {
    4
}

fn default_request_timeout() -> u64 {
    30000
}

/// RPC endpoint configuration
#[derive(Debug, Clone, Deserialize)]
pub struct RpcConfig {
    /// Primary RPC provider name
    #[serde(default = "default_primary_provider")]
    pub primary_provider: String,
    /// Primary RPC endpoint URL
    #[serde(default = "default_primary_url")]
    pub primary_url: String,
    /// Fallback RPC endpoint URL (QuickNode/Triton)
    pub fallback_url: Option<String>,
    /// Rate limit per second
    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_second: u32,
    /// Connection timeout in milliseconds
    #[serde(default = "default_rpc_timeout")]
    pub timeout_ms: u64,
    /// Max consecutive failures before fallback
    #[serde(default = "default_max_failures")]
    pub max_consecutive_failures: u32,
    /// When true, follows `getHealth` with a `getLatestBlockhash` probe to detect providers
    /// that return "ok" unconditionally regardless of actual node state.
    #[serde(default = "default_functional_health_check")]
    pub functional_health_check: bool,
}

fn default_primary_provider() -> String {
    "helius".to_string()
}

fn default_primary_url() -> String {
    "https://api.mainnet-beta.solana.com".to_string()
}

fn default_rate_limit() -> u32 {
    40
}

fn default_rpc_timeout() -> u64 {
    2000
}

fn default_max_failures() -> u32 {
    3
}

fn default_functional_health_check() -> bool {
    true
}

/// Database configuration
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// Path to SQLite database file
    #[serde(default = "default_db_path")]
    pub path: PathBuf,
    /// Maximum connections in pool
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

fn default_db_path() -> PathBuf {
    PathBuf::from("data/chimera.db")
}

fn default_max_connections() -> u32 {
    5
}

/// Security configuration
#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    /// HMAC secret for webhook verification (loaded from env)
    #[serde(default)]
    pub webhook_secret: String,
    /// Previous HMAC secret (for rotation grace period)
    #[serde(default)]
    pub webhook_secret_previous: Option<String>,
    /// Maximum timestamp drift in seconds for replay protection
    #[serde(default = "default_max_timestamp_drift")]
    pub max_timestamp_drift_secs: i64,
    /// Rate limit: max requests per second
    #[serde(default = "default_webhook_rate_limit")]
    pub webhook_rate_limit: u32,
    /// Rate limit: burst size
    #[serde(default = "default_webhook_burst")]
    pub webhook_burst_size: u32,
    /// API keys for management endpoints (format: "key:role")
    #[serde(default)]
    pub api_keys: Vec<ApiKeyConfig>,
    /// Admin wallets for management endpoints
    #[serde(default)]
    pub admin_wallets: Vec<AdminWalletConfig>,
}

/// API key configuration
///
/// # Security note
/// The `key` field is read from config.yaml as plaintext. Prefer setting
/// `CHIMERA_RPC__API_KEY` / `CHIMERA_RPC__FALLBACK_API_KEY` as environment
/// variables or storing the key in the vault (`vault.rs`), which loads those
/// env vars into an encrypted `VaultSecrets` bundle. The YAML field is a
/// fallback for local development only and should never be committed to git.
#[derive(Clone, Deserialize)]
pub struct ApiKeyConfig {
    /// The API key value
    pub key: String,
    /// The role: admin, operator, readonly
    pub role: String,
}

impl std::fmt::Debug for ApiKeyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeyConfig")
            .field("key", &"[REDACTED]")
            .field("role", &self.role)
            .finish()
    }
}

/// Admin wallet configuration
#[derive(Debug, Clone, Deserialize)]
pub struct AdminWalletConfig {
    /// The wallet address
    pub address: String,
    /// The role: admin, operator, readonly
    pub role: String,
}

impl SecurityConfig {
    /// Get all valid secrets for HMAC verification (current + previous)
    pub fn get_all_secrets(&self) -> Vec<String> {
        let mut secrets = vec![self.webhook_secret.clone()];
        if let Some(ref prev) = self.webhook_secret_previous {
            if !prev.is_empty() {
                secrets.push(prev.clone());
            }
        }
        secrets
    }
}

fn default_max_timestamp_drift() -> i64 {
    60
}

fn default_webhook_rate_limit() -> u32 {
    100
}

fn default_webhook_burst() -> u32 {
    150
}

/// Circuit breaker configuration
#[derive(Debug, Clone, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Maximum loss in 24h (USD) before halting
    #[serde(default = "default_max_loss")]
    pub max_loss_24h_usd: Decimal,
    /// Maximum consecutive losses before pausing Spear
    #[serde(default = "default_max_consecutive_losses")]
    pub max_consecutive_losses: u32,
    /// Maximum drawdown percentage before emergency exit
    #[serde(default = "default_max_drawdown")]
    pub max_drawdown_percent: Decimal,
    /// Maximum portfolio loss in 24h (percent) before halting
    #[serde(default = "default_portfolio_stop_loss_percent")]
    pub portfolio_stop_loss_percent: Decimal,
    /// Cooldown period in minutes after circuit trips
    #[serde(default = "default_cooldown")]
    pub cooldown_minutes: u32,
    /// Maximum consecutive Jupiter API failures before halting
    #[serde(default = "default_max_jupiter_failures")]
    pub max_jupiter_failures: u32,
}

fn default_max_loss() -> Decimal {
    dec!(500.0)
}

fn default_max_consecutive_losses() -> u32 {
    5
}

fn default_max_drawdown() -> Decimal {
    dec!(15.0)
}

fn default_portfolio_stop_loss_percent() -> Decimal {
    dec!(-5.0)
}

fn default_cooldown() -> u32 {
    30
}

fn default_max_jupiter_failures() -> u32 {
    10  // Allow up to 10 consecutive Jupiter API failures before halting
}

/// Strategy allocation configuration
#[derive(Debug, Clone, Deserialize)]
pub struct StrategyConfig {
    /// Percentage of capital for Shield strategy
    #[serde(default = "default_shield_percent")]
    pub shield_percent: u32,
    /// Percentage of capital for Spear strategy
    #[serde(default = "default_spear_percent")]
    pub spear_percent: u32,
    /// Maximum position size in SOL
    #[serde(default = "default_max_position")]
    pub max_position_sol: Decimal,
    /// Minimum position size in SOL
    #[serde(default = "default_min_position")]
    pub min_position_sol: Decimal,
    /// Minimum signal quality score to accept a Shield trade (0.0–1.0)
    #[serde(default = "default_shield_signal_quality_threshold")]
    pub shield_signal_quality_threshold: f64,
    /// Minimum signal quality score to accept a Spear trade (0.0–1.0)
    #[serde(default = "default_spear_signal_quality_threshold")]
    pub spear_signal_quality_threshold: f64,
    /// DEX fee rate (e.g. 0.003 for 0.3%)
    #[serde(default = "default_dex_fee_rate")]
    pub dex_fee_rate: Decimal,
    /// Maximum total execution cost (tip + fee + slippage) for Shield as a fraction of trade size (e.g. 0.05 for 5%)
    #[serde(default = "default_shield_max_cost")]
    pub shield_max_total_cost_percent: Decimal,
    /// Maximum total execution cost for Spear as a fraction of trade size (e.g. 0.08 for 8%)
    #[serde(default = "default_spear_max_cost")]
    pub spear_max_total_cost_percent: Decimal,
    /// Fallback slippage fraction for trades below `slippage_fallback_threshold_sol` when
    /// Jupiter price impact is unavailable (e.g. 0.005 = 0.5%)
    #[serde(default = "default_slippage_fallback_small")]
    pub slippage_fallback_small_percent: Decimal,
    /// Fallback slippage fraction for trades at or above `slippage_fallback_threshold_sol`
    /// (e.g. 0.01 = 1.0%)
    #[serde(default = "default_slippage_fallback_large")]
    pub slippage_fallback_large_percent: Decimal,
    /// SOL amount boundary separating "small" from "large" trades for slippage fallback
    #[serde(default = "default_slippage_fallback_threshold")]
    pub slippage_fallback_threshold_sol: Decimal,
}

fn default_shield_percent() -> u32 {
    70
}

fn default_shield_signal_quality_threshold() -> f64 {
    0.55
}

fn default_spear_signal_quality_threshold() -> f64 {
    0.55
}

fn default_spear_percent() -> u32 {
    30
}

fn default_max_position() -> Decimal {
    dec!(1.0)
}

fn default_min_position() -> Decimal {
    dec!(0.01)
}

fn default_dex_fee_rate() -> Decimal {
    dec!(0.003)
}

fn default_shield_max_cost() -> Decimal {
    dec!(0.05)
}

fn default_spear_max_cost() -> Decimal {
    dec!(0.08)
}

fn default_slippage_fallback_small() -> Decimal {
    dec!(0.005)
}

fn default_slippage_fallback_large() -> Decimal {
    dec!(0.01)
}

fn default_slippage_fallback_threshold() -> Decimal {
    dec!(0.5)
}

/// Jito bundle tip configuration
#[derive(Debug, Clone, Deserialize)]
pub struct JitoConfig {
    /// Enabled flag
    #[serde(default = "default_jito_enabled")]
    pub enabled: bool,
    /// Jito Searcher endpoint URL (for direct integration)
    #[serde(default = "default_jito_searcher_endpoint")]
    pub searcher_endpoint: Option<String>,
    /// Use Helius Sender API as fallback if direct Jito fails
    #[serde(default = "default_helius_fallback")]
    pub helius_fallback: bool,
    /// Minimum tip in SOL
    #[serde(default = "default_tip_floor")]
    pub tip_floor_sol: Decimal,
    /// Maximum tip in SOL
    #[serde(default = "default_tip_ceiling")]
    pub tip_ceiling_sol: Decimal,
    /// Percentile of recent tips to use
    #[serde(default = "default_tip_percentile")]
    pub tip_percentile: u32,
    /// Maximum tip as percentage of trade size
    #[serde(default = "default_tip_percent_max")]
    pub tip_percent_max: Decimal,
}

fn default_jito_enabled() -> bool {
    true
}

fn default_jito_searcher_endpoint() -> Option<String> {
    Some("https://mainnet.block-engine.jito.wtf".to_string())
}

fn default_helius_fallback() -> bool {
    true
}

fn default_tip_floor() -> Decimal {
    dec!(0.001)
}

fn default_tip_ceiling() -> Decimal {
    dec!(0.01)
}

fn default_tip_percentile() -> u32 {
    50
}

fn default_tip_percent_max() -> Decimal {
    dec!(0.10)
}

/// Jupiter API configuration
#[derive(Clone, Default, Deserialize)]
pub struct JupiterConfig {
    /// Jupiter API base URL
    #[serde(default = "default_jupiter_api_url")]
    pub api_url: String,
    /// Jupiter API key (sent as `x-api-key` on every Jupiter request).
    ///
    /// Load via env `CHIMERA_JUPITER__API_KEY`. Required in `Live` trade mode
    /// (see [`AppConfig::validate`]); keyless access is being phased out by
    /// Jupiter (legacy rate limits expire 2026-06-30).
    #[serde(default)]
    pub api_key: Option<String>,
    /// Enable V0 message reconstruction on blockhash expiry
    #[serde(default = "default_reconstruct_v0")]
    pub reconstruct_v0_on_blockhash_expiry: bool,
    /// Reject V0 transactions entirely (fallback if reconstruction fails)
    #[serde(default = "default_reject_v0")]
    pub reject_v0_transactions: bool,
    /// Use the Swap v2 Meta-Aggregator (`/order`) instead of the
    /// deprecated v1 Metis endpoint (`/swap/v1/quote` + `/swap/v1/swap`).
    ///
    /// v2 provides RTSE, Jupiter Beam (MEV protection), gasless support,
    /// and multi-router competition (Metis, JupiterZ RFQ, Dflow, OKX).
    #[serde(default = "default_use_swap_v2")]
    pub use_swap_v2: bool,
    /// Compare per-DEX routes (via Jupiter `dexes=`) against the aggregate quote
    /// and pick the best `outAmount`. On by default; disable to issue a single
    /// aggregate quote (lower Jupiter API quota use) when routing diversity
    /// isn't needed.
    #[serde(default = "default_multi_dex_comparison")]
    pub multi_dex_comparison: bool,
    /// Enable RTSE (Real-Time Slippage Estimation) for automatic slippage
    /// optimization based on current market conditions. Only applies to v2.
    #[serde(default = "default_enable_rtse")]
    pub enable_rtse: bool,
    /// Comma-separated list of routers to exclude (e.g., "metis,jupiterz,dflow,okx").
    /// Only applies to v2 Meta-Aggregator.
    #[serde(default)]
    pub exclude_routers: Option<String>,
    /// Comma-separated list of DEXes to exclude from Metis router
    /// (e.g., "Raydium,Orca+V2,Meteora+DLMM"). Only affects Metis, not other routers.
    #[serde(default)]
    pub exclude_dexes: Option<String>,
}

impl std::fmt::Debug for JupiterConfig {
    /// Redact `api_key` so any `{:?}`/tracing print of `AppConfig` cannot leak
    /// the live Jupiter credential (mirrors `ApiKeyConfig`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JupiterConfig")
            .field("api_url", &self.api_url)
            .field(
                "api_key",
                &self
                    .api_key
                    .as_ref()
                    .map(|_| "[REDACTED]")
                    .unwrap_or("[unset]"),
            )
            .field(
                "reconstruct_v0_on_blockhash_expiry",
                &self.reconstruct_v0_on_blockhash_expiry,
            )
            .field("reject_v0_transactions", &self.reject_v0_transactions)
            .field("use_swap_v2", &self.use_swap_v2)
            .field("multi_dex_comparison", &self.multi_dex_comparison)
            .finish()
    }
}

fn default_jupiter_api_url() -> String {
    "https://api.jup.ag/swap/v1".to_string()
}

fn default_reconstruct_v0() -> bool {
    true
}

fn default_reject_v0() -> bool {
    false
}

fn default_use_swap_v2() -> bool {
    false
}

fn default_multi_dex_comparison() -> bool {
    true
}

fn default_enable_rtse() -> bool {
    true  // Enable RTSE by default for better slippage protection
}

fn default_jupiter_api_url() -> String {
    "https://api.jup.ag/swap/v2".to_string()  // Updated to v2
}

/// Queue configuration
#[derive(Debug, Clone, Deserialize)]
pub struct QueueConfig {
    /// Maximum queue capacity
    #[serde(default = "default_queue_capacity")]
    pub capacity: usize,
    /// Threshold for load shedding (percentage of capacity)
    #[serde(default = "default_load_shed_threshold")]
    pub load_shed_threshold_percent: u32,
    /// Enable parallel processing with worker pool
    #[serde(default = "default_parallel_enabled")]
    pub parallel_enabled: bool,
    /// Number of parallel workers (default: 4, should match DB connection pool size)
    #[serde(default = "default_num_workers")]
    pub num_workers: Option<usize>,
    /// Maximum concurrent RPC requests across all workers
    #[serde(default = "default_max_concurrent_rpc")]
    pub max_concurrent_rpc: Option<usize>,
}

fn default_queue_capacity() -> usize {
    1000
}

fn default_load_shed_threshold() -> u32 {
    80
}

fn default_parallel_enabled() -> bool {
    true
}

fn default_num_workers() -> Option<usize> {
    Some(4)
}

fn default_max_concurrent_rpc() -> Option<usize> {
    Some(8)
}

/// Token safety configuration
#[derive(Debug, Clone, Deserialize)]
pub struct TokenSafetyConfig {
    /// Token mints allowed to have freeze authority
    #[serde(default = "default_authority_whitelist")]
    pub freeze_authority_whitelist: Vec<String>,
    /// Token mints allowed to have mint authority
    #[serde(default = "default_authority_whitelist")]
    pub mint_authority_whitelist: Vec<String>,
    /// Minimum liquidity for Shield strategy (USD)
    #[serde(default = "default_min_liquidity_shield")]
    pub min_liquidity_shield_usd: Decimal,
    /// Minimum liquidity for Spear strategy (USD)
    #[serde(default = "default_min_liquidity_spear")]
    pub min_liquidity_spear_usd: Decimal,
    /// Enable honeypot detection
    #[serde(default = "default_honeypot_detection")]
    pub honeypot_detection_enabled: bool,
    /// Token cache capacity
    #[serde(default = "default_token_cache_capacity")]
    pub cache_capacity: usize,
    /// Token cache TTL in seconds
    #[serde(default = "default_token_cache_ttl")]
    pub cache_ttl_seconds: i64,
    /// When true, fall back to supply-based heuristic for tokens not indexed by DexScreener.
    /// Default false (strict mode — unlisted tokens are rejected as $0 liquidity).
    #[serde(default = "default_allow_unlisted_heuristic")]
    pub allow_unlisted_heuristic: bool,
    /// Minimum token age in hours to allow a BUY signal through. 0.0 disables the check.
    /// Default 1.0 (reject tokens deployed less than 1 hour ago).
    #[serde(default = "default_min_token_age_hours")]
    pub min_token_age_hours: f64,
}

fn default_allow_unlisted_heuristic() -> bool {
    false
}

fn default_min_token_age_hours() -> f64 {
    1.0
}

fn default_authority_whitelist() -> Vec<String> {
    vec![
        // USDC
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
        // USDT
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(),
        // Wrapped SOL
        "So11111111111111111111111111111111111111112".to_string(),
    ]
}

fn default_min_liquidity_shield() -> Decimal {
    dec!(10000.0)
}

fn default_min_liquidity_spear() -> Decimal {
    dec!(5000.0)
}

fn default_honeypot_detection() -> bool {
    true
}

fn default_token_cache_capacity() -> usize {
    1000
}

fn default_token_cache_ttl() -> i64 {
    3600 // 1 hour
}

impl Default for TokenSafetyConfig {
    fn default() -> Self {
        Self {
            freeze_authority_whitelist: default_authority_whitelist(),
            mint_authority_whitelist: default_authority_whitelist(),
            min_liquidity_shield_usd: default_min_liquidity_shield(),
            min_liquidity_spear_usd: default_min_liquidity_spear(),
            honeypot_detection_enabled: default_honeypot_detection(),
            cache_capacity: default_token_cache_capacity(),
            cache_ttl_seconds: default_token_cache_ttl(),
            allow_unlisted_heuristic: default_allow_unlisted_heuristic(),
            min_token_age_hours: default_min_token_age_hours(),
        }
    }
}

/// Notification configuration
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NotificationsConfig {
    /// Telegram notification settings
    #[serde(default)]
    pub telegram: TelegramNotificationConfig,
    /// Notification rules for different events
    #[serde(default)]
    pub rules: NotificationRulesConfig,
    /// Daily summary settings
    #[serde(default)]
    pub daily_summary: DailySummaryConfig,
}

/// Telegram-specific notification configuration
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramNotificationConfig {
    /// Whether Telegram notifications are enabled
    #[serde(default)]
    pub enabled: bool,
    /// Bot token (from environment: TELEGRAM_BOT_TOKEN)
    #[serde(default)]
    pub bot_token: String,
    /// Chat ID to send notifications to (from environment: TELEGRAM_CHAT_ID)
    #[serde(default)]
    pub chat_id: String,
    /// Rate limit in seconds between similar notifications
    #[serde(default = "default_notification_rate_limit")]
    pub rate_limit_seconds: u64,
}

fn default_notification_rate_limit() -> u64 {
    60 // 1 minute
}

impl Default for TelegramNotificationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bot_token: String::new(),
            chat_id: String::new(),
            rate_limit_seconds: default_notification_rate_limit(),
        }
    }
}

/// Notification rules configuration
#[derive(Debug, Clone, Deserialize)]
pub struct NotificationRulesConfig {
    /// Send notification when circuit breaker trips
    #[serde(default = "default_true")]
    pub circuit_breaker_triggered: bool,
    /// Send notification when wallet balance drops significantly
    #[serde(default = "default_true")]
    pub wallet_drained: bool,
    /// Send notification when a position is exited
    #[serde(default = "default_true")]
    pub position_exited: bool,
    /// Send notification when a wallet is promoted
    #[serde(default = "default_true")]
    pub wallet_promoted: bool,
    /// Send daily trading summary
    #[serde(default = "default_true")]
    pub daily_summary: bool,
    /// Send notification on RPC fallback
    #[serde(default = "default_true")]
    pub rpc_fallback: bool,
    /// Send notification on critical system errors
    #[serde(default = "default_true")]
    pub system_crash: bool,
}

impl Default for NotificationRulesConfig {
    fn default() -> Self {
        Self {
            circuit_breaker_triggered: true,
            wallet_drained: true,
            position_exited: true,
            wallet_promoted: true,
            daily_summary: true,
            rpc_fallback: true,
            system_crash: true,
        }
    }
}

/// Daily summary notification configuration
#[derive(Debug, Clone, Deserialize)]
pub struct DailySummaryConfig {
    /// Whether daily summary is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Hour of day to send summary (24h format, UTC)
    #[serde(default = "default_summary_hour")]
    pub hour_utc: u8,
    /// Minute of hour to send summary
    #[serde(default)]
    pub minute: u8,
}

fn default_summary_hour() -> u8 {
    20 // 8 PM UTC
}

impl Default for DailySummaryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hour_utc: default_summary_hour(),
            minute: 0,
        }
    }
}

/// Monitoring configuration
#[derive(Debug, Clone, Deserialize)]
pub struct MonitoringConfig {
    /// Enable automatic monitoring
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Helius API key
    #[serde(default)]
    pub helius_api_key: Option<String>,
    /// Webhook URL for Helius to send transactions
    #[serde(default)]
    pub helius_webhook_url: Option<String>,
    /// Batch size for webhook registration
    #[serde(default = "default_webhook_batch_size")]
    pub webhook_registration_batch_size: usize,
    /// Delay between webhook registration batches (ms)
    #[serde(default = "default_webhook_delay")]
    pub webhook_registration_delay_ms: u64,
    /// Rate limit for webhook processing (req/sec)
    #[serde(default = "default_monitoring_webhook_rate_limit")]
    pub webhook_processing_rate_limit: u32,
    /// Enable RPC polling fallback
    #[serde(default = "default_true")]
    pub rpc_polling_enabled: bool,
    /// RPC poll interval in seconds
    #[serde(default = "default_rpc_poll_interval")]
    pub rpc_poll_interval_secs: u64,
    /// RPC poll batch size
    #[serde(default = "default_rpc_poll_batch")]
    pub rpc_poll_batch_size: usize,
    /// RPC poll rate limit (req/sec)
    #[serde(default = "default_rpc_poll_rate_limit")]
    pub rpc_poll_rate_limit: u32,
    /// Delay (seconds) before detecting a SELL as a position exit, allowing the
    /// on-chain transaction to settle before reconciliation.
    #[serde(default = "default_exit_detection_delay")]
    pub exit_detection_delay_secs: u64,
    /// Maximum active wallets to monitor
    #[serde(default = "default_max_active_wallets")]
    pub max_active_wallets: usize,
    /// Enable automatic wallet demotion based on copy performance
    #[serde(default = "default_auto_demote_wallets")]
    pub auto_demote_wallets: bool,
    /// Webhook lifecycle management configuration
    #[serde(default)]
    pub webhook_lifecycle: Option<WebhookLifecycleConfig>,
}

/// Webhook lifecycle management configuration
#[derive(Debug, Clone, Deserialize)]
pub struct WebhookLifecycleConfig {
    /// Enable automatic webhook registration (default: true)
    #[serde(default = "default_auto_webhook_register")]
    pub auto_register_enabled: bool,
    /// Enable automatic webhook cleanup (default: true)
    #[serde(default = "default_auto_webhook_cleanup")]
    pub auto_cleanup_enabled: bool,
    /// Health check interval in seconds (default: 3600 = 1 hour)
    #[serde(default = "default_webhook_health_interval")]
    pub health_check_interval_secs: u64,
    /// Stale webhook threshold in days (default: 7)
    #[serde(default = "default_stale_webhook_threshold")]
    pub stale_threshold_days: u32,
    /// Maximum registration retries (default: 3)
    #[serde(default = "default_max_registration_retries")]
    pub max_registration_retries: u32,
    /// Enable Helius dashboard reconciliation on startup (default: true)
    #[serde(default = "default_helius_reconciliation_enabled")]
    pub helius_reconciliation_enabled: bool,
    /// Delete orphaned webhooks during reconciliation (default: true)
    #[serde(default = "default_helius_delete_orphaned")]
    pub helius_delete_orphaned: bool,
    /// Dry-run mode - log only, don't delete (default: false)
    #[serde(default = "default_helius_dry_run")]
    pub helius_dry_run: bool,
}

fn default_auto_webhook_register() -> bool {
    true
}

fn default_auto_webhook_cleanup() -> bool {
    true
}

fn default_webhook_health_interval() -> u64 {
    3600 // 1 hour
}

fn default_stale_webhook_threshold() -> u32 {
    7 // 7 days
}

fn default_max_registration_retries() -> u32 {
    3
}

fn default_helius_reconciliation_enabled() -> bool {
    true
}

fn default_helius_delete_orphaned() -> bool {
    true
}

fn default_helius_dry_run() -> bool {
    false
}

fn default_true() -> bool {
    true
}

fn default_webhook_batch_size() -> usize {
    10
}

fn default_webhook_delay() -> u64 {
    200
}

fn default_monitoring_webhook_rate_limit() -> u32 {
    45
}

fn default_rpc_poll_interval() -> u64 {
    8
}

fn default_rpc_poll_batch() -> usize {
    6
}

fn default_rpc_poll_rate_limit() -> u32 {
    40
}

fn default_exit_detection_delay() -> u64 {
    5
}

fn default_max_active_wallets() -> usize {
    20
}

fn default_auto_demote_wallets() -> bool {
    false
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            helius_api_key: None,
            helius_webhook_url: None,
            webhook_registration_batch_size: default_webhook_batch_size(),
            webhook_registration_delay_ms: default_webhook_delay(),
            webhook_processing_rate_limit: default_webhook_rate_limit(),
            rpc_polling_enabled: true,
            rpc_poll_interval_secs: default_rpc_poll_interval(),
            rpc_poll_batch_size: default_rpc_poll_batch(),
            rpc_poll_rate_limit: default_rpc_poll_rate_limit(),
            exit_detection_delay_secs: default_exit_detection_delay(),
            max_active_wallets: default_max_active_wallets(),
            auto_demote_wallets: default_auto_demote_wallets(),
            webhook_lifecycle: None,
        }
    }
}

/// Profit management configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ProfitManagementConfig {
    /// Profit targets (percentages)
    #[serde(default = "default_profit_targets")]
    pub targets: Vec<Decimal>,
    /// Percentage to sell at each target
    #[serde(default = "default_tiered_exit_percent")]
    pub tiered_exit_percent: Decimal,
    /// Activate trailing stop after this profit %
    #[serde(default = "default_trailing_stop_activation")]
    pub trailing_stop_activation: Decimal,
    /// Trailing stop distance from peak (%)
    #[serde(default = "default_trailing_stop_distance")]
    pub trailing_stop_distance: Decimal,
    /// Maximum allowable loss before stop fires (floor on the dynamic stop, not a fixed trigger).
    /// The adaptive stop may widen due to volatility/consensus, but never beyond this value.
    #[serde(default = "default_max_stop_loss_distance", alias = "hard_stop_loss")]
    pub max_stop_loss_distance: Decimal,
    /// Time-based exit (hours)
    #[serde(default = "default_time_exit_hours")]
    pub time_exit_hours: u64,
    /// Grace period after entry before stop-loss is allowed to fire (wick protection).
    /// Set to 10s — covers most Solana confirmation delays without leaving positions
    /// exposed to extended crashes. A hard -25% stop always bypasses this grace period.
    #[serde(default = "default_wick_protection_secs")]
    pub wick_protection_secs: u64,
    /// Losing time-based exit for Shield strategy (hours)
    #[serde(default = "default_losing_time_exit_hours_shield")]
    pub losing_time_exit_hours_shield: u64,
    /// Losing time-based exit for Spear strategy (hours)
    #[serde(default = "default_losing_time_exit_hours_spear")]
    pub losing_time_exit_hours_spear: u64,
    /// Minimum loss percentage to trigger time-based exit (e.g. -3.0 for -3%)
    #[serde(default = "default_losing_time_exit_threshold")]
    pub losing_time_exit_threshold_percent: Decimal,
    /// Minimum viable position size in SOL — tiered exits that would leave less
    /// than this amount trigger a full exit instead, avoiding dust positions.
    #[serde(default = "default_min_size_sol")]
    pub min_size_sol: Decimal,
    /// ATR-based stop-loss multiplier (1.5x for ATR-based dynamic stops)
    #[serde(default = "default_atr_multiplier")]
    pub atr_multiplier: Decimal,
    /// ATR period for calculation (default 14)
    #[serde(default = "default_atr_period")]
    pub atr_period: u32,
    /// Market regime: BULL (widen stops), BEAR (tighten stops), VOLATILE (widen stops)
    #[serde(default = "default_market_regime")]
    pub market_regime: String,
    /// Bull market multiplier for ATR stops (default 1.5x)
    #[serde(default = "default_bull_market_multiplier")]
    pub bull_market_multiplier: Decimal,
    /// Bear market multiplier for ATR stops (default 1.0x)
    #[serde(default = "default_bear_market_multiplier")]
    pub bear_market_multiplier: Decimal,
    /// Volatile market multiplier for ATR stops (default 2.0x)
    #[serde(default = "default_volatile_market_multiplier")]
    pub volatile_market_multiplier: Decimal,
    /// Enable ATR-based stop-loss (default false for backward compatibility)
    #[serde(default = "default_atr_stop_loss_enabled")]
    pub atr_stop_loss_enabled: bool,
}

fn default_profit_targets() -> Vec<Decimal> {
    vec![dec!(25.0), dec!(50.0), dec!(100.0), dec!(200.0)]
}

fn default_tiered_exit_percent() -> Decimal {
    // Each exit sells this fraction of the *remaining* balance (compound, not original).
    // Four tiers at 33%: 33% + 22% + 15% + 10% ≈ 80% total; trailing stop handles the tail.
    dec!(33.0)
}

fn default_trailing_stop_activation() -> Decimal {
    dec!(50.0)
}

fn default_trailing_stop_distance() -> Decimal {
    dec!(15.0)
}

fn default_max_stop_loss_distance() -> Decimal {
    dec!(-25.0)
}

fn default_time_exit_hours() -> u64 {
    24
}

fn default_wick_protection_secs() -> u64 {
    10
}

fn default_losing_time_exit_hours_shield() -> u64 {
    4
}

fn default_losing_time_exit_hours_spear() -> u64 {
    2
}

fn default_losing_time_exit_threshold() -> Decimal {
    dec!(-3.0)
}

impl Default for ProfitManagementConfig {
    fn default() -> Self {
        Self {
            targets: default_profit_targets(),
            tiered_exit_percent: default_tiered_exit_percent(),
            trailing_stop_activation: default_trailing_stop_activation(),
            trailing_stop_distance: default_trailing_stop_distance(),
            max_stop_loss_distance: default_max_stop_loss_distance(),
            time_exit_hours: default_time_exit_hours(),
            wick_protection_secs: default_wick_protection_secs(),
            losing_time_exit_hours_shield: default_losing_time_exit_hours_shield(),
            losing_time_exit_hours_spear: default_losing_time_exit_hours_spear(),
            losing_time_exit_threshold_percent: default_losing_time_exit_threshold(),
            min_size_sol: default_min_size_sol(),
            atr_multiplier: default_atr_multiplier(),
            atr_period: default_atr_period(),
            market_regime: default_market_regime(),
            bull_market_multiplier: default_bull_market_multiplier(),
            bear_market_multiplier: default_bear_market_multiplier(),
            volatile_market_multiplier: default_volatile_market_multiplier(),
            atr_stop_loss_enabled: default_atr_stop_loss_enabled(),
        }
    }
}

/// Position sizing configuration
#[derive(Debug, Clone, Deserialize)]
pub struct PositionSizingConfig {
    /// Base position size in SOL
    #[serde(default = "default_base_size_sol")]
    pub base_size_sol: Decimal,
    /// Maximum position size in SOL (legacy; overridden per-strategy by shield/spear max)
    #[serde(default = "default_max_size_sol")]
    pub max_size_sol: Decimal,
    /// Minimum position size in SOL
    #[serde(default = "default_min_size_sol")]
    pub min_size_sol: Decimal,
    /// Maximum position size for Shield strategy (conservative, larger allocation)
    #[serde(default = "default_shield_max_size_sol")]
    pub shield_max_size_sol: Decimal,
    /// Maximum position size for Spear strategy (aggressive, smaller allocation)
    #[serde(default = "default_spear_max_size_sol")]
    pub spear_max_size_sol: Decimal,
    /// Consensus multiplier (when multiple wallets buy same token)
    #[serde(default = "default_consensus_multiplier")]
    pub consensus_multiplier: Decimal,
    /// Maximum concurrent positions
    #[serde(default = "default_max_concurrent_positions")]
    pub max_concurrent_positions: usize,
    /// Enable Kelly Criterion sizing
    #[serde(default = "default_use_kelly_sizing")]
    pub use_kelly_sizing: bool,
    /// Total trading capital in SOL (used for Kelly sizing and portfolio heat)
    #[serde(default = "default_total_capital_sol")]
    pub total_capital_sol: Decimal,
    /// Kelly fraction for both strategies (conservative; default 25% of full Kelly).
    /// Spear positions are additionally bounded by spear_max_size_sol.
    #[serde(default = "default_kelly_fraction")]
    pub kelly_fraction: Decimal,
    /// Size multiplier applied during off-hours (02:00–06:00 UTC) to reduce exposure
    /// to low-liquidity windows. Set to 1.0 to disable the reduction.
    #[serde(default = "default_off_hours_size_multiplier")]
    pub off_hours_size_multiplier: Decimal,
}

fn default_base_size_sol() -> Decimal {
    dec!(0.1)
}

fn default_max_size_sol() -> Decimal {
    dec!(2.0)
}

fn default_min_size_sol() -> Decimal {
    dec!(0.05)
}

fn default_atr_multiplier() -> Decimal {
    dec!(1.5)
}

fn default_atr_period() -> u32 {
    14
}

fn default_market_regime() -> String {
    "NEUTRAL".to_string()
}

fn default_bull_market_multiplier() -> Decimal {
    dec!(1.5)
}

fn default_bear_market_multiplier() -> Decimal {
    dec!(1.0)
}

fn default_volatile_market_multiplier() -> Decimal {
    dec!(2.0)
}

fn default_atr_stop_loss_enabled() -> bool {
    false
}

fn default_shield_max_size_sol() -> Decimal {
    dec!(2.0)
}

fn default_spear_max_size_sol() -> Decimal {
    dec!(0.5)
}

fn default_consensus_multiplier() -> Decimal {
    dec!(1.5)
}

fn default_max_concurrent_positions() -> usize {
    5
}

fn default_use_kelly_sizing() -> bool {
    false
}

fn default_total_capital_sol() -> Decimal {
    dec!(10.0)
}

fn default_kelly_fraction() -> Decimal {
    dec!(0.25) // 25% of full Kelly (conservative)
}

fn default_off_hours_size_multiplier() -> Decimal {
    dec!(0.5) // 50% of normal size during 02:00–06:00 UTC low-liquidity window
}

impl Default for PositionSizingConfig {
    fn default() -> Self {
        Self {
            base_size_sol: default_base_size_sol(),
            max_size_sol: default_max_size_sol(),
            min_size_sol: default_min_size_sol(),
            shield_max_size_sol: default_shield_max_size_sol(),
            spear_max_size_sol: default_spear_max_size_sol(),
            consensus_multiplier: default_consensus_multiplier(),
            max_concurrent_positions: default_max_concurrent_positions(),
            use_kelly_sizing: default_use_kelly_sizing(),
            total_capital_sol: default_total_capital_sol(),
            kelly_fraction: default_kelly_fraction(),
            off_hours_size_multiplier: default_off_hours_size_multiplier(),
        }
    }
}

/// MEV protection configuration
#[derive(Debug, Clone, Deserialize)]
pub struct MevProtectionConfig {
    /// Always use Jito bundles
    #[serde(default = "default_always_use_jito")]
    pub always_use_jito: bool,
    /// Tip for exit signals (SOL)
    #[serde(default = "default_exit_tip_sol")]
    pub exit_tip_sol: Decimal,
    /// Tip for consensus signals (SOL)
    #[serde(default = "default_consensus_tip_sol")]
    pub consensus_tip_sol: Decimal,
    /// Tip for standard signals (SOL)
    #[serde(default = "default_standard_tip_sol")]
    pub standard_tip_sol: Decimal,
}

fn default_always_use_jito() -> bool {
    true
}

fn default_exit_tip_sol() -> Decimal {
    dec!(0.007)
}

fn default_consensus_tip_sol() -> Decimal {
    dec!(0.003)
}

fn default_standard_tip_sol() -> Decimal {
    dec!(0.0015)
}

impl Default for MevProtectionConfig {
    fn default() -> Self {
        Self {
            always_use_jito: default_always_use_jito(),
            exit_tip_sol: default_exit_tip_sol(),
            consensus_tip_sol: default_consensus_tip_sol(),
            standard_tip_sol: default_standard_tip_sol(),
        }
    }
}

/// Degradation and reliability monitoring configuration
#[derive(Debug, Clone, Deserialize)]
pub struct DegradationConfig {
    /// Memory pressure threshold (0.0-1.0, default: 0.90)
    #[serde(default = "default_memory_pressure_threshold")]
    pub memory_pressure_threshold: f64,
    /// Disk space warning threshold (0.0-1.0, default: 0.10)
    #[serde(default = "default_disk_space_warning_threshold")]
    pub disk_space_warning_threshold: f64,
    /// Enable automatic log pruning when disk space is low
    #[serde(default = "default_log_pruning_enabled")]
    pub log_pruning_enabled: bool,
    /// Maximum log file size in MB before pruning
    #[serde(default = "default_max_log_size_mb")]
    pub max_log_size_mb: u64,
    /// Enable memory pressure monitoring
    #[serde(default = "default_memory_monitoring_enabled")]
    pub memory_monitoring_enabled: bool,
    /// Enable disk space monitoring
    #[serde(default = "default_disk_monitoring_enabled")]
    pub disk_monitoring_enabled: bool,
    /// Enable RPC rate limit degradation handling
    #[serde(default = "default_rpc_rate_limit_enabled")]
    pub rpc_rate_limit_enabled: bool,
}

fn default_memory_pressure_threshold() -> f64 {
    0.90
}

fn default_disk_space_warning_threshold() -> f64 {
    0.10
}

fn default_log_pruning_enabled() -> bool {
    true
}

fn default_max_log_size_mb() -> u64 {
    100
}

fn default_memory_monitoring_enabled() -> bool {
    true
}

fn default_disk_monitoring_enabled() -> bool {
    true
}

fn default_rpc_rate_limit_enabled() -> bool {
    true
}

impl Default for DegradationConfig {
    fn default() -> Self {
        Self {
            memory_pressure_threshold: default_memory_pressure_threshold(),
            disk_space_warning_threshold: default_disk_space_warning_threshold(),
            log_pruning_enabled: default_log_pruning_enabled(),
            max_log_size_mb: default_max_log_size_mb(),
            memory_monitoring_enabled: default_memory_monitoring_enabled(),
            disk_monitoring_enabled: default_disk_monitoring_enabled(),
            rpc_rate_limit_enabled: default_rpc_rate_limit_enabled(),
        }
    }
}

impl AppConfig {
    /// Load configuration from files and environment with optional custom path
    ///
    /// Priority (highest to lowest):
    /// 1. Environment variables (CHIMERA_*)
    /// 2. Custom config path (if provided)
    /// 3. config/config.yaml (if exists)
    /// 4. config.yaml (if exists)
    /// 5. Default values
    pub fn load(path: Option<&PathBuf>) -> Result<Self, ConfigError> {
        let mut builder = Config::builder()
            // Default config file
            .add_source(File::with_name("config").required(false));
        // Optional custom path
        if let Some(p) = path {
            builder = builder.add_source(File::from(p.as_path()).required(false));
        }
        let config = builder
            // Environment overrides
            .add_source(
                Environment::with_prefix("CHIMERA")
                    .separator("__")
                    .try_parsing(true)
                    .list_separator(","),
            )
            .build()?;
        config.try_deserialize()
    }

    /// Load configuration from files and environment (backward compatible)
    ///
    /// Priority (highest to lowest):
    /// 1. Environment variables (CHIMERA_*)
    /// 2. config/config.yaml (if exists)
    /// 3. config.yaml (if exists)
    /// 4. Default values
    pub fn load_config() -> Result<Self, ConfigError> {
        let config = Config::builder()
            // Start with default values
            .set_default("server.host", "0.0.0.0")?
            .set_default("server.port", 8080)?
            .set_default("server.request_timeout_ms", 30000)?
            .set_default("database.path", "data/chimera.db")?
            .set_default("database.max_connections", 5)?
            .set_default("security.max_timestamp_drift_secs", 60)?
            .set_default("security.webhook_rate_limit", 100)?
            .set_default("security.webhook_burst_size", 150)?
            .set_default("queue.capacity", 1000)?
            .set_default("queue.load_shed_threshold_percent", 80)?
            .set_default("rpc.primary_provider", "helius")?
            .set_default("rpc.primary_url", "https://api.mainnet-beta.solana.com")?
            .set_default("rpc.rate_limit_per_second", 40)?
            .set_default("rpc.timeout_ms", 2000)?
            .set_default("rpc.max_consecutive_failures", 3)?
            // Load from config files (lower priority)
            .add_source(File::with_name("config").required(false))
            .add_source(File::with_name("config/config").required(false))
            // Override with environment variables (highest priority - loaded last)
            // CHIMERA_SERVER__PORT=8081 -> server.port = 8081
            // CHIMERA_TRADE_MODE=paper -> trade_mode = Paper
            .add_source(
                Environment::with_prefix("CHIMERA")
                    .separator("__")
                    .try_parsing(true)
                    .list_separator(","),
            )
            .build()?;

        config.try_deserialize()
    }

    /// Validate configuration values
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Check strategy allocation sums to 100
        if self.strategy.shield_percent + self.strategy.spear_percent != 100 {
            return Err(ConfigError::Message(
                "Strategy allocation (shield_percent + spear_percent) must equal 100".to_string(),
            ));
        }

        // Check webhook secret is set and meets minimum length for security
        if self.security.webhook_secret.is_empty() {
            return Err(ConfigError::Message(
                "Webhook secret must be set via CHIMERA_SECURITY__WEBHOOK_SECRET".to_string(),
            ));
        }
        if self.security.webhook_secret.len() < 32 {
            return Err(ConfigError::Message(
                "Webhook secret must be at least 32 characters (use: openssl rand -hex 32)"
                    .to_string(),
            ));
        }

        // Check RPC URL is set
        if self.rpc.primary_url.is_empty() {
            return Err(ConfigError::Message(
                "RPC primary URL must be set".to_string(),
            ));
        }

        // Validate Jito tip bounds
        if self.jito.tip_floor_sol >= self.jito.tip_ceiling_sol {
            return Err(ConfigError::Message(
                "Jito tip floor must be less than ceiling".to_string(),
            ));
        }

        // Validate position size bounds
        if self.strategy.max_position_sol <= self.strategy.min_position_sol {
            return Err(ConfigError::Message(
                "Max position size must be greater than min position size".to_string(),
            ));
        }

        if self.position_sizing.max_size_sol <= self.position_sizing.min_size_sol {
            return Err(ConfigError::Message(
                "Max position size must be greater than min position size".to_string(),
            ));
        }

        if self.notifications.daily_summary.hour_utc > 23 {
            return Err(ConfigError::Message(format!(
                "notifications.daily_summary.hour_utc must be 0–23, got {}",
                self.notifications.daily_summary.hour_utc
            )));
        }
        if self.notifications.daily_summary.minute > 59 {
            return Err(ConfigError::Message(format!(
                "notifications.daily_summary.minute must be 0–59, got {}",
                self.notifications.daily_summary.minute
            )));
        }

        if self.position_sizing.total_capital_sol <= Decimal::ZERO {
            return Err(ConfigError::Message(
                "position_sizing.total_capital_sol must be greater than zero".to_string(),
            ));
        }

        // Validate worker threads
        if self.server.worker_threads == 0 {
            return Err(ConfigError::Message(
                "server.worker_threads must be > 0".into(),
            ));
        }

        // Validate RPC timeout bounds
        if self.rpc.timeout_ms < 1000 || self.rpc.timeout_ms > 60000 {
            return Err(ConfigError::Message(
                "rpc.timeout_ms must be between 1000 and 60000".into(),
            ));
        }

        // Validate circuit breaker cooldown
        if self.circuit_breakers.cooldown_minutes == 0 {
            return Err(ConfigError::Message(
                "circuit_breakers.cooldown_minutes must be > 0".into(),
            ));
        }

        // Validate max loss threshold
        if self.circuit_breakers.max_loss_24h_usd <= Decimal::ZERO {
            return Err(ConfigError::Message(
                "circuit_breakers.max_loss_24h_usd must be > 0".into(),
            ));
        }

        // Validate consecutive losses
        if self.circuit_breakers.max_consecutive_losses == 0 {
            return Err(ConfigError::Message(
                "circuit_breakers.max_consecutive_losses must be > 0".into(),
            ));
        }

        // Validate portfolio stop loss is negative
        if self.circuit_breakers.portfolio_stop_loss_percent >= Decimal::ZERO {
            return Err(ConfigError::Message(
                "circuit_breakers.portfolio_stop_loss_percent must be negative".into(),
            ));
        }

        // Validate database connection pool bounds
        if self.database.max_connections < 2 || self.database.max_connections > 100 {
            return Err(ConfigError::Message(
                "database.max_connections must be between 2 and 100".into(),
            ));
        }

        // Validate queue capacity
        if self.queue.capacity == 0 {
            return Err(ConfigError::Message("queue.capacity must be > 0".into()));
        }

        // Validate admin wallet addresses are valid Solana public keys
        for wallet in &self.security.admin_wallets {
            use std::str::FromStr;
            solana_sdk::pubkey::Pubkey::from_str(&wallet.address).map_err(|e| {
                ConfigError::Message(format!(
                    "Invalid admin wallet address '{}': {}",
                    wallet.address, e
                ))
            })?;
        }

        // FIX 6: Validate kelly_fraction bounds
        if self.position_sizing.kelly_fraction <= Decimal::ZERO
            || self.position_sizing.kelly_fraction > Decimal::ONE
        {
            return Err(ConfigError::Message(format!(
                "position_sizing.kelly_fraction must be in range (0, 1], got {}",
                self.position_sizing.kelly_fraction
            )));
        }

        // FIX 7: Validate profit_management bounds
        if self.profit_management.tiered_exit_percent <= Decimal::ZERO
            || self.profit_management.tiered_exit_percent > Decimal::from(100)
        {
            return Err(ConfigError::Message(format!(
                "profit_management.tiered_exit_percent must be in range (0, 100], got {}",
                self.profit_management.tiered_exit_percent
            )));
        }
        if self.profit_management.trailing_stop_distance <= Decimal::ZERO {
            return Err(ConfigError::Message(format!(
                "profit_management.trailing_stop_distance must be > 0, got {}",
                self.profit_management.trailing_stop_distance
            )));
        }
        if self.profit_management.trailing_stop_activation <= Decimal::ZERO {
            return Err(ConfigError::Message(format!(
                "profit_management.trailing_stop_activation must be > 0, got {}",
                self.profit_management.trailing_stop_activation
            )));
        }
        if self.profit_management.max_stop_loss_distance >= Decimal::ZERO {
            return Err(ConfigError::Message(format!(
                "profit_management.max_stop_loss_distance must be < 0 (negative percentage), got {}",
                self.profit_management.max_stop_loss_distance
            )));
        }

        // Validate webhook URL format if monitoring is enabled
        if let Some(ref monitoring_config) = self.monitoring {
            if monitoring_config.enabled {
                // If monitoring is enabled, validate required API key
                if monitoring_config
                    .helius_api_key
                    .as_ref()
                    .map(|k| k.is_empty())
                    .unwrap_or(true)
                {
                    return Err(ConfigError::Message(
                        "Monitoring is enabled but helius_api_key is not set or empty".to_string(),
                    ));
                }

                if let Some(ref webhook_url) = monitoring_config.helius_webhook_url {
                    if !webhook_url.is_empty() {
                        // Validate URL format
                        if !webhook_url.starts_with("http://")
                            && !webhook_url.starts_with("https://")
                        {
                            return Err(ConfigError::Message(format!(
                                "Monitoring webhook URL must start with http:// or https://, got: {}",
                                webhook_url
                            )));
                        }
                        // Basic URL format validation
                        if !webhook_url.contains("://") || webhook_url.len() < 10 {
                            return Err(ConfigError::Message(format!(
                                "Monitoring webhook URL format is invalid: {}",
                                webhook_url
                            )));
                        }
                    }
                }
            }
        }

        // Validate telegram notification configuration
        if self.notifications.telegram.enabled {
            if self.notifications.telegram.bot_token.is_empty() {
                return Err(ConfigError::Message(
                    "Telegram notifications are enabled but bot_token is not set (set TELEGRAM_BOT_TOKEN)".to_string(),
                ));
            }
            if self.notifications.telegram.chat_id.is_empty() {
                return Err(ConfigError::Message(
                    "Telegram notifications are enabled but chat_id is not set (set TELEGRAM_CHAT_ID)".to_string(),
                ));
            }
        }

        // Jupiter API key is mandatory in Live mode (keyless access is being
        // phased out; legacy rate limits expire 2026-06-30). Paper/Devnet may run
        // without it but will be rate-limited.
        if self.trade_mode == TradeMode::Live {
            let key_missing = self
                .jupiter
                .api_key
                .as_ref()
                .map(|k| k.trim().is_empty())
                .unwrap_or(true);
            if key_missing {
                return Err(ConfigError::Message(
                    "jupiter.api_key is required in Live trade mode (set CHIMERA_JUPITER__API_KEY). \
                     Jupiter keyless access is deprecated."
                        .to_string(),
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        // Just test that defaults compile correctly
        assert_eq!(default_port(), 8080);
        assert_eq!(default_max_timestamp_drift(), 60);
        assert_eq!(default_queue_capacity(), 1000);
    }
}
