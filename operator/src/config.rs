//! Configuration management for Chimera Operator
//!
//! Loads configuration from YAML files and environment variables.
//! Environment variables override YAML values.

use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use std::path::PathBuf;

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
}

/// API key configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ApiKeyConfig {
    /// The API key value
    pub key: String,
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
    pub max_loss_24h_usd: f64,
    /// Maximum consecutive losses before pausing Spear
    #[serde(default = "default_max_consecutive_losses")]
    pub max_consecutive_losses: u32,
    /// Maximum drawdown percentage before emergency exit
    #[serde(default = "default_max_drawdown")]
    pub max_drawdown_percent: f64,
    /// Cooldown period in minutes after circuit trips
    #[serde(default = "default_cooldown")]
    pub cooldown_minutes: u32,
}

fn default_max_loss() -> f64 {
    500.0
}

fn default_max_consecutive_losses() -> u32 {
    5
}

fn default_max_drawdown() -> f64 {
    15.0
}

fn default_cooldown() -> u32 {
    30
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
    pub max_position_sol: f64,
    /// Minimum position size in SOL
    #[serde(default = "default_min_position")]
    pub min_position_sol: f64,
}

fn default_shield_percent() -> u32 {
    70
}

fn default_spear_percent() -> u32 {
    30
}

fn default_max_position() -> f64 {
    1.0
}

fn default_min_position() -> f64 {
    0.01
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
    pub tip_floor_sol: f64,
    /// Maximum tip in SOL
    #[serde(default = "default_tip_ceiling")]
    pub tip_ceiling_sol: f64,
    /// Percentile of recent tips to use
    #[serde(default = "default_tip_percentile")]
    pub tip_percentile: u32,
    /// Maximum tip as percentage of trade size
    #[serde(default = "default_tip_percent_max")]
    pub tip_percent_max: f64,
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

fn default_tip_floor() -> f64 {
    0.001
}

fn default_tip_ceiling() -> f64 {
    0.01
}

fn default_tip_percentile() -> u32 {
    50
}

fn default_tip_percent_max() -> f64 {
    0.10
}

/// Jupiter API configuration
#[derive(Debug, Clone, Default, Deserialize)]
pub struct JupiterConfig {
    /// Jupiter API base URL
    #[serde(default = "default_jupiter_api_url")]
    pub api_url: String,
    /// Enable devnet simulation mode (skip Jupiter API, simulate trades)
    #[serde(default)]
    pub devnet_simulation_mode: bool,
}

fn default_jupiter_api_url() -> String {
    "https://lite-api.jup.ag/swap/v1".to_string()
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
}

fn default_queue_capacity() -> usize {
    1000
}

fn default_load_shed_threshold() -> u32 {
    80
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
    pub min_liquidity_shield_usd: f64,
    /// Minimum liquidity for Spear strategy (USD)
    #[serde(default = "default_min_liquidity_spear")]
    pub min_liquidity_spear_usd: f64,
    /// Enable honeypot detection
    #[serde(default = "default_honeypot_detection")]
    pub honeypot_detection_enabled: bool,
    /// Token cache capacity
    #[serde(default = "default_token_cache_capacity")]
    pub cache_capacity: usize,
    /// Token cache TTL in seconds
    #[serde(default = "default_token_cache_ttl")]
    pub cache_ttl_seconds: i64,
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

fn default_min_liquidity_shield() -> f64 {
    10_000.0
}

fn default_min_liquidity_spear() -> f64 {
    5_000.0
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
    /// Maximum active wallets to monitor
    #[serde(default = "default_max_active_wallets")]
    pub max_active_wallets: usize,
    /// Enable automatic wallet demotion based on copy performance
    #[serde(default = "default_auto_demote_wallets")]
    pub auto_demote_wallets: bool,
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
            max_active_wallets: default_max_active_wallets(),
            auto_demote_wallets: default_auto_demote_wallets(),
        }
    }
}

/// Profit management configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ProfitManagementConfig {
    /// Profit targets (percentages)
    #[serde(default = "default_profit_targets")]
    pub targets: Vec<f64>,
    /// Percentage to sell at each target
    #[serde(default = "default_tiered_exit_percent")]
    pub tiered_exit_percent: f64,
    /// Activate trailing stop after this profit %
    #[serde(default = "default_trailing_stop_activation")]
    pub trailing_stop_activation: f64,
    /// Trailing stop distance from peak (%)
    #[serde(default = "default_trailing_stop_distance")]
    pub trailing_stop_distance: f64,
    /// Hard stop loss (%)
    #[serde(default = "default_hard_stop_loss")]
    pub hard_stop_loss: f64,
    /// Time-based exit (hours)
    #[serde(default = "default_time_exit_hours")]
    pub time_exit_hours: u64,
}

