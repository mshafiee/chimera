//! Monitoring handlers for automatic copy trading
//!
//! Handles Helius webhook endpoint and monitoring status

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::Serialize;
use std::sync::Arc;
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
        active_wallets: 0, // TODO: Query from database
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
    State(_state): State<Arc<MonitoringState>>,
    Path(wallet_address): Path<String>,
) -> Result<StatusCode, StatusCode> {
    // TODO: Implement wallet monitoring enable
    tracing::info!(wallet = %wallet_address, "Enable monitoring requested");
    Ok(StatusCode::OK)
}

/// Disable monitoring for a wallet
pub async fn disable_wallet_monitoring(
    State(_state): State<Arc<MonitoringState>>,
    Path(wallet_address): Path<String>,
) -> Result<StatusCode, StatusCode> {
    // TODO: Implement wallet monitoring disable
    tracing::info!(wallet = %wallet_address, "Disable monitoring requested");
    Ok(StatusCode::OK)
}
