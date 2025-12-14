//! Webhook handler for incoming trading signals

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::Utc;
use serde::Serialize;
use std::sync::Arc;

use crate::circuit_breaker::CircuitBreaker;
use crate::db::{self, DbPool};
use crate::engine::{EngineHandle, SignalQuality};
use crate::error::AppError;
use crate::middleware::TIMESTAMP_HEADER;
use crate::models::{Signal, SignalPayload, Strategy};
use crate::monitoring::{HeliusClient, SignalAggregator};
use crate::token::TokenParser;
use rust_decimal::prelude::*;

/// Webhook request - already validated by HMAC middleware
/// Body is the SignalPayload
pub type WebhookRequest = SignalPayload;

/// Webhook response
#[derive(Debug, Serialize)]
pub struct WebhookResponse {
    /// Status of the request
    pub status: WebhookStatus,
    /// Trade UUID assigned to this signal
    pub trade_uuid: String,
    /// Optional reason for rejection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Webhook status
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookStatus {
    /// Signal accepted and queued for processing
    Accepted,
    /// Signal rejected
    Rejected,
}

/// State needed by the webhook handler
pub struct WebhookState {
    /// Database pool
    pub db: DbPool,
    /// Engine handle for queueing signals
    pub engine: EngineHandle,
    /// Token parser for safety checks
    pub token_parser: Arc<TokenParser>,
    /// Circuit breaker
    pub circuit_breaker: Arc<CircuitBreaker>,
    /// Portfolio heat manager (optional)
    pub portfolio_heat: Option<Arc<crate::engine::PortfolioHeat>>,
    /// Signal aggregator for consensus detection
    pub signal_aggregator: Option<Arc<SignalAggregator>>,
    /// Helius client for token age fetching
    pub helius_client: Option<Arc<HeliusClient>>,
}

/// Webhook handler
///
/// POST /api/v1/webhook
///
/// Receives trading signals, validates them, and queues for execution.
/// HMAC signature verification is handled by middleware.
///
/// Security checks performed:
/// 1. Circuit breaker check
/// 2. Payload validation
/// 3. Idempotency check (duplicate detection)
/// 4. Token safety fast-path check (freeze/mint authority)
pub async fn webhook_handler(
    State(state): State<Arc<WebhookState>>,
    headers: HeaderMap,
    Json(payload): Json<WebhookRequest>,
) -> Result<(StatusCode, Json<WebhookResponse>), AppError> {
    // Check circuit breaker first
    if !state.circuit_breaker.is_trading_allowed() {
        let reason = state
            .circuit_breaker
            .trip_reason()
            .map(|r| r.to_string())
            .unwrap_or_else(|| "Circuit breaker tripped".to_string());

        tracing::warn!(reason = %reason, "Signal rejected by circuit breaker");

        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(WebhookResponse {
                status: WebhookStatus::Rejected,
                trade_uuid: String::new(),
                reason: Some(format!("circuit_breaker_triggered: {}", reason)),
            }),
        ));
    }

    // Extract timestamp from header (already validated by middleware)
    let timestamp = headers
        .get(TIMESTAMP_HEADER)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or_else(|| Utc::now().timestamp());

    // Validate signal payload
    if let Err(validation_error) = payload.validate() {
        tracing::warn!(error = %validation_error, "Signal validation failed");
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(WebhookResponse {
                status: WebhookStatus::Rejected,
                trade_uuid: String::new(),
                reason: Some(validation_error),
            }),
        ));
    }

    // Generate trade UUID
    let trade_uuid = payload.generate_trade_uuid(timestamp);

    // Check for duplicate (idempotency)
    if db::trade_uuid_exists(&state.db, &trade_uuid).await? {
        tracing::info!(trade_uuid = %trade_uuid, "Duplicate signal rejected");
        // Return PDD-shaped response: normal HTTP 200/202 with status: rejected
        return Ok((
            StatusCode::OK,
            Json(WebhookResponse {
                status: WebhookStatus::Rejected,
                trade_uuid,
                reason: Some("duplicate_signal".to_string()),
            }),
        ));
    }

    // Fast path token safety check (for BUY signals only)
    // EXIT signals don't need token validation, SELL signals already own the token
    if payload.strategy != Strategy::Exit {
        if let Some(ref token_address) = payload.token_address {
            match state
                .token_parser
                .fast_check(token_address, payload.strategy)
                .await
            {
                Ok(result) => {
                    if !result.safe {
                        let reason = result
                            .rejection_reason
                            .unwrap_or_else(|| "Token failed safety check".to_string());

                        tracing::warn!(
                            trade_uuid = %trade_uuid,
                            token = %token_address,
                            reason = %reason,
                            "Token rejected by fast-path safety check"
                        );

                        // Log to dead letter queue
                        let _ = db::insert_dead_letter(
                            &state.db,
                            Some(&trade_uuid),
                            &serde_json::to_string(&payload).unwrap_or_default(),
                            "TOKEN_SAFETY_FAILED",
                            Some(&reason),
                            None,
                        )
                        .await;

                        return Ok((
                            StatusCode::BAD_REQUEST,
                            Json(WebhookResponse {
                                status: WebhookStatus::Rejected,
                                trade_uuid,
                                reason: Some(reason),
                            }),
                        ));
                    }
                }
                Err(e) => {
                    // Log error but allow through - slow path will do full check
                    tracing::warn!(
                        token = %token_address,
                        error = %e,
                        "Fast-path token check failed, allowing to slow path"
                    );
                }
            }
        }
    }

    // Create signal
    let signal = Signal::new(payload, timestamp, None);

    // Signal quality check (for BUY signals only, EXIT/SELL don't need quality check)
    if signal.payload.action == crate::models::Action::Buy {
        // Get wallet WQS
        let wallet_wqs = match db::get_wallet_by_address(&state.db, &signal.payload.wallet_address).await {
            Ok(Some(wallet)) => wallet.wqs_score.unwrap_or(50.0),
            Ok(None) => 50.0,  // Default if wallet not found
            Err(_) => 50.0,  // Default on error
        };

        // Check if consensus signal using SignalAggregator
        let is_consensus = if let Some(ref aggregator) = state.signal_aggregator {
            if let Some(ref token_address) = signal.payload.token_address {
                // Add signal to aggregator and check for consensus
                if let Some(consensus) = aggregator
                    .add_signal(
                        &signal.payload.wallet_address,
                        token_address,
                        "BUY",
                        signal.payload.amount_sol.to_f64().unwrap_or(0.0),
                    )
                    .await
                {
                    // Consensus detected (2+ wallets buying same token)
                    tracing::debug!(
                        trade_uuid = %signal.trade_uuid,
                        token_address = token_address,
                        wallet_count = consensus.wallet_count,
                        "Consensus signal detected"
                    );
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        // Get liquidity from token safety check result (if available)
        let liquidity_usd = if let Some(ref token_address) = signal.payload.token_address {
            // Try to get liquidity from token parser cache or metadata
            // For now, use a conservative estimate - will be checked in slow path
            match state.token_parser.fast_check(token_address, signal.payload.strategy).await {
                Ok(result) => result.liquidity_usd.unwrap_or(0.0),
                Err(_) => 0.0,
            }
        } else {
            0.0
        };

        // Get token age from Helius client
        let token_age_hours = if let Some(ref helius_client) = state.helius_client {
            if let Some(ref token_address) = signal.payload.token_address {
                match helius_client.get_token_age_hours(token_address).await {
                    Ok(age) => age,
                    Err(e) => {
                        tracing::debug!(
                            trade_uuid = %signal.trade_uuid,
                            token_address = token_address,
                            error = %e,
                            "Failed to fetch token age from Helius, using None"
                        );
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        // Calculate signal quality
        let quality = SignalQuality::calculate(
            wallet_wqs,
            is_consensus,
            liquidity_usd,
            token_age_hours,
        );

        // Reject if quality too low
        if !quality.should_enter(0.7) {
            tracing::warn!(
                trade_uuid = %signal.trade_uuid,
                quality_score = quality.score,
                wallet_wqs = wallet_wqs,
                liquidity_usd = liquidity_usd,
                "Signal rejected due to low quality"
            );

            // Log to dead letter queue
            let _ = db::insert_dead_letter(
                &state.db,
                Some(&signal.trade_uuid),
                &serde_json::to_string(&signal.payload).unwrap_or_default(),
                "SIGNAL_QUALITY_TOO_LOW",
                Some(&format!("Quality score: {:.2} < 0.7", quality.score)),
                None,
            )
            .await;

            return Ok((
                StatusCode::BAD_REQUEST,
                Json(WebhookResponse {
                    status: WebhookStatus::Rejected,
                    trade_uuid: signal.trade_uuid,
                    reason: Some(format!("Signal quality too low: {:.2}", quality.score)),
                }),
            ));
        }

        tracing::debug!(
            trade_uuid = %signal.trade_uuid,
            quality_score = quality.score,
            category = %quality.category(),
            "Signal quality check passed"
        );
    }

    // Check portfolio heat (if enabled)
    if let Some(ref portfolio_heat) = state.portfolio_heat {
        match portfolio_heat.can_open_position(signal.payload.amount_sol.to_f64().unwrap_or(0.0)).await {
            Ok(false) => {
                tracing::warn!(
                    trade_uuid = %signal.trade_uuid,
                    amount_sol = signal.payload.amount_sol.to_f64().unwrap_or(0.0),
                    "Signal rejected: portfolio heat limit reached"
                );

                // Log to dead letter queue
                let _ = db::insert_dead_letter(
                    &state.db,
                    Some(&signal.trade_uuid),
                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                    "PORTFOLIO_HEAT_LIMIT",
                    Some("Portfolio heat limit (20%) reached"),
                    None,
                )
                .await;

                return Ok((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(WebhookResponse {
                        status: WebhookStatus::Rejected,
                        trade_uuid: signal.trade_uuid,
                        reason: Some("Portfolio heat limit reached".to_string()),
                    }),
                ));
            }
            Ok(true) => {
                // Heat check passed
            }
            Err(e) => {
                tracing::warn!(
                    trade_uuid = %signal.trade_uuid,
                    error = %e,
                    "Portfolio heat check failed, allowing trade"
                );
            }
        }
    }

    // Insert into database as PENDING
    db::insert_trade(
        &state.db,
        &signal.trade_uuid,
        &signal.payload.wallet_address,
        signal.token_address(),
        Some(&signal.payload.token),
        &signal.payload.strategy.to_string(),
        &signal.payload.action.to_string(),
        signal.payload.amount_sol,
        "PENDING",
    )
    .await?;

    tracing::info!(
        trade_uuid = %signal.trade_uuid,
        strategy = %signal.payload.strategy,
        token = %signal.payload.token,
        amount_sol = signal.payload.amount_sol.to_f64().unwrap_or(0.0),
        action = %signal.payload.action,
        amount_sol = signal.payload.amount_sol.to_f64().unwrap_or(0.0),
        "Signal received and validated"
    );

    // Get wallet WQS for queue routing (if available)
    let wallet_wqs = if signal.payload.action == crate::models::Action::Buy {
        match db::get_wallet_by_address(&state.db, &signal.payload.wallet_address).await {
            Ok(Some(wallet)) => wallet.wqs_score,
            _ => None,
        }
    } else {
        None
    };

    // Queue for execution
    match state.engine.queue_signal(signal.clone(), wallet_wqs).await {
        Ok(()) => {
            // Update status to QUEUED
            db::update_trade_status(&state.db, &signal.trade_uuid, "QUEUED", None, None).await?;

            tracing::info!(trade_uuid = %signal.trade_uuid, "Signal queued for execution");

            Ok((
                StatusCode::ACCEPTED,
                Json(WebhookResponse {
                    status: WebhookStatus::Accepted,
                    trade_uuid: signal.trade_uuid,
                    reason: None,
                }),
            ))
        }
        Err(e) => {
            // Queue failed (full or load shedding)
            tracing::warn!(
                trade_uuid = %signal.trade_uuid,
                error = %e,
                "Failed to queue signal"
            );

            // Log to dead letter queue
            db::insert_dead_letter(
                &state.db,
                Some(&signal.trade_uuid),
                &serde_json::to_string(&signal.payload).unwrap_or_default(),
                "QUEUE_FULL",
                Some(&e.to_string()),
                None,
            )
            .await?;

            // Update trade status
            db::update_trade_status(
                &state.db,
                &signal.trade_uuid,
                "DEAD_LETTER",
                None,
                Some(&e.to_string()),
            )
            .await?;

            Err(AppError::Queue(e.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Action, Strategy};

    #[test]
    fn test_webhook_response_serialization() {
        let response = WebhookResponse {
            status: WebhookStatus::Accepted,
            trade_uuid: "test-uuid-123".to_string(),
            reason: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("accepted"));
        assert!(json.contains("test-uuid-123"));
        assert!(!json.contains("reason")); // Should be skipped when None
    }

    #[test]
    fn test_signal_payload_parsing() {
        let json = r#"{
            "strategy": "SHIELD",
            "token": "BONK",
            "action": "BUY",
            "amount_sol": 0.5,
            "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
        }"#;

        let payload: SignalPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.strategy, Strategy::Shield);
        assert_eq!(payload.token, "BONK");
        assert_eq!(payload.action, Action::Buy);
        assert_eq!(payload.amount_sol, 0.5);
    }
}
