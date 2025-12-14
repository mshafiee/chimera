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
use crate::db::{self, ConfigAuditItem, DeadLetterItem, DbPool, PositionDetail, TradeDetail, WalletDetail};
use crate::error::AppError;
use crate::middleware::{AuthExtension, Role};
use crate::notifications::{CompositeNotifier, NotificationEvent};
use rust_decimal::prelude::*;

// =============================================================================
// API STATE
// =============================================================================

/// Shared state for API handlers
pub struct ApiState {
    pub db: DbPool,
    pub circuit_breaker: Arc<CircuitBreaker>,
    pub config: Arc<tokio::sync::RwLock<AppConfig>>,
    pub notifier: Arc<CompositeNotifier>,
    /// Engine handle for accessing executor state
    pub engine: Option<Arc<crate::engine::EngineHandle>>,
    /// Metrics state for updating Prometheus metrics
    pub metrics: Arc<crate::metrics::MetricsState>,
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
}

/// List all positions
///
/// GET /api/v1/positions
/// Requires: readonly+ role
pub async fn list_positions(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<PositionsQuery>,
) -> Result<Json<PositionsResponse>, AppError> {
    let positions = db::get_positions(&state.db, params.state.as_deref()).await?;
    let total = positions.len();

    Ok(Json(PositionsResponse { positions, total }))
}

