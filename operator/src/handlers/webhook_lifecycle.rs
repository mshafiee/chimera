//! Webhook Lifecycle Administrative API Endpoints
//!
//! Provides REST API endpoints for webhook lifecycle management including
//! statistics, bulk operations, manual reconciliation, and audit logging.

use crate::db_abstraction::{WebhookAuditLog, WebhookStats};
use crate::error::{AppError, AppResult};
use crate::middleware::AuthExtension;
use crate::monitoring::webhook_lifecycle::{
    BulkOperationResult, HealthCheckResult, ReconciliationResult, WebhookLifecycleManager,
};
use crate::monitoring::webhook_health_task::{get_webhook_statistics, manual_health_check as health_check_internal, manual_reconcile_webhooks as reconcile_internal};
use crate::monitoring::MonitoringState;
use crate::Role;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info, warn};

/// Helper to get webhook URL from monitoring config
fn get_webhook_url(state: &MonitoringState) -> String {
    state
        .config
        .monitoring
        .as_ref()
        .and_then(|m| m.helius_webhook_url.clone())
        .unwrap_or_else(|| String::from(""))
}

/// Webhook statistics response
#[derive(Debug, Serialize)]
pub struct WebhookStatsResponse {
    pub success: bool,
    pub data: Option<WebhookStats>,
    pub error: Option<String>,
}

/// Bulk register request
#[derive(Debug, Deserialize)]
pub struct BulkRegisterRequest {
    pub wallets: Vec<String>,
    #[serde(default)]
    pub force_recreate: bool,
}

/// Bulk cleanup request
#[derive(Debug, Deserialize)]
pub struct BulkCleanupRequest {
    pub wallets: Vec<String>,
}

/// Audit query parameters
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub wallet_address: Option<String>,
    pub action: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

/// Generic API response wrapper
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    pub message: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            message: None,
        }
    }

    pub fn success_with_message(data: T, message: String) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            message: Some(message),
        }
    }

    pub fn error(error: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error),
            message: None,
        }
    }
}

/// GET /api/v1/monitoring/webhooks/stats
///
/// Get webhook lifecycle statistics
pub async fn get_webhook_stats(
    State(state): State<Arc<MonitoringState>>,
) -> AppResult<Json<ApiResponse<WebhookStats>>> {
    let stats = get_webhook_statistics(state.db.as_ref()).await.map_err(|e| {
        AppError::Internal(format!("Failed to get webhook statistics: {}", e))
    })?;

    Ok(Json(ApiResponse::success(stats)))
}

/// POST /api/v1/monitoring/webhooks/bulk-register
///
/// Bulk register webhooks for multiple wallets
pub async fn bulk_register_webhooks(
    State(state): State<Arc<MonitoringState>>,
    Extension(auth): Extension<AuthExtension>,
    Json(request): Json<BulkRegisterRequest>,
) -> AppResult<Json<ApiResponse<BulkOperationResult>>> {
    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden(
            "Requires operator role or higher".to_string(),
        ));
    }

    if request.wallets.is_empty() {
        return Err(AppError::Validation("No wallets provided".to_string()));
    }

    let webhook_url = get_webhook_url(&state);
    let lifecycle_config = crate::monitoring::webhook_lifecycle::WebhookLifecycleConfig {
        auto_register_enabled: true,
        auto_cleanup_enabled: true,
        health_check_interval_secs: 3600,
        stale_threshold_days: 7,
        max_registration_retries: 3,
        webhook_url,
    };

    let manager = WebhookLifecycleManager::new(
        state.db.clone(),
        state.helius_client.clone(),
        state.webhook_rate_limiter.clone(),
        lifecycle_config,
    );

    let result = manager.bulk_register_webhooks(request.wallets).await.map_err(|e| {
        AppError::Internal(format!("Bulk webhook registration failed: {}", e))
    })?;

    let total = result.total;
    let succeeded = result.succeeded;
    let failed = result.failed;

    Ok(Json(ApiResponse::success_with_message(
        result,
        format!(
            "Processed {} wallets ({} succeeded, {} failed)",
            total, succeeded, failed
        ),
    )))
}

