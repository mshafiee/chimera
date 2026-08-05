//! Configuration management for Chimera Operator
//!
//! Loads configuration from YAML files and environment variables.
//! Environment variables override YAML values.

use config::{Config, ConfigError, Environment, File};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
/// Trade execution mode.
///
/// **Safety:** the default is `Paper`. A trading bot must never silently fall into
/// real-money (`Live`) mode from a missing/incomplete config — that must be an
/// explicit opt-in via `CHIMERA_TRADE_MODE=live` or `trade_mode: live` in the
/// config file. Previously this defaulted to `Live`, so any deploy that forgot to
/// set the trade mode would trade real SOL.
#[serde(rename_all = "lowercase")]
pub enum TradeMode {
    Devnet,
    #[default]
    Paper,
    Live,
}

/// Resolve trade mode from an optional explicit override, config value, and RPC URL.
///
/// Rules, in order:
/// 1. `explicit` is `Some` → return it (user chose explicitly via env).
/// 2. `rpc_url` contains `"devnet"` → `Devnet` (auto-detect with log), unless
///    the config explicitly requested `Live` (a production deploy pointing its
///    RPC at a devnet endpoint must not silently flip to Devnet).
/// 3. `config_mode` is non-default (not `Live`) → return it (set in YAML config).
/// 4. else `Live`.
///
/// Note: devnet auto-detection is intentionally skipped when `config_mode` is
/// `Paper` — the Paper default must never be overridden by a devnet-looking RPC
/// URL, and a fresh deploy pointing at devnet should set
/// `trade_mode: devnet` explicitly.
pub fn resolve_trade_mode(
    explicit: Option<TradeMode>,
    config_mode: TradeMode,
    rpc_url: &str,
) -> TradeMode {
    if let Some(mode) = explicit {
        return mode;
    }
    if config_mode == TradeMode::Live && rpc_url.contains("devnet") {
        tracing::info!(rpc_url = %rpc_url, "Auto-detected devnet RPC URL → TradeMode::Devnet");
        return TradeMode::Devnet;
    }
    if config_mode != TradeMode::Live {
        return config_mode;
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
#[derive(Default)]
pub struct AppConfig {
    /// Server configuration
    #[serde(default)]
    pub server: ServerConfig,
    /// RPC endpoint configuration
    #[serde(default)]
    pub rpc: RpcConfig,
    /// Database configuration
    #[serde(default)]
    pub database: DatabaseConfig,
    /// Security settings
    #[serde(default)]
    pub security: SecurityConfig,
    /// Circuit breaker thresholds
    #[serde(default)]
    pub circuit_breakers: CircuitBreakerConfig,
    /// Strategy allocation
    #[serde(default)]
    pub strategy: StrategyConfig,
    /// Jito tip configuration
    #[serde(default)]
    pub jito: JitoConfig,
    /// Trade mode: devnet, paper, or live
    #[serde(default)]
    pub trade_mode: TradeMode,
    /// Jupiter API configuration
    #[serde(default)]
    pub jupiter: JupiterConfig,
    /// Queue configuration
    #[serde(default)]
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
    /// Execution lock configuration for idempotency
    #[serde(default)]
    pub execution_lock: crate::engine::ExecutionLockConfig,
    /// Forward test experiment configuration
    #[serde(default)]
    pub experiment: ExperimentConfig,
    /// Profitability gate configuration for live trading enforcement
    #[serde(default)]
    pub profitability_gate: ProfitabilityGateConfig,
    /// Rejection-rate wallet mute configuration
    #[serde(default)]
    pub rejection_mute: RejectionMuteConfig,
    /// Token shadow blacklist configuration
    #[serde(default)]
    pub shadow_blacklist: ShadowBlacklistConfig,
    /// Dune Analytics integration configuration
    #[serde(default)]
    pub dune: DuneConfig,
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
    /// Number of worker threads for the Tokio runtime (must be > 0; 0 is
    /// rejected by `validate()` — there is no auto-detect handling)
    #[serde(default = "default_worker_threads")]
    pub worker_threads: usize,
    /// Request timeout in milliseconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_ms: u64,
}

/// Manual Default delegating to the serde default fns so `ServerConfig::default()`
/// and a deserialized config are equivalent.
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            worker_threads: default_worker_threads(),
            request_timeout_ms: default_request_timeout(),
        }
    }
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

/// RPC rate limiting configuration for request-weighted limiting
#[derive(Debug, Clone, Deserialize)]
pub struct RpcRateLimitConfig {
    /// Enable request-weighted limiting (default: true)
    /// When true, different RPC methods consume different amounts of rate limit capacity
    /// based on their actual cost and latency (e.g., getTransaction weight 5, getLatestBlockhash weight 1)
    #[serde(default = "default_weighted_limiting_enabled")]
    pub weighted_limiting_enabled: bool,

    /// Maximum weighted requests per second (default: inherits rate_limit_per_second)
    /// This represents the maximum credits per second available for RPC calls
    #[serde(default = "default_max_weighted_rate")]
    pub max_weighted_rate: Option<u32>,

    /// Priority-based wait reduction (default: true)
    /// When true, higher priority requests (Exit/Entry) get reduced wait times
    #[serde(default = "default_priority_wait_reduction")]
    pub priority_wait_reduction: bool,
}

fn default_weighted_limiting_enabled() -> bool {
    true
}

fn default_max_weighted_rate() -> Option<u32> {
    None // Inherits rate_limit_per_second by default
}

fn default_priority_wait_reduction() -> bool {
    true
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
    /// Request-weighted rate limiting configuration
    #[serde(default)]
    pub rate_limit_config: Option<RpcRateLimitConfig>,
}

fn default_primary_provider() -> String {
    "helius".to_string()
}

/// Manual Default delegating to the serde default fns.
impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            primary_provider: default_primary_provider(),
            primary_url: default_primary_url(),
            fallback_url: None,
            rate_limit_per_second: default_rate_limit(),
            timeout_ms: default_rpc_timeout(),
            max_consecutive_failures: default_max_failures(),
            functional_health_check: default_functional_health_check(),
            rate_limit_config: None,
        }
    }
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
    /// Path component retained for legacy config compatibility (PostgreSQL
    /// backend connects via `url`); ignored in production.
    #[serde(default = "default_db_path")]
    pub path: PathBuf,
    /// PostgreSQL connection URL (for production)
    #[serde(default)]
    pub url: Option<String>,
    /// Maximum connections in pool
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

fn default_db_path() -> PathBuf {
    PathBuf::from("data/chimera.db")
}

/// Manual Default delegating to the serde default fns.
impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: default_db_path(),
            url: None,
            max_connections: default_max_connections(),
        }
    }
}

fn default_max_connections() -> u32 {
    5
}

/// Security configuration
#[derive(Clone, Deserialize)]
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

/// Manual Default delegating to the serde default fns.
impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            webhook_secret: String::new(),
            webhook_secret_previous: None,
            max_timestamp_drift_secs: default_max_timestamp_drift(),
            webhook_rate_limit: default_webhook_rate_limit(),
            webhook_burst_size: default_webhook_burst(),
            api_keys: Vec::new(),
            admin_wallets: Vec::new(),
        }
    }
}

/// Redacting Debug: `webhook_secret` must never leak through `{:?}`/tracing
/// prints of `AppConfig` (mirrors `ApiKeyConfig`/`JupiterConfig`).
impl std::fmt::Debug for SecurityConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityConfig")
            .field("webhook_secret", &"[REDACTED]")
            .field(
                "webhook_secret_previous",
                &self
                    .webhook_secret_previous
                    .as_ref()
                    .map(|_| "[REDACTED]")
                    .unwrap_or("None"),
            )
            .field("max_timestamp_drift_secs", &self.max_timestamp_drift_secs)
            .field("webhook_rate_limit", &self.webhook_rate_limit)
            .field("webhook_burst_size", &self.webhook_burst_size)
            .field("api_keys", &self.api_keys)
            .field("admin_wallets", &self.admin_wallets)
            .finish()
    }
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

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            max_loss_24h_usd: default_max_loss(),
            max_consecutive_losses: default_max_consecutive_losses(),
            max_drawdown_percent: default_max_drawdown(),
            portfolio_stop_loss_percent: default_portfolio_stop_loss_percent(),
            cooldown_minutes: default_cooldown(),
            max_jupiter_failures: default_max_jupiter_failures(),
        }
    }
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
    /// Enable dynamic friction gating: reject trades where expected edge (from Kelly sizing)
    /// is less than or equal to total transaction friction (tip + fee + slippage)
    #[serde(default = "default_friction_gating_enabled")]
    pub friction_gating_enabled: bool,
    /// When true (default), copy wallet SELL signals to close positions immediately.
    /// When false, ignore wallet SELLs — positions are managed solely by profit targets,
    /// stop-loss, momentum exit, and time exit. This transforms the system from
    /// copy-trading (follow both BUY and SELL) to signal-trading (use wallet BUYs
    /// as entry signals only).
    #[serde(default = "default_copy_wallet_sells")]
    pub copy_wallet_sells: bool,
}