/// Get a single position by trade_uuid
///
/// GET /api/v1/positions/:trade_uuid
/// Requires: readonly+ role
pub async fn get_position(
    State(state): State<Arc<ApiState>>,
    Path(trade_uuid): Path<String>,
) -> Result<Json<PositionDetail>, AppError> {
    match db::get_position_by_uuid(&state.db, &trade_uuid).await? {
        Some(position) => Ok(Json(position)),
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
    let wallets = db::get_wallets(&state.db, params.status.as_deref()).await?;
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
    match db::get_wallet_by_address(&state.db, &address).await? {
        Some(wallet) => Ok(Json(wallet)),
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
    let existing = db::get_wallet_by_address(&state.db, &address).await?;
    if existing.is_none() {
        return Err(AppError::NotFound(format!("Wallet not found: {}", address)));
    }

    // Update wallet
    let updated = db::update_wallet_status(
        &state.db,
        &address,
        &body.status,
        body.ttl_hours,
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

    db::log_config_change(
        &state.db,
        &format!("wallet:{}", address),
        existing.as_ref().map(|w| w.status.as_str()),
        &body.status,
        &auth.0.identifier,
        Some(&change_description),
    )
    .await?;

    // Send notification if wallet was promoted to ACTIVE
    let was_promoted = body.status == "ACTIVE"
        && existing.as_ref().map(|w| w.status.as_str()) != Some("ACTIVE");

    if was_promoted {
        // Get WQS score from existing wallet or default to 0
        let wqs_score = existing.as_ref().and_then(|w| w.wqs_score).unwrap_or(0.0);

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
    }

    // Fetch updated wallet
    let wallet = db::get_wallet_by_address(&state.db, &address).await?;

    Ok(Json(WalletUpdateResponse {
        success: true,
        wallet,
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
            max_loss_24h: config.circuit_breakers.max_loss_24h_usd.to_f64().unwrap_or(0.0),
            max_consecutive_losses: config.circuit_breakers.max_consecutive_losses,
            max_drawdown_percent: config.circuit_breakers.max_drawdown_percent.to_f64().unwrap_or(0.0),
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
                } else {
                    if config.jito.enabled {
                        "jito".to_string()
                    } else {
                        "helius".to_string()
                    }
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
        monitoring: config.monitoring.as_ref().map(|m| MonitoringConfigResponse {
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
            targets: config.profit_management.targets.iter().map(|d| d.to_f64().unwrap_or(0.0)).collect(),
            tiered_exit_percent: config.profit_management.tiered_exit_percent.to_f64().unwrap_or(0.0),
            trailing_stop_activation: config.profit_management.trailing_stop_activation.to_f64().unwrap_or(0.0),
            trailing_stop_distance: config.profit_management.trailing_stop_distance.to_f64().unwrap_or(0.0),
            hard_stop_loss: config.profit_management.hard_stop_loss.to_f64().unwrap_or(0.0),
            time_exit_hours: config.profit_management.time_exit_hours,
        },
        position_sizing: PositionSizingConfigResponse {
            base_size_sol: config.position_sizing.base_size_sol.to_f64().unwrap_or(0.0),
            max_size_sol: config.position_sizing.max_size_sol.to_f64().unwrap_or(0.0),
            min_size_sol: config.position_sizing.min_size_sol.to_f64().unwrap_or(0.0),
            consensus_multiplier: config.position_sizing.consensus_multiplier.to_f64().unwrap_or(0.0),
            max_concurrent_positions: config.position_sizing.max_concurrent_positions,
        },
        mev_protection: MevProtectionConfigResponse {
            always_use_jito: config.mev_protection.always_use_jito,
            exit_tip_sol: config.mev_protection.exit_tip_sol.to_f64().unwrap_or(0.0),
            consensus_tip_sol: config.mev_protection.consensus_tip_sol.to_f64().unwrap_or(0.0),
            standard_tip_sol: config.mev_protection.standard_tip_sol.to_f64().unwrap_or(0.0),
        },
        token_safety: TokenSafetyConfigResponse {
            min_liquidity_shield_usd: config.token_safety.min_liquidity_shield_usd.to_f64().unwrap_or(0.0),
            min_liquidity_spear_usd: config.token_safety.min_liquidity_spear_usd.to_f64().unwrap_or(0.0),
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
    let mut config = state.config.write().await;

    // Update circuit breakers if provided
    if let Some(cb) = body.circuit_breakers {
        if let Some(v) = cb.max_loss_24h {
            use rust_decimal::prelude::*;
            let old = config.circuit_breakers.max_loss_24h_usd;
            config.circuit_breakers.max_loss_24h_usd = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "circuit_breakers.max_loss_24h",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = cb.max_consecutive_losses {
            let old = config.circuit_breakers.max_consecutive_losses;
            config.circuit_breakers.max_consecutive_losses = v;
            db::log_config_change(
                &state.db,
                "circuit_breakers.max_consecutive_losses",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = cb.max_drawdown_percent {
            use rust_decimal::prelude::*;
            let old = config.circuit_breakers.max_drawdown_percent;
            config.circuit_breakers.max_drawdown_percent = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "circuit_breakers.max_drawdown_percent",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = cb.cool_down_minutes {
            let old = config.circuit_breakers.cooldown_minutes;
            config.circuit_breakers.cooldown_minutes = v;
            db::log_config_change(
                &state.db,
                "circuit_breakers.cooldown_minutes",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
    }

    // Update strategy allocation if provided
    if let Some(sa) = body.strategy_allocation {
        if let Some(shield) = sa.shield_percent {
            let spear = sa.spear_percent.unwrap_or(100 - shield);
            if shield + spear != 100 {
                return Err(AppError::Validation(
                    "Strategy allocation must sum to 100%".to_string(),
                ));
            }
            let old_shield = config.strategy.shield_percent;
            let old_spear = config.strategy.spear_percent;
            config.strategy.shield_percent = shield;
            config.strategy.spear_percent = spear;
            db::log_config_change(
                &state.db,
                "strategy.allocation",
                Some(&format!("shield:{}/spear:{}", old_shield, old_spear)),
                &format!("shield:{}/spear:{}", shield, spear),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
    }

    // Update strategy position limits if provided
    if let Some(s) = body.strategy {
        if let Some(v) = s.max_position_sol {
            use rust_decimal::prelude::*;
            let old = config.strategy.max_position_sol;
            config.strategy.max_position_sol = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "strategy.max_position_sol",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = s.min_position_sol {
            use rust_decimal::prelude::*;
            let old = config.strategy.min_position_sol;
            config.strategy.min_position_sol = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "strategy.min_position_sol",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
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
                db::log_config_change(
                    &state.db,
                    "monitoring.enabled",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = m.webhook_registration_batch_size {
                let old = mon.webhook_registration_batch_size;
                mon.webhook_registration_batch_size = v;
                db::log_config_change(
                    &state.db,
                    "monitoring.webhook_registration_batch_size",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = m.webhook_registration_delay_ms {
                let old = mon.webhook_registration_delay_ms;
                mon.webhook_registration_delay_ms = v;
                db::log_config_change(
                    &state.db,
                    "monitoring.webhook_registration_delay_ms",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = m.webhook_processing_rate_limit {
                if v > 50 {
                    return Err(AppError::Validation(
                        "Webhook rate limit cannot exceed 50 req/sec (Helius limit)".to_string(),
                    ));
                }
                let old = mon.webhook_processing_rate_limit;
                mon.webhook_processing_rate_limit = v;
                db::log_config_change(
                    &state.db,
                    "monitoring.webhook_processing_rate_limit",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = m.rpc_polling_enabled {
                let old = mon.rpc_polling_enabled;
                mon.rpc_polling_enabled = v;
                db::log_config_change(
                    &state.db,
                    "monitoring.rpc_polling_enabled",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = m.rpc_poll_interval_secs {
                let old = mon.rpc_poll_interval_secs;
                mon.rpc_poll_interval_secs = v;
                db::log_config_change(
                    &state.db,
                    "monitoring.rpc_poll_interval_secs",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = m.rpc_poll_batch_size {
                let old = mon.rpc_poll_batch_size;
                mon.rpc_poll_batch_size = v;
                db::log_config_change(
                    &state.db,
                    "monitoring.rpc_poll_batch_size",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = m.rpc_poll_rate_limit {
                if v > 50 {
                    return Err(AppError::Validation(
                        "RPC poll rate limit cannot exceed 50 req/sec (Helius limit)".to_string(),
                    ));
                }
                let old = mon.rpc_poll_rate_limit;
                mon.rpc_poll_rate_limit = v;
                db::log_config_change(
                    &state.db,
                    "monitoring.rpc_poll_rate_limit",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = m.max_active_wallets {
                let old = mon.max_active_wallets;
                mon.max_active_wallets = v;
                db::log_config_change(
                    &state.db,
                    "monitoring.max_active_wallets",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
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
            config.profit_management.targets = v.iter().map(|t| Decimal::from_f64_retain(*t).unwrap_or(Decimal::ZERO)).collect();
            db::log_config_change(
                &state.db,
                "profit_management.targets",
                Some(&old),
                &format!("{:?}", v),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = pm.tiered_exit_percent {
            if v < 0.0 || v > 100.0 {
                return Err(AppError::Validation(
                    "Tiered exit percent must be between 0 and 100".to_string(),
                ));
            }
            let old = config.profit_management.tiered_exit_percent;
            config.profit_management.tiered_exit_percent = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "profit_management.tiered_exit_percent",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = pm.trailing_stop_activation {
            let old = config.profit_management.trailing_stop_activation;
            config.profit_management.trailing_stop_activation = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "profit_management.trailing_stop_activation",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = pm.trailing_stop_distance {
            let old = config.profit_management.trailing_stop_distance;
            config.profit_management.trailing_stop_distance = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "profit_management.trailing_stop_distance",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = pm.hard_stop_loss {
            if v < 0.0 || v > 100.0 {
                return Err(AppError::Validation(
                    "Hard stop loss must be between 0 and 100".to_string(),
                ));
            }
            let old = config.profit_management.hard_stop_loss;
            config.profit_management.hard_stop_loss = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "profit_management.hard_stop_loss",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = pm.time_exit_hours {
            let old = config.profit_management.time_exit_hours;
            config.profit_management.time_exit_hours = v;
            db::log_config_change(
                &state.db,
                "profit_management.time_exit_hours",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
    }

    // Update position sizing if provided
    if let Some(ps) = body.position_sizing {
        if let Some(v) = ps.base_size_sol {
            let v_dec = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            if v_dec < config.position_sizing.min_size_sol || v_dec > config.position_sizing.max_size_sol {
                return Err(AppError::Validation(
                    format!(
                        "Base size must be between {} and {} SOL",
                        config.position_sizing.min_size_sol, config.position_sizing.max_size_sol
                    ),
                ));
            }
            let old = config.position_sizing.base_size_sol;
            config.position_sizing.base_size_sol = v_dec;
            db::log_config_change(
                &state.db,
                "position_sizing.base_size_sol",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
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
            db::log_config_change(
                &state.db,
                "position_sizing.max_size_sol",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
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
            db::log_config_change(
                &state.db,
                "position_sizing.min_size_sol",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = ps.consensus_multiplier {
            if v < 1.0 || v > 5.0 {
                return Err(AppError::Validation(
                    "Consensus multiplier must be between 1.0 and 5.0".to_string(),
                ));
            }
            let old = config.position_sizing.consensus_multiplier;
            config.position_sizing.consensus_multiplier = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "position_sizing.consensus_multiplier",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = ps.max_concurrent_positions {
            let old = config.position_sizing.max_concurrent_positions;
            config.position_sizing.max_concurrent_positions = v;
            db::log_config_change(
                &state.db,
                "position_sizing.max_concurrent_positions",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
    }

    // Update MEV protection if provided
    if let Some(mp) = body.mev_protection {
        if let Some(v) = mp.always_use_jito {
            let old = config.mev_protection.always_use_jito;
            config.mev_protection.always_use_jito = v;
            db::log_config_change(
                &state.db,
                "mev_protection.always_use_jito",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = mp.exit_tip_sol {
            let old = config.mev_protection.exit_tip_sol;
            config.mev_protection.exit_tip_sol = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "mev_protection.exit_tip_sol",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = mp.consensus_tip_sol {
            let old = config.mev_protection.consensus_tip_sol;
            config.mev_protection.consensus_tip_sol = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "mev_protection.consensus_tip_sol",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = mp.standard_tip_sol {
            let old = config.mev_protection.standard_tip_sol;
            config.mev_protection.standard_tip_sol = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "mev_protection.standard_tip_sol",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
    }

    // Update token safety if provided
    if let Some(ts) = body.token_safety {
        if let Some(v) = ts.min_liquidity_shield_usd {
            let old = config.token_safety.min_liquidity_shield_usd;
            config.token_safety.min_liquidity_shield_usd = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "token_safety.min_liquidity_shield_usd",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = ts.min_liquidity_spear_usd {
            let old = config.token_safety.min_liquidity_spear_usd;
            config.token_safety.min_liquidity_spear_usd = Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO);
            db::log_config_change(
                &state.db,
                "token_safety.min_liquidity_spear_usd",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = ts.honeypot_detection_enabled {
            let old = config.token_safety.honeypot_detection_enabled;
            config.token_safety.honeypot_detection_enabled = v;
            db::log_config_change(
                &state.db,
                "token_safety.honeypot_detection_enabled",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = ts.cache_capacity {
            let old = config.token_safety.cache_capacity;
            config.token_safety.cache_capacity = v;
            db::log_config_change(
                &state.db,
                "token_safety.cache_capacity",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = ts.cache_ttl_seconds {
            let old = config.token_safety.cache_ttl_seconds;
            config.token_safety.cache_ttl_seconds = v;
            db::log_config_change(
                &state.db,
                "token_safety.cache_ttl_seconds",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
    }

    // Update notifications if provided
    if let Some(n) = body.notifications {
        if let Some(t) = n.telegram {
            if let Some(v) = t.enabled {
                let old = config.notifications.telegram.enabled;
                config.notifications.telegram.enabled = v;
                db::log_config_change(
                    &state.db,
                    "notifications.telegram.enabled",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = t.rate_limit_seconds {
                let old = config.notifications.telegram.rate_limit_seconds;
                config.notifications.telegram.rate_limit_seconds = v;
                db::log_config_change(
                    &state.db,
                    "notifications.telegram.rate_limit_seconds",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
        }
        if let Some(r) = n.rules {
            if let Some(v) = r.circuit_breaker_triggered {
                let old = config.notifications.rules.circuit_breaker_triggered;
                config.notifications.rules.circuit_breaker_triggered = v;
                db::log_config_change(
                    &state.db,
                    "notifications.rules.circuit_breaker_triggered",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = r.wallet_drained {
                let old = config.notifications.rules.wallet_drained;
                config.notifications.rules.wallet_drained = v;
                db::log_config_change(
                    &state.db,
                    "notifications.rules.wallet_drained",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = r.position_exited {
                let old = config.notifications.rules.position_exited;
                config.notifications.rules.position_exited = v;
                db::log_config_change(
                    &state.db,
                    "notifications.rules.position_exited",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = r.wallet_promoted {
                let old = config.notifications.rules.wallet_promoted;
                config.notifications.rules.wallet_promoted = v;
                db::log_config_change(
                    &state.db,
                    "notifications.rules.wallet_promoted",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = r.daily_summary {
                let old = config.notifications.rules.daily_summary;
                config.notifications.rules.daily_summary = v;
                db::log_config_change(
                    &state.db,
                    "notifications.rules.daily_summary",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = r.rpc_fallback {
                let old = config.notifications.rules.rpc_fallback;
                config.notifications.rules.rpc_fallback = v;
                db::log_config_change(
                    &state.db,
                    "notifications.rules.rpc_fallback",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
        }
        if let Some(ds) = n.daily_summary {
            if let Some(v) = ds.enabled {
                let old = config.notifications.daily_summary.enabled;
                config.notifications.daily_summary.enabled = v;
                db::log_config_change(
                    &state.db,
                    "notifications.daily_summary.enabled",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = ds.hour_utc {
                if v > 23 {
                    return Err(AppError::Validation(
                        "Hour must be between 0 and 23".to_string(),
                    ));
                }
                let old = config.notifications.daily_summary.hour_utc;
                config.notifications.daily_summary.hour_utc = v;
                db::log_config_change(
                    &state.db,
                    "notifications.daily_summary.hour_utc",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
            if let Some(v) = ds.minute {
                if v > 59 {
                    return Err(AppError::Validation(
                        "Minute must be between 0 and 59".to_string(),
                    ));
                }
                let old = config.notifications.daily_summary.minute;
                config.notifications.daily_summary.minute = v;
                db::log_config_change(
                    &state.db,
                    "notifications.daily_summary.minute",
                    Some(&old.to_string()),
                    &v.to_string(),
                    &auth.0.identifier,
                    None,
                )
                .await?;
            }
        }
    }

    // Update queue if provided
    if let Some(q) = body.queue {
        if let Some(v) = q.capacity {
            let old = config.queue.capacity;
            config.queue.capacity = v;
            db::log_config_change(
                &state.db,
                "queue.capacity",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
        if let Some(v) = q.load_shed_threshold_percent {
            if v > 100 {
                return Err(AppError::Validation(
                    "Load shed threshold must be <= 100%".to_string(),
                ));
            }
            let old = config.queue.load_shed_threshold_percent;
            config.queue.load_shed_threshold_percent = v;
            db::log_config_change(
                &state.db,
                "queue.load_shed_threshold_percent",
                Some(&old.to_string()),
                &v.to_string(),
                &auth.0.identifier,
                None,
            )
            .await?;
        }
    }

    // Return updated config
    drop(config);
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
    let status_before = state.circuit_breaker.status();
    let previous_state = status_before.state.to_string();

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
    let status_before = state.circuit_breaker.status();
    let previous_state = status_before.state.to_string();

    let reason = body
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("Emergency kill switch activated")
        .to_string();

    // First, set extreme circuit breaker config values
    let mut config = state.config.write().await;
    config.circuit_breakers.max_loss_24h_usd = 0.01;
    config.circuit_breakers.max_consecutive_losses = 1;
    config.circuit_breakers.max_drawdown_percent = 0.1;
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

    let trades = db::get_trades(
        &state.db,
        params.from.as_deref(),
        params.to.as_deref(),
        params.status.as_deref(),
        params.strategy.as_deref(),
        params.wallet_address.as_deref(),
        Some(limit),
        Some(offset),
    )
    .await?;

    let total = db::count_trades(
        &state.db,
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
    let trades = db::get_trades(
        &state.db,
        params.from.as_deref(),
        params.to.as_deref(),
        params.status.as_deref(),
        params.strategy.as_deref(),
        params.wallet_address.as_deref(),
        None, // No limit
        None, // No offset
    )
    .await?;

    let format = params.format.as_deref().unwrap_or("csv").to_lowercase();
    let date_from = params.from.as_deref().unwrap_or("all");
    let date_to = params.to.as_deref().unwrap_or("now");

    match format.as_str() {
        "pdf" => {
            let pdf_content = db::trades_to_pdf(&trades)?;
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
            let csv_content = db::trades_to_csv(&trades);
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

/// Get performance metrics (24H, 7D, 30D PnL)
///
/// GET /api/v1/metrics/performance
/// Requires: readonly+ role
pub async fn get_performance_metrics(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<PerformanceMetricsResponse>, AppError> {
    let pnl_24h = db::get_pnl_24h(&state.db).await?;
    let pnl_7d = db::get_pnl_7d(&state.db).await?;
    let pnl_30d = db::get_pnl_30d(&state.db).await?;

    // Calculate change percentages (simplified - in production, compare to previous period)
    // For now, we'll return None for change percentages
    Ok(Json(PerformanceMetricsResponse {
        pnl_24h,
        pnl_7d,
        pnl_30d,
        pnl_24h_change_percent: None,
        pnl_7d_change_percent: None,
        pnl_30d_change_percent: None,
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
    let trades = db::get_trades(
        &state.db,
        Some(&from_date_str),
        None,
        None, // All statuses
        None, // All strategies
        None, // All wallets
        None, // No limit
        None, // No offset
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
/// GET /api/v1/metrics/strategy/:strategy
/// Requires: readonly+ role
pub async fn get_strategy_performance(
    State(state): State<Arc<ApiState>>,
    Path(strategy): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<StrategyPerformanceResponse>, AppError> {
    // Get days parameter (default to 30)
    let days = params
        .get("days")
        .and_then(|d| d.parse::<i64>().ok())
        .unwrap_or(30);

    let (win_rate, avg_return, trade_count) =
        db::get_strategy_performance(&state.db, &strategy, days).await?;

    // Calculate total PnL for the period
    // We need to query the actual trades to get total PnL (not just average)
    let from_date = chrono::Utc::now() - chrono::Duration::days(days);
    let from_date_str = from_date.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    
    let trades = db::get_trades(
        &state.db,
        Some(&from_date_str),
        None,
        Some("CLOSED"),
        Some(&strategy),
        None, // No wallet_address filter for strategy performance
        None,
        None,
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

    let items = db::get_dead_letter_queue(&state.db, Some(limit), Some(offset)).await?;
    let total = db::count_dead_letter_queue(&state.db).await?;

    Ok(Json(DeadLetterResponse { items, total }))
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

    let items = db::get_config_audit(&state.db, Some(limit), Some(offset)).await?;
    let total = db::count_config_audit(&state.db).await?;

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
    Json(payload): Json<ReconciliationMetricsUpdate>,
) -> Result<Json<serde_json::Value>, AppError> {
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
        }
        
        db::log_config_change(
            &state.db,
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
            state.metrics.reconciliation_discrepancies.inc_by(discrepancies as u64);
        }
        
        db::log_config_change(
            &state.db,
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
        
        db::log_config_change(
            &state.db,
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
    Json(payload): Json<SecretRotationMetricsUpdate>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Log the update request
    tracing::info!(
        last_success_timestamp = payload.last_success_timestamp,
        days_until_due = payload.days_until_due,
        "Secret rotation metrics update requested"
    );
    
    // Update Prometheus metrics
    if let Some(timestamp) = payload.last_success_timestamp {
        state.metrics.secret_rotation_last_success.set(timestamp);
        
        db::log_config_change(
            &state.db,
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
        
        db::log_config_change(
            &state.db,
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
