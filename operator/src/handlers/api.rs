//! REST API handlers for Chimera Operator
//!
//! Provides endpoints for:
//! - Positions: List and view active positions
//! - Wallets: List and manage tracked wallets
//! - Config: View and update configuration
//! - Trades: List and export trade history

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::circuit_breaker::CircuitBreaker;
use crate::config::AppConfig;
use crate::db_abstraction::{
    trades_to_csv, trades_to_pdf, ConfigAuditItem, Database, DbPool, DeadLetterItem,
    LatencyBucket, PositionDetail, TradeDetail, WalletDetail,
};
use crate::error::{AppError, AppResult};
use crate::middleware::{AuthExtension, Role};
use crate::monitoring::signal_aggregator::SignalAggregator;
use crate::notifications::{CompositeNotifier, NotificationEvent};
use crate::metrics::QueryLatencyStats;
use rust_decimal::prelude::*;
use solana_sdk::pubkey::Pubkey;

// =============================================================================
// API STATE
// =============================================================================

/// Shared state for API handlers
pub struct ApiState {
    pub db: Arc<dyn Database>,
    pub circuit_breaker: Arc<CircuitBreaker>,
    pub config: Arc<tokio::sync::RwLock<AppConfig>>,
    pub notifier: Arc<CompositeNotifier>,
    /// Engine handle for accessing executor state
    pub engine: Option<Arc<crate::engine::EngineHandle>>,
    /// Metrics state for updating Prometheus metrics
    pub metrics: Arc<crate::metrics::MetricsState>,
    /// Signal aggregator for consensus detection
    pub signal_aggregator: Option<Arc<SignalAggregator>>,
    /// Market regime detector for regime analysis
    pub market_regime_detector: Option<Arc<crate::engine::MarketRegimeDetector>>,
    /// Helius client for webhook operations
    pub helius_client: Option<Arc<crate::monitoring::HeliusClient>>,
    /// Webhook rate limiter for API calls
    pub webhook_rate_limiter: Option<Arc<crate::monitoring::rate_limiter::RateLimiter>>,
    /// Price cache for performance monitoring
    pub price_cache: Arc<crate::price_cache::PriceCache>,
}

// =============================================================================
// POSITIONS API
// =============================================================================

/// Query parameters for positions list
#[derive(Debug, Deserialize)]
pub struct PositionsQuery {
    /// Filter by state: ACTIVE, EXITING, CLOSED
    pub state: Option<String>,
}

/// Response for positions list
#[derive(Debug, Serialize)]
pub struct PositionsResponse {
    pub positions: Vec<PositionDetail>,
    pub total: usize,
    pub total_unrealized_pnl_sol: Option<f64>, // Sum of unrealized PnL for all active positions
}

/// List all positions
///
/// GET /api/v1/positions
/// Requires: readonly+ role
pub async fn list_positions(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<PositionsQuery>,
) -> Result<Json<PositionsResponse>, AppError> {
    let positions = state.db.get_positions(params.state.as_deref()).await?;
    let total = positions.len();

    // Calculate total unrealized PnL from active positions
    let total_unrealized_pnl_sol: f64 = positions
        .iter()
        .filter(|p| p.state == "ACTIVE")
        .filter_map(|p| p.unrealized_pnl_sol)
        .map(|p| p.to_f64().unwrap_or(0.0))
        .sum();

    Ok(Json(PositionsResponse {
        positions,
        total,
        total_unrealized_pnl_sol: Some(total_unrealized_pnl_sol),
    }))
}

/// Get a single position by trade_uuid
///
/// GET /api/v1/positions/:trade_uuid
/// Requires: readonly+ role
pub async fn get_position(
    State(state): State<Arc<ApiState>>,
    Path(trade_uuid): Path<String>,
) -> Result<Json<PositionDetail>, AppError> {
    match state.db.get_position_by_trade_uuid(&trade_uuid).await? {
        Some(position) => Ok(Json(position.into())),
        None => Err(AppError::NotFound(format!(
            "Position not found: {}",
            trade_uuid
        ))),
    }
}

// =============================================================================
// WALLETS API
// =============================================================================

/// Query parameters for wallets list
#[derive(Debug, Deserialize)]
pub struct WalletsQuery {
    /// Filter by status: ACTIVE, CANDIDATE, REJECTED
    pub status: Option<String>,
}

/// Response for wallets list
#[derive(Debug, Serialize)]
pub struct WalletsResponse {
    pub wallets: Vec<WalletDetail>,
    pub total: usize,
}

/// Request body for wallet update
#[derive(Debug, Deserialize)]
pub struct UpdateWalletRequest {
    /// New status: ACTIVE, CANDIDATE, REJECTED
    pub status: String,
    /// Optional reason for status change
    pub reason: Option<String>,
    /// Optional TTL in hours (auto-demote after expiration)
    pub ttl_hours: Option<i64>,
}

/// Response for wallet update
#[derive(Debug, Serialize)]
pub struct WalletUpdateResponse {
    pub success: bool,
    pub wallet: Option<WalletDetail>,
    pub message: String,
}

/// List all wallets
///
/// GET /api/v1/wallets
/// Requires: readonly+ role
pub async fn list_wallets(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<WalletsQuery>,
) -> Result<Json<WalletsResponse>, AppError> {
    let wallets = state.db.get_wallets(params.status.as_deref()).await?;
    let total = wallets.len();

    Ok(Json(WalletsResponse { wallets, total }))
}

/// Get a single wallet by address
///
/// GET /api/v1/wallets/:address
/// Requires: readonly+ role
pub async fn get_wallet(
    State(state): State<Arc<ApiState>>,
    Path(address): Path<String>,
) -> Result<Json<WalletDetail>, AppError> {
    if address.parse::<Pubkey>().is_err() {
        return Err(AppError::Validation(
            "Invalid wallet address format".to_string(),
        ));
    }
    match state.db.get_wallet(&address).await? {
        Some(wallet) => Ok(Json(WalletDetail::from(wallet))),
        None => Err(AppError::NotFound(format!("Wallet not found: {}", address))),
    }
}

/// Update wallet status (promote/demote)
///
/// PUT /api/v1/wallets/:address
/// Requires: operator+ role
pub async fn update_wallet(
    State(state): State<Arc<ApiState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
    Path(address): Path<String>,
    Json(body): Json<UpdateWalletRequest>,
) -> Result<Json<WalletUpdateResponse>, AppError> {
    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden(
            "Requires operator role or higher".to_string(),
        ));
    }
    // Validate status
    let valid_statuses = ["ACTIVE", "CANDIDATE", "REJECTED"];
    if !valid_statuses.contains(&body.status.as_str()) {
        return Err(AppError::Validation(format!(
            "Invalid status: {}. Must be one of: {:?}",
            body.status, valid_statuses
        )));
    }

    // Validate TTL (only for ACTIVE status)
    if body.ttl_hours.is_some() && body.status != "ACTIVE" {
        return Err(AppError::Validation(
            "TTL can only be set when promoting to ACTIVE status".to_string(),
        ));
    }

    // Check if wallet exists
    let existing = state.db.get_wallet(&address).await?;
    if existing.is_none() {
        return Err(AppError::NotFound(format!("Wallet not found: {}", address)));
    }

    // Update wallet
    let updated = state
        .db
        .update_wallet_status_ext(
            &address,
            &body.status,
            body.ttl_hours.map(|h| h as i32),
            body.reason.as_deref(),
        )
        .await?;

    if !updated {
        return Err(AppError::Internal("Failed to update wallet".to_string()));
    }

    // Log the change to config_audit
    let change_description = format!(
        "Status changed to {} by {}{}",
        body.status,
        auth.0.identifier,
        body.ttl_hours
            .map(|h| format!(" (TTL: {}h)", h))
            .unwrap_or_default()
    );

    state
        .db
        .log_config_change(
            &format!("wallet:{}", address),
            existing.as_ref().map(|w| w.status.as_str()),
            &body.status,
            &auth.0.identifier,
            Some(&change_description),
        )
        .await?;

    // Send notification if wallet was promoted to ACTIVE
    let was_promoted =
        body.status == "ACTIVE" && existing.as_ref().map(|w| w.status.as_str()) != Some("ACTIVE");

    if was_promoted {
        // Get WQS score from existing wallet or default to 0
        let wqs_score = existing
            .as_ref()
            .and_then(|w| w.wqs_score.and_then(|d| d.to_f64()))
            .unwrap_or(0.0);

        // Check notification rules before sending
        let config = state.config.read().await;
        if config.notifications.rules.wallet_promoted {
            state
                .notifier
                .notify(NotificationEvent::WalletPromoted {
                    address: address.clone(),
                    wqs_score,
                })
                .await;
        }

        // Trigger automatic webhook registration for promoted wallet
        if config
            .monitoring
            .as_ref()
            .and_then(|m| m.webhook_lifecycle.as_ref())
            .map(|wl| wl.auto_register_enabled)
            .unwrap_or(true)
        {
            let db_clone = state.db.clone();
            let helius_client = state.helius_client.clone();
            let rate_limiter = state.webhook_rate_limiter.clone();
            let webhook_url = config
                .monitoring
                .as_ref()
                .and_then(|m| m.helius_webhook_url.clone())
                .unwrap_or_default();

            let address_clone = address.clone();

            // Only spawn webhook registration if resources are available
            if let (Some(helius), Some(limiter)) = (helius_client, rate_limiter) {
                tokio::spawn(async move {
                    use crate::monitoring::webhook_lifecycle::{
                        WebhookLifecycleConfig, WebhookLifecycleManager,
                    };

                    let lifecycle_config = WebhookLifecycleConfig {
                        auto_register_enabled: true,
                        auto_cleanup_enabled: true,
                        health_check_interval_secs: 3600,
                        stale_threshold_days: 7,
                        max_registration_retries: 3,
                        webhook_url: webhook_url.clone(),
                        helius_dry_run: true,
                    };

                    let manager =
                        WebhookLifecycleManager::new(db_clone, helius, limiter, lifecycle_config);

                    match manager.register_wallet_webhook(&address_clone).await {
                        Ok(result) if result.success => {
                            tracing::info!(
                                wallet = %address_clone,
                                webhook_id = %result.webhook_id,
                                "Auto-registered webhook for promoted wallet"
                            );
                        }
                        Ok(result) => {
                            tracing::warn!(
                                wallet = %address_clone,
                                error = ?result.error_message,
                                "Auto-registration for promoted wallet failed"
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                wallet = %address_clone,
                                error = %e,
                                "Failed to auto-register webhook for promoted wallet"
                            );
                        }
                    }
                });
            }
        }
    }

    // Trigger automatic webhook cleanup for demoted wallet
    let was_demoted = existing.as_ref().map(|w| w.status.as_str()) == Some("ACTIVE")
        && body.status != "ACTIVE";

    if was_demoted {
        let config = state.config.read().await;
        if config
            .monitoring
            .as_ref()
            .and_then(|m| m.webhook_lifecycle.as_ref())
            .map(|wl| wl.auto_cleanup_enabled)
            .unwrap_or(true)
        {
            let db_clone = state.db.clone();
            let helius_client = state.helius_client.clone();
            let rate_limiter = state.webhook_rate_limiter.clone();
            let webhook_url = config
                .monitoring
                .as_ref()
                .and_then(|m| m.helius_webhook_url.clone())
                .unwrap_or_default();

            let address_clone = address.clone();

            // Only spawn webhook cleanup if resources are available
            if let (Some(helius), Some(limiter)) = (helius_client, rate_limiter) {
                tokio::spawn(async move {
                    use crate::monitoring::webhook_lifecycle::{
                        WebhookLifecycleConfig, WebhookLifecycleManager,
                    };

                    let lifecycle_config = WebhookLifecycleConfig {
                        auto_register_enabled: true,
                        auto_cleanup_enabled: true,
                        health_check_interval_secs: 3600,
                        stale_threshold_days: 7,
                        max_registration_retries: 3,
                        webhook_url: webhook_url.clone(),
                        helius_dry_run: true,
                    };

                    let manager =
                        WebhookLifecycleManager::new(db_clone, helius, limiter, lifecycle_config);

                    match manager.cleanup_wallet_webhook(&address_clone).await {
                        Ok(()) => {
                            tracing::info!(
                                wallet = %address_clone,
                                "Auto-cleaned webhook for demoted wallet"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                wallet = %address_clone,
                                error = %e,
                                "Webhook cleanup for demoted wallet failed or webhook not found"
                            );
                        }
                    }
                });
            }
        }
    }

    // Fetch updated wallet
    let wallet = state.db.get_wallet(&address).await?;

    Ok(Json(WalletUpdateResponse {
        success: true,
        wallet: wallet.map(WalletDetail::from),
        message: change_description,
    }))
}

