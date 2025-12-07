//! Webhook handler for incoming trading signals

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::db::{self, DbPool};
use crate::engine::EngineHandle;
use crate::error::{AppError, AppResult};
use crate::middleware::TIMESTAMP_HEADER;
use crate::models::{Signal, SignalPayload};

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
}

/// Webhook handler
///
/// POST /api/v1/webhook
///
/// Receives trading signals, validates them, and queues for execution.
/// HMAC signature verification is handled by middleware.
pub async fn webhook_handler(
    State(state): State<Arc<WebhookState>>,
    headers: HeaderMap,
    Json(payload): Json<WebhookRequest>,
) -> Result<(StatusCode, Json<WebhookResponse>), AppError> {
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
        return Err(AppError::Duplicate(trade_uuid));
    }

    // Create signal
    let signal = Signal::new(payload, timestamp, None);

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
        action = %signal.payload.action,
        amount_sol = signal.payload.amount_sol,
        "Signal received and validated"
    );

    // Queue for execution
    match state.engine.queue_signal(signal.clone()).await {
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