fn default_profit_targets() -> Vec<f64> {
    vec![25.0, 50.0, 100.0, 200.0]
}

fn default_tiered_exit_percent() -> f64 {
    25.0
}

fn default_trailing_stop_activation() -> f64 {
    50.0
}

fn default_trailing_stop_distance() -> f64 {
    20.0
}

fn default_hard_stop_loss() -> f64 {
    15.0
}

fn default_time_exit_hours() -> u64 {
    24
}

impl Default for ProfitManagementConfig {
    fn default() -> Self {
        Self {
            targets: default_profit_targets(),
            tiered_exit_percent: default_tiered_exit_percent(),
            trailing_stop_activation: default_trailing_stop_activation(),
            trailing_stop_distance: default_trailing_stop_distance(),
            hard_stop_loss: default_hard_stop_loss(),
            time_exit_hours: default_time_exit_hours(),
        }
    }
}

/// Position sizing configuration
#[derive(Debug, Clone, Deserialize)]
pub struct PositionSizingConfig {
    /// Base position size in SOL
    #[serde(default = "default_base_size_sol")]
    pub base_size_sol: f64,
    /// Maximum position size in SOL
    #[serde(default = "default_max_size_sol")]
    pub max_size_sol: f64,
    /// Minimum position size in SOL
    #[serde(default = "default_min_size_sol")]
    pub min_size_sol: f64,
    /// Consensus multiplier (when multiple wallets buy same token)
    #[serde(default = "default_consensus_multiplier")]
    pub consensus_multiplier: f64,
    /// Maximum concurrent positions
    #[serde(default = "default_max_concurrent_positions")]
    pub max_concurrent_positions: usize,
    /// Enable Kelly Criterion sizing
    #[serde(default = "default_use_kelly_sizing")]
    pub use_kelly_sizing: bool,
}

fn default_base_size_sol() -> f64 {
    0.1
}

fn default_max_size_sol() -> f64 {
    2.0
}

fn default_min_size_sol() -> f64 {
    0.02
}

fn default_consensus_multiplier() -> f64 {
    1.5
}

fn default_max_concurrent_positions() -> usize {
    5
}

fn default_use_kelly_sizing() -> bool {
    false
}

impl Default for PositionSizingConfig {
    fn default() -> Self {
        Self {
            base_size_sol: default_base_size_sol(),
            max_size_sol: default_max_size_sol(),
            min_size_sol: default_min_size_sol(),
            consensus_multiplier: default_consensus_multiplier(),
            max_concurrent_positions: default_max_concurrent_positions(),
            use_kelly_sizing: default_use_kelly_sizing(),
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
    pub exit_tip_sol: f64,
    /// Tip for consensus signals (SOL)
    #[serde(default = "default_consensus_tip_sol")]
    pub consensus_tip_sol: f64,
    /// Tip for standard signals (SOL)
    #[serde(default = "default_standard_tip_sol")]
    pub standard_tip_sol: f64,
}

fn default_always_use_jito() -> bool {
    true
}

fn default_exit_tip_sol() -> f64 {
    0.007
}

fn default_consensus_tip_sol() -> f64 {
    0.003
}

fn default_standard_tip_sol() -> f64 {
    0.0015
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

impl AppConfig {
    /// Load configuration from files and environment
    ///
    /// Priority (highest to lowest):
    /// 1. Environment variables (CHIMERA_*)
    /// 2. config/config.yaml (if exists)
    /// 3. config.yaml (if exists)
    /// 4. Default values
    pub fn load() -> Result<Self, ConfigError> {
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
            // CHIMERA_JUPITER__DEVNET_SIMULATION_MODE=true -> jupiter.devnet_simulation_mode = true
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

        // Check webhook secret is set
        if self.security.webhook_secret.is_empty() {
            return Err(ConfigError::Message(
                "Webhook secret must be set via CHIMERA_SECURITY__WEBHOOK_SECRET".to_string(),
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
