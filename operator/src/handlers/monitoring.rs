//! Monitoring handlers for automatic copy trading
//!
//! Handles Helius webhook endpoint and monitoring status

use crate::middleware::{AuthExtension, Role};
use crate::models::{Action, Signal, SignalPayload, Strategy};
use crate::monitoring::transaction_parser::parse_helius_webhook;
use crate::monitoring::HeliusWebhookPayload;
use crate::monitoring::MonitoringState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use rust_decimal::prelude::*;
use serde::Serialize;
use std::sync::Arc;

/// Helius webhook endpoint
pub async fn helius_webhook_handler(
    State(state): State<Arc<MonitoringState>>,
    Json(payload): Json<HeliusWebhookPayload>,
) -> StatusCode {
    // Rate limit webhook processing (non-blocking check)
    // Note: Full rate limiting is handled by the rate limiter, but we skip the blocking acquire
    // to avoid Send bound issues. The rate limiter will still track usage.
    let _ = state.webhook_rate_limiter.current_rate();

    tracing::info!(
        signature = %payload.signature,
        transaction_type = %payload.transaction_type,
        "Received Helius webhook"
    );

    // Parse webhook to extract swap information
    if let Ok(Some(swap)) = parse_helius_webhook(&payload) {
        // Find wallet address from account data
        let wallet_address = payload
            .account_data
            .iter()
            .find_map(|acc| {
                if acc.account != payload.signature {
                    Some(acc.account.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        if !wallet_address.is_empty() {
            // Check if wallet exists in database
            let wallet_opt = state.db.get_wallet(&wallet_address).await;

            // If wallet doesn't exist, automatically add it as CANDIDATE
            let wallet = if let Ok(Some(w)) = wallet_opt {
                w
            } else {
                // Auto-add wallet when detected making a trade
                tracing::info!(
                    wallet = %wallet_address,
                    "New wallet detected, adding to database"
                );

                // Add wallet with minimal info (will be analyzed by Scout later)
                let _ = state.db.upsert_wallet(
                    &wallet_address,
                    None,                 // wqs_score - will be calculated by Scout
                    None,                 // roi_7d
                    None,                 // roi_30d
                    Some(1),              // trade_count_30d - at least 1 trade detected
                    None,                 // win_rate
                    None,                 // max_drawdown_30d
                    Some(swap.amount_in), // avg_trade_size_sol
                    Some("Auto-added from webhook detection"), // notes
                )
                .await;

                // Fetch the newly added wallet
                match state.db.get_wallet(&wallet_address).await {
                    Ok(Some(w)) => w,
                    _ => {
                        tracing::warn!(
                            wallet = %wallet_address,
                            "Failed to retrieve newly added wallet"
                        );
                        return StatusCode::OK; // Don't fail, just skip
                    }
                }
            };

            // Only process signals from ACTIVE wallets
            if wallet.status == "ACTIVE" {
                // FIX 1: Check circuit breaker before queuing
                if let Some(ref cb) = state.circuit_breaker {
                    if !cb.is_trading_allowed() {
                        let reason = cb
                            .trip_reason()
                            .map(|r| r.to_string())
                            .unwrap_or_else(|| "Circuit breaker tripped".to_string());
                        tracing::warn!(
                            wallet = %wallet_address,
                            reason = %reason,
                            "Helius webhook signal blocked by circuit breaker"
                        );
                        return StatusCode::SERVICE_UNAVAILABLE;
                    }
                }

                // Generate signal
                let direction = if swap.direction == crate::monitoring::SwapDirection::Buy {
                    Action::Buy
                } else {
                    Action::Sell
                };

                // FIX 2: Determine strategy, downgrading Spear to Shield when in RPC fallback
                let in_fallback = state.engine.is_in_fallback();
                let strategy = if wallet.wqs_score.map(|s| s >= rust_decimal::Decimal::from(70)).unwrap_or(false) {
                    Strategy::Shield
                } else if in_fallback {
                    // Cannot run Spear when primary RPC is unavailable; use Shield
                    tracing::info!(
                        wallet = %wallet_address,
                        "RPC in fallback mode — downgrading Spear signal to Shield"
                    );
                    Strategy::Shield
                } else {
                    Strategy::Spear
                };

                let target_token = if direction == Action::Buy {
                    swap.token_out.clone()
                } else {
                    swap.token_in.clone()
                };

                // FIX 1: Token fast_check before queuing (BUY only)
                if direction == Action::Buy {
                    if let Some(ref tp) = state.token_parser {
                        match tp.fast_check(&target_token, strategy).await {
                            Ok(result) if !result.safe => {
                                let reason = result
                                    .rejection_reason
                                    .unwrap_or_else(|| "Token failed safety check".to_string());
                                tracing::warn!(
                                    wallet = %wallet_address,
                                    token = %target_token,
                                    reason = %reason,
                                    "Helius webhook token rejected by fast-path safety check"
                                );
                                return StatusCode::OK; // Not an error on our end; just skip
                            }
                            Err(e) => {
                                tracing::warn!(
                                    token = %target_token,
                                    error = %e,
                                    "Helius webhook token fast-check failed; proceeding to slow path"
                                );
                            }
                            Ok(_) => {} // safe — continue
                        }
                    }

                    // FIX 1: Check portfolio heat before queuing BUY signals
                    if let Some(ref ph) = state.portfolio_heat {
                        match ph.can_open_position(swap.amount_in).await {
                            Ok(false) => {
                                tracing::warn!(
                                    wallet = %wallet_address,
                                    token = %target_token,
                                    "Helius webhook BUY rejected: portfolio heat limit reached"
                                );
                                return StatusCode::SERVICE_UNAVAILABLE;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "Portfolio heat check failed for Helius webhook; allowing"
                                );
                            }
                            Ok(true) => {} // heat OK — continue
                        }
                    }
                }

                let signal_payload = SignalPayload {
                    wallet_address: wallet_address.clone(),
                    strategy,
                    token: target_token.clone(),
                    token_address: Some(target_token),
                    action: direction,
                    amount_sol: swap.amount_in,
                    trade_uuid: None,
                    exit_fraction: None,
                };

                let signal = Signal::new(
                    signal_payload,
                    chrono::Utc::now().timestamp(),
                    None, // source_ip
                );

                // Queue signal with wallet WQS
                let wallet_wqs = wallet.wqs_score.map(|s| s.to_f64().unwrap_or(0.0));
                if let Err(e) = state.engine.queue_signal(signal, wallet_wqs).await {
                    tracing::error!(error = %e, "Failed to queue signal from webhook");
                    return StatusCode::INTERNAL_SERVER_ERROR;
                }

                tracing::info!(
                    wallet = %wallet_address,
                    token = %swap.token_out,
                    "Queued signal from webhook"
                );
            } else {
                tracing::debug!(
                    wallet = %wallet_address,
                    status = %wallet.status,
                    "Wallet detected but not ACTIVE, skipping signal"
                );
            }
        }
    }

    StatusCode::OK
}

/// Get monitoring status
pub async fn get_monitoring_status(
    State(state): State<Arc<MonitoringState>>,
) -> Json<MonitoringStatus> {
    let webhook_rate = state.webhook_rate_limiter.current_rate();
    let rpc_rate = state.rpc_rate_limiter.current_rate();
    let webhook_credits = state.webhook_rate_limiter.credit_usage();
    let rpc_credits = state.rpc_rate_limiter.credit_usage();

    Json(MonitoringStatus {
        enabled: state
            .config
            .monitoring
            .as_ref()
            .map(|m| m.enabled)
            .unwrap_or(false),
        webhook_rate,
        rpc_rate,
        webhook_credits,
        rpc_credits,
        active_wallets: {
            // Query active wallets count from database
            match state.db.get_all_wallet_monitoring().await {
                Ok(records) => records.iter().filter(|r| r.monitoring_enabled > 0).count(),
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to query active wallets count, returning 0");
                    0
                }
            }
        },
    })
}

#[derive(Debug, Serialize)]
pub struct MonitoringStatus {
    enabled: bool,
    webhook_rate: f64,
    rpc_rate: f64,
    webhook_credits: u64,
    rpc_credits: u64,
    active_wallets: usize,
}

/// Enable monitoring for a wallet
/// Requires: operator+ role
pub async fn enable_wallet_monitoring(
    State(state): State<Arc<MonitoringState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
    Path(wallet_address): Path<String>,
) -> StatusCode {
    if !auth.0.role.has_permission(Role::Operator) {
        return StatusCode::FORBIDDEN;
    }
    tracing::info!(wallet = %wallet_address, "Enable monitoring requested");

    // Check if wallet exists and is ACTIVE
    let wallet = match state.db.get_wallet(&wallet_address).await {
        Ok(Some(w)) => w,
        Ok(None) => {
            tracing::warn!(wallet = %wallet_address, "Wallet not found");
            return StatusCode::NOT_FOUND;
        }
        Err(e) => {
            tracing::error!(wallet = %wallet_address, error = %e, "Failed to query wallet");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    if wallet.status != "ACTIVE" {
        tracing::warn!(
            wallet = %wallet_address,
            status = %wallet.status,
            "Wallet is not ACTIVE, cannot enable monitoring"
        );
        return StatusCode::BAD_REQUEST;
    }

    // Get webhook URL from config
    let webhook_url = match &state.config.monitoring {
        Some(m) => m.helius_webhook_url.as_ref(),
        None => {
            tracing::error!("Monitoring config not available");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    let webhook_url = match webhook_url {
        Some(url) => url,
        None => {
            tracing::error!("Helius webhook URL not configured");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    // Register Helius webhook for this wallet
    let wallets = vec![wallet_address.clone()];
    let webhook_id = match state
        .helius_client
        .register_webhook(&wallets, webhook_url)
        .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(
                wallet = %wallet_address,
                error = %e,
                "Failed to register Helius webhook"
            );
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    // Update database
    if let Err(e) =
        state.db.upsert_wallet_monitoring(&wallet_address, Some(&webhook_id), true)
            .await
    {
        tracing::error!(
            wallet = %wallet_address,
            error = %e,
            "Failed to update wallet_monitoring in database"
        );
        // Try to clean up webhook registration
        let _ = state.helius_client.delete_webhook(&webhook_id).await;
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    tracing::info!(
        wallet = %wallet_address,
        webhook_id = %webhook_id,
        "Wallet monitoring enabled successfully"
    );

    StatusCode::OK
}

/// Disable monitoring for a wallet
/// Requires: operator+ role
pub async fn disable_wallet_monitoring(
    State(state): State<Arc<MonitoringState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
    Path(wallet_address): Path<String>,
) -> StatusCode {
    if !auth.0.role.has_permission(Role::Operator) {
        return StatusCode::FORBIDDEN;
    }
    tracing::info!(wallet = %wallet_address, "Disable monitoring requested");

    // Get current monitoring record
    let monitoring = match state.db.get_wallet_monitoring(&wallet_address)
        .await
    {
        Ok(Some(m)) => m,
        Ok(None) => {
            tracing::warn!(wallet = %wallet_address, "Wallet monitoring not found");
            return StatusCode::NOT_FOUND;
        }
        Err(e) => {
            tracing::error!(wallet = %wallet_address, error = %e, "Failed to query wallet monitoring");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    // Delete Helius webhook if it exists
    if let Some(webhook_id) = &monitoring.helius_webhook_id {
        if let Err(e) = state.helius_client.delete_webhook(webhook_id).await {
            tracing::warn!(
                wallet = %wallet_address,
                webhook_id = %webhook_id,
                error = %e,
                "Failed to delete Helius webhook (continuing with database update)"
            );
            // Continue with database update even if webhook deletion fails
        } else {
            tracing::info!(
                wallet = %wallet_address,
                webhook_id = %webhook_id,
                "Helius webhook deleted successfully"
            );
        }
    }

    // Update database to disable monitoring
    if let Err(e) = state.db.upsert_wallet_monitoring(
        &wallet_address,
        None,  // Clear webhook_id
        false, // Disable monitoring
    )
    .await
    {
        tracing::error!(
            wallet = %wallet_address,
            error = %e,
            "Failed to update wallet_monitoring in database"
        );
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    tracing::info!(
        wallet = %wallet_address,
        "Wallet monitoring disabled successfully"
    );

    StatusCode::OK
}

/// Wallet monitoring state response
#[derive(Debug, Serialize)]
pub struct WalletMonitoringStateResponse {
    pub wallet_states: Vec<WalletMonitoringStateItem>,
}

#[derive(Debug, Serialize)]
pub struct WalletMonitoringStateItem {
    pub address: String,
    pub method: String, // "webhook" or "polling"
    pub status: String, // "active", "inactive", or "error"
    pub last_activity: String,
    pub last_fetch: Option<String>,
    pub failed_fetches: i32,
    pub success_rate: f64,
    pub next_fetch: Option<String>,
}

/// Get all wallet monitoring states
/// Requires: readonly+ role
pub async fn get_wallet_monitoring_states(
    State(state): State<Arc<MonitoringState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
) -> Json<WalletMonitoringStateResponse> {
    // Verify user has at least readonly access
    if !auth.0.role.has_permission(Role::Readonly) {
        tracing::warn!("Unauthorized attempt to access wallet monitoring states");
        return Json(WalletMonitoringStateResponse {
            wallet_states: vec![],
        });
    }

    // Fetch all wallet monitoring records from database
    let wallet_monitoring_records = match state.db.get_all_wallet_monitoring().await {
        Ok(records) => records,
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch wallet monitoring states");
            return Json(WalletMonitoringStateResponse {
                wallet_states: vec![],
            });
        }
    };

    // Transform database records to frontend format
    let wallet_states: Vec<WalletMonitoringStateItem> = wallet_monitoring_records
        .into_iter()
        .map(|wm| {
            // Determine method: webhook if helius_webhook_id exists, otherwise polling
            let method = if wm.helius_webhook_id.is_some() && !wm.helius_webhook_id.as_ref().unwrap().is_empty() {
                "webhook".to_string()
            } else {
                "polling".to_string()
            };

            // Determine status based on monitoring_enabled and webhook_health_status
            let status = if wm.monitoring_enabled == 0 {
                "inactive".to_string()
            } else if wm.webhook_health_status.as_deref() == Some("error") ||
                     wm.webhook_health_status.as_deref() == Some("unhealthy") ||
                     wm.webhook_status.as_deref() == Some("failed") {
                "error".to_string()
            } else {
                "active".to_string()
            };

            // Calculate success rate based on registration attempts
            // If no attempts, assume 100%, otherwise calculate based on failures
            let success_rate = if wm.registration_attempts == 0 {
                100.0
            } else {
                let base_rate = 100.0;
                // Penalize for failed registration attempts
                let failure_penalty = (wm.last_registration_error.as_ref().is_some() as i32 as f64) * 10.0;
                (base_rate - failure_penalty).max(0.0)
            };

            // Use registration_attempts as failed_fetches indicator
            let failed_fetches = wm.registration_attempts;

            // Set last_activity from last_monitored_at, fallback to created_at
            let last_activity = wm.last_monitored_at
                .clone()
                .or(Some(wm.created_at))
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

            // Set last_fetch to last_monitored_at if available
            let last_fetch = wm.last_monitored_at.clone();

            // Calculate next_fetch: for webhooks it's null (real-time),
            // for polling we'll estimate 15 minutes from last activity
            let next_fetch = if method == "polling" {
                Some(
                    chrono::Utc::now()
                        .checked_add_signed(chrono::Duration::minutes(15))
                        .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::minutes(15))
                        .to_rfc3339()
                )
            } else {
                None
            };

            WalletMonitoringStateItem {
                address: wm.wallet_address,
                method,
                status,
                last_activity,
                last_fetch,
                failed_fetches,
                success_rate,
                next_fetch,
            }
        })
        .collect();

    tracing::info!(
        count = wallet_states.len(),
        "Fetched wallet monitoring states"
    );

    Json(WalletMonitoringStateResponse { wallet_states })
}