/// POST /api/v1/monitoring/webhooks/bulk-cleanup
///
/// Bulk cleanup webhooks for multiple wallets
pub async fn bulk_cleanup_webhooks(
    State(state): State<Arc<MonitoringState>>,
    Extension(auth): Extension<AuthExtension>,
    Json(request): Json<BulkCleanupRequest>,
) -> AppResult<Json<ApiResponse<BulkOperationResult>>> {
    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden(
            "Requires operator role or higher".to_string(),
        ));
    }

    if request.wallets.is_empty() {
        return Err(AppError::Validation("No wallets provided".to_string()));
    }

    let webhook_url = get_webhook_url(&state);
    let lifecycle_config = crate::monitoring::webhook_lifecycle::WebhookLifecycleConfig {
        auto_register_enabled: true,
        auto_cleanup_enabled: true,
        health_check_interval_secs: 3600,
        stale_threshold_days: 7,
        max_registration_retries: 3,
        webhook_url,
    };

    let manager = WebhookLifecycleManager::new(
        state.db.clone(),
        state.helius_client.clone(),
        state.webhook_rate_limiter.clone(),
        lifecycle_config,
    );

    let result = manager.bulk_cleanup_webhooks(request.wallets).await.map_err(|e| {
        AppError::Internal(format!("Bulk webhook cleanup failed: {}", e))
    })?;

    let total = result.total;
    let succeeded = result.succeeded;
    let failed = result.failed;

    Ok(Json(ApiResponse::success_with_message(
        result,
        format!(
            "Processed {} wallets ({} succeeded, {} failed)",
            total, succeeded, failed
        ),
    )))
}

/// POST /api/v1/monitoring/webhooks/reconcile
///
/// Manual trigger for webhook reconciliation
pub async fn manual_reconcile_webhooks(
    State(state): State<Arc<MonitoringState>>,
    Extension(auth): Extension<AuthExtension>,
) -> AppResult<Json<ApiResponse<ReconciliationResult>>> {
    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden(
            "Requires operator role or higher".to_string(),
        ));
    }

    let webhook_url = get_webhook_url(&state);
    let result = reconcile_internal(
        state.db.clone(),
        &state.helius_client,
        &state.webhook_rate_limiter,
        &webhook_url,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Manual webhook reconciliation failed: {}", e)))?;

    let registered = result.registered;
    let orphaned = result.orphaned;
    let updated = result.updated;

    info!(
        registered = registered,
        orphaned = orphaned,
        updated = updated,
        "Manual webhook reconciliation completed"
    );

    Ok(Json(ApiResponse::success_with_message(
        result,
        format!(
            "Reconciliation completed: {} registered, {} orphaned, {} updated",
            registered, orphaned, updated
        ),
    )))
}

/// POST /api/v1/monitoring/webhooks/health-check
///
/// Manual trigger for webhook health check
pub async fn manual_health_check(
    State(state): State<Arc<MonitoringState>>,
    Extension(auth): Extension<AuthExtension>,
) -> AppResult<Json<ApiResponse<HealthCheckResult>>> {
    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden(
            "Requires operator role or higher".to_string(),
        ));
    }

    let webhook_url = get_webhook_url(&state);
    let stale_threshold = state
        .config
        .monitoring
        .as_ref()
        .and_then(|m| m.webhook_lifecycle.as_ref())
        .map(|wl| wl.stale_threshold_days)
        .unwrap_or(7);

    let result = health_check_internal(
        state.db.clone(),
        &state.helius_client,
        &state.webhook_rate_limiter,
        &webhook_url,
        stale_threshold,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Manual webhook health check failed: {}", e)))?;

    let healthy = result.healthy;
    let unhealthy = result.unhealthy;
    let cleaned_up = result.cleaned_up;

    info!(
        total_checked = result.total_checked,
        healthy = healthy,
        unhealthy = unhealthy,
        "Manual webhook health check completed"
    );

    Ok(Json(ApiResponse::success_with_message(
        result,
        format!(
            "Health check completed: {} healthy, {} unhealthy, {} cleaned up",
            healthy, unhealthy, cleaned_up
        ),
    )))
}