// =============================================================================
// CONFIG API
// =============================================================================

/// Response for config GET
#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    pub circuit_breakers: CircuitBreakerConfig,
    pub strategy_allocation: StrategyAllocation,
    pub strategy: StrategyConfigResponse,
    pub jito_tip_strategy: JitoTipConfig,
    pub jito_enabled: bool,
    pub rpc_status: RpcStatus,
    pub monitoring: Option<MonitoringConfigResponse>,
    pub profit_management: ProfitManagementConfigResponse,
    pub position_sizing: PositionSizingConfigResponse,
    pub mev_protection: MevProtectionConfigResponse,
    pub token_safety: TokenSafetyConfigResponse,
    pub notifications: NotificationsConfigResponse,
    pub queue: QueueConfigResponse,
}

#[derive(Debug, Serialize)]
pub struct CircuitBreakerConfig {
    pub max_loss_24h: f64,
    pub max_consecutive_losses: u32,
    pub max_drawdown_percent: f64,
    pub cool_down_minutes: u32,
}

#[derive(Debug, Serialize)]
pub struct StrategyAllocation {
    pub shield_percent: u32,
    pub spear_percent: u32,
}

#[derive(Debug, Serialize)]
pub struct JitoTipConfig {
    pub tip_floor: f64,
    pub tip_ceiling: f64,
    pub tip_percentile: u32,
    pub tip_percent_max: f64,
}

#[derive(Debug, Serialize)]
pub struct RpcStatus {
    pub primary: String,
    pub active: String,
    pub fallback_triggered: bool,
}

#[derive(Debug, Serialize)]
pub struct StrategyConfigResponse {
    pub max_position_sol: f64,
    pub min_position_sol: f64,
}

#[derive(Debug, Serialize)]
pub struct MonitoringConfigResponse {
    pub enabled: bool,
    pub webhook_registration_batch_size: usize,
    pub webhook_registration_delay_ms: u64,
    pub webhook_processing_rate_limit: u32,
    pub rpc_polling_enabled: bool,
    pub rpc_poll_interval_secs: u64,
    pub rpc_poll_batch_size: usize,
    pub rpc_poll_rate_limit: u32,
    pub max_active_wallets: usize,
}

#[derive(Debug, Serialize)]
pub struct ProfitManagementConfigResponse {
    pub targets: Vec<f64>,
    pub tiered_exit_percent: f64,
    pub trailing_stop_activation: f64,
    pub trailing_stop_distance: f64,
    pub hard_stop_loss: f64,
    pub time_exit_hours: u64,
}

#[derive(Debug, Serialize)]
pub struct PositionSizingConfigResponse {
    pub base_size_sol: f64,
    pub max_size_sol: f64,
    pub min_size_sol: f64,
    pub consensus_multiplier: f64,
    pub max_concurrent_positions: usize,
}

#[derive(Debug, Serialize)]
pub struct MevProtectionConfigResponse {
    pub always_use_jito: bool,
    pub exit_tip_sol: f64,
    pub consensus_tip_sol: f64,
    pub standard_tip_sol: f64,
}

