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
use crate::db_abstraction::{Database, DbPool, InsertTrade, UpdateTradeStatus};
use crate::engine::position_sizer::SizingFactors;
use crate::engine::{EngineHandle, PositionSizer, SignalQuality};
use crate::error::AppError;
use crate::middleware::TIMESTAMP_HEADER;
use crate::models::{Signal, SignalPayload, Strategy};
use crate::monitoring::{HeliusClient, SignalAggregator};
use crate::token::TokenParser;
use rust_decimal::prelude::*;
use solana_sdk::pubkey::Pubkey;

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
    pub db: Arc<dyn Database>,
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
    /// Market regime detector (optional)
    pub market_regime: Option<Arc<crate::engine::MarketRegimeDetector>>,
    /// Helius client for token age fetching
    pub helius_client: Option<Arc<HeliusClient>>,
    /// Position sizer for Kelly/confidence-based sizing
    pub position_sizer: Option<Arc<PositionSizer>>,
    /// Total trading capital in SOL (from config.position_sizing.total_capital_sol)
    pub total_capital_sol: Decimal,
    /// Maximum single-position size in SOL (used to cap SELL amounts)
    pub max_position_sol: Decimal,
    /// Minimum signal quality score to accept a Shield trade
    pub shield_signal_quality_threshold: f64,
    /// Minimum signal quality score to accept a Spear trade
    pub spear_signal_quality_threshold: f64,
    /// Shield strategy allocation percentage
    pub shield_percent: u32,
    /// Spear strategy allocation percentage
    pub spear_percent: u32,
    /// Minimum liquidity in USD for Shield (hard floor — reject below this)
    pub min_liquidity_shield_usd: rust_decimal::Decimal,
    /// Minimum liquidity in USD for Spear (hard floor — reject below this)
    pub min_liquidity_spear_usd: rust_decimal::Decimal,
    /// Unified selection engine (B1): shared BUY/SELL decision pipeline used
    /// by both this webhook path and the Helius monitoring handler.
    pub selection: Arc<crate::engine::SelectionService>,
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
#[tracing::instrument(skip(state, payload))]
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
    if state.db.trade_uuid_exists(&trade_uuid).await? {
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

    // Create signal
    let mut signal = Signal::new(payload, timestamp, None);

    // Populate token decimals from on-chain metadata for correct fill price conversion.
    // [B-M1] Without correct decimals, lamports-per-base-unit → SOL-per-token assumes 9 decimals,
    // which is wrong for USDC (6), USDT (6), and other non-standard tokens.
    if let Some(ref token_address) = signal.payload.token_address {
        signal.token_decimals = state.token_parser.get_token_decimals(token_address).await;
    }

    // B1: Unified decision pipeline — wallet gate, WQS, token safety, liquidity,
    // consensus, quality, regime, PositionSizer sizing, and portfolio/strategy
    // heat all run inside SelectionService. Both this webhook path and the
    // Helius monitoring path call the same function.
    let req = crate::engine::SelectionRequest {
        wallet_address: signal.payload.wallet_address.clone(),
        token_address: signal.token_address().to_string(),
        action: signal.payload.action,
        source_amount_sol: signal.payload.amount_sol,
        ingress: crate::engine::Ingress::Webhook,
        source_slot: None,
        exit_fraction: signal.payload.exit_fraction,
    };
    let decision = state.selection.decide(&req).await;

    if !decision.admitted {
        let reason = decision
            .rejection_reason
            .clone()
            .unwrap_or_else(|| "rejected".to_string());
        let code = decision.rejection_code.unwrap_or("REJECTED");
        tracing::info!(
            trade_uuid = %signal.trade_uuid,
            code = code,
            reason = %reason,
            "Signal rejected by selection service"
        );
        // Log to dead letter queue (best-effort)
        let _ = state
            .db
            .insert_dlq(
                Some(&signal.trade_uuid),
                &serde_json::to_string(&signal.payload).unwrap_or_default(),
                code,
                Some(&reason),
                None,
            )
            .await;
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(WebhookResponse {
                status: WebhookStatus::Rejected,
                trade_uuid: signal.trade_uuid,
                reason: Some(reason),
            }),
        ));
    }

    // Apply the decision to the signal.
    let trade_amount_sol = decision.size_sol.unwrap_or(signal.payload.amount_sol);
    signal.payload.amount_sol = trade_amount_sol;
    if let Some(strategy) = decision.strategy {
        signal.payload.strategy = strategy;
    }
    signal.liquidity_usd = decision.liquidity_usd;
    // If the fast-path safety check errored, flag for mandatory slow-path.
    if decision.fast_check_errored {
        signal.force_slow_path = true;
    }

    tracing::info!(
        trade_uuid = %signal.trade_uuid,
        decision_id = %decision.decision_id,
        strategy = ?signal.payload.strategy,
        token = %signal.payload.token,
        amount_sol = %trade_amount_sol,
        wqs = ?decision.wqs,
        quality = ?decision.quality_score,
        consensus = ?decision.consensus_wallet_count,
        action = %signal.payload.action,
        "Signal admitted by selection service"
    );

    // Insert into database as PENDING
    state
        .db
        .insert_trade(&InsertTrade {
            trade_uuid: signal.trade_uuid.clone(),
            wallet_address: signal.payload.wallet_address.clone(),
            token_address: signal.token_address().to_string(),
            token_symbol: Some(signal.payload.token.clone()),
            strategy: signal.payload.strategy.to_string(),
            side: signal.payload.action.to_string(),
            amount_sol: trade_amount_sol,
            status: "PENDING".to_string(),
        })
        .await?;

    // C1: link the persisted decision record to its trade (fire-and-forget).
    if let Some(recorder) = state.selection.decision_recorder() {
        recorder.link_trade(decision.decision_id.clone(), signal.trade_uuid.clone());
    }

    tracing::info!(
        trade_uuid = %signal.trade_uuid,
        strategy = %signal.payload.strategy,
        token = %signal.payload.token,
        amount_sol = trade_amount_sol.to_f64().unwrap_or(0.0),
        action = %signal.payload.action,
        "Signal received and validated"
    );

    // Queue for execution — use the real WQS from the decision (not the
    // historically-buggy tuple field that passed wqs_confidence instead).
    match state.engine.queue_signal(signal.clone(), decision.wqs).await {
        Ok(()) => {
            // Update status to QUEUED
            state
                .db
                .update_trade_status(&UpdateTradeStatus {
                    trade_uuid: signal.trade_uuid.clone(),
                    status: "QUEUED".to_string(),
                    tx_signature: None,
                    error_message: None,
                    network_fee_sol: None,
                })
                .await?;

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

            // Update trade status to DEAD_LETTER first, then insert the DLQ entry.
            // The status update is authoritative; the DLQ entry is supplementary audit data.
            state
                .db
                .update_trade_status(&UpdateTradeStatus {
                    trade_uuid: signal.trade_uuid.clone(),
                    status: "DEAD_LETTER".to_string(),
                    tx_signature: None,
                    error_message: Some(e.to_string()),
                    network_fee_sol: None,
                })
                .await?;

            // Log to dead letter queue (best-effort — status is already DEAD_LETTER above).
            if let Err(dlq_err) = state
                .db
                .insert_dlq(
                    Some(&signal.trade_uuid),
                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                    "QUEUE_FULL",
                    Some(&e.to_string()),
                    None,
                )
                .await
            {
                tracing::error!(
                    error = %dlq_err,
                    trade_uuid = %signal.trade_uuid,
                    "Failed to insert into dead-letter queue — trade status is DEAD_LETTER but has no DLQ entry. Manual investigation required."
                );
            }

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
        assert_eq!(payload.amount_sol, Decimal::from_str("0.5").unwrap());
    }
}