/// GET /api/v1/monitoring/webhooks/audit
///
/// Get webhook lifecycle audit log
pub async fn get_webhook_audit_log(
    State(state): State<Arc<MonitoringState>>,
    Extension(auth): Extension<AuthExtension>,
    Query(params): Query<AuditQuery>,
) -> AppResult<Json<ApiResponse<Vec<WebhookAuditLog>>>> {
    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden(
            "Requires operator role or higher".to_string(),
        ));
    }

    let logs = state
        .db
        .get_webhook_audit_log(
        params.wallet_address.as_deref(),
        params.action.as_deref(),
        params.status.as_deref(),
        params.limit,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to get webhook audit log: {}", e)))?;

    let log_count = logs.len();

    Ok(Json(ApiResponse::success_with_message(
        logs,
        format!("Retrieved {} audit log entries", log_count),
    )))
}

/// POST /api/v1/monitoring/webhooks/:wallet_address/retry
///
/// Retry failed webhook registration
pub async fn retry_webhook_registration(
    State(state): State<Arc<MonitoringState>>,
    Extension(auth): Extension<AuthExtension>,
    Path(wallet_address): Path<String>,
) -> AppResult<StatusCode> {
    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden(
            "Requires operator role or higher".to_string(),
        ));
    }

    let webhook_url = get_webhook_url(&state);
    let lifecycle_config = crate::monitoring::webhook_lifecycle::WebhookLifecycleConfig {
        auto_register_enabled: true,
        auto_cleanup_enabled: true,
        health_check_interval_secs: 3600,
        stale_threshold_days: 7,
        max_registration_retries: 3,
        webhook_url,
    };

    let manager = WebhookLifecycleManager::new(
        state.db.clone(),
        state.helius_client.clone(),
        state.webhook_rate_limiter.clone(),
        lifecycle_config,
    );

    match manager.register_wallet_webhook(&wallet_address).await {
        Ok(result) if result.success => {
            info!(
                wallet = %wallet_address,
                webhook_id = %result.webhook_id,
                "Webhook registration retry succeeded"
            );
            Ok(StatusCode::OK)
        }
        Ok(result) => {
            warn!(
                wallet = %wallet_address,
                error = ?result.error_message,
                "Webhook registration retry failed"
            );
            Ok(StatusCode::INTERNAL_SERVER_ERROR)
        }
        Err(e) => {
            error!(
                wallet = %wallet_address,
                error = %e,
                "Webhook registration retry error"
            );
            Ok(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// POST /api/v1/monitoring/webhooks/:wallet_address/toggle
///
/// Toggle webhook enable/disable
pub async fn toggle_wallet_webhook(
    State(state): State<Arc<MonitoringState>>,
    Extension(auth): Extension<AuthExtension>,
    Path(wallet_address): Path<String>,
    Json(body): Json<ToggleWebhookRequest>,
) -> AppResult<StatusCode> {
    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden(
            "Requires operator role or higher".to_string(),
        ));
    }

    let webhook_url = get_webhook_url(&state);
    let lifecycle_config = crate::monitoring::webhook_lifecycle::WebhookLifecycleConfig {
        auto_register_enabled: true,
        auto_cleanup_enabled: true,
        health_check_interval_secs: 3600,
        stale_threshold_days: 7,
        max_registration_retries: 3,
        webhook_url,
    };

    let manager = WebhookLifecycleManager::new(
        state.db.clone(),
        state.helius_client.clone(),
        state.webhook_rate_limiter.clone(),
        lifecycle_config,
    );

    match manager.toggle_wallet_webhook(&wallet_address, body.enabled).await {
        Ok(()) => {
            info!(
                wallet = %wallet_address,
                enabled = body.enabled,
                "Webhook toggled successfully"
            );
            Ok(StatusCode::OK)
        }
        Err(e) => {
            error!(
                wallet = %wallet_address,
                error = %e,
                "Failed to toggle webhook"
            );
            Ok(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Toggle webhook request
#[derive(Debug, Deserialize)]
pub struct ToggleWebhookRequest {
    pub enabled: bool,
}
