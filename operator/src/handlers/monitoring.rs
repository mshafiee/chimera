//! Monitoring handlers for automatic copy trading
//!
//! Handles Helius webhook endpoint and monitoring status

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::Serialize;
use std::sync::Arc;
use sqlx;
use crate::monitoring::HeliusWebhookPayload;
use crate::monitoring::MonitoringState;
use crate::monitoring::transaction_parser::parse_helius_webhook;
use crate::monitoring::rate_limiter::RequestPriority;
use crate::models::{Signal, SignalPayload, Strategy, Action};

/// Helius webhook endpoint
pub async fn helius_webhook_handler(
    State(state): State<Arc<MonitoringState>>,
    Json(payload): Json<HeliusWebhookPayload>,
) -> Result<StatusCode, StatusCode> {
    // Rate limit webhook processing
    state.webhook_rate_limiter
        .acquire(RequestPriority::Entry)
        .await;

    tracing::info!(
        signature = %payload.signature,
        transaction_type = %payload.transaction_type,
        "Received Helius webhook"
    );

    // Parse webhook to extract swap information
    if let Ok(Some(swap)) = parse_helius_webhook(&payload) {
        // Find wallet address from account data
        let wallet_address = payload.account_data
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
            // Check if wallet is ACTIVE
            if let Ok(Some(wallet)) = crate::db::get_wallet_by_address(&state.db, &wallet_address).await {
                if wallet.status == "ACTIVE" {
                    // Generate signal
                    let direction = if swap.direction == crate::monitoring::SwapDirection::Buy {
                        Action::Buy
                    } else {
                        Action::Sell
                    };

                    let strategy = if wallet.wqs_score.unwrap_or(0.0) >= 70.0 {
                        Strategy::Shield
                    } else {
                        Strategy::Spear
                    };

                    let signal_payload = SignalPayload {
                        wallet_address: wallet_address.clone(),
                        strategy,
                        token: swap.token_out.clone(),
                        token_address: Some(swap.token_out.clone()),
                        action: direction,
                        amount_sol: swap.amount_in,
                        trade_uuid: None,
                    };

                    let signal = Signal::new(
                        signal_payload,
                        chrono::Utc::now().timestamp(),
                        None, // source_ip
                    );

                    // Queue signal
                    if let Err(e) = state.engine.queue_signal(signal).await {
                        tracing::error!(error = %e, "Failed to queue signal from webhook");
                        return Err(StatusCode::INTERNAL_SERVER_ERROR);
                    }

                    tracing::info!(
                        wallet = %wallet_address,
                        token = %swap.token_out,
                        "Queued signal from webhook"
                    );
                }
            }
        }
    }

    Ok(StatusCode::OK)
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
        enabled: state.config.monitoring.as_ref().map(|m| m.enabled).unwrap_or(false),
        webhook_rate,
        rpc_rate,
        webhook_credits,
        rpc_credits,
        active_wallets: {
            // Query active wallets count from database
            match sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM wallet_monitoring WHERE monitoring_enabled = 1"
            )
            .fetch_one(&state.db)
            .await
            {
                Ok(count) => count as usize,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to query active wallets count, returning 0");
                    0
                }
            }
        },
    })
}

#[derive(Debug, Serialize)]
struct MonitoringStatus {
    enabled: bool,
    webhook_rate: f64,
    rpc_rate: f64,
    webhook_credits: u64,
    rpc_credits: u64,
    active_wallets: usize,
}

/// Enable monitoring for a wallet
pub async fn enable_wallet_monitoring(
    State(state): State<Arc<MonitoringState>>,
    Path(wallet_address): Path<String>,
) -> Result<StatusCode, StatusCode> {
    tracing::info!(wallet = %wallet_address, "Enable monitoring requested");

    // Check if wallet exists and is ACTIVE
    let wallet = match crate::db::get_wallet_by_address(&state.db, &wallet_address).await {
        Ok(Some(w)) => w,
        Ok(None) => {
            tracing::warn!(wallet = %wallet_address, "Wallet not found");
            return Err(StatusCode::NOT_FOUND);
        }
        Err(e) => {
            tracing::error!(wallet = %wallet_address, error = %e, "Failed to query wallet");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    if wallet.status != "ACTIVE" {
        tracing::warn!(
            wallet = %wallet_address,
            status = %wallet.status,
            "Wallet is not ACTIVE, cannot enable monitoring"
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    // Get webhook URL from config
    let webhook_url = match &state.config.monitoring {
        Some(m) => m.helius_webhook_url.as_ref(),
        None => {
            tracing::error!("Monitoring config not available");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let webhook_url = match webhook_url {
        Some(url) => url,
        None => {
            tracing::error!("Helius webhook URL not configured");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Register Helius webhook for this wallet
    let wallets = vec![wallet_address.clone()];
    let webhook_id = match state.helius_client
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
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Update database
    if let Err(e) = crate::db::upsert_wallet_monitoring(
        &state.db,
        &wallet_address,
        Some(&webhook_id),
        true,
    )
    .await
    {
        tracing::error!(
            wallet = %wallet_address,
            error = %e,
            "Failed to update wallet_monitoring in database"
        );
        // Try to clean up webhook registration
        let _ = state.helius_client.delete_webhook(&webhook_id).await;
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    tracing::info!(
        wallet = %wallet_address,
        webhook_id = %webhook_id,
        "Wallet monitoring enabled successfully"
    );

    Ok(StatusCode::OK)
}

/// Disable monitoring for a wallet
pub async fn disable_wallet_monitoring(
    State(state): State<Arc<MonitoringState>>,
    Path(wallet_address): Path<String>,
) -> Result<StatusCode, StatusCode> {
    tracing::info!(wallet = %wallet_address, "Disable monitoring requested");

    // Get current monitoring record
    let monitoring = match crate::db::get_wallet_monitoring_by_address(&state.db, &wallet_address).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            tracing::warn!(wallet = %wallet_address, "Wallet monitoring not found");
            return Err(StatusCode::NOT_FOUND);
        }
        Err(e) => {
            tracing::error!(wallet = %wallet_address, error = %e, "Failed to query wallet monitoring");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
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
    if let Err(e) = crate::db::upsert_wallet_monitoring(
        &state.db,
        &wallet_address,
        None, // Clear webhook_id
        false, // Disable monitoring
    )
    .await
    {
        tracing::error!(
            wallet = %wallet_address,
            error = %e,
            "Failed to update wallet_monitoring in database"
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    tracing::info!(
        wallet = %wallet_address,
        "Wallet monitoring disabled successfully"
    );

    Ok(StatusCode::OK)
}
