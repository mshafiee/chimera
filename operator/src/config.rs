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
    /// Queue configuration
    pub queue: QueueConfig,
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
    /// Maximum timestamp drift in seconds for replay protection
    #[serde(default = "default_max_timestamp_drift")]
    pub max_timestamp_drift_secs: i64,
    /// Rate limit: max requests per second
    #[serde(default = "default_webhook_rate_limit")]
    pub webhook_rate_limit: u32,
    /// Rate limit: burst size
    #[serde(default = "default_webhook_burst")]
    pub webhook_burst_size: u32,
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
            // Load from config files
            .add_source(File::with_name("config").required(false))
            .add_source(File::with_name("config/config").required(false))
            // Override with environment variables
            // CHIMERA_SERVER__PORT=8081 -> server.port = 8081
            .add_source(
                Environment::with_prefix("CHIMERA")
                    .separator("__")
                    .try_parsing(true),
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
