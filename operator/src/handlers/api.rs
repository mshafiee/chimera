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
    pub jito_tip_strategy: JitoTipConfig,
    pub jito_enabled: bool,
    pub rpc_status: RpcStatus,
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

/// Request body for config update
#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
    pub circuit_breakers: Option<UpdateCircuitBreakerConfig>,
    pub strategy_allocation: Option<UpdateStrategyAllocation>,
    pub notification_rules: Option<UpdateNotificationRulesConfig>,
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
            max_loss_24h: config.circuit_breakers.max_loss_24h_usd,
            max_consecutive_losses: config.circuit_breakers.max_consecutive_losses,
            max_drawdown_percent: config.circuit_breakers.max_drawdown_percent,
            cool_down_minutes: config.circuit_breakers.cooldown_minutes,
        },
        strategy_allocation: StrategyAllocation {
            shield_percent: config.strategy.shield_percent,
            spear_percent: config.strategy.spear_percent,
        },
        jito_tip_strategy: JitoTipConfig {
            tip_floor: config.jito.tip_floor_sol,
            tip_ceiling: config.jito.tip_ceiling_sol,
            tip_percentile: config.jito.tip_percentile,
            tip_percent_max: config.jito.tip_percent_max,
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
            let old = config.circuit_breakers.max_loss_24h_usd;
            config.circuit_breakers.max_loss_24h_usd = v;
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
            let old = config.circuit_breakers.max_drawdown_percent;
            config.circuit_breakers.max_drawdown_percent = v;
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
    pub pnl_24h: f64,
    pub pnl_7d: f64,
    pub pnl_30d: f64,
    pub pnl_24h_change_percent: Option<f64>,
    pub pnl_7d_change_percent: Option<f64>,
    pub pnl_30d_change_percent: Option<f64>,
}

/// Strategy performance response
#[derive(Debug, Serialize)]
pub struct StrategyPerformanceResponse {
    pub strategy: String,
    pub win_rate: f64,
    pub avg_return: f64,
    pub trade_count: u32,
    pub total_pnl: f64,
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
        .sum::<f64>();

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
        // For counters, scripts typically send the total checked in this run
        // We increment by that amount (assuming it's a delta from last update)
        if checked > 0 {
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