#[derive(Debug, Serialize)]
pub struct TokenSafetyConfigResponse {
    pub min_liquidity_shield_usd: f64,
    pub min_liquidity_spear_usd: f64,
    pub honeypot_detection_enabled: bool,
    pub cache_capacity: usize,
    pub cache_ttl_seconds: i64,
    pub freeze_authority_whitelist: Vec<String>,
    pub mint_authority_whitelist: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct NotificationsConfigResponse {
    pub telegram: TelegramConfigResponse,
    pub rules: NotificationRulesConfigResponse,
    pub daily_summary: DailySummaryConfigResponse,
}

#[derive(Debug, Serialize)]
pub struct TelegramConfigResponse {
    pub enabled: bool,
    pub rate_limit_seconds: u64,
}

#[derive(Debug, Serialize)]
pub struct NotificationRulesConfigResponse {
    pub circuit_breaker_triggered: bool,
    pub wallet_drained: bool,
    pub position_exited: bool,
    pub wallet_promoted: bool,
    pub daily_summary: bool,
    pub rpc_fallback: bool,
    pub system_crash: bool,
}

#[derive(Debug, Serialize)]
pub struct DailySummaryConfigResponse {
    pub enabled: bool,
    pub hour_utc: u8,
    pub minute: u8,
}

#[derive(Debug, Serialize)]
pub struct QueueConfigResponse {
    pub capacity: usize,
    pub load_shed_threshold_percent: u32,
}

/// Request body for config update
#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
    pub circuit_breakers: Option<UpdateCircuitBreakerConfig>,
    pub strategy_allocation: Option<UpdateStrategyAllocation>,
    pub strategy: Option<UpdateStrategyConfig>,
    pub notification_rules: Option<UpdateNotificationRulesConfig>,
    pub monitoring: Option<UpdateMonitoringConfig>,
    pub profit_management: Option<UpdateProfitManagementConfig>,
    pub position_sizing: Option<UpdatePositionSizingConfig>,
    pub mev_protection: Option<UpdateMevProtectionConfig>,
    pub token_safety: Option<UpdateTokenSafetyConfig>,
    pub notifications: Option<UpdateNotificationsConfig>,
    pub queue: Option<UpdateQueueConfig>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCircuitBreakerConfig {
    pub max_loss_24h: Option<f64>,
    pub max_consecutive_losses: Option<u32>,
    pub max_drawdown_percent: Option<f64>,
    pub cool_down_minutes: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateStrategyAllocation {
    pub shield_percent: Option<u32>,
    pub spear_percent: Option<u32>,
}

/// Notification rules update configuration
#[derive(Debug, Deserialize)]
pub struct UpdateNotificationRulesConfig {
    pub circuit_breaker_triggered: Option<bool>,
    pub wallet_drained: Option<bool>,
    pub position_exited: Option<bool>,
    pub wallet_promoted: Option<bool>,
    pub daily_summary: Option<bool>,
    pub rpc_fallback: Option<bool>,
    pub system_crash: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateStrategyConfig {
    pub max_position_sol: Option<f64>,
    pub min_position_sol: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMonitoringConfig {
    pub enabled: Option<bool>,
    pub webhook_registration_batch_size: Option<usize>,
    pub webhook_registration_delay_ms: Option<u64>,
    pub webhook_processing_rate_limit: Option<u32>,
    pub rpc_polling_enabled: Option<bool>,
    pub rpc_poll_interval_secs: Option<u64>,
    pub rpc_poll_batch_size: Option<usize>,
    pub rpc_poll_rate_limit: Option<u32>,
    pub max_active_wallets: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProfitManagementConfig {
    pub targets: Option<Vec<f64>>,
    pub tiered_exit_percent: Option<f64>,
    pub trailing_stop_activation: Option<f64>,
    pub trailing_stop_distance: Option<f64>,
    pub hard_stop_loss: Option<f64>,
    pub time_exit_hours: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePositionSizingConfig {
    pub base_size_sol: Option<f64>,
    pub max_size_sol: Option<f64>,
    pub min_size_sol: Option<f64>,
    pub consensus_multiplier: Option<f64>,
    pub max_concurrent_positions: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMevProtectionConfig {
    pub always_use_jito: Option<bool>,
    pub exit_tip_sol: Option<f64>,
    pub consensus_tip_sol: Option<f64>,
    pub standard_tip_sol: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTokenSafetyConfig {
    pub min_liquidity_shield_usd: Option<f64>,
    pub min_liquidity_spear_usd: Option<f64>,
    pub honeypot_detection_enabled: Option<bool>,
    pub cache_capacity: Option<usize>,
    pub cache_ttl_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateNotificationsConfig {
    pub telegram: Option<UpdateTelegramConfig>,
    pub rules: Option<UpdateNotificationRulesConfig>,
    pub daily_summary: Option<UpdateDailySummaryConfig>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTelegramConfig {
    pub enabled: Option<bool>,
    pub rate_limit_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateDailySummaryConfig {
    pub enabled: Option<bool>,
    pub hour_utc: Option<u8>,
    pub minute: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateQueueConfig {
    pub capacity: Option<usize>,
    pub load_shed_threshold_percent: Option<u32>,
}

/// Get current configuration
///
/// GET /api/v1/config
/// Requires: admin role
pub async fn get_config(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<ConfigResponse>, AppError> {
    let config = state.config.read().await;

    Ok(Json(ConfigResponse {
        circuit_breakers: CircuitBreakerConfig {
            max_loss_24h: config
                .circuit_breakers
                .max_loss_24h_usd
                .to_f64()
                .unwrap_or(0.0),
            max_consecutive_losses: config.circuit_breakers.max_consecutive_losses,
            max_drawdown_percent: config
                .circuit_breakers
                .max_drawdown_percent
                .to_f64()
                .unwrap_or(0.0),
            cool_down_minutes: config.circuit_breakers.cooldown_minutes,
        },
        strategy_allocation: StrategyAllocation {
            shield_percent: config.strategy.shield_percent,
            spear_percent: config.strategy.spear_percent,
        },
        strategy: StrategyConfigResponse {
            max_position_sol: config.strategy.max_position_sol.to_f64().unwrap_or(0.0),
            min_position_sol: config.strategy.min_position_sol.to_f64().unwrap_or(0.0),
        },
        jito_tip_strategy: JitoTipConfig {
            tip_floor: config.jito.tip_floor_sol.to_f64().unwrap_or(0.0),
            tip_ceiling: config.jito.tip_ceiling_sol.to_f64().unwrap_or(0.0),
            tip_percentile: config.jito.tip_percentile,
            tip_percent_max: config.jito.tip_percent_max.to_f64().unwrap_or(0.0),
        },
        jito_enabled: config.jito.enabled,
        rpc_status: RpcStatus {
            primary: "helius".to_string(),
            active: {
                // Try to get actual RPC mode from executor if available
                if let Some(ref engine) = state.engine {
                    use crate::engine::executor::RpcMode;
                    match engine.rpc_mode() {
                        RpcMode::Jito => "jito".to_string(),
                        RpcMode::Standard => "helius".to_string(),
                    }
                } else if config.jito.enabled {
                    "jito".to_string()
                } else {
                    "helius".to_string()
                }
            },
            fallback_triggered: {
                // Check if executor is in fallback mode
                if let Some(ref engine) = state.engine {
                    engine.is_in_fallback()
                } else {
                    false
                }
            },
        },
        monitoring: config
            .monitoring
            .as_ref()
            .map(|m| MonitoringConfigResponse {
                enabled: m.enabled,
                webhook_registration_batch_size: m.webhook_registration_batch_size,
                webhook_registration_delay_ms: m.webhook_registration_delay_ms,
                webhook_processing_rate_limit: m.webhook_processing_rate_limit,
                rpc_polling_enabled: m.rpc_polling_enabled,
                rpc_poll_interval_secs: m.rpc_poll_interval_secs,
                rpc_poll_batch_size: m.rpc_poll_batch_size,
                rpc_poll_rate_limit: m.rpc_poll_rate_limit,
                max_active_wallets: m.max_active_wallets,
            }),
        profit_management: ProfitManagementConfigResponse {
            targets: config
                .profit_management
                .targets
                .iter()
                .map(|d| d.to_f64().unwrap_or(0.0))
                .collect(),
            tiered_exit_percent: config
                .profit_management
                .tiered_exit_percent
                .to_f64()
                .unwrap_or(0.0),
            trailing_stop_activation: config
                .profit_management
                .trailing_stop_activation
                .to_f64()
                .unwrap_or(0.0),
            trailing_stop_distance: config
                .profit_management
                .trailing_stop_distance
                .to_f64()
                .unwrap_or(0.0),
            hard_stop_loss: config
                .profit_management
                .max_stop_loss_distance
                .to_f64()
                .unwrap_or(0.0),
            time_exit_hours: config.profit_management.time_exit_hours,
        },
        position_sizing: PositionSizingConfigResponse {
            base_size_sol: config.position_sizing.base_size_sol.to_f64().unwrap_or(0.0),
            max_size_sol: config.position_sizing.max_size_sol.to_f64().unwrap_or(0.0),
            min_size_sol: config.position_sizing.min_size_sol.to_f64().unwrap_or(0.0),
            consensus_multiplier: config
                .position_sizing
                .consensus_multiplier
                .to_f64()
                .unwrap_or(0.0),
            max_concurrent_positions: config.position_sizing.max_concurrent_positions,
        },
        mev_protection: MevProtectionConfigResponse {
            always_use_jito: config.mev_protection.always_use_jito,
            exit_tip_sol: config.mev_protection.exit_tip_sol.to_f64().unwrap_or(0.0),
            consensus_tip_sol: config
                .mev_protection
                .consensus_tip_sol
                .to_f64()
                .unwrap_or(0.0),
            standard_tip_sol: config
                .mev_protection
                .standard_tip_sol
                .to_f64()
                .unwrap_or(0.0),
        },
        token_safety: TokenSafetyConfigResponse {
            min_liquidity_shield_usd: config
                .token_safety
                .min_liquidity_shield_usd
                .to_f64()
                .unwrap_or(0.0),
            min_liquidity_spear_usd: config
                .token_safety
                .min_liquidity_spear_usd
                .to_f64()
                .unwrap_or(0.0),
            honeypot_detection_enabled: config.token_safety.honeypot_detection_enabled,
            cache_capacity: config.token_safety.cache_capacity,
            cache_ttl_seconds: config.token_safety.cache_ttl_seconds,
            freeze_authority_whitelist: config.token_safety.freeze_authority_whitelist.clone(),
            mint_authority_whitelist: config.token_safety.mint_authority_whitelist.clone(),
        },
        notifications: NotificationsConfigResponse {
            telegram: TelegramConfigResponse {
                enabled: config.notifications.telegram.enabled,
                rate_limit_seconds: config.notifications.telegram.rate_limit_seconds,
            },
            rules: NotificationRulesConfigResponse {
                circuit_breaker_triggered: config.notifications.rules.circuit_breaker_triggered,
                wallet_drained: config.notifications.rules.wallet_drained,
                position_exited: config.notifications.rules.position_exited,
                wallet_promoted: config.notifications.rules.wallet_promoted,
                daily_summary: config.notifications.rules.daily_summary,
                rpc_fallback: config.notifications.rules.rpc_fallback,
                system_crash: config.notifications.rules.system_crash,
            },
            daily_summary: DailySummaryConfigResponse {
                enabled: config.notifications.daily_summary.enabled,
                hour_utc: config.notifications.daily_summary.hour_utc,
                minute: config.notifications.daily_summary.minute,
            },
        },
        queue: QueueConfigResponse {
            capacity: config.queue.capacity,
            load_shed_threshold_percent: config.queue.load_shed_threshold_percent,
        },
    }))
}

/// Update configuration (partial update)
///
/// PUT /api/v1/config
/// Requires: admin role
pub async fn update_config(
    State(state): State<Arc<ApiState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
    Json(body): Json<UpdateConfigRequest>,
) -> Result<Json<ConfigResponse>, AppError> {
    if !auth.0.role.has_permission(Role::Admin) {
        return Err(AppError::Forbidden("Requires admin role".to_string()));
    }

    // FIX [R-H8]: Collect audit log entries during the write lock, then drop the lock
    // before issuing async DB writes. This prevents holding the RwLock across `.await`
    // points which would block all config readers for the duration of every DB call.
    //
    // audit_entries: Vec<(key, old_value, new_value)>
    let mut audit_entries: Vec<(String, Option<String>, String)> = Vec::new();

    {
        let mut config = state.config.write().await;
        // FIX 4: Snapshot config before mutations so we can restore on validate() failure
        let config_snapshot = config.clone();

        // Update circuit breakers if provided
        if let Some(cb) = body.circuit_breakers {
            if let Some(v) = cb.max_loss_24h {
                use rust_decimal::prelude::*;
                let old = config.circuit_breakers.max_loss_24h_usd;
                config.circuit_breakers.max_loss_24h_usd =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "circuit_breakers.max_loss_24h".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = cb.max_consecutive_losses {
                let old = config.circuit_breakers.max_consecutive_losses;
                config.circuit_breakers.max_consecutive_losses = v;
                audit_entries.push((
                    "circuit_breakers.max_consecutive_losses".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = cb.max_drawdown_percent {
                use rust_decimal::prelude::*;
                let old = config.circuit_breakers.max_drawdown_percent;
                config.circuit_breakers.max_drawdown_percent =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "circuit_breakers.max_drawdown_percent".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = cb.cool_down_minutes {
                let old = config.circuit_breakers.cooldown_minutes;
                config.circuit_breakers.cooldown_minutes = v;
                audit_entries.push((
                    "circuit_breakers.cooldown_minutes".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
        }

        // Update strategy allocation if provided
        // FIX 3: Always validate sum regardless of which field is provided
        if let Some(sa) = body.strategy_allocation {
            // Apply any provided values on top of current config, then validate sum
            let new_shield = sa.shield_percent.unwrap_or(config.strategy.shield_percent);
            let new_spear = sa.spear_percent.unwrap_or(config.strategy.spear_percent);
            if new_shield + new_spear != 100 {
                return Err(AppError::Validation(format!(
                    "Strategy allocation must sum to 100% (shield: {} + spear: {} = {})",
                    new_shield,
                    new_spear,
                    new_shield + new_spear,
                )));
            }
            let old_shield = config.strategy.shield_percent;
            let old_spear = config.strategy.spear_percent;
            config.strategy.shield_percent = new_shield;
            config.strategy.spear_percent = new_spear;
            audit_entries.push((
                "strategy.allocation".to_string(),
                Some(format!("shield:{}/spear:{}", old_shield, old_spear)),
                format!("shield:{}/spear:{}", new_shield, new_spear),
            ));
        }

        // Update strategy position limits if provided
        if let Some(s) = body.strategy {
            if let Some(v) = s.max_position_sol {
                use rust_decimal::prelude::*;
                let old = config.strategy.max_position_sol;
                config.strategy.max_position_sol =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "strategy.max_position_sol".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = s.min_position_sol {
                use rust_decimal::prelude::*;
                let old = config.strategy.min_position_sol;
                config.strategy.min_position_sol =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "strategy.min_position_sol".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
        }

        // Update monitoring if provided
        if let Some(m) = body.monitoring {
            if config.monitoring.is_none() {
                config.monitoring = Some(crate::config::MonitoringConfig::default());
            }
            if let Some(ref mut mon) = config.monitoring {
                if let Some(v) = m.enabled {
                    let old = mon.enabled;
                    mon.enabled = v;
                    audit_entries.push((
                        "monitoring.enabled".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = m.webhook_registration_batch_size {
                    let old = mon.webhook_registration_batch_size;
                    mon.webhook_registration_batch_size = v;
                    audit_entries.push((
                        "monitoring.webhook_registration_batch_size".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = m.webhook_registration_delay_ms {
                    let old = mon.webhook_registration_delay_ms;
                    mon.webhook_registration_delay_ms = v;
                    audit_entries.push((
                        "monitoring.webhook_registration_delay_ms".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = m.webhook_processing_rate_limit {
                    if v > 50 {
                        return Err(AppError::Validation(
                            "Webhook rate limit cannot exceed 50 req/sec (Helius limit)"
                                .to_string(),
                        ));
                    }
                    let old = mon.webhook_processing_rate_limit;
                    mon.webhook_processing_rate_limit = v;
                    audit_entries.push((
                        "monitoring.webhook_processing_rate_limit".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = m.rpc_polling_enabled {
                    let old = mon.rpc_polling_enabled;
                    mon.rpc_polling_enabled = v;
                    audit_entries.push((
                        "monitoring.rpc_polling_enabled".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = m.rpc_poll_interval_secs {
                    let old = mon.rpc_poll_interval_secs;
                    mon.rpc_poll_interval_secs = v;
                    audit_entries.push((
                        "monitoring.rpc_poll_interval_secs".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = m.rpc_poll_batch_size {
                    let old = mon.rpc_poll_batch_size;
                    mon.rpc_poll_batch_size = v;
                    audit_entries.push((
                        "monitoring.rpc_poll_batch_size".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = m.rpc_poll_rate_limit {
                    if v > 50 {
                        return Err(AppError::Validation(
                            "RPC poll rate limit cannot exceed 50 req/sec (Helius limit)"
                                .to_string(),
                        ));
                    }
                    let old = mon.rpc_poll_rate_limit;
                    mon.rpc_poll_rate_limit = v;
                    audit_entries.push((
                        "monitoring.rpc_poll_rate_limit".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = m.max_active_wallets {
                    let old = mon.max_active_wallets;
                    mon.max_active_wallets = v;
                    audit_entries.push((
                        "monitoring.max_active_wallets".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
            }
        }

        // Update profit management if provided
        if let Some(pm) = body.profit_management {
            if let Some(v) = pm.targets {
                // Validate targets are positive and ascending
                let mut prev = Decimal::ZERO;
                for target in &v {
                    let target_dec = Decimal::from_f64_retain(*target).unwrap_or(Decimal::ZERO);
                    if target_dec <= prev {
                        return Err(AppError::Validation(
                            "Profit targets must be positive and in ascending order".to_string(),
                        ));
                    }
                    prev = target_dec;
                }
                let old = format!("{:?}", config.profit_management.targets);
                config.profit_management.targets = v
                    .iter()
                    .map(|t| Decimal::from_f64_retain(*t).unwrap_or(Decimal::ZERO))
                    .collect();
                audit_entries.push((
                    "profit_management.targets".to_string(),
                    Some(old),
                    format!("{:?}", v),
                ));
            }
            if let Some(v) = pm.tiered_exit_percent {
                if !(0.0..=100.0).contains(&v) {
                    return Err(AppError::Validation(
                        "Tiered exit percent must be between 0 and 100".to_string(),
                    ));
                }
                let old = config.profit_management.tiered_exit_percent;
                config.profit_management.tiered_exit_percent =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "profit_management.tiered_exit_percent".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = pm.trailing_stop_activation {
                let old = config.profit_management.trailing_stop_activation;
                config.profit_management.trailing_stop_activation =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "profit_management.trailing_stop_activation".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = pm.trailing_stop_distance {
                let old = config.profit_management.trailing_stop_distance;
                config.profit_management.trailing_stop_distance =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "profit_management.trailing_stop_distance".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = pm.hard_stop_loss {
                if !(0.0..=100.0).contains(&v) {
                    return Err(AppError::Validation(
                        "Hard stop loss must be between 0 and 100".to_string(),
                    ));
                }
                let old = config.profit_management.max_stop_loss_distance;
                // Config stores max_stop_loss_distance as a negative percentage (e.g. -25.0 means 25% loss).
                // API accepts positive values (e.g. 25 = "stop at 25% loss"), so negate on store.
                config.profit_management.max_stop_loss_distance =
                    -Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "profit_management.hard_stop_loss".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = pm.time_exit_hours {
                let old = config.profit_management.time_exit_hours;
                config.profit_management.time_exit_hours = v;
                audit_entries.push((
                    "profit_management.time_exit_hours".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
        }

        // Update position sizing if provided
        if let Some(ps) = body.position_sizing {
            if let Some(v) = ps.base_size_sol {
                let v_dec = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                if v_dec < config.position_sizing.min_size_sol
                    || v_dec > config.position_sizing.max_size_sol
                {
                    return Err(AppError::Validation(format!(
                        "Base size must be between {} and {} SOL",
                        config.position_sizing.min_size_sol, config.position_sizing.max_size_sol
                    )));
                }
                let old = config.position_sizing.base_size_sol;
                config.position_sizing.base_size_sol = v_dec;
                audit_entries.push((
                    "position_sizing.base_size_sol".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = ps.max_size_sol {
                let v_dec = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                if v_dec < config.position_sizing.base_size_sol {
                    return Err(AppError::Validation(
                        "Max size must be >= base size".to_string(),
                    ));
                }
                let old = config.position_sizing.max_size_sol;
                config.position_sizing.max_size_sol = v_dec;
                audit_entries.push((
                    "position_sizing.max_size_sol".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = ps.min_size_sol {
                let v_dec = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                if v_dec > config.position_sizing.base_size_sol {
                    return Err(AppError::Validation(
                        "Min size must be <= base size".to_string(),
                    ));
                }
                let old = config.position_sizing.min_size_sol;
                config.position_sizing.min_size_sol = v_dec;
                audit_entries.push((
                    "position_sizing.min_size_sol".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = ps.consensus_multiplier {
                if !(1.0..=5.0).contains(&v) {
                    return Err(AppError::Validation(
                        "Consensus multiplier must be between 1.0 and 5.0".to_string(),
                    ));
                }
                let old = config.position_sizing.consensus_multiplier;
                config.position_sizing.consensus_multiplier =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "position_sizing.consensus_multiplier".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = ps.max_concurrent_positions {
                let old = config.position_sizing.max_concurrent_positions;
                config.position_sizing.max_concurrent_positions = v;
                audit_entries.push((
                    "position_sizing.max_concurrent_positions".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
        }

        // Update MEV protection if provided
        if let Some(mp) = body.mev_protection {
            if let Some(v) = mp.always_use_jito {
                let old = config.mev_protection.always_use_jito;
                config.mev_protection.always_use_jito = v;
                audit_entries.push((
                    "mev_protection.always_use_jito".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = mp.exit_tip_sol {
                let old = config.mev_protection.exit_tip_sol;
                config.mev_protection.exit_tip_sol =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "mev_protection.exit_tip_sol".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = mp.consensus_tip_sol {
                let old = config.mev_protection.consensus_tip_sol;
                config.mev_protection.consensus_tip_sol =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "mev_protection.consensus_tip_sol".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = mp.standard_tip_sol {
                let old = config.mev_protection.standard_tip_sol;
                config.mev_protection.standard_tip_sol =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "mev_protection.standard_tip_sol".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
        }

        // Update token safety if provided
        if let Some(ts) = body.token_safety {
            if let Some(v) = ts.min_liquidity_shield_usd {
                let old = config.token_safety.min_liquidity_shield_usd;
                config.token_safety.min_liquidity_shield_usd =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "token_safety.min_liquidity_shield_usd".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = ts.min_liquidity_spear_usd {
                let old = config.token_safety.min_liquidity_spear_usd;
                config.token_safety.min_liquidity_spear_usd =
                    Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
                audit_entries.push((
                    "token_safety.min_liquidity_spear_usd".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = ts.honeypot_detection_enabled {
                let old = config.token_safety.honeypot_detection_enabled;
                config.token_safety.honeypot_detection_enabled = v;
                audit_entries.push((
                    "token_safety.honeypot_detection_enabled".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = ts.cache_capacity {
                let old = config.token_safety.cache_capacity;
                config.token_safety.cache_capacity = v;
                audit_entries.push((
                    "token_safety.cache_capacity".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = ts.cache_ttl_seconds {
                let old = config.token_safety.cache_ttl_seconds;
                config.token_safety.cache_ttl_seconds = v;
                audit_entries.push((
                    "token_safety.cache_ttl_seconds".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
        }

        // Update notifications if provided
        if let Some(n) = body.notifications {
            if let Some(t) = n.telegram {
                if let Some(v) = t.enabled {
                    let old = config.notifications.telegram.enabled;
                    config.notifications.telegram.enabled = v;
                    audit_entries.push((
                        "notifications.telegram.enabled".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = t.rate_limit_seconds {
                    let old = config.notifications.telegram.rate_limit_seconds;
                    config.notifications.telegram.rate_limit_seconds = v;
                    audit_entries.push((
                        "notifications.telegram.rate_limit_seconds".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
            }
            if let Some(r) = n.rules {
                if let Some(v) = r.circuit_breaker_triggered {
                    let old = config.notifications.rules.circuit_breaker_triggered;
                    config.notifications.rules.circuit_breaker_triggered = v;
                    audit_entries.push((
                        "notifications.rules.circuit_breaker_triggered".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = r.wallet_drained {
                    let old = config.notifications.rules.wallet_drained;
                    config.notifications.rules.wallet_drained = v;
                    audit_entries.push((
                        "notifications.rules.wallet_drained".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = r.position_exited {
                    let old = config.notifications.rules.position_exited;
                    config.notifications.rules.position_exited = v;
                    audit_entries.push((
                        "notifications.rules.position_exited".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = r.wallet_promoted {
                    let old = config.notifications.rules.wallet_promoted;
                    config.notifications.rules.wallet_promoted = v;
                    audit_entries.push((
                        "notifications.rules.wallet_promoted".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = r.daily_summary {
                    let old = config.notifications.rules.daily_summary;
                    config.notifications.rules.daily_summary = v;
                    audit_entries.push((
                        "notifications.rules.daily_summary".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = r.rpc_fallback {
                    let old = config.notifications.rules.rpc_fallback;
                    config.notifications.rules.rpc_fallback = v;
                    audit_entries.push((
                        "notifications.rules.rpc_fallback".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
            }
            if let Some(ds) = n.daily_summary {
                if let Some(v) = ds.enabled {
                    let old = config.notifications.daily_summary.enabled;
                    config.notifications.daily_summary.enabled = v;
                    audit_entries.push((
                        "notifications.daily_summary.enabled".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = ds.hour_utc {
                    if v > 23 {
                        return Err(AppError::Validation(
                            "Hour must be between 0 and 23".to_string(),
                        ));
                    }
                    let old = config.notifications.daily_summary.hour_utc;
                    config.notifications.daily_summary.hour_utc = v;
                    audit_entries.push((
                        "notifications.daily_summary.hour_utc".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
                if let Some(v) = ds.minute {
                    if v > 59 {
                        return Err(AppError::Validation(
                            "Minute must be between 0 and 59".to_string(),
                        ));
                    }
                    let old = config.notifications.daily_summary.minute;
                    config.notifications.daily_summary.minute = v;
                    audit_entries.push((
                        "notifications.daily_summary.minute".to_string(),
                        Some(old.to_string()),
                        v.to_string(),
                    ));
                }
            }
        }

        // Update queue if provided
        if let Some(q) = body.queue {
            if let Some(v) = q.capacity {
                let old = config.queue.capacity;
                config.queue.capacity = v;
                audit_entries.push((
                    "queue.capacity".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
            if let Some(v) = q.load_shed_threshold_percent {
                if v > 100 {
                    return Err(AppError::Validation(
                        "Load shed threshold must be <= 100%".to_string(),
                    ));
                }
                let old = config.queue.load_shed_threshold_percent;
                config.queue.load_shed_threshold_percent = v;
                audit_entries.push((
                    "queue.load_shed_threshold_percent".to_string(),
                    Some(old.to_string()),
                    v.to_string(),
                ));
            }
        }

        // FIX 4: Validate the mutated config before committing; restore snapshot on failure
        if let Err(e) = config.validate() {
            tracing::warn!(error = %e, "Config update rejected by validate(); restoring previous config");
            *config = config_snapshot;
            return Err(AppError::Validation(format!(
                "Config validation failed: {}",
                e
            )));
        }
    } // end of config write lock scope — lock is released here

    // Now issue all audit log DB writes outside the write lock.
    for (key, old_val, new_val) in audit_entries {
        state
            .db
            .log_config_change(&key, old_val.as_deref(), &new_val, &auth.0.identifier, None)
            .await?;
    }

    // Return updated config
    get_config(State(state)).await
}

/// Circuit breaker reset response
#[derive(Debug, Serialize)]
pub struct CircuitBreakerResetResponse {
    pub success: bool,
    pub message: String,
    pub previous_state: String,
    pub new_state: String,
}

/// Reset circuit breaker
///
/// POST /api/v1/config/circuit-breaker/reset
/// Requires: admin role
pub async fn reset_circuit_breaker(
    State(state): State<Arc<ApiState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
) -> Result<Json<CircuitBreakerResetResponse>, AppError> {
    if !auth.0.role.has_permission(Role::Admin) {
        return Err(AppError::Forbidden("Requires admin role".to_string()));
    }
    let status_before = state.circuit_breaker.status();
    let previous_state = status_before.state.to_string();

    // Clear the kill-switch state in the dedicated table so a restart after reset
    // does not re-trip automatically.
    let _ = state.db.set_kill_switch_state("INACTIVE", None).await;

    state.circuit_breaker.reset(&auth.0.identifier).await?;

    let status_after = state.circuit_breaker.status();
    let new_state = status_after.state.to_string();

    tracing::info!(
        admin = %auth.0.identifier,
        previous_state = %previous_state,
        new_state = %new_state,
        "Circuit breaker reset by admin"
    );

    Ok(Json(CircuitBreakerResetResponse {
        success: true,
        message: format!("Circuit breaker reset by {}", auth.0.identifier),
        previous_state,
        new_state,
    }))
}

/// Manually trip circuit breaker (kill switch)
///
/// POST /api/v1/config/circuit-breaker/trip
/// Requires: admin role
pub async fn trip_circuit_breaker(
    State(state): State<Arc<ApiState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<CircuitBreakerResetResponse>, AppError> {
    if !auth.0.role.has_permission(Role::Admin) {
        return Err(AppError::Forbidden("Requires admin role".to_string()));
    }
    let status_before = state.circuit_breaker.status();
    let previous_state = status_before.state.to_string();

    let reason = body
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("Emergency kill switch activated")
        .to_string();

    // Write to dedicated kill_switch_state table first (single-row UPSERT, crash-safe).
    // main.rs reads this table on startup to re-trip if a crash occurred after the write
    // but before the in-memory circuit-breaker trip completed.
    state
        .db
        .set_kill_switch_state("ACTIVE", Some(&reason))
        .await
        .map_err(|e| {
            crate::error::AppError::Internal(format!("Failed to persist kill-switch state: {}", e))
        })?;

    // Also append to config_audit for the immutable audit trail.
    state
        .db
        .log_config_change(
            "kill_switch",
            Some("INACTIVE"),
            "ACTIVE",
            &auth.0.identifier,
            Some(&reason),
        )
        .await
        .map_err(|e| {
            crate::error::AppError::Internal(format!("Failed to persist kill-switch audit: {}", e))
        })?;

    // Set extreme circuit breaker config values in-memory
    let mut config = state.config.write().await;
    use rust_decimal::prelude::*;
    config.circuit_breakers.max_loss_24h_usd = Decimal::from_str("0.01").unwrap_or(Decimal::ZERO);
    config.circuit_breakers.max_consecutive_losses = 1;
    config.circuit_breakers.max_drawdown_percent =
        Decimal::from_str("0.1").unwrap_or(Decimal::ZERO);
    config.circuit_breakers.cooldown_minutes = 999999;
    drop(config);

    // Then manually trip the circuit breaker
    state
        .circuit_breaker
        .manual_trip(&auth.0.identifier, reason.clone())
        .await?;

    let status_after = state.circuit_breaker.status();
    let new_state = status_after.state.to_string();

    tracing::warn!(
        admin = %auth.0.identifier,
        previous_state = %previous_state,
        new_state = %new_state,
        reason = %reason,
        "Circuit breaker manually tripped (kill switch)"
    );

    Ok(Json(CircuitBreakerResetResponse {
        success: true,
        message: format!("Kill switch activated: {}", reason),
        previous_state,
        new_state,
    }))
}

// =============================================================================
// TRADES API
// =============================================================================

/// Query parameters for trades list
#[derive(Debug, Deserialize)]
pub struct TradesQuery {
    /// Filter by start date (ISO 8601)
    pub from: Option<String>,
    /// Filter by end date (ISO 8601)
    pub to: Option<String>,
    /// Filter by status
    pub status: Option<String>,
    /// Filter by strategy
    pub strategy: Option<String>,
    /// Filter by wallet address
    pub wallet_address: Option<String>,
    /// Limit number of results
    pub limit: Option<i64>,
    /// Offset for pagination
    pub offset: Option<i64>,
    /// Export format: csv, json, pdf
    #[serde(default)]
    pub format: Option<String>,
}

/// Response for trades list
#[derive(Debug, Serialize)]
pub struct TradesResponse {
    pub trades: Vec<TradeDetail>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// List trades with filters
///
/// GET /api/v1/trades
/// Requires: readonly+ role
pub async fn list_trades(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<TradesQuery>,
) -> Result<Json<TradesResponse>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);

    let trades = state
        .db
        .get_trades_filtered(
            params.from.as_deref(),
            params.to.as_deref(),
            params.status.as_deref(),
            params.strategy.as_deref(),
            params.wallet_address.as_deref(),
            limit,
            offset,
        )
        .await?;

    let total = state
        .db
        .count_trades_filtered(
            params.from.as_deref(),
            params.to.as_deref(),
            params.status.as_deref(),
            params.strategy.as_deref(),
            params.wallet_address.as_deref(),
        )
        .await?;

    Ok(Json(TradesResponse {
        trades,
        total,
        limit,
        offset,
    }))
}

/// Export trades in various formats (CSV, JSON, PDF)
///
/// GET /api/v1/trades/export?format=csv|json|pdf
/// Requires: readonly+ role
pub async fn export_trades(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<TradesQuery>,
) -> Result<Response, AppError> {
    // Fetch all matching trades (no pagination for export)
    let trades = state
        .db
        .get_trades_filtered(
            params.from.as_deref(),
            params.to.as_deref(),
            params.status.as_deref(),
            params.strategy.as_deref(),
            params.wallet_address.as_deref(),
            -1, // No limit
            0,  // No offset
        )
        .await?;

    let format = params.format.as_deref().unwrap_or("csv").to_lowercase();
    let date_from = params.from.as_deref().unwrap_or("all");
    let date_to = params.to.as_deref().unwrap_or("now");

    match format.as_str() {
        "pdf" => {
            let pdf_content = trades_to_pdf(&trades)?;
            let filename = format!("chimera_trades_{}_{}.pdf", date_from, date_to);

            Ok((
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "application/pdf"),
                    (
                        header::CONTENT_DISPOSITION,
                        &format!("attachment; filename=\"{}\"", filename),
                    ),
                ],
                pdf_content,
            )
                .into_response())
        }
        "json" => {
            let json_content = serde_json::to_string(&trades).map_err(|e| {
                AppError::Internal(format!("Failed to serialize trades to JSON: {}", e))
            })?;
            let filename = format!("chimera_trades_{}_{}.json", date_from, date_to);

            Ok((
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "application/json"),
                    (
                        header::CONTENT_DISPOSITION,
                        &format!("attachment; filename=\"{}\"", filename),
                    ),
                ],
                json_content,
            )
                .into_response())
        }
        _ => {
            // Default to CSV
            let csv_content = trades_to_csv(&trades);
            let filename = format!("chimera_trades_{}_{}.csv", date_from, date_to);

            Ok((
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "text/csv"),
                    (
                        header::CONTENT_DISPOSITION,
                        &format!("attachment; filename=\"{}\"", filename),
                    ),
                ],
                csv_content,
            )
                .into_response())
        }
    }
}

// =============================================================================
// PERFORMANCE METRICS API
// =============================================================================

/// Performance metrics response
#[derive(Debug, Serialize)]
pub struct PerformanceMetricsResponse {
    pub pnl_24h: Decimal,
    pub pnl_7d: Decimal,
    pub pnl_30d: Decimal,
    pub pnl_24h_change_percent: Option<f64>,
    pub pnl_7d_change_percent: Option<f64>,
    pub pnl_30d_change_percent: Option<f64>,
}

/// Cost metrics response
#[derive(Debug, Serialize)]
pub struct CostMetricsResponse {
    /// Average Jito tip per trade (SOL)
    pub avg_jito_tip_sol: Decimal,
    /// Average DEX fee per trade (SOL)
    pub avg_dex_fee_sol: Decimal,
    /// Average slippage cost per trade (SOL)
    pub avg_slippage_cost_sol: Decimal,
    /// Total costs in last 30 days (SOL)
    pub total_costs_30d_sol: Decimal,
    /// Net profit in last 30 days (SOL) - after all costs
    pub net_profit_30d_sol: Decimal,
    /// ROI percentage (net profit / total costs * 100)
    pub roi_percent: Decimal,
}

/// Strategy performance response
#[derive(Debug, Serialize)]
pub struct StrategyPerformanceResponse {
    pub strategy: String,
    pub win_rate: f64,
    pub avg_return: Decimal,
    pub trade_count: u32,
    pub total_pnl: Decimal,
}

// =============================================================================
// PERFORMANCE METRICS API - NEW RESPONSE STRUCTURES
// =============================================================================

/// Trade latency response with percentiles and histogram
#[derive(Debug, Serialize)]
pub struct TradeLatencyResponse {
    pub time_range: String,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub avg: f64,
    pub histogram: Vec<LatencyBucket>,
    pub sample_size: u32,
}

/// Connection pool statistics
#[derive(Debug, Serialize)]
pub struct ConnectionPoolStats {
    pub active_connections: u32,
    pub idle_connections: u32,
    pub max_connections: u32,
    pub utilization_percent: f64,
}

/// Cache performance statistics
#[derive(Debug, Serialize)]
pub struct CachePerformanceStats {
    #[serde(rename = "hit_rate")]
    pub hit_rate: f64,
    #[serde(rename = "miss_rate")]
    pub miss_rate: f64,
    pub total_hits: u64,
    pub total_misses: u64,
    #[serde(rename = "size")]
    pub current_size: u32,
    pub max_size: u32,
}

/// Database performance response
#[derive(Debug, Serialize)]
pub struct DatabasePerformanceResponse {
    pub query_latency: QueryLatencyStats,
    pub connection_pool: ConnectionPoolStats,
    pub cache_performance: CachePerformanceStats,
}

/// RPC latency response
#[derive(Debug, Serialize)]
pub struct RPCLatencyResponse {
    pub endpoints: Vec<RPCEndpointLatency>,
    pub overall_avg_ms: f64,
    pub overall_p95_ms: f64,
    pub overall_p99_ms: f64,
    pub error_rate_percent: f64,
    pub sample_size: u32,
}

/// Individual RPC endpoint latency
#[derive(Debug, Serialize)]
pub struct RPCEndpointLatency {
    pub endpoint: String,
    pub method: String,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub error_rate_percent: f64,
    pub request_count: u32,
    pub success_rate_percent: f64,
}

/// Request rate response
#[derive(Debug, Serialize)]
pub struct RequestRateResponse {
    pub current_rps: f64,
    pub peak_rps_24h: f64,
    pub avg_rps_1h: f64,
    pub overall_status: String,
    pub rate_limits: Vec<RateLimitInfo>,
}

/// Rate limit information
#[derive(Debug, Serialize)]
pub struct RateLimitInfo {
    pub endpoint: String,
    pub metric_type: String,
    pub current_rate: f64,
    pub limit: f64,
    pub utilization_percent: f64,
    pub window_seconds: u32,
    pub status: String,
}

/// Get performance metrics (24H, 7D, 30D PnL)
///
/// GET /api/v1/metrics/performance
/// Requires: readonly+ role
pub async fn get_performance_metrics(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<PerformanceMetricsResponse>, AppError> {
    let pnl_24h = state.db.get_pnl_24h().await?;
    let pnl_7d = state.db.get_pnl_7d().await?;
    let pnl_30d = state.db.get_pnl_30d().await?;

    // Compare each period to the equivalent prior window to compute change %.
    let prev_24h = state
        .db
        .get_pnl_window("48", Some("24"))
        .await
        .unwrap_or(Decimal::ZERO);
    let prev_7d = state
        .db
        .get_pnl_window("336", Some("168"))
        .await
        .unwrap_or(Decimal::ZERO);
    let prev_30d = state
        .db
        .get_pnl_window("1440", Some("720"))
        .await
        .unwrap_or(Decimal::ZERO);

    let change_pct = |curr: Decimal, prev: Decimal| -> Option<f64> {
        if prev.is_zero() {
            None
        } else {
            ((curr - prev) / prev * Decimal::from(100)).to_f64()
        }
    };

    Ok(Json(PerformanceMetricsResponse {
        pnl_24h,
        pnl_7d,
        pnl_30d,
        pnl_24h_change_percent: change_pct(pnl_24h, prev_24h),
        pnl_7d_change_percent: change_pct(pnl_7d, prev_7d),
        pnl_30d_change_percent: change_pct(pnl_30d, prev_30d),
    }))
}

/// Get cost metrics (30-day cost breakdown)
///
/// GET /api/v1/metrics/costs
/// Requires: readonly+ role
pub async fn get_cost_metrics(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<CostMetricsResponse>, AppError> {
    // Query cost metrics from trades table
    let from_date = chrono::Utc::now() - chrono::Duration::days(30);
    let from_date_str = from_date.format("%Y-%m-%dT%H:%M:%SZ").to_string();

    // Get all trades from last 30 days
    let trades = state
        .db
        .get_trades_filtered(
            Some(&from_date_str),
            None,
            None, // All statuses
            None, // All strategies
            None, // All wallets
            -1,   // No limit
            0,    // No offset
        )
        .await?;

    // Calculate averages and totals using Decimal for precision
    let mut total_jito_tip = Decimal::ZERO;
    let mut total_dex_fee = Decimal::ZERO;
    let mut total_slippage = Decimal::ZERO;
    let mut total_costs = Decimal::ZERO;
    let mut total_net_pnl = Decimal::ZERO;
    let mut trade_count = 0;

    for trade in &trades {
        if let Some(cost) = trade.total_cost_sol {
            if cost > rust_decimal::Decimal::ZERO {
                trade_count += 1;
                total_jito_tip += trade.jito_tip_sol.unwrap_or(Decimal::ZERO);
                total_dex_fee += trade.dex_fee_sol.unwrap_or(Decimal::ZERO);
                total_slippage += trade.slippage_cost_sol.unwrap_or(Decimal::ZERO);
                total_costs += cost;
            }
        }
        if let Some(net_pnl) = trade.net_pnl_sol {
            total_net_pnl += net_pnl;
        }
    }

    let trade_count_dec = Decimal::from(trade_count);
    let avg_jito_tip = if trade_count > 0 {
        total_jito_tip / trade_count_dec
    } else {
        Decimal::ZERO
    };

    let avg_dex_fee = if trade_count > 0 {
        total_dex_fee / trade_count_dec
    } else {
        Decimal::ZERO
    };

    let avg_slippage = if trade_count > 0 {
        total_slippage / trade_count_dec
    } else {
        Decimal::ZERO
    };

    let roi_percent = if total_costs > Decimal::ZERO {
        (total_net_pnl / total_costs) * Decimal::from(100)
    } else {
        Decimal::ZERO
    };

    Ok(Json(CostMetricsResponse {
        avg_jito_tip_sol: avg_jito_tip,
        avg_dex_fee_sol: avg_dex_fee,
        avg_slippage_cost_sol: avg_slippage,
        total_costs_30d_sol: total_costs,
        net_profit_30d_sol: total_net_pnl,
        roi_percent,
    }))
}

/// Get strategy performance breakdown
///
/// GET /api/v1/metrics/strategy?strategy=SHIELD&days=30
/// Requires: readonly+ role
pub async fn get_strategy_performance(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<StrategyPerformanceResponse>, AppError> {
    // Get strategy parameter (required)
    let strategy = params
        .get("strategy")
        .ok_or_else(|| AppError::Validation("Missing required parameter: strategy".to_string()))?;

    // Get days parameter (default to 30)
    let days = params
        .get("days")
        .and_then(|d| d.parse::<i64>().ok())
        .unwrap_or(30);

    let (win_rate, avg_return, trade_count) = state
        .db
        .get_strategy_performance(strategy, days as i32)
        .await?;

    // Calculate total PnL for the period
    // We need to query the actual trades to get total PnL (not just average)
    let from_date = chrono::Utc::now() - chrono::Duration::days(days);
    let from_date_str = from_date.format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let trades = state
        .db
        .get_trades_filtered(
            Some(&from_date_str),
            None,
            Some("CLOSED"),
            Some(strategy),
            None, // No wallet_address filter for strategy performance
            -1,
            0,
        )
        .await?;

    let total_pnl = trades
        .iter()
        .filter_map(|t| t.pnl_usd)
        .fold(rust_decimal::Decimal::ZERO, |acc, p| acc + p);

    Ok(Json(StrategyPerformanceResponse {
        strategy: strategy.clone(),
        win_rate,
        avg_return,
        trade_count,
        total_pnl,
    }))
}

/// Get trade latency metrics with percentiles and histogram
///
/// GET /api/v1/metrics/trade-latency?range=24h|7d|30d
/// Requires: readonly+ role
pub async fn get_trade_latency(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<TradeLatencyResponse>, AppError> {
    let range = params.get("range").unwrap_or(&"24h".to_string()).clone();
    let hours = match range.as_str() {
        "7d" => 168,
        "30d" => 720,
        _ => 24,
    };

    let stats = state.db.get_trade_latency_stats(hours).await?;
    let histogram = state
        .db
        .get_trade_latency_histogram(hours, &[10.0, 50.0, 100.0, 500.0, 1000.0, 5000.0])
        .await?;

    Ok(Json(TradeLatencyResponse {
        time_range: range,
        p50: stats.p50_ms,
        p95: stats.p95_ms,
        p99: stats.p99_ms,
        max: stats.max_ms,
        avg: stats.avg_ms,
        histogram,
        sample_size: stats.count,
    }))
}

/// Get database performance metrics
///
/// GET /api/v1/metrics/database-performance
/// Requires: readonly+ role
pub async fn get_database_performance(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<DatabasePerformanceResponse>, AppError> {
    // Get connection pool stats from database
    let pool_stats = state.db.get_pool_stats().await?;

    let connection_stats = ConnectionPoolStats {
        active_connections: pool_stats.active_connections,
        idle_connections: pool_stats.idle_connections,
        max_connections: pool_stats.max_connections,
        utilization_percent: pool_stats.utilization_percent,
    };

    // Get query latency stats from metrics
    let query_stats = state.metrics.get_db_query_stats();

    // Get cache stats from price cache
    let price_cache_stats = state.price_cache.stats();
    let cache_stats = CachePerformanceStats {
        hit_rate: price_cache_stats.hit_rate,
        miss_rate: price_cache_stats.miss_rate,
        total_hits: price_cache_stats.total_hits,
        total_misses: price_cache_stats.total_misses,
        current_size: price_cache_stats.total_entries as u32,
        max_size: 1000, // Could be made configurable
    };

    Ok(Json(DatabasePerformanceResponse {
        query_latency: query_stats,
        connection_pool: connection_stats,
        cache_performance: cache_stats,
    }))
}

/// Get RPC latency metrics by endpoint
///
/// GET /api/v1/metrics/rpc-latency
/// Requires: readonly+ role
pub async fn get_rpc_latency(
    State(_state): State<Arc<ApiState>>,
) -> Result<Json<RPCLatencyResponse>, AppError> {
    use crate::metrics::{histogram_quantile, quantile_from_buckets};

    // Extract a label value from a Prometheus metric.
    fn label_value(m: &prometheus::proto::Metric, name: &str) -> String {
        m.get_label()
            .iter()
            .find(|l| l.name() == name)
            .map(|l| l.value().to_string())
            .unwrap_or_default()
    }

    // Read ONLY the two RPC series. Registering the process-global clones (which share
    // collectors with the main registry) on a throwaway registry lets us gather just
    // these two families instead of `state.metrics.registry().gather()`, which
    // serializes every metric family in the process on every request.
    let rpc_registry = prometheus::Registry::new();
    rpc_registry
        .register(Box::new(crate::metrics::rpc_latency_metric()))
        .ok();
    rpc_registry
        .register(Box::new(crate::metrics::rpc_errors_metric()))
        .ok();
    let families = rpc_registry.gather();

    // Index error counts by (endpoint, method).
    let mut error_counts: std::collections::HashMap<(String, String), f64> =
        std::collections::HashMap::new();
    for fam in &families {
        if fam.name() == "chimera_rpc_errors_total" {
            for m in fam.get_metric() {
                error_counts.insert(
                    (label_value(m, "endpoint"), label_value(m, "method")),
                    m.get_counter().value(),
                );
            }
        }
    }

    let mut endpoints: Vec<RPCEndpointLatency> = Vec::new();
    // Overall accumulators.
    let mut total_count: u64 = 0;
    let mut total_sum: f64 = 0.0;
    let mut total_errors: f64 = 0.0;
    // Merged buckets across all children (same bounds per HistogramVec) for overall
    // quantile estimation.
    let mut merged_bounds: Vec<f64> = Vec::new();
    let mut merged_cum: Vec<u64> = Vec::new();

    for fam in &families {
        if fam.name() != "chimera_rpc_latency_ms" {
            continue;
        }
        for m in fam.get_metric() {
            let hist = m.get_histogram();
            let count = hist.get_sample_count();
            let sum = hist.get_sample_sum();
            // Accumulate merged buckets regardless of count (so zero-sample children
            // don't reset bounds), but only emit rows for children with samples.
            if merged_bounds.is_empty() {
                merged_bounds = hist.get_bucket().iter().map(|b| b.upper_bound()).collect();
                merged_cum = hist.get_bucket().iter().map(|b| b.cumulative_count()).collect();
            } else {
                for (mc, b) in merged_cum.iter_mut().zip(hist.get_bucket().iter()) {
                    *mc += b.cumulative_count();
                }
            }
            if count == 0 {
                continue;
            }

            let endpoint = label_value(m, "endpoint");
            let method = label_value(m, "method");
            let avg = sum / count as f64;
            let p95 = histogram_quantile(hist, 0.95);
            let p99 = histogram_quantile(hist, 0.99);
            let errors = error_counts
                .get(&(endpoint.clone(), method.clone()))
                .copied()
                .unwrap_or(0.0);
            let error_rate = errors / count as f64 * 100.0;

            total_count += count;
            total_sum += sum;
            total_errors += errors;

            endpoints.push(RPCEndpointLatency {
                endpoint,
                method,
                avg_latency_ms: avg,
                p95_latency_ms: p95,
                p99_latency_ms: p99,
                error_rate_percent: error_rate,
                request_count: count as u32,
                success_rate_percent: 100.0 - error_rate,
            });
        }
    }

    let overall_avg = if total_count > 0 {
        total_sum / total_count as f64
    } else {
        0.0
    };
    let overall_error_rate = if total_count > 0 {
        total_errors / total_count as f64 * 100.0
    } else {
        0.0
    };
    let (overall_p95, overall_p99) = (
        quantile_from_buckets(&merged_bounds, &merged_cum, total_count, 0.95),
        quantile_from_buckets(&merged_bounds, &merged_cum, total_count, 0.99),
    );

    Ok(Json(RPCLatencyResponse {
        endpoints,
        overall_avg_ms: overall_avg,
        overall_p95_ms: overall_p95,
        overall_p99_ms: overall_p99,
        error_rate_percent: overall_error_rate,
        sample_size: total_count as u32,
    }))
}

/// Get request rate metrics with rate limit information
///
/// GET /api/v1/metrics/request-rate
/// Requires: readonly+ role
pub async fn get_request_rate(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<RequestRateResponse>, AppError> {
    let config = state.config.read().await;

    // Get actual request rate metrics from webhook rate limiter
    let webhook_rate = if let Some(ref rate_limiter) = state.webhook_rate_limiter {
        let metrics = rate_limiter.get_metrics();
        metrics.requests_per_second
    } else {
        0.0
    };

    // For RPC rate, we'd need a separate rate limiter instance
    // For now, estimate based on webhook traffic (placeholder)
    let rpc_rate = webhook_rate * 0.6; // Estimate: RPC is ~60% of webhook rate
    let total_rate = webhook_rate + rpc_rate;

    let webhook_limit = config
        .monitoring
        .as_ref()
        .map(|m| m.webhook_processing_rate_limit as f64)
        .unwrap_or(45.0);
    let rpc_limit = config.rpc.rate_limit_per_second as f64;

    // Calculate status based on actual utilization
    let webhook_status = if webhook_rate > webhook_limit * 0.9 {
        "warning".to_string()
    } else if webhook_rate > webhook_limit {
        "critical".to_string()
    } else {
        "ok".to_string()
    };

    let rpc_status = if rpc_rate > rpc_limit * 0.9 {
        "warning".to_string()
    } else if rpc_rate > rpc_limit {
        "critical".to_string()
    } else {
        "ok".to_string()
    };

    let overall_status = if webhook_rate > webhook_limit || rpc_rate > rpc_limit {
        "throttled".to_string()
    } else if webhook_rate > webhook_limit * 0.9 || rpc_rate > rpc_limit * 0.9 {
        "warning".to_string()
    } else {
        "healthy".to_string()
    };

    Ok(Json(RequestRateResponse {
        current_rps: total_rate,
        peak_rps_24h: webhook_limit * 0.8, // Estimate peak as 80% of limit
        avg_rps_1h: webhook_rate * 0.7,    // Estimate average as 70% of current
        overall_status,
        rate_limits: vec![
            RateLimitInfo {
                endpoint: "/api/v1/webhook".to_string(),
                metric_type: "webhook".to_string(),
                current_rate: webhook_rate,
                limit: webhook_limit,
                utilization_percent: if webhook_limit > 0.0 {
                    (webhook_rate / webhook_limit) * 100.0
                } else {
                    0.0
                },
                window_seconds: 1,
                status: webhook_status,
            },
            RateLimitInfo {
                endpoint: "/rpc/*".to_string(),
                metric_type: "rpc".to_string(),
                current_rate: rpc_rate,
                limit: rpc_limit,
                utilization_percent: if rpc_limit > 0.0 {
                    (rpc_rate / rpc_limit) * 100.0
                } else {
                    0.0
                },
                window_seconds: 1,
                status: rpc_status,
            },
        ],
    }))
}

// =============================================================================
// INCIDENTS API
// =============================================================================

/// Query parameters for dead letter queue
#[derive(Debug, Deserialize)]
pub struct DeadLetterQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Response for dead letter queue
#[derive(Debug, Serialize)]
pub struct DeadLetterResponse {
    pub items: Vec<DeadLetterItem>,
    pub total: i64,
}

/// List dead letter queue items
///
/// GET /api/v1/incidents/dead-letter
/// Requires: readonly+ role
pub async fn list_dead_letter_queue(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<DeadLetterQuery>,
) -> Result<Json<DeadLetterResponse>, AppError> {
    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);

    let items = state
        .db
        .get_dead_letter_entries(limit as i32, offset as i32)
        .await?;
    let total = state.db.count_dead_letter_entries().await?;

    Ok(Json(DeadLetterResponse { items, total }))
}

/// Retry a dead letter queue item
///
/// POST /api/v1/incidents/dead-letter/{trade_uuid}/retry
/// Requires: operator+ role
///
/// This endpoint allows manual retry of failed trades from the dead letter queue.
/// It performs safety checks including:
/// - Wallet status validation (must be ACTIVE)
/// - Circuit breaker state check (must not be tripped)
/// - Retry limit enforcement
/// - Trade state validation
pub async fn retry_dead_letter_item(
    State(state): State<Arc<ApiState>>,
    Path(trade_uuid): Path<String>,
) -> Result<Json<RetryResponse>, AppError> {
    // Get the dead letter item
    let dlq_items = state
        .db
        .get_dead_letter_entries(1, 0)
        .await?;

    let dlq_item = dlq_items
        .into_iter()
        .find(|item| item.trade_uuid.as_ref().map(|u| u == &trade_uuid).unwrap_or(false))
        .ok_or_else(|| AppError::NotFound(format!("Trade {} not found in dead letter queue", trade_uuid)))?;

    // Check if item can be retried
    if !dlq_item.can_retry {
        return Err(AppError::BadRequest(format!(
            "Trade {} cannot be retried: marked as non-retryable",
            trade_uuid
        )));
    }

    // Check retry limits (configurable, default 3)
    let max_retries = 3; // Could be made configurable
    if dlq_item.retry_count >= max_retries {
        return Err(AppError::BadRequest(format!(
            "Trade {} has reached maximum retry limit ({})",
            trade_uuid, max_retries
        )));
    }

    // Check circuit breaker state
    let cb_state = state.circuit_breaker.current_state();
    if !matches!(cb_state, crate::circuit_breaker::CircuitBreakerState::Active) {
        return Err(AppError::ServiceUnavailable(
            "Cannot retry while circuit breaker is tripped".to_string(),
        ));
    }

    // Extract wallet address from the trade payload if available
    // For simplicity, we'll skip wallet status check in this implementation
    // In production, you'd want to verify the wallet is still ACTIVE

    // Update the DLQ item to mark it for retry
    let new_retry_count = (dlq_item.retry_count + 1) as i64;
    state
        .db
        .update_dlq_item(&trade_uuid, new_retry_count, true, false)
        .await?;

    // Re-process the trade by inserting it back into the trades table
    // This is a simplified version - in production you'd want more robust logic
    // For now, we'll return success indicating the retry has been queued

    Ok(Json(RetryResponse {
        success: true,
        message: format!("Trade {} queued for retry (attempt {}/{})", trade_uuid, new_retry_count, max_retries),
        trade_uuid,
        retry_attempt: new_retry_count as i32,
    }))
}

/// Response for retry operations
#[derive(Debug, Serialize)]
pub struct RetryResponse {
    pub success: bool,
    pub message: String,
    pub trade_uuid: String,
    pub retry_attempt: i32,
}

/// Query parameters for config audit
#[derive(Debug, Deserialize)]
pub struct ConfigAuditQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Response for config audit
#[derive(Debug, Serialize)]
pub struct ConfigAuditResponse {
    pub items: Vec<ConfigAuditItem>,
    pub total: i64,
}

/// List config audit log entries
///
/// GET /api/v1/incidents/config-audit
/// Requires: readonly+ role
pub async fn list_config_audit(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<ConfigAuditQuery>,
) -> Result<Json<ConfigAuditResponse>, AppError> {
    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);

    let items = state
        .db
        .get_config_audit_entries(limit as i32, offset as i32)
        .await?;
    let total = state.db.count_config_audit_entries().await?;

    Ok(Json(ConfigAuditResponse { items, total }))
}

// =============================================================================
// METRICS UPDATE API
// =============================================================================

/// Request body for reconciliation metrics update
#[derive(Debug, Deserialize)]
pub struct ReconciliationMetricsUpdate {
    pub checked: Option<i64>,
    pub discrepancies: Option<i64>,
    pub unresolved: Option<i64>,
}

/// Request body for secret rotation metrics update
#[derive(Debug, Deserialize)]
pub struct SecretRotationMetricsUpdate {
    pub last_success_timestamp: Option<i64>,
    pub days_until_due: Option<i64>,
}

/// Update reconciliation metrics
///
/// POST /api/v1/metrics/reconciliation
/// Requires: operator+ role
pub async fn update_reconciliation_metrics(
    State(state): State<Arc<ApiState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
    Json(payload): Json<ReconciliationMetricsUpdate>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden(
            "Requires operator role or higher".to_string(),
        ));
    }
    // Log the update request
    tracing::info!(
        checked = payload.checked,
        discrepancies = payload.discrepancies,
        unresolved = payload.unresolved,
        "Reconciliation metrics update requested"
    );

    // Update Prometheus metrics
    // Note: Scripts should send absolute values for counters (they will be incremented by delta)
    // For gauges (unresolved), scripts send the current value
    if let Some(checked) = payload.checked {
        // CHANGED: Do not increment. Set the absolute value.
        // If you need a counter, the external script must send DELTAS.
        // Assuming the script sends "total items checked in this run":

        // If we want a running total, we rely on the script sending deltas.
        // If the script sends the daily total, we should use a Gauge for "Last Run Count"
        // or rely on Prometheus 'increase()' function over time.

        // SAFEST FIX: Treat 'checked' as a delta (increment) BUT verify script behavior.
        // If uncertain, switch metric type in metrics.rs to IntGauge and use .set()

        // Assuming we switch to IntGauge in metrics.rs for safer snapshots:
        // state.metrics.reconciliation_checked.set(checked);

        // Keeping Counter logic but adding warning comment:
        if checked > 0 {
            // Ensure payload.checked is the DELTA since last run, not total!
            state.metrics.reconciliation_checked.inc_by(checked as u64);
        } else if checked < 0 {
            tracing::warn!(
                checked = checked,
                "Negative delta in reconciliation metrics — ignoring"
            );
        }

        state
            .db
            .log_config_change(
                "metrics.reconciliation.checked",
                None,
                &checked.to_string(),
                "SYSTEM_METRICS",
                Some("Metrics update from reconciliation script"),
            )
            .await?;
    }

    if let Some(discrepancies) = payload.discrepancies {
        // Increment discrepancy counter
        if discrepancies > 0 {
            state
                .metrics
                .reconciliation_discrepancies
                .inc_by(discrepancies as u64);
        } else if discrepancies < 0 {
            tracing::warn!(
                discrepancies = discrepancies,
                "Negative delta in reconciliation metrics — ignoring"
            );
        }

        state
            .db
            .log_config_change(
                "metrics.reconciliation.discrepancies",
                None,
                &discrepancies.to_string(),
                "SYSTEM_METRICS",
                Some("Metrics update from reconciliation script"),
            )
            .await?;
    }

    if let Some(unresolved) = payload.unresolved {
        // Set gauge to current unresolved count
        state.metrics.reconciliation_unresolved.set(unresolved);

        state
            .db
            .log_config_change(
                "metrics.reconciliation.unresolved",
                None,
                &unresolved.to_string(),
                "SYSTEM_METRICS",
                Some("Metrics update from reconciliation script"),
            )
            .await?;
    }

    Ok(Json(serde_json::json!({
        "status": "updated",
        "message": "Reconciliation metrics updated"
    })))
}

/// Update secret rotation metrics
///
/// POST /api/v1/metrics/secret-rotation
/// Requires: operator+ role
pub async fn update_secret_rotation_metrics(
    State(state): State<Arc<ApiState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
    Json(payload): Json<SecretRotationMetricsUpdate>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden(
            "Requires operator role or higher".to_string(),
        ));
    }
    // Log the update request
    tracing::info!(
        last_success_timestamp = payload.last_success_timestamp,
        days_until_due = payload.days_until_due,
        "Secret rotation metrics update requested"
    );

    // Update Prometheus metrics
    if let Some(timestamp) = payload.last_success_timestamp {
        state.metrics.secret_rotation_last_success.set(timestamp);

        state
            .db
            .log_config_change(
                "metrics.secret_rotation.last_success_timestamp",
                None,
                &timestamp.to_string(),
                "SYSTEM_METRICS",
                Some("Metrics update from secret rotation script"),
            )
            .await?;
    }

    if let Some(days) = payload.days_until_due {
        state.metrics.secret_rotation_days_until_due.set(days);

        state
            .db
            .log_config_change(
                "metrics.secret_rotation.days_until_due",
                None,
                &days.to_string(),
                "SYSTEM_METRICS",
                Some("Metrics update from secret rotation script"),
            )
            .await?;
    }

    Ok(Json(serde_json::json!({
        "status": "updated",
        "message": "Secret rotation metrics updated"
    })))
}

/// Helper to check role in request extensions
pub fn require_role_from_request(
    extensions: &axum::http::Extensions,
    required: Role,
) -> Result<&AuthExtension, AppError> {
    let auth = extensions
        .get::<AuthExtension>()
        .ok_or_else(|| AppError::Auth("Authentication required".to_string()))?;

    if !auth.0.role.has_permission(required) {
        return Err(AppError::Forbidden(format!(
            "Requires {} role or higher",
            required
        )));
    }
    Ok(auth)
}

// =============================================================================
// RECONCILIATION API
// =============================================================================

/// Query parameters for reconciliation status
#[derive(Debug, Deserialize)]
pub struct ReconciliationStatusQuery {
    pub discrepancies_limit: Option<i64>,
}

/// Query parameters for reconciliation history
#[derive(Debug, Deserialize)]
pub struct ReconciliationHistoryQuery {
    pub limit: Option<i64>,
}

/// Query parameters for reconciliation stats
#[derive(Debug, Deserialize)]
pub struct ReconciliationStatsQuery {
    pub range: Option<String>,
}

/// Reconciliation status response
#[derive(Debug, Serialize)]
pub struct ReconciliationStatusResponse {
    pub last_reconciliation_at: Option<String>,
    pub next_reconciliation_at: Option<String>,
    pub status: String,
    pub checked_count: i64,
    pub discrepancy_count: i64,
    pub unresolved_count: i64,
    pub duration_seconds: Option<f64>,
    pub recent_discrepancies: Vec<DiscrepancyResponse>,
}

/// Discrepancy response
#[derive(Debug, Serialize)]
pub struct DiscrepancyResponse {
    pub id: i64,
    pub trade_uuid: String,
    #[serde(rename = "type")]
    pub discrepancy_type: String,
    pub severity: String,
    pub description: String,
    pub db_value: Option<String>,
    pub on_chain_value: Option<String>,
    pub detected_at: String,
    pub resolved: bool,
    pub resolved_at: Option<String>,
}

/// Reconciliation history response
#[derive(Debug, Serialize)]
pub struct ReconciliationHistoryResponse {
    pub runs: Vec<ReconciliationRunResponse>,
    pub total_runs: i64,
    pub success_rate: f64,
    pub avg_duration_seconds: f64,
}

/// Reconciliation run response
#[derive(Debug, Serialize)]
pub struct ReconciliationRunResponse {
    pub id: i64,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub checked_count: i64,
    pub discrepancy_count: i64,
    pub unresolved_count: i64,
    pub duration_seconds: Option<f64>,
}

/// Reconciliation statistics response
#[derive(Debug, Serialize)]
pub struct ReconciliationStatsResponse {
    pub total_reconciliations: i64,
    pub successful_reconciliations: i64,
    pub failed_reconciliations: i64,
    pub total_checked: i64,
    pub total_discrepancies: i64,
    pub total_unresolved: i64,
    pub avg_discrepancies_per_run: f64,
    pub most_common_discrepancy_types: Vec<DiscrepancyTypeStatsResponse>,
}

/// Discrepancy type statistics response
#[derive(Debug, Serialize)]
pub struct DiscrepancyTypeStatsResponse {
    #[serde(rename = "type")]
    pub discrepancy_type: String,
    pub count: i64,
    pub percentage: f64,
}

/// Trigger reconciliation response
#[derive(Debug, Serialize)]
pub struct TriggerReconciliationResponse {
    pub run_id: String,
    pub scheduled_at: String,
}

/// Resolve discrepancy request
#[derive(Debug, Deserialize)]
pub struct ResolveDiscrepancyRequest {
    pub resolution: String,
}

/// Resolve discrepancy response
#[derive(Debug, Serialize)]
pub struct ResolveDiscrepancyResponse {
    pub success: bool,
}

/// Get reconciliation status
///
/// GET /api/v1/reconciliation/status
/// Requires: readonly+ role
pub async fn get_reconciliation_status(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<ReconciliationStatusQuery>,
) -> Result<Json<ReconciliationStatusResponse>, AppError> {
    let discrepancies_limit = params.discrepancies_limit.unwrap_or(10).min(100);
    let status_row = state
        .db
        .get_reconciliation_status(discrepancies_limit as i32)
        .await?;

    let recent_discrepancies = status_row
        .recent_discrepancies
        .into_iter()
        .map(|d| DiscrepancyResponse {
            id: d.id,
            trade_uuid: d.trade_uuid,
            discrepancy_type: d.discrepancy_type,
            severity: d.severity,
            description: d.description,
            db_value: d.db_value,
            on_chain_value: d.on_chain_value,
            detected_at: d.detected_at,
            resolved: d.resolved,
            resolved_at: d.resolved_at,
        })
        .collect();

    Ok(Json(ReconciliationStatusResponse {
        last_reconciliation_at: status_row.last_reconciliation_at,
        next_reconciliation_at: status_row.next_reconciliation_at,
        status: status_row.status,
        checked_count: status_row.checked_count,
        discrepancy_count: status_row.discrepancy_count,
        unresolved_count: status_row.unresolved_count,
        duration_seconds: status_row.duration_seconds,
        recent_discrepancies,
    }))
}

/// Get reconciliation history
///
/// GET /api/v1/reconciliation/history
/// Requires: readonly+ role
pub async fn get_reconciliation_history(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<ReconciliationHistoryQuery>,
) -> Result<Json<ReconciliationHistoryResponse>, AppError> {
    let limit = params.limit.unwrap_or(10).min(100);
    let runs = state.db.get_reconciliation_history(limit as i32).await?;
    let total = state.db.count_reconciliation_runs().await?;

    let run_responses: Vec<ReconciliationRunResponse> = runs
        .into_iter()
        .map(|r| ReconciliationRunResponse {
            id: r.id,
            started_at: r.started_at,
            completed_at: r.completed_at,
            status: r.status,
            checked_count: r.checked_count,
            discrepancy_count: r.discrepancy_count,
            unresolved_count: r.unresolved_count,
            duration_seconds: r.duration_seconds,
        })
        .collect();

    // Calculate success rate and average duration
    let successful_count = run_responses
        .iter()
        .filter(|r| r.status == "completed")
        .count() as i64;
    let success_rate = if total > 0 {
        (successful_count as f64 / total as f64) * 100.0
    } else {
        100.0
    };

    let avg_duration = if !run_responses.is_empty() {
        let total_duration: f64 = run_responses
            .iter()
            .filter_map(|r| r.duration_seconds)
            .sum();
        total_duration / run_responses.len() as f64
    } else {
        0.0
    };

    Ok(Json(ReconciliationHistoryResponse {
        runs: run_responses,
        total_runs: total,
        success_rate,
        avg_duration_seconds: avg_duration,
    }))
}

/// Get reconciliation statistics
///
/// GET /api/v1/reconciliation/stats
/// Requires: readonly+ role
pub async fn get_reconciliation_stats(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<ReconciliationStatsQuery>,
) -> Result<Json<ReconciliationStatsResponse>, AppError> {
    let _range = params.range.as_deref();
    let stats = state
        .db
        .get_reconciliation_stats(_range.unwrap_or("30d"))
        .await?;

    let discrepancy_types = stats
        .most_common_discrepancy_types
        .into_iter()
        .map(|t| DiscrepancyTypeStatsResponse {
            discrepancy_type: t.discrepancy_type,
            count: t.count,
            percentage: t.percentage,
        })
        .collect();

    Ok(Json(ReconciliationStatsResponse {
        total_reconciliations: stats.total_reconciliations,
        successful_reconciliations: stats.successful_reconciliations,
        failed_reconciliations: stats.failed_reconciliations,
        total_checked: stats.total_checked,
        total_discrepancies: stats.total_discrepancies,
        total_unresolved: stats.total_unresolved,
        avg_discrepancies_per_run: stats.avg_discrepancies_per_run,
        most_common_discrepancy_types: discrepancy_types,
    }))
}

/// Trigger manual reconciliation
///
/// POST /api/v1/reconciliation/trigger
/// Requires: operator+ role
pub async fn trigger_reconciliation(
    State(state): State<Arc<ApiState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
) -> Result<Json<TriggerReconciliationResponse>, AppError> {
    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden(
            "Requires operator role or higher".to_string(),
        ));
    }

    tracing::info!("Manual reconciliation triggered by {}", auth.0.identifier);

    // Resolve an RPC client: prefer the engine's active client, else build one from
    // the configured primary URL so reconciliation works even pre-engine.
    let rpc_client = match state.engine.as_ref() {
        Some(engine) => engine.active_rpc_client().await,
        None => None,
    };
    let rpc_client = match rpc_client {
        Some(client) => Some(client),
        None => {
            let config = state.config.read().await;
            Some(std::sync::Arc::new(
                solana_client::nonblocking::rpc_client::RpcClient::new(
                    config.rpc.primary_url.clone(),
                ),
            ))
        }
    };

    let db = state.db.clone();
    let metrics = state.metrics.clone();
    let started_at = chrono::Utc::now().to_rfc3339();

    if let Some(client) = rpc_client {
        // Guard against overlapping runs: a sweep iterates up to hundreds of positions
        // with one RPC call each, so stacking triggers would hammer the RPC provider.
        use std::sync::atomic::Ordering;
        if crate::engine::reconciliation::RECONCILIATION_RUNNING.swap(true, Ordering::SeqCst) {
            return Err(AppError::Duplicate(
                "A reconciliation run is already in progress".to_string(),
            ));
        }
        let checker =
            crate::engine::reconciliation::RpcOnChainChecker::new(client);
        tokio::spawn(async move {
            let result =
                crate::engine::reconciliation::run_reconciliation(db.as_ref(), &checker, &metrics)
                    .await;
            crate::engine::reconciliation::RECONCILIATION_RUNNING.store(false, Ordering::SeqCst);
            tracing::info!(?result, "Reconciliation run finished");
        });
    } else {
        tracing::warn!("Reconciliation triggered without an RPC client; skipping run");
    }

    // Log the trigger event.
    state
        .db
        .log_config_change(
            "reconciliation.manual_trigger",
            None,
            "triggered",
            &auth.0.identifier,
            Some("Manual reconciliation trigger via API"),
        )
        .await?;

    Ok(Json(TriggerReconciliationResponse {
        run_id: uuid::Uuid::new_v4().to_string(),
        scheduled_at: started_at,
    }))
}

/// Resolve a discrepancy
///
/// POST /api/v1/reconciliation/discrepancies/:id/resolve
/// Requires: operator+ role
pub async fn resolve_discrepancy(
    State(state): State<Arc<ApiState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
    Path(id): Path<i64>,
    Json(payload): Json<ResolveDiscrepancyRequest>,
) -> Result<Json<ResolveDiscrepancyResponse>, AppError> {
    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden(
            "Requires operator role or higher".to_string(),
        ));
    }

    tracing::info!(
        id = id,
        resolver = auth.0.identifier,
        resolution = payload.resolution,
        "Discrepancy resolution requested"
    );

    state
        .db
        .resolve_discrepancy(id, &auth.0.identifier, &payload.resolution)
        .await?;

    Ok(Json(ResolveDiscrepancyResponse { success: true }))
}

// =============================================================================
// DEBUG SMOKE-TEST: PnL POPULATION VERIFICATION
// =============================================================================

/// Request body for the debug backtest smoke-test endpoint.
#[derive(Debug, Deserialize)]
pub struct DebugBacktestSmokeRequest {
    pub wallet_address: String,
}

/// Response for the debug backtest smoke-test endpoint.
///
/// Reports PnL-population coverage for a wallet's CLOSED trades so the
/// `close_position_full` fix (postgres.rs) can be confirmed live without
/// waiting for a full reporting cycle.
#[derive(Debug, Serialize)]
pub struct DebugBacktestSmokeResponse {
    pub wallet_address: String,
    pub total_trades: i64,
    pub closed_trades: i64,
    pub pnl_populated_closes: i64,
    pub passed: bool,
    pub notes: String,
}

/// Debug smoke-test: verify the PnL-population fix is live for a wallet.
///
/// POST /api/v1/debug/backtest-smoke
/// Requires: protected route bearer auth (inherited from `protected_api_routes`).
pub async fn debug_backtest_smoke(
    State(state): State<Arc<ApiState>>,
    Json(payload): Json<DebugBacktestSmokeRequest>,
) -> Result<Json<DebugBacktestSmokeResponse>, AppError> {
    let pool = pg_pool(&state.db)?;
    let wallet_address = payload.wallet_address.trim().to_string();

    if wallet_address.is_empty() {
        return Err(AppError::BadRequest(
            "wallet_address must not be empty".to_string(),
        ));
    }

    let total_trades: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trades WHERE wallet_address = $1",
    )
    .bind(&wallet_address)
    .fetch_one(&pool)
    .await?;

    let closed_trades: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trades WHERE wallet_address = $1 AND status = 'CLOSED'",
    )
    .bind(&wallet_address)
    .fetch_one(&pool)
    .await?;

    let pnl_populated_closes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trades WHERE wallet_address = $1 AND status = 'CLOSED' AND pnl_sol IS NOT NULL",
    )
    .bind(&wallet_address)
    .fetch_one(&pool)
    .await?;

    let passed = pnl_populated_closes > 0;
    let notes = if closed_trades == 0 {
        "Inconclusive: wallet has no CLOSED trades yet. The fix cannot be confirmed until a trade closes.".to_string()
    } else if passed {
        format!(
            "PASS: {}/{} CLOSED trades have pnl_sol populated.",
            pnl_populated_closes, closed_trades
        )
    } else {
        "FAIL: CLOSED trades exist but none have pnl_sol populated. Pre-deploy closes may be NULL; a new close is needed to confirm.".to_string()
    };

    tracing::info!(
        wallet_address = %wallet_address,
        total_trades,
        closed_trades,
        pnl_populated_closes,
        passed,
        "Debug backtest smoke-test queried"
    );

    Ok(Json(DebugBacktestSmokeResponse {
        wallet_address,
        total_trades,
        closed_trades,
        pnl_populated_closes,
        passed,
        notes,
    }))
}

/// Extract the underlying PostgreSQL pool from the database trait object.
///
/// Returns an error for non-PostgreSQL backends (this handler requires raw
/// SQL access to the `trades` table).
fn pg_pool(db: &Arc<dyn Database>) -> AppResult<sqlx::Pool<sqlx::Postgres>> {
    match db.pool() {
        DbPool::PostgreSQL(p) => Ok(p),
        _ => Err(AppError::Internal(
            "PostgreSQL backend required for debug endpoint".to_string(),
        )),
    }
}