/// Profitability gate configuration for live trading enforcement.
#[derive(Debug, Clone, Deserialize)]
pub struct ProfitabilityGateConfig {
    /// Enable profitability gating (fail-open by default for safety)
    #[serde(default = "default_profitability_gate_enabled")]
    pub enabled: bool,
    /// Refresh interval in seconds (default 300s / 5 minutes)
    #[serde(default = "default_profitability_gate_refresh_interval")]
    pub refresh_interval_seconds: u64,
    /// Scale factor for INCONCLUSIVE verdicts (default 0.5 = 50% of original size)
    #[serde(default = "default_profitability_gate_inconclusive_factor")]
    pub inconclusive_size_factor: f64,
}

/// Manual Default delegating to the serde default fns (derive(Default) would
/// yield refresh_interval_seconds=0 / inconclusive_size_factor=0.0).
impl Default for ProfitabilityGateConfig {
    fn default() -> Self {
        Self {
            enabled: default_profitability_gate_enabled(),
            refresh_interval_seconds: default_profitability_gate_refresh_interval(),
            inconclusive_size_factor: default_profitability_gate_inconclusive_factor(),
        }
    }
}

fn default_profitability_gate_enabled() -> bool {
    false // Disabled by default to prevent accidental blocking
}

fn default_profitability_gate_refresh_interval() -> u64 {
    300
}

fn default_profitability_gate_inconclusive_factor() -> f64 {
    0.5
}

fn default_shield_percent() -> u32 {
    50
}

fn default_shield_signal_quality_threshold() -> f64 {
    0.55
}

fn default_spear_signal_quality_threshold() -> f64 {
    0.55
}

fn default_spear_percent() -> u32 {
    50
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

fn default_friction_gating_enabled() -> bool {
    true // Enabled by default to prevent unprofitable micro-trades
}

fn default_copy_wallet_sells() -> bool {
    true // Backward compatible: copy both BUY and SELL by default
}

/// Manual Default delegating to the serde default fns so `StrategyConfig::default()`
/// and a deserialized config are equivalent.
impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            shield_percent: default_shield_percent(),
            spear_percent: default_spear_percent(),
            max_position_sol: default_max_position(),
            min_position_sol: default_min_position(),
            shield_signal_quality_threshold: default_shield_signal_quality_threshold(),
            spear_signal_quality_threshold: default_spear_signal_quality_threshold(),
            dex_fee_rate: default_dex_fee_rate(),
            shield_max_total_cost_percent: default_shield_max_cost(),
            spear_max_total_cost_percent: default_spear_max_cost(),
            slippage_fallback_small_percent: default_slippage_fallback_small(),
            slippage_fallback_large_percent: default_slippage_fallback_large(),
            slippage_fallback_threshold_sol: default_slippage_fallback_threshold(),
            friction_gating_enabled: default_friction_gating_enabled(),
            copy_wallet_sells: default_copy_wallet_sells(),
        }
    }
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
    /// Minimum consecutive failures before considering fallback (increased from 3 to 10)
    #[serde(default = "default_jito_min_failures_before_fallback")]
    pub min_failures_before_fallback: u32,
    /// Whether to completely disable fallback to Standard TPU (default: false)
    #[serde(default = "default_jito_disable_fallback")]
    pub disable_fallback: bool,
    /// Maximum retry attempts for Jito-specific errors (default: 5)
    #[serde(default = "default_jito_max_retries")]
    pub max_retries: u32,
    /// Use Helius Staked Connections for exit trades (higher landing rate during congestion)
    /// Default: true for production efficiency
    #[serde(default = "default_helius_staked_exits")]
    pub helius_staked_exits: bool,
}

fn default_jito_enabled() -> bool {
    true
}

fn default_jito_searcher_endpoint() -> Option<String> {
    // Tokyo regional endpoint: the only Jito block-engine region reachable
    // from the production server network. Global (mainnet.*) and
    // ny/amsterdam/frankfurt all resolve but connection times out from the
    // server (tested 2026-08-04); tokyo responds normally.
    Some("https://tokyo.mainnet.block-engine.jito.wtf".to_string())
}

fn default_helius_fallback() -> bool {
    true
}

fn default_tip_floor() -> Decimal {
    dec!(0.0005)
}

fn default_tip_ceiling() -> Decimal {
    dec!(0.005)
}

fn default_tip_percentile() -> u32 {
    50
}

fn default_tip_percent_max() -> Decimal {
    dec!(0.02)
}

fn default_jito_min_failures_before_fallback() -> u32 {
    10
}

fn default_jito_disable_fallback() -> bool {
    false
}

fn default_jito_max_retries() -> u32 {
    5
}

fn default_helius_staked_exits() -> bool {
    true  // Enable by default for production
}

/// Manual Default delegating to the serde default fns so `JitoConfig::default()`
/// and a deserialized config are equivalent.
impl Default for JitoConfig {
    fn default() -> Self {
        Self {
            enabled: default_jito_enabled(),
            searcher_endpoint: default_jito_searcher_endpoint(),
            helius_fallback: default_helius_fallback(),
            tip_floor_sol: default_tip_floor(),
            tip_ceiling_sol: default_tip_ceiling(),
            tip_percentile: default_tip_percentile(),
            tip_percent_max: default_tip_percent_max(),
            min_failures_before_fallback: default_jito_min_failures_before_fallback(),
            disable_fallback: default_jito_disable_fallback(),
            max_retries: default_jito_max_retries(),
            helius_staked_exits: default_helius_staked_exits(),
        }
    }
}

/// Jupiter API configuration
#[derive(Clone, Deserialize)]
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
    /// Jupiter Price API base URL for fetching token prices (v3+).
    #[serde(default = "default_jupiter_price_api_url")]
    pub price_api_url: String,
    /// Deprecation deadline (ISO date) for Jupiter's legacy keyless access.
    /// Used by the skills-integration module to report migration deadlines.
    #[serde(default = "default_jupiter_deprecation_deadline")]
    pub deprecation_deadline: String,
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

/// Manual Default (NOT derive): delegates to the same serde default fns so
/// `JupiterConfig::default()` and a deserialized config are equivalent —
/// derive(Default) would yield empty api_url / disabled flags.
impl Default for JupiterConfig {
    fn default() -> Self {
        Self {
            api_url: default_jupiter_api_url(),
            api_key: None,
            reconstruct_v0_on_blockhash_expiry: default_reconstruct_v0(),
            reject_v0_transactions: default_reject_v0(),
            use_swap_v2: default_use_swap_v2(),
            multi_dex_comparison: default_multi_dex_comparison(),
            enable_rtse: default_enable_rtse(),
            exclude_routers: None,
            exclude_dexes: None,
            price_api_url: default_jupiter_price_api_url(),
            deprecation_deadline: default_jupiter_deprecation_deadline(),
        }
    }
}

fn default_jupiter_price_api_url() -> String {
    "https://api.jup.ag/price".to_string()
}

fn default_jupiter_deprecation_deadline() -> String {
    "2026-06-30".to_string()
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

/// Manual Default delegating to the serde default fns.
impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            capacity: default_queue_capacity(),
            load_shed_threshold_percent: default_load_shed_threshold(),
            parallel_enabled: default_parallel_enabled(),
            num_workers: default_num_workers(),
            max_concurrent_rpc: default_max_concurrent_rpc(),
        }
    }
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
    /// Minimum liquidity for graduated pump.fun tokens (USD).
    /// Higher than normal tokens because pump.fun tokens are inherently riskier.
    #[serde(default = "default_min_liquidity_pumpfun")]
    pub min_liquidity_pumpfun_usd: Decimal,
    /// When true, pump.fun tokens with sufficient DEX liquidity are allowed.
    /// When false, all pump.fun tokens are blanket-rejected (legacy behavior).
    #[serde(default = "default_allow_graduated_pumpfun")]
    pub allow_graduated_pumpfun: bool,
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
    /// Minimum token age in hours for pump.fun tokens.
    /// Lower than normal tokens because pump.fun tokens are inherently newer.
    /// The $25K liquidity gate (gate 6) is the primary risk filter for these.
    #[serde(default = "default_min_token_age_pumpfun_hours")]
    pub min_token_age_pumpfun_hours: f64,
    /// FIX 1: Liquidity cache TTL in seconds (default: 60)
    #[serde(default = "default_liquidity_cache_ttl")]
    pub liquidity_cache_ttl_secs: u64,
    /// FIX 1: FDV cache TTL in seconds (default: 300 / 5 minutes)
    #[serde(default = "default_fdv_cache_ttl")]
    pub fdv_cache_ttl_secs: u64,
    /// FIX 1: Background liquidity updater interval in seconds (default: 30)
    #[serde(default = "default_liquidity_update_interval")]
    pub liquidity_update_interval_secs: u64,
    /// Cache backend type: "memory" or "redis"
    #[serde(default = "default_cache_backend")]
    pub cache_backend: String,
    /// Redis connection URL (only used if cache_backend is "redis")
    /// Example: "redis://127.0.0.1:6379"
    #[serde(default)]
    pub redis_url: Option<String>,
    /// Enable holder-concentration rug check (LP-aware top-10).
    #[serde(default = "default_false")]
    pub holder_concentration_check_enabled: bool,
    /// Max top-10 non-DEX holder concentration (% of supply) before rejection.
    #[serde(default = "default_max_holder_concentration_pct")]
    pub max_holder_concentration_pct: f64,
}

fn default_allow_unlisted_heuristic() -> bool {
    false
}

fn default_min_token_age_hours() -> f64 {
    1.0
}

fn default_min_token_age_pumpfun_hours() -> f64 {
    4.0
}

fn default_liquidity_cache_ttl() -> u64 {
    60
}

fn default_fdv_cache_ttl() -> u64 {
    300
}

fn default_liquidity_update_interval() -> u64 {
    60 // Updated from 30 to 60 for unified cache updater
}

fn default_cache_backend() -> String {
    "memory".to_string()
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

fn default_min_liquidity_pumpfun() -> Decimal {
    dec!(25000.0)
}

fn default_allow_graduated_pumpfun() -> bool {
    true
}

fn default_honeypot_detection() -> bool {
    true
}

fn default_token_cache_capacity() -> usize {
    1000
}

pub fn default_token_cache_ttl() -> i64 {
    86400 // 24 hours (immutable token metadata)
}

fn default_max_holder_concentration_pct() -> f64 {
    25.0
}

impl Default for TokenSafetyConfig {
    fn default() -> Self {
        Self {
            freeze_authority_whitelist: default_authority_whitelist(),
            mint_authority_whitelist: default_authority_whitelist(),
            min_liquidity_shield_usd: default_min_liquidity_shield(),
            min_liquidity_spear_usd: default_min_liquidity_spear(),
            min_liquidity_pumpfun_usd: default_min_liquidity_pumpfun(),
            allow_graduated_pumpfun: default_allow_graduated_pumpfun(),
            honeypot_detection_enabled: default_honeypot_detection(),
            cache_capacity: default_token_cache_capacity(),
            cache_ttl_seconds: default_token_cache_ttl(),
            allow_unlisted_heuristic: default_allow_unlisted_heuristic(),
            min_token_age_hours: default_min_token_age_hours(),
            min_token_age_pumpfun_hours: default_min_token_age_pumpfun_hours(),
            liquidity_cache_ttl_secs: default_liquidity_cache_ttl(),
            fdv_cache_ttl_secs: default_fdv_cache_ttl(),
            liquidity_update_interval_secs: default_liquidity_update_interval(),
            cache_backend: default_cache_backend(),
            redis_url: None,
            holder_concentration_check_enabled: default_false(),
            max_holder_concentration_pct: default_max_holder_concentration_pct(),
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
#[derive(Clone, Deserialize)]
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

/// Redacting Debug: `bot_token` must never leak through `{:?}`/tracing prints.
impl std::fmt::Debug for TelegramNotificationConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramNotificationConfig")
            .field("enabled", &self.enabled)
            .field("bot_token", &"[REDACTED]")
            .field("chat_id", &self.chat_id)
            .field("rate_limit_seconds", &self.rate_limit_seconds)
            .finish()
    }
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

/// Wallet conviction tier for dynamic polling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConvictionTier {
    High,     // WQS > 80
    Regular,  // WQS 60-80
    Emerging, // WQS < 60 or CANDIDATE status
}

/// Dynamic polling configuration per conviction tier
#[derive(Debug, Clone, Deserialize)]
pub struct TieredPollingConfig {
    /// High conviction wallets (WQS > 80): poll every N seconds
    #[serde(default = "default_high_conviction_interval")]
    pub high_conviction_interval_secs: u64,

    /// Regular conviction wallets (WQS 60-80): poll every N seconds
    #[serde(default = "default_regular_conviction_interval")]
    pub regular_conviction_interval_secs: u64,

    /// Emerging conviction wallets (WQS < 60 or CANDIDATE): poll every N seconds
    #[serde(default = "default_emerging_conviction_interval")]
    pub emerging_conviction_interval_secs: u64,

    /// WQS threshold for high conviction
    #[serde(default = "default_high_conviction_threshold")]
    pub high_conviction_wqs_threshold: i32,

    /// WQS threshold for regular conviction
    #[serde(default = "default_regular_conviction_threshold")]
    pub regular_conviction_wqs_threshold: i32,
}

impl Default for TieredPollingConfig {
    fn default() -> Self {
        Self {
            high_conviction_interval_secs: default_high_conviction_interval(),
            regular_conviction_interval_secs: default_regular_conviction_interval(),
            emerging_conviction_interval_secs: default_emerging_conviction_interval(),
            high_conviction_wqs_threshold: default_high_conviction_threshold(),
            regular_conviction_wqs_threshold: default_regular_conviction_threshold(),
        }
    }
}

fn default_high_conviction_interval() -> u64 {
    5
}

fn default_regular_conviction_interval() -> u64 {
    8
}

fn default_emerging_conviction_interval() -> u64 {
    30
}

fn default_high_conviction_threshold() -> i32 {
    80
}

fn default_regular_conviction_threshold() -> i32 {
    60
}

/// Inactivity rotation configuration for wallet demotion
#[derive(Debug, Clone, Deserialize)]
pub struct InactivityRotationConfig {
    /// Inactivity threshold for high conviction wallets (WQS > 80) in seconds
    #[serde(default = "default_inactivity_high_conviction_threshold")]
    pub high_conviction_threshold_secs: u64,
    /// Inactivity threshold for regular conviction wallets (WQS 60-80) in seconds
    #[serde(default = "default_inactivity_regular_conviction_threshold")]
    pub regular_conviction_threshold_secs: u64,
    /// Inactivity threshold for low conviction wallets (WQS < 60) in seconds
    #[serde(default = "default_inactivity_low_conviction_threshold")]
    pub low_conviction_threshold_secs: u64,
    /// WQS threshold for high conviction
    #[serde(default = "default_inactivity_high_conviction_wqs_threshold")]
    pub high_conviction_wqs_threshold: f64,
    /// WQS threshold for regular conviction
    #[serde(default = "default_inactivity_regular_conviction_wqs_threshold")]
    pub regular_conviction_wqs_threshold: f64,
    /// Maximum oscillation cycles before escalating to REJECTED
    #[serde(default = "default_inactivity_max_oscillation_cycles")]
    pub max_oscillation_cycles: u32,
}

impl Default for InactivityRotationConfig {
    fn default() -> Self {
        Self {
            high_conviction_threshold_secs: default_inactivity_high_conviction_threshold(),
            regular_conviction_threshold_secs: default_inactivity_regular_conviction_threshold(),
            low_conviction_threshold_secs: default_inactivity_low_conviction_threshold(),
            high_conviction_wqs_threshold: default_inactivity_high_conviction_wqs_threshold(),
            regular_conviction_wqs_threshold: default_inactivity_regular_conviction_wqs_threshold(),
            max_oscillation_cycles: default_inactivity_max_oscillation_cycles(),
        }
    }
}

fn default_inactivity_high_conviction_threshold() -> u64 {
    259200
}

fn default_inactivity_regular_conviction_threshold() -> u64 {
    172800
}

fn default_inactivity_low_conviction_threshold() -> u64 {
    86400
}

fn default_inactivity_high_conviction_wqs_threshold() -> f64 {
    80.0
}

fn default_inactivity_regular_conviction_wqs_threshold() -> f64 {
    60.0
}

fn default_inactivity_max_oscillation_cycles() -> u32 {
    3
}

/// Monitoring configuration
#[derive(Clone, Deserialize)]
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
    /// RPC poll interval in seconds (legacy, used if tiered polling not enabled)
    #[serde(default = "default_rpc_poll_interval")]
    pub rpc_poll_interval_secs: u64,
    /// Enable tiered polling based on wallet conviction level
    #[serde(default = "default_true")]
    pub tiered_polling_enabled: bool,
    /// Tiered polling configuration (optional)
    #[serde(default)]
    pub tiered_polling: Option<TieredPollingConfig>,
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
    /// Enable inactivity-based wallet demotion (rotates dormant/stablecoin-only ACTIVE wallets).
    #[serde(default = "default_false")]
    pub inactivity_rotation_enabled: bool,
    /// Tiered inactivity thresholds + oscillation limit.
    #[serde(default)]
    pub inactivity_rotation: Option<InactivityRotationConfig>,
    /// Auto-promote high-WQS CANDIDATE wallets to ACTIVE when the roster is
    /// below `max_active_wallets`. Counterbalances auto-demote so the monitored
    /// roster self-replenishes instead of only draining. Default false (opt-in).
    #[serde(default = "default_false")]
    pub auto_promote_enabled: bool,
    /// Minimum WQS for auto-promotion eligibility (default 60.0 — "regular"
    /// conviction). Candidates below this are not promoted automatically.
    #[serde(default = "default_auto_promote_min_wqs")]
    pub auto_promote_min_wqs: f64,
    /// TTL (hours) applied to auto-promoted wallets (default 168 = 7 days).
    /// Combined with promoted_at; auto-demote re-evaluates them on performance.
    #[serde(default = "default_auto_promote_ttl_hours")]
    pub auto_promote_ttl_hours: i64,
    /// Max age (days) of a CANDIDATE's last on-chain trade to be eligible for
    /// auto-promotion (default 7). Surfaces wallets that actually trade rather
    /// than dormant high-historical-WQS ones. ACTIVE wallets whose last trade
    /// exceeds this are demoted to CANDIDATE by the auto-promote task, freeing
    /// slots for active candidates.
    #[serde(default = "default_auto_promote_max_age_days")]
    pub auto_promote_max_age_days: i64,
    /// Enable tiered per-wallet copy-performance sizing. Proven wallets (those
    /// passing the sample, recency, win-rate, and net-PnL gates) get a larger
    /// allocation; others stay at the floor. Default OFF (opt-in).
    #[serde(default = "default_false")]
    pub wallet_boost_enabled: bool,
    /// Minimum CLOSED copy trades in the window to be eligible for a boost.
    #[serde(default = "default_wallet_boost_min_sample")]
    pub wallet_boost_min_sample: u32,
    /// Window: consider the last N copy trades.
    #[serde(default = "default_wallet_boost_window_trades")]
    pub wallet_boost_window_trades: u32,
    /// Window: ignore copy trades older than this many days.
    #[serde(default = "default_wallet_boost_window_days")]
    pub wallet_boost_window_days: i64,
    /// Minimum net PnL (SOL) over the window to qualify.
    #[serde(default = "default_wallet_boost_min_net_sol")]
    pub wallet_boost_min_net_sol: rust_decimal::Decimal,
    /// Minimum win rate over the window (0.0–1.0).
    #[serde(default = "default_wallet_boost_min_winrate")]
    pub wallet_boost_min_winrate: f64,
    /// A wallet whose last copy trade is older than this many days loses its boost.
    #[serde(default = "default_wallet_boost_recency_days")]
    pub wallet_boost_recency_days: i64,
    /// The BOOSTED target size (SOL). Hard cap; the floor still applies below it.
    #[serde(default = "default_wallet_boost_size_sol")]
    pub wallet_boost_size_sol: rust_decimal::Decimal,
    /// Webhook lifecycle management configuration
    #[serde(default)]
    pub webhook_lifecycle: Option<WebhookLifecycleConfig>,
    /// Enable Helius LaserStream WebSocket (experimental)
    #[serde(default = "default_false")]
    pub use_websocket: bool,
    /// Helius WebSocket URL (wss://)
    #[serde(default)]
    pub helius_websocket_url: Option<String>,
    /// WebSocket reconnection configuration
    #[serde(default)]
    pub websocket_reconnect: Option<WebSocketReconnectConfig>,
    /// Health check timeout (seconds)
    #[serde(default = "default_websocket_health_timeout")]
    pub websocket_health_timeout_secs: u64,
    /// Commitment level for WebSocket subscriptions (processed, confirmed, finalized)
    #[serde(default = "default_websocket_commitment")]
    pub websocket_commitment: String,
    /// Shared secret sent in the `authHeader` field of every Helius webhook
    /// registration/update. Helius echoes it back in the `Authorization`
    /// header on each delivery. When `None`, no auth header is set and the
    /// receipt handler accepts all events (legacy behaviour).
    #[serde(default)]
    pub helius_webhook_auth_header: Option<String>,
    /// Enforce mode for Helius webhook auth header.
    /// `true` (default) = reject non-matching requests with HTTP 401.
    /// `false` = dry-run / fail-open: log `auth_ok` / `auth_mismatch`
    /// but always accept.
    #[serde(default = "default_true")]
    pub helius_auth_enforce: bool,
    /// Enforce mode for RPC signature verification.
    /// `true` (default) = drop events whose on-chain deltas do not match the
    /// webhook claim.
    /// `false` = dry-run / fail-open: log `rpc_verify_ok` /
    /// `rpc_verify_failed` but always accept.
    #[serde(default = "default_true")]
    pub rpc_verify_enforce: bool,
    /// Minutes threshold for the stale trade reaper. PENDING/QUEUED trades
    /// older than this value are automatically cancelled. Set to 0 to disable.
    #[serde(default = "default_stale_trade_max_age")]
    pub stale_trade_reaper_minutes: i32,
}

impl MonitoringConfig {
    /// Resolve the Helius webhook auth header, expanding `${VAR}` placeholders
    /// from the environment. Returns `None` when unset or empty.
    pub fn resolved_helius_auth_header(&self) -> Option<String> {
        self.helius_webhook_auth_header
            .as_deref()
            .map(|h| {
                if h.starts_with("${") {
                    std::env::var("HELIUS_WEBHOOK_AUTH").unwrap_or_default()
                } else {
                    h.to_string()
                }
            })
            .filter(|h| !h.is_empty())
    }
}

/// WebSocket reconnection configuration
#[derive(Debug, Clone, Deserialize)]
pub struct WebSocketReconnectConfig {
    /// Initial backoff in seconds
    #[serde(default = "default_ws_initial_backoff")]
    pub initial_backoff_secs: u64,
    /// Maximum backoff in seconds
    #[serde(default = "default_ws_max_backoff")]
    pub max_backoff_secs: u64,
    /// Backoff multiplier
    #[serde(default = "default_ws_backoff_multiplier")]
    pub backoff_multiplier: f64,
    /// Maximum retry attempts (0 = infinite)
    #[serde(default = "default_ws_max_attempts")]
    pub max_attempts: u32,
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
    /// Dry-run mode for reconciliation - log only, don't delete (default: true)
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

fn default_helius_dry_run() -> bool {
    true
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_stale_trade_max_age() -> i32 {
    30
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

fn default_auto_promote_min_wqs() -> f64 {
    60.0
}

fn default_auto_promote_ttl_hours() -> i64 {
    168
}

fn default_auto_promote_max_age_days() -> i64 {
    7
}

fn default_wallet_boost_min_sample() -> u32 {
    15
}

fn default_wallet_boost_window_trades() -> u32 {
    20
}

fn default_wallet_boost_window_days() -> i64 {
    30
}

fn default_wallet_boost_min_net_sol() -> rust_decimal::Decimal {
    rust_decimal::Decimal::new(1, 2) // 0.01
}

fn default_wallet_boost_min_winrate() -> f64 {
    0.40
}

fn default_wallet_boost_recency_days() -> i64 {
    7
}

fn default_wallet_boost_size_sol() -> rust_decimal::Decimal {
    rust_decimal::Decimal::new(50, 2) // 0.50
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        // Keep in sync with the serde default fns (enabled, rate limits).
        Self {
            enabled: default_true(),
            helius_api_key: None,
            helius_webhook_url: None,
            webhook_registration_batch_size: default_webhook_batch_size(),
            webhook_registration_delay_ms: default_webhook_delay(),
            webhook_processing_rate_limit: default_monitoring_webhook_rate_limit(),
            rpc_polling_enabled: true,
            rpc_poll_interval_secs: default_rpc_poll_interval(),
            tiered_polling_enabled: true,
            tiered_polling: None,
            rpc_poll_batch_size: default_rpc_poll_batch(),
            rpc_poll_rate_limit: default_rpc_poll_rate_limit(),
            exit_detection_delay_secs: default_exit_detection_delay(),
            max_active_wallets: default_max_active_wallets(),
            auto_demote_wallets: default_auto_demote_wallets(),
            inactivity_rotation_enabled: default_false(),
            inactivity_rotation: None,
            auto_promote_enabled: default_false(),
            auto_promote_min_wqs: default_auto_promote_min_wqs(),
            auto_promote_ttl_hours: default_auto_promote_ttl_hours(),
            auto_promote_max_age_days: default_auto_promote_max_age_days(),
            wallet_boost_enabled: default_false(),
            wallet_boost_min_sample: default_wallet_boost_min_sample(),
            wallet_boost_window_trades: default_wallet_boost_window_trades(),
            wallet_boost_window_days: default_wallet_boost_window_days(),
            wallet_boost_min_net_sol: default_wallet_boost_min_net_sol(),
            wallet_boost_min_winrate: default_wallet_boost_min_winrate(),
            wallet_boost_recency_days: default_wallet_boost_recency_days(),
            wallet_boost_size_sol: default_wallet_boost_size_sol(),
            webhook_lifecycle: None,
            use_websocket: false,
            helius_websocket_url: None,
            websocket_reconnect: None,
            websocket_health_timeout_secs: default_websocket_health_timeout(),
            websocket_commitment: default_websocket_commitment(),
            helius_webhook_auth_header: None,
            helius_auth_enforce: true,
            rpc_verify_enforce: true,
            stale_trade_reaper_minutes: default_stale_trade_max_age(),
        }
    }
}

/// Redacting Debug: `helius_api_key` and `helius_webhook_auth_header` must
/// never leak through `{:?}`/tracing prints of `AppConfig`.
impl std::fmt::Debug for MonitoringConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonitoringConfig")
            .field("enabled", &self.enabled)
            .field(
                "helius_api_key",
                &self
                    .helius_api_key
                    .as_ref()
                    .map(|_| "[REDACTED]")
                    .unwrap_or("None"),
            )
            .field("helius_webhook_url", &self.helius_webhook_url)
            .field(
                "webhook_registration_batch_size",
                &self.webhook_registration_batch_size,
            )
            .field(
                "webhook_registration_delay_ms",
                &self.webhook_registration_delay_ms,
            )
            .field(
                "webhook_processing_rate_limit",
                &self.webhook_processing_rate_limit,
            )
            .field("rpc_polling_enabled", &self.rpc_polling_enabled)
            .field("rpc_poll_interval_secs", &self.rpc_poll_interval_secs)
            .field("tiered_polling_enabled", &self.tiered_polling_enabled)
            .field("tiered_polling", &self.tiered_polling)
            .field("rpc_poll_batch_size", &self.rpc_poll_batch_size)
            .field("rpc_poll_rate_limit", &self.rpc_poll_rate_limit)
            .field(
                "exit_detection_delay_secs",
                &self.exit_detection_delay_secs,
            )
            .field("max_active_wallets", &self.max_active_wallets)
            .field("auto_demote_wallets", &self.auto_demote_wallets)
            .field(
                "inactivity_rotation_enabled",
                &self.inactivity_rotation_enabled,
            )
            .field("inactivity_rotation", &self.inactivity_rotation)
            .field("webhook_lifecycle", &self.webhook_lifecycle)
            .field("use_websocket", &self.use_websocket)
            .field("helius_websocket_url", &self.helius_websocket_url)
            .field("websocket_reconnect", &self.websocket_reconnect)
            .field(
                "websocket_health_timeout_secs",
                &self.websocket_health_timeout_secs,
            )
            .field("websocket_commitment", &self.websocket_commitment)
            .field(
                "helius_webhook_auth_header",
                &self
                    .helius_webhook_auth_header
                    .as_ref()
                    .map(|_| "[REDACTED]")
                    .unwrap_or("None"),
            )
            .field("helius_auth_enforce", &self.helius_auth_enforce)
            .field("rpc_verify_enforce", &self.rpc_verify_enforce)
            .field(
                "stale_trade_reaper_minutes",
                &self.stale_trade_reaper_minutes,
            )
            .finish()
    }
}

impl MonitoringConfig {
    /// Get effective polling interval for a wallet based on WQS score and status
    pub fn get_polling_interval_for_wallet(&self, wqs_score: Option<rust_decimal::Decimal>, status: &str) -> u64 {
        if !self.tiered_polling_enabled {
            return self.rpc_poll_interval_secs;
        }

        let (high_interval, regular_interval, emerging_interval, high_threshold, regular_threshold) = match &self.tiered_polling {
            Some(config) => (
                config.high_conviction_interval_secs,
                config.regular_conviction_interval_secs,
                config.emerging_conviction_interval_secs,
                config.high_conviction_wqs_threshold,
                config.regular_conviction_wqs_threshold,
            ),
            None => (
                default_high_conviction_interval(),
                default_regular_conviction_interval(),
                default_emerging_conviction_interval(),
                default_high_conviction_threshold(),
                default_regular_conviction_threshold(),
            ),
        };

        // CANDIDATE wallets always use emerging interval
        if status == "CANDIDATE" {
            return emerging_interval;
        }

        // Convert WQS Decimal to integer threshold comparison
        let wqs = wqs_score
            .map(|d| {
                // Round the decimal to nearest integer
                d.round().to_u32().unwrap_or(0) as i32
            })
            .unwrap_or(0);

        if wqs >= high_threshold {
            high_interval
        } else if wqs >= regular_threshold {
            regular_interval
        } else {
            emerging_interval
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
    /// Volatility threshold above which targets are used at full strength (%).
    /// Tokens with volatility below this get proportionally scaled-down targets.
    /// Example: threshold=30.0, token vol=15.0 → scale=0.5 → target[0] halved.
    #[serde(default = "default_target_vol_scale_threshold")]
    pub target_vol_scale_threshold: Decimal,
    /// Floor for scaled profit targets (%). Prevents targets from dropping
    /// below break-even (~4% round-trip cost). Must be less than targets[0].
    #[serde(default = "default_min_target_pct")]
    pub min_target_pct: Decimal,
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
    /// Large-loss override for wick protection: if an open position's loss
    /// reaches this percentage during the wick-protection grace period, exit
    /// anyway (a sustained −X% drop is a genuine dump, not an entry wick).
    /// Default −10.0. Only the −25% hard stop bypassed wick protection before,
    /// which let fast pump.fun dumps ride to −12%…−14% in the first 60s.
    #[serde(default = "default_wick_protection_max_loss_percent")]
    pub wick_protection_max_loss_percent: Decimal,
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
    /// Recovery gate: seconds after entry to check if the position has recovered
    /// from its initial dip. Winners typically recover above -1% within 48s;
    /// losers stay below -2.5%. Default: 90 (wick_protection 60 + 30s buffer).
    #[serde(default = "default_recovery_gate_secs")]
    pub recovery_gate_secs: u64,
    /// Recovery gate threshold: exit if unrealized PnL is below this percent
    /// after recovery_gate_secs. Default -2.5 (exit if still below -2.5%).
    #[serde(default = "default_recovery_gate_threshold")]
    pub recovery_gate_threshold: Decimal,
    /// Per-wallet exit profiles: derive per-wallet time-exit and trailing-stop
    /// parameters from the wallet's on-chain round-trip behavior (hold
    /// duration, win/loss size), blended with these global defaults via
    /// Bayesian shrinkage. Loss-side params (stop loss, recovery gate, wick
    /// protection) always stay global — they are safety rails.
    #[serde(default)]
    pub exit_profiles: ExitProfileConfig,
}

/// Per-wallet exit profile configuration (Bayesian shrinkage against the
/// global `ProfitManagementConfig`).
#[derive(Debug, Clone, Deserialize)]
pub struct ExitProfileConfig {
    /// Master switch.
    #[serde(default = "default_exit_profiles_enabled")]
    pub enabled: bool,
    /// Wallets with fewer round-trip samples than this keep the global
    /// params unchanged (no shrinkage applied).
    #[serde(default = "default_exit_profiles_min_samples")]
    pub min_samples: usize,
    /// Shrinkage constant K: weight = samples / (samples + K). At K=25 a
    /// wallet with 25 samples is 50% trusted; 200 samples -> 89%.
    #[serde(default = "default_exit_profiles_shrinkage_k")]
    pub shrinkage_k: f64,
    /// Reference hold time (hours) for the hold multiplier: a wallet whose
    /// median round-trip hold equals this maps to multiplier 1.0.
    #[serde(default = "default_exit_profiles_reference_hold_hours")]
    pub reference_hold_hours: f64,
    /// Clamp for the hold multiplier (0.25 = 4x faster exits, 4.0 = 4x longer).
    #[serde(default = "default_exit_profiles_hold_mult_min")]
    pub hold_mult_min: f64,
    #[serde(default = "default_exit_profiles_hold_mult_max")]
    pub hold_mult_max: f64,
    /// Clamp for per-wallet trailing distance (percent).
    #[serde(default = "default_exit_profiles_trailing_min_pct")]
    pub trailing_min_distance_pct: f64,
    #[serde(default = "default_exit_profiles_trailing_max_pct")]
    pub trailing_max_distance_pct: f64,
    /// Clamp for per-wallet trailing activation (percent).
    #[serde(default = "default_exit_profiles_activation_min_pct")]
    pub trailing_min_activation_pct: f64,
    #[serde(default = "default_exit_profiles_activation_max_pct")]
    pub trailing_max_activation_pct: f64,
    /// Floor below which the exit-profile cache never refreshes (seconds).
    #[serde(default = "default_exit_profiles_refresh_secs")]
    pub refresh_secs: u64,
}

fn default_exit_profiles_enabled() -> bool {
    true
}
fn default_exit_profiles_min_samples() -> usize {
    5
}
fn default_exit_profiles_shrinkage_k() -> f64 {
    25.0
}
fn default_exit_profiles_reference_hold_hours() -> f64 {
    12.0
}
fn default_exit_profiles_hold_mult_min() -> f64 {
    0.25
}
fn default_exit_profiles_hold_mult_max() -> f64 {
    4.0
}
fn default_exit_profiles_trailing_min_pct() -> f64 {
    3.0
}
fn default_exit_profiles_trailing_max_pct() -> f64 {
    40.0
}
fn default_exit_profiles_activation_min_pct() -> f64 {
    2.0
}
fn default_exit_profiles_activation_max_pct() -> f64 {
    25.0
}
fn default_exit_profiles_refresh_secs() -> u64 {
    600
}

impl Default for ExitProfileConfig {
    fn default() -> Self {
        Self {
            enabled: default_exit_profiles_enabled(),
            min_samples: default_exit_profiles_min_samples(),
            shrinkage_k: default_exit_profiles_shrinkage_k(),
            reference_hold_hours: default_exit_profiles_reference_hold_hours(),
            hold_mult_min: default_exit_profiles_hold_mult_min(),
            hold_mult_max: default_exit_profiles_hold_mult_max(),
            trailing_min_distance_pct: default_exit_profiles_trailing_min_pct(),
            trailing_max_distance_pct: default_exit_profiles_trailing_max_pct(),
            trailing_min_activation_pct: default_exit_profiles_activation_min_pct(),
            trailing_max_activation_pct: default_exit_profiles_activation_max_pct(),
            refresh_secs: default_exit_profiles_refresh_secs(),
        }
    }
}

fn default_recovery_gate_secs() -> u64 {
    90
}

fn default_recovery_gate_threshold() -> Decimal {
    dec!(-2.5)
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

fn default_target_vol_scale_threshold() -> Decimal {
    dec!(30.0)
}

fn default_min_target_pct() -> Decimal {
    dec!(5.0)
}

fn default_max_stop_loss_distance() -> Decimal {
    dec!(-5.0)
}

fn default_time_exit_hours() -> u64 {
    24
}

fn default_wick_protection_secs() -> u64 {
    10
}

fn default_wick_protection_max_loss_percent() -> Decimal {
    Decimal::new(-100, 1) // -10.0
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
            target_vol_scale_threshold: default_target_vol_scale_threshold(),
            min_target_pct: default_min_target_pct(),
            max_stop_loss_distance: default_max_stop_loss_distance(),
            time_exit_hours: default_time_exit_hours(),
            wick_protection_secs: default_wick_protection_secs(),
            wick_protection_max_loss_percent: default_wick_protection_max_loss_percent(),
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
            recovery_gate_secs: default_recovery_gate_secs(),
            recovery_gate_threshold: default_recovery_gate_threshold(),
            exit_profiles: ExitProfileConfig::default(),
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
    /// Minimum position size in SOL (paper trading only)
    #[serde(default = "default_min_size_sol")]
    pub min_size_sol: Decimal,
    /// Minimum position size in SOL for live trading (distinct from paper min_size_sol)
    /// Rejects trades below this threshold to avoid uneconomical execution due to fixed costs
    #[serde(default = "default_min_live_position_sol")]
    pub min_live_position_sol: Decimal,
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
    dec!(0.5)
}

fn default_max_size_sol() -> Decimal {
    dec!(2.0)
}

fn default_min_size_sol() -> Decimal {
    dec!(0.05)
}

fn default_min_live_position_sol() -> Decimal {
    dec!(0.05) // Same as min_size_sol by default (no behavior change unless configured)
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
            min_live_position_sol: default_min_live_position_sol(),
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

/// Forward test experiment configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ExperimentConfig {
    /// Enable live tracer trades alongside paper mode
    #[serde(default)]
    pub tracer_enabled: bool,
    /// Fraction of paper trades to also execute as live tracers (0.0-1.0)
    #[serde(default = "default_tracer_sample_rate")]
    pub tracer_sample_rate: f64,
    /// Maximum number of tracer trades to execute (capped to limit live capital exposure)
    #[serde(default = "default_tracer_cap")]
    pub tracer_cap: u32,
    /// Number of days to run the experiment
    #[serde(default = "default_experiment_days")]
    pub experiment_days: u32,
    /// Minimum number of trades required for verdict
    #[serde(default = "default_min_trades")]
    pub min_trades: u32,
    /// Enable control arms (random-token + SOL benchmark)
    #[serde(default = "default_true")]
    pub controls_enabled: bool,
    /// Toxic-flow detection: percentage threshold for wallet kill (default: 30%)
    #[serde(default = "default_toxic_threshold_percent")]
    pub toxic_threshold_percent: u32,
    /// Toxic-flow detection: local-top decline percentage (default: 8%)
    #[serde(default = "default_local_top_decline_pct")]
    pub local_top_decline_pct: Decimal,
    /// Enable 24h shake-down mode (no verdict, just verification)
    #[serde(default)]
    pub shakedown_mode: bool,
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

fn default_tracer_sample_rate() -> f64 {
    1.0 // 100% of paper trades initially
}

fn default_tracer_cap() -> u32 {
    60 // Maximum live tracer trades
}

fn default_experiment_days() -> u32 {
    21 // Verdict window in days
}

fn default_min_trades() -> u32 {
    50 // Minimum trades for verdict
}

fn default_toxic_threshold_percent() -> u32 {
    30 // Kill if >30% of roster toxic
}

fn default_local_top_decline_pct() -> Decimal {
    dec!(8.0) // 8% decline for local-top detection
}

impl Default for ExperimentConfig {
    fn default() -> Self {
        Self {
            tracer_enabled: false,
            tracer_sample_rate: default_tracer_sample_rate(),
            tracer_cap: default_tracer_cap(),
            experiment_days: default_experiment_days(),
            min_trades: default_min_trades(),
            controls_enabled: true,
            toxic_threshold_percent: default_toxic_threshold_percent(),
            local_top_decline_pct: default_local_top_decline_pct(),
            shakedown_mode: false,
        }
    }
}

/// ── Rejection-rate wallet mute ──────────────────────────────────────────
///
/// Mutes wallets whose BUY signals are overwhelmingly rejected for hard,
/// structural reasons (non-speculative / unsafe / illiquid pump.fun tokens).
/// Prevents wasted decision processing on wallets that can never produce an
/// actionable trade.
#[derive(Debug, Clone, Deserialize)]
pub struct RejectionMuteConfig {
    /// Master switch. When false, the mute gate and recording are no-ops.
    #[serde(default = "default_rejection_mute_enabled")]
    pub enabled: bool,
    /// Rolling window size: number of most-recent BUY decisions tracked.
    #[serde(default = "default_rejection_mute_window_size")]
    pub window_size: u32,
    /// Minimum samples in the window before a wallet can be muted
    /// (avoids muting on tiny sample sizes).
    #[serde(default = "default_rejection_mute_min_samples")]
    pub min_window_samples: u32,
    /// Hard-rejection rate threshold (0.0–1.0) that triggers a mute.
    #[serde(default = "default_rejection_mute_threshold")]
    pub hard_rejection_threshold: f64,
    /// How long (in hours) a wallet stays muted before re-evaluation.
    #[serde(default = "default_rejection_mute_duration_hours")]
    pub mute_duration_hours: u32,
}

fn default_rejection_mute_enabled() -> bool {
    true
}
fn default_rejection_mute_window_size() -> u32 {
    50
}
fn default_rejection_mute_min_samples() -> u32 {
    20
}
fn default_rejection_mute_threshold() -> f64 {
    0.90
}
fn default_rejection_mute_duration_hours() -> u32 {
    6
}

impl Default for RejectionMuteConfig {
    fn default() -> Self {
        Self {
            enabled: default_rejection_mute_enabled(),
            window_size: default_rejection_mute_window_size(),
            min_window_samples: default_rejection_mute_min_samples(),
            hard_rejection_threshold: default_rejection_mute_threshold(),
            mute_duration_hours: default_rejection_mute_duration_hours(),
        }
    }
}

/// Token shadow blacklist: rejects BUY signals for tokens whose shadow
/// performance under our own exits (mirror_main) is consistently negative
/// across enough samples. Prevents re-entering dump tokens like 6GmAFSYs4g
/// (-13% avg over 40 shadow signals) that bleed capital every entry.
#[derive(Debug, Clone, Deserialize)]
pub struct ShadowBlacklistConfig {
    /// Master switch.
    #[serde(default = "default_shadow_blacklist_enabled")]
    pub enabled: bool,
    /// Minimum shadow exits (mirror_main, any wallet) before a token can be
    /// blacklisted — protects against small-sample noise.
    #[serde(default = "default_shadow_blacklist_min_samples")]
    pub min_samples: i64,
    /// Average shadow PnL% below this threshold triggers the blacklist.
    #[serde(default = "default_shadow_blacklist_threshold_pct")]
    pub threshold_pct: f64,
    /// Cost adjustment: shadow mirror PnL ignores trading costs (jito tip +
    /// dex fee ≈ 0.72% per round trip at 0.5 SOL). Live PnL ≈ mirror − this
    /// amount, so gates calibrated to mirror PnL must add it back to the
    /// threshold: tokens with mirror avg < threshold + cost_adjustment are
    /// blacklisted (live avg ≈ threshold).
    #[serde(default)]
    pub cost_adjustment_pct: f64,
    /// Rolling window (hours) over which shadow exits are evaluated.
    #[serde(default = "default_shadow_blacklist_window_hours")]
    pub window_hours: i64,
}

fn default_shadow_blacklist_enabled() -> bool {
    true
}
fn default_shadow_blacklist_min_samples() -> i64 {
    10
}
fn default_shadow_blacklist_threshold_pct() -> f64 {
    // mirror_main exits cut losers at -2% (recovery gate), so dump tokens
    // average ~-2% to -3% — -5% would never fire. -1.5% catches them.
    -1.5
}
fn default_shadow_blacklist_window_hours() -> i64 {
    48
}

impl Default for ShadowBlacklistConfig {
    fn default() -> Self {
        Self {
            enabled: default_shadow_blacklist_enabled(),
            min_samples: default_shadow_blacklist_min_samples(),
            threshold_pct: default_shadow_blacklist_threshold_pct(),
            cost_adjustment_pct: 0.0,
            window_hours: default_shadow_blacklist_window_hours(),
        }
    }
}

/// On-chain wallet assessment: verifies a wallet's ACTUAL round-trip trading
/// on Solana (win rate + expectancy from Helius history) before it is
/// admitted to live trading. Complements shadow trading — the shadow needs
/// signals to accumulate slowly; on-chain history gives 100s of round trips
/// immediately and measures the true copy-trading edge (per-trade
/// expectancy), not the wallet's aggregate PnL.
#[derive(Debug, Clone, Deserialize)]
pub struct OnchainAssessmentConfig {
    /// Master switch.
    #[serde(default = "default_onchain_assessment_enabled")]
    pub enabled: bool,
    /// Minimum completed round trips required before a wallet qualifies.
    #[serde(default = "default_onchain_assessment_min_round_trips")]
    pub min_round_trips: usize,
    /// Minimum average round-trip PnL% (expectancy) to admit to live trading.
    #[serde(default = "default_onchain_assessment_min_expectancy_pct")]
    pub min_expectancy_pct: f64,
    /// Max SWAP transactions fetched per wallet.
    #[serde(default = "default_onchain_assessment_tx_limit")]
    pub tx_limit: usize,
    /// Retroactive audit: assess ACTIVE wallets with recent activity and
    /// demote those failing the same round-trip expectancy bar used for
    /// admission (catches wallets admitted under the old criteria).
    #[serde(default = "default_onchain_audit_actives_enabled")]
    pub audit_actives_enabled: bool,
    /// Max ACTIVE wallets audited per cycle (API cost control).
    #[serde(default = "default_onchain_audit_max_per_cycle")]
    pub audit_max_per_cycle: usize,
}

fn default_onchain_assessment_enabled() -> bool {
    true
}
fn default_onchain_assessment_min_round_trips() -> usize {
    10
}
fn default_onchain_assessment_min_expectancy_pct() -> f64 {
    0.0
}
fn default_onchain_assessment_tx_limit() -> usize {
    200
}
fn default_onchain_audit_actives_enabled() -> bool {
    true
}
fn default_onchain_audit_max_per_cycle() -> usize {
    10
}

impl Default for OnchainAssessmentConfig {
    fn default() -> Self {
        Self {
            enabled: default_onchain_assessment_enabled(),
            min_round_trips: default_onchain_assessment_min_round_trips(),
            min_expectancy_pct: default_onchain_assessment_min_expectancy_pct(),
            tx_limit: default_onchain_assessment_tx_limit(),
            audit_actives_enabled: default_onchain_audit_actives_enabled(),
            audit_max_per_cycle: default_onchain_audit_max_per_cycle(),
        }
    }
}

/// Dune Analytics integration for wallet PnL monitoring and fast demotion.
/// API key is read from the `DUNE_API_KEY` environment variable.
#[derive(Debug, Clone, Deserialize)]
pub struct DuneConfig {
    /// Master switch for Dune integration.
    #[serde(default)]
    pub enabled: bool,
    /// Dune query ID for the 24h wallet PnL monitor (losing wallets).
    #[serde(default = "default_dune_pnl_query_id")]
    pub pnl_query_id: u64,
    /// How often (in seconds) to poll Dune for wallet PnL. Default: 2 hours.
    #[serde(default = "default_dune_check_interval_secs")]
    pub check_interval_secs: u64,
    /// How often (in seconds) to run the Dune promote query + on-chain audit
    /// of ACTIVE wallets. Both are expensive (Dune + Helius API credits).
    /// Default: 6 hours.
    #[serde(default = "default_promote_check_interval_secs")]
    pub promote_check_interval_secs: u64,
    /// Dune 24h-PnL demote-losers monitor. Disabled by default — redundant
    /// with shadow-quality demote + on-chain audit, and aggregate Dune PnL
    /// masks per-signal losses. Re-enable only for investigation.
    #[serde(default)]
    pub demote_losers_enabled: bool,
    /// Promote Dune-verified profitable CANDIDATE wallets to ACTIVE.
    #[serde(default = "default_dune_promote_enabled")]
    pub promote_enabled: bool,
    /// Dune query ID for the top profitable traders query.
    #[serde(default = "default_dune_promote_query_id")]
    pub promote_query_id: u64,
    /// Minimum ROI (net PnL / buy volume) for a wallet to be promoted.
    #[serde(default = "default_dune_promote_min_roi")]
    pub promote_min_roi: f64,
    /// Max wallets promoted per cycle (avoids flooding webhook registration).
    #[serde(default = "default_dune_promote_max_per_cycle")]
    pub promote_max_per_cycle: u32,
    /// Skip promotion when ACTIVE wallet count is at or above this cap.
    #[serde(default = "default_dune_promote_max_active_total")]
    pub promote_max_active_total: u32,
    /// Wallets demoted within this many hours are ineligible for Dune
    /// promotion. Prevents the churn loop where shadow quality demotes a
    /// wallet on recent 48h signal performance and Dune re-promotes it
    /// minutes later on historical 7d PnL. Default: 24h.
    #[serde(default = "default_dune_promote_demote_cooldown_hours")]
    pub promote_demote_cooldown_hours: i64,
    /// Demote ACTIVE wallets whose admitted DEX signals lose money under our
    /// own exit logic (shadow mirror_main, rolling window).
    #[serde(default = "default_shadow_quality_enabled")]
    pub shadow_quality_enabled: bool,
    /// Minimum shadow exits before a wallet can be demoted on quality.
    #[serde(default = "default_shadow_quality_min_samples")]
    pub shadow_quality_min_samples: i64,
    /// Demote when average shadow PnL% is below this threshold.
    #[serde(default = "default_shadow_quality_demote_threshold_pct")]
    pub shadow_quality_demote_threshold_pct: f64,
    /// Cost adjustment for shadow mirror PnL (same rationale as
    /// ShadowBlacklistConfig::cost_adjustment_pct): live PnL ≈ mirror − this,
    /// so the demote threshold is effectively threshold + adjustment.
    #[serde(default)]
    pub shadow_quality_cost_adjustment_pct: f64,
    /// Rolling window (hours) for shadow quality evaluation.
    #[serde(default = "default_shadow_quality_window_hours")]
    pub shadow_quality_window_hours: i64,
    /// On-chain wallet assessment (copy-trading admission gate) — verifies a
    /// candidate's ACTUAL round-trip expectancy on Solana before promotion.
    #[serde(default)]
    pub onchain_assessment: OnchainAssessmentConfig,
}

fn default_dune_pnl_query_id() -> u64 {
    8221776
}
fn default_dune_check_interval_secs() -> u64 {
    7200
}
fn default_promote_check_interval_secs() -> u64 {
    21600
}
fn default_dune_promote_enabled() -> bool {
    true
}
fn default_dune_promote_query_id() -> u64 {
    8221520
}
fn default_dune_promote_min_roi() -> f64 {
    1.2
}
fn default_dune_promote_max_per_cycle() -> u32 {
    10
}
fn default_dune_promote_max_active_total() -> u32 {
    50
}
fn default_dune_promote_demote_cooldown_hours() -> i64 {
    24
}
fn default_shadow_quality_enabled() -> bool {
    true
}
fn default_shadow_quality_min_samples() -> i64 {
    5
}
fn default_shadow_quality_demote_threshold_pct() -> f64 {
    -1.5
}
fn default_shadow_quality_window_hours() -> i64 {
    48
}

impl Default for DuneConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            pnl_query_id: default_dune_pnl_query_id(),
            check_interval_secs: default_dune_check_interval_secs(),
            promote_check_interval_secs: default_promote_check_interval_secs(),
            demote_losers_enabled: false,
            promote_enabled: default_dune_promote_enabled(),
            promote_query_id: default_dune_promote_query_id(),
            promote_min_roi: default_dune_promote_min_roi(),
            promote_max_per_cycle: default_dune_promote_max_per_cycle(),
            promote_max_active_total: default_dune_promote_max_active_total(),
            promote_demote_cooldown_hours: default_dune_promote_demote_cooldown_hours(),
            shadow_quality_enabled: default_shadow_quality_enabled(),
            shadow_quality_min_samples: default_shadow_quality_min_samples(),
            shadow_quality_demote_threshold_pct: default_shadow_quality_demote_threshold_pct(),
            shadow_quality_cost_adjustment_pct: 0.0,
            shadow_quality_window_hours: default_shadow_quality_window_hours(),
            onchain_assessment: OnchainAssessmentConfig::default(),
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

// WebSocket configuration defaults
fn default_websocket_health_timeout() -> u64 {
    60 // 1 minute
}

fn default_websocket_commitment() -> String {
    "confirmed".to_string() // Balanced latency (~2-3s) and safety
}

fn default_ws_initial_backoff() -> u64 {
    1 // Start with 1 second
}

fn default_ws_max_backoff() -> u64 {
    60 // Cap at 60 seconds
}

fn default_ws_backoff_multiplier() -> f64 {
    2.0 // Exponential doubling
}

fn default_ws_max_attempts() -> u32 {
    0 // Infinite retries
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

        let mut config: AppConfig = config.try_deserialize()?;

        // The config crate does NOT interpolate ${VAR} placeholders in YAML/JSON
        // files. Resolve security secrets against their env vars now, BEFORE
        // validation, so a missing env var fails closed (empty -> rejected by
        // validate) instead of silently using the literal placeholder string as a
        // forge-able HMAC secret. (Same class of bug as the Helius key: a literal
        // "${CHIMERA_SECURITY__WEBHOOK_SECRET}" reached consumers verbatim.)
        config.security.webhook_secret = Self::resolve_env_placeholder(
            &config.security.webhook_secret,
            "CHIMERA_SECURITY__WEBHOOK_SECRET",
        );
        if let Some(prev) = config.security.webhook_secret_previous.as_mut() {
            *prev = Self::resolve_env_placeholder(prev, "CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS");
        }

        // Resolve ${HELIUS_API_KEY} placeholders in RPC URLs (config crate does
        // not interpolate ${VAR} in YAML files, and CHIMERA_RPC__* env overrides
        // are unreliable in config 0.15 — the file is the source of truth).
        if config.rpc.primary_url.contains("${HELIUS_API_KEY}") {
            let key = std::env::var("HELIUS_API_KEY").unwrap_or_default();
            config.rpc.primary_url = config
                .rpc
                .primary_url
                .replacen("${HELIUS_API_KEY}", &key, 1);
        }
        if let Some(fallback) = config.rpc.fallback_url.as_mut() {
            if fallback.contains("${HELIUS_API_KEY}") {
                let key = std::env::var("HELIUS_API_KEY").unwrap_or_default();
                *fallback = fallback.replacen("${HELIUS_API_KEY}", &key, 1);
            }
        }

        Ok(config)
    }

    /// If `value` is a `${VAR}` placeholder, resolve it from the environment;
    /// otherwise return it unchanged. The config crate does not expand `${VAR}` in
    /// loaded files, so YAML placeholders reach consumers verbatim without this.
    fn resolve_env_placeholder(value: &str, env_var: &str) -> String {
        if value.starts_with("${") {
            std::env::var(env_var).unwrap_or_default()
        } else {
            value.to_string()
        }
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
        // Reject unresolved ${VAR} placeholders — means env-var resolution was
        // bypassed or misconfigured. A literal placeholder is forge-able (it
        // appears verbatim in the committed config.yaml), so fail closed rather
        // than accept it as the HMAC secret.
        if self.security.webhook_secret.contains("${") {
            return Err(ConfigError::Message(
                "webhook_secret is an unresolved ${...} placeholder — set \
                 CHIMERA_SECURITY__WEBHOOK_SECRET in the environment".to_string(),
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

    #[test]
    fn test_pumpfun_config_defaults() {
        let defaults = TokenSafetyConfig::default();
        assert_eq!(defaults.min_liquidity_pumpfun_usd, dec!(25000.0));
        assert!(defaults.allow_graduated_pumpfun);
        assert_eq!(defaults.min_token_age_pumpfun_hours, 4.0);
    }

    #[test]
    fn test_pumpfun_age_override_parses() {
        let mut config = TokenSafetyConfig::default();
        config.min_token_age_pumpfun_hours = 2.0;
        config.min_token_age_hours = 24.0;
        assert_eq!(config.min_token_age_hours, 24.0);
        assert_eq!(config.min_token_age_pumpfun_hours, 2.0);
    }

    #[test]
    fn test_default_strategy_allocation_sums_to_100() {
        // Serde defaults must satisfy the shield_percent + spear_percent == 100
        // invariant enforced by validate() (config.rs:1889), so a config file
        // that omits the strategy allocation block still loads cleanly.
        assert_eq!(default_shield_percent(), 50);
        assert_eq!(default_spear_percent(), 50);
        assert_eq!(default_shield_percent() + default_spear_percent(), 100);
    }

    /// Loads the committed repo-root `config.yaml`, runs full `validate()`,
    /// and verifies the SPEAR heat-budget arithmetic so that a misconfigured
    /// allocation (e.g. a sum != 100 or a budget too small for `base_size_sol`)
    /// fails the test suite rather than silently blocking SPEAR trades at runtime.
    #[test]
    fn test_repo_config_yaml_parses_validates_and_spear_budget() {
        // Tests run from operator/. Validate BOTH committed config files:
        // the root config.yaml AND config/config.yaml (the file production
        // actually mounts — see docker-compose.yml `./config:/app/config`).
        let candidates = [
            "../config/config.yaml",
            "../config.yaml",
            "config.yaml",
            "../../config.yaml",
        ];
        let mut validated = 0usize;
        for cand in candidates {
            let path = std::path::Path::new(cand);
            if !path.exists() {
                continue;
            }
            let mut config = AppConfig::load(Some(&std::path::PathBuf::from(cand)))
                .unwrap_or_else(|e| panic!("committed config {} must deserialize: {e}", cand));

            // config.yaml carries an unresolved ${...} webhook-secret placeholder;
            // resolve it to a non-empty value so validate() exercises every check
            // (including the shield+spear==100 invariant at config.rs:1889) instead
            // of bailing on the secret.
            config.security.webhook_secret = "test-secret-for-config-validation".to_string();
            config
                .validate()
                .unwrap_or_else(|e| panic!("committed config {} must pass validate(): {e}", cand));

            // SPEAR heat budget = total_capital_sol × max_heat(20%) × spear_percent/100.
            // The 20% global heat is hardcoded in signal_pipeline.rs (open item to
            // make configurable); mirror it here so this test fails if the budget no
            // longer admits at least one SPEAR position at base_size_sol.
            let capital = config.position_sizing.total_capital_sol;
            let max_heat = Decimal::from(20) / Decimal::from(100); // 0.20 (matches signal_pipeline.rs)
            let spear_budget = capital
                * max_heat
                * (Decimal::from(config.strategy.spear_percent) / Decimal::from(100));
            let base_size = config.position_sizing.base_size_sol;
            assert!(
                spear_budget >= base_size,
                "{}: SPEAR budget {} SOL must be >= base_size {} SOL, else Spear trades are fully blocked",
                cand, spear_budget, base_size,
            );
            validated += 1;
        }
        assert!(validated > 0, "no committed config.yaml found to validate");
    }
}

#[cfg(test)]
mod vol_target_config_tests {
    use super::*;

    #[test]
    fn test_new_config_fields_have_defaults() {
        let config = ProfitManagementConfig::default();
        assert_eq!(config.target_vol_scale_threshold, dec!(30.0));
        assert_eq!(config.min_target_pct, dec!(5.0));
    }

    #[test]
    fn test_config_parses_new_fields() {
        // serde_yaml isn't a project dependency; serde_json exercises the same
        // #[derive(Deserialize)] path, which is what we're verifying here.
        let json = r#"{"targets": [10, 20, 40, 80], "target_vol_scale_threshold": 25.0, "min_target_pct": 6.0}"#;
        let config: ProfitManagementConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.target_vol_scale_threshold, dec!(25.0));
        assert_eq!(config.min_target_pct, dec!(6.0));
        assert_eq!(config.targets, vec![dec!(10), dec!(20), dec!(40), dec!(80)]);
    }
}
