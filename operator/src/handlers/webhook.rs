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

    // Tracks whether the fast-path check returned an error (vs. clean pass/reject).
    // Used to set Signal::force_slow_path after the signal is constructed.
    let mut fast_check_errored = false;
    // FIX 9: Store fast_check liquidity result here to avoid calling fast_check twice
    let mut fast_check_liquidity_usd: Option<rust_decimal::Decimal> = None;

    // Fast path token safety check (for BUY signals only)
    // EXIT signals don't need token validation, SELL signals already own the token
    if payload.strategy != Strategy::Exit {
        if let Some(ref token_address) = payload.token_address {
            if token_address.parse::<Pubkey>().is_err() {
                return Ok((
                    StatusCode::BAD_REQUEST,
                    Json(WebhookResponse {
                        status: WebhookStatus::Rejected,
                        trade_uuid,
                        reason: Some(format!("Invalid token address format: {}", token_address)),
                    }),
                ));
            }
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
                        let _ = state
                            .db
                            .insert_dlq(
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
                    // FIX 9: Capture liquidity_usd from this result for reuse below
                    fast_check_liquidity_usd = result.liquidity_usd;
                }
                Err(e) => {
                    // Fast-check itself returned an error (RPC/network failure, not just
                    // "unknown/unchecked"). Mark the signal so the engine enforces the
                    // slow-path. If slow-path is also unavailable the trade must be
                    // blocked — do NOT silently pass through an unchecked token.
                    tracing::warn!(
                        token = %token_address,
                        error = %e,
                        "Fast-path token check errored; signal flagged for mandatory slow-path verification"
                    );
                    // force_slow_path is set on the Signal below after it is constructed
                    fast_check_errored = true;
                }
            }
        }
    }

    // Create signal
    let mut signal = Signal::new(payload, timestamp, None);

    // Populate token decimals from on-chain metadata for correct fill price conversion.
    // [B-M1] Without correct decimals, lamports-per-base-unit → SOL-per-token assumes 9 decimals,
    // which is wrong for USDC (6), USDT (6), and other non-standard tokens.
    if let Some(ref token_address) = signal.payload.token_address {
        signal.token_decimals = state.token_parser.get_token_decimals(token_address).await;
    }

    // If fast-check errored (RPC/network failure), flag signal for mandatory slow-path.
    // The engine will reject the trade if slow-path is also unavailable.
    if fast_check_errored {
        signal.force_slow_path = true;
    }

    // Fetch wallet data once (used for both quality check and queue routing).
    // For BUY signals, also gate on wallet status — only ACTIVE wallets may trigger buys.
    let wallet_data = if signal.payload.action == crate::models::Action::Buy {
        match state.db.get_wallet(&signal.payload.wallet_address).await {
            Ok(Some(wallet)) => {
                if wallet.status != "ACTIVE" {
                    tracing::warn!(
                        trade_uuid = %signal.trade_uuid,
                        wallet = %signal.payload.wallet_address,
                        status = %wallet.status,
                        "BUY signal from non-ACTIVE wallet rejected"
                    );
                    return Ok((
                        StatusCode::BAD_REQUEST,
                        Json(WebhookResponse {
                            status: WebhookStatus::Rejected,
                            trade_uuid: signal.trade_uuid,
                            reason: Some(format!(
                                "Wallet status is {}, only ACTIVE wallets may trigger buys",
                                wallet.status
                            )),
                        }),
                    ));
                }
                let win_rate = wallet
                    .win_rate
                    .unwrap_or(Decimal::from_f64_retain(0.5).unwrap_or(Decimal::ZERO));
                Some((
                    wallet.wqs_score.and_then(|d| d.to_f64()).unwrap_or(50.0),
                    wallet.wqs_score,
                    win_rate,
                ))
            }
            Ok(None) => {
                tracing::warn!(
                    trade_uuid = %signal.trade_uuid,
                    wallet = %signal.payload.wallet_address,
                    "BUY signal from unknown wallet rejected"
                );
                return Ok((
                    StatusCode::BAD_REQUEST,
                    Json(WebhookResponse {
                        status: WebhookStatus::Rejected,
                        trade_uuid: signal.trade_uuid,
                        reason: Some("Unknown wallet — not in roster".to_string()),
                    }),
                ));
            }
            Err(e) => {
                tracing::error!(
                    trade_uuid = %signal.trade_uuid,
                    error = %e,
                    "DB error fetching wallet status, rejecting BUY (fail-closed)"
                );
                return Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(WebhookResponse {
                        status: WebhookStatus::Rejected,
                        trade_uuid: signal.trade_uuid,
                        reason: Some("DB error during wallet status check".to_string()),
                    }),
                ));
            }
        }
    } else {
        None
    };

    // For SELL/EXIT signals: validate wallet exists in roster and cap amount to max_position_sol.
    // BUY signals already went through the full wallet gate above; SELL signals previously
    // skipped it entirely, allowing arbitrary amount_sol from the caller.
    if signal.payload.action != crate::models::Action::Buy {
        match state.db.get_wallet(&signal.payload.wallet_address).await {
            Ok(None) => {
                return Ok((
                    StatusCode::BAD_REQUEST,
                    Json(WebhookResponse {
                        status: WebhookStatus::Rejected,
                        trade_uuid: signal.trade_uuid,
                        reason: Some("Unknown wallet — not in roster".to_string()),
                    }),
                ));
            }
            Err(e) => {
                tracing::error!(error = %e, "DB error on SELL wallet check, rejecting (fail-closed)");
                return Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(WebhookResponse {
                        status: WebhookStatus::Rejected,
                        trade_uuid: signal.trade_uuid,
                        reason: Some("DB error during wallet check".to_string()),
                    }),
                ));
            }
            Ok(Some(_)) => {} // wallet exists, proceed
        }
    }

    // Trade amount: starts as payload value for SELL/EXIT; overridden by PositionSizer for BUY.
    let mut trade_amount_sol = signal.payload.amount_sol;

    // Cap SELL/EXIT amounts to max_position_sol — prevents a caller from supplying an
    // arbitrarily large amount and closing more than we actually hold.
    if signal.payload.action != crate::models::Action::Buy
        && trade_amount_sol > state.max_position_sol
    {
        tracing::warn!(
            trade_uuid = %signal.trade_uuid,
            original = %trade_amount_sol,
            capped_to = %state.max_position_sol,
            "SELL amount exceeds max_position_sol, capping"
        );
        trade_amount_sol = state.max_position_sol;
    }

    // Signal quality check (for BUY signals only, EXIT/SELL don't need quality check)
    if signal.payload.action == crate::models::Action::Buy {
        let wallet_wqs = wallet_data
            .as_ref()
            .map(|(wqs, _, _)| wqs)
            .copied()
            .unwrap_or(50.0);

        // Check if consensus signal using SignalAggregator
        let mut consensus_wallet_count = None;
        let is_consensus = if let Some(ref aggregator) = state.signal_aggregator {
            if let Some(ref token_address) = signal.payload.token_address {
                // Add signal to aggregator and check for consensus
                if let Some(consensus) = aggregator
                    .add_signal(
                        &signal.payload.wallet_address,
                        token_address,
                        "BUY",
                        signal.payload.amount_sol,
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
                    consensus_wallet_count = Some(consensus.wallet_count);
                    true
                } else {
                    consensus_wallet_count = Some(1);
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        // Persist BUY signal to signal_aggregation table so stop-loss manager
        // can detect consensus when checking existing open positions.
        // The stop-loss query counts DISTINCT wallet_address entries within the last 5 minutes;
        // inserting a new row each time is correct (NULLs don't conflict in UNIQUE indexes).
        if let Some(ref token_address) = signal.payload.token_address {
            let amount_f64 = signal.payload.amount_sol.to_f64().unwrap_or(0.0);
            let pool = match state.db.pool() {
                DbPool::PostgreSQL(p) => p,
                _ => {
                    return Err(AppError::Internal(
                        "PostgreSQL backend required".to_string(),
                    ))
                }
            };
            if let Err(e) = sqlx::query(
                r#"
                INSERT INTO signal_aggregation
                    (token_address, wallet_address, direction, amount_sol, is_consensus)
                VALUES ($1, $2, 'BUY', $3, $4)
                "#,
            )
            .bind(token_address)
            .bind(&signal.payload.wallet_address)
            .bind(amount_f64)
            .bind(is_consensus)
            .execute(&pool)
            .await
            {
                tracing::warn!(
                    error = %e,
                    trade_uuid = %signal.trade_uuid,
                    "Failed to record signal aggregation — consensus detection may be degraded"
                );
                // Do NOT return early — proceed with trade
            }
        }

        // FIX 9: Reuse liquidity_usd captured from the first fast_check above (no second call)
        let liquidity_usd = fast_check_liquidity_usd.unwrap_or(rust_decimal::Decimal::ZERO);

        // Attach liquidity to the signal so the executor can compute a liquidity-aware
        // slippage estimate when Jupiter price impact data is unavailable.
        if liquidity_usd > rust_decimal::Decimal::ZERO {
            signal.liquidity_usd = Some(liquidity_usd);
        }

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

        // Hard liquidity floor — reject signals for tokens below the strategy-specific
        // minimum before computing the full quality score. Low-liquidity tokens can
        // otherwise pass the quality gate if wallet WQS is high enough.
        let min_liquidity = match signal.payload.strategy {
            Strategy::Shield => state.min_liquidity_shield_usd,
            Strategy::Spear => state.min_liquidity_spear_usd,
            Strategy::Exit => rust_decimal::Decimal::ZERO,
        };
        if !SignalQuality::passes_liquidity_floor(liquidity_usd, min_liquidity) {
            tracing::warn!(
                trade_uuid = %signal.trade_uuid,
                strategy = ?signal.payload.strategy,
                liquidity_usd = %liquidity_usd,
                min_required = %min_liquidity,
                "Signal rejected: liquidity below strategy minimum"
            );
            let _ = state
                .db
                .insert_dlq(
                    Some(&signal.trade_uuid),
                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                    "LIQUIDITY_BELOW_MINIMUM",
                    Some(&format!(
                        "Liquidity ${} < required ${}",
                        liquidity_usd, min_liquidity
                    )),
                    None,
                )
                .await;
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(WebhookResponse {
                    status: WebhookStatus::Rejected,
                    trade_uuid: signal.trade_uuid,
                    reason: Some(format!(
                        "Token liquidity ${} below strategy minimum ${}",
                        liquidity_usd, min_liquidity
                    )),
                }),
            ));
        }

        // Calculate signal quality — pass the wallet count for graduated consensus scoring
        let quality = SignalQuality::calculate(
            wallet_wqs,
            consensus_wallet_count,
            liquidity_usd,
            token_age_hours,
        );

        // Reject if quality too low (threshold from config.strategy.signal_quality_threshold)
        let quality_threshold = match signal.payload.strategy {
            Strategy::Shield => state.shield_signal_quality_threshold,
            Strategy::Spear => state.spear_signal_quality_threshold,
            Strategy::Exit => 0.0,
        };
        if !quality.should_enter(quality_threshold) {
            tracing::warn!(
                trade_uuid = %signal.trade_uuid,
                quality_score = quality.score,
                threshold = quality_threshold,
                wallet_wqs = wallet_wqs,
                liquidity_usd = %liquidity_usd,
                "Signal rejected due to low quality"
            );

            // Log to dead letter queue
            let _ = state
                .db
                .insert_dlq(
                    Some(&signal.trade_uuid),
                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                    "SIGNAL_QUALITY_TOO_LOW",
                    Some(&format!(
                        "Quality score: {:.2} < {:.2}",
                        quality.score, quality_threshold
                    )),
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

        // Compute position size via PositionSizer (Kelly + confidence multipliers).
        // Ignores payload amount_sol — caller must not control trade size.
        if let Some(ref sizer) = state.position_sizer {
            let wallet_success_rate = wallet_data
                .as_ref()
                .map(|(_, _, wr)| *wr)
                .unwrap_or(Decimal::from_f64_retain(0.5).unwrap_or(Decimal::ZERO));

            let regime_multiplier = if let Some(ref regime_detector) = state.market_regime {
                if let Some(ref token_address) = signal.payload.token_address {
                    regime_detector.get_regime_multiplier(token_address)
                } else {
                    Decimal::ONE
                }
            } else {
                Decimal::ONE
            };

            let factors = SizingFactors {
                is_consensus,
                wallet_wqs,
                wallet_success_rate,
                token_age_hours,
                estimated_slippage: Decimal::ZERO,
                signal_quality: Decimal::from_f64_retain(quality.score),
                token_volatility_24h: None,
                wallet_address: signal.payload.wallet_address.clone(),
                total_capital_sol: state.total_capital_sol,
                strategy: signal.payload.strategy,
                consensus_wallet_count,
                regime_multiplier,
            };
            trade_amount_sol = sizer.calculate_size(factors).await;
            if trade_amount_sol.is_zero() {
                tracing::warn!(
                    trade_uuid = %signal.trade_uuid,
                    "Signal rejected: position sizer returned zero size (strategy_max < min_size_sol — check config)"
                );
                let _ = state.db.insert_dlq(
                    Some(&signal.trade_uuid),
                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                    "POSITION_SIZE_ZERO",
                    Some("strategy_max is below min_size_sol; trade rejected to prevent dust transaction"),
                    None,
                )
                .await;
                return Ok((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(WebhookResponse {
                        status: WebhookStatus::Rejected,
                        trade_uuid: signal.trade_uuid,
                        reason: Some(
                            "Position size zero: strategy_max below min_size_sol".to_string(),
                        ),
                    }),
                ));
            }
            tracing::info!(
                trade_uuid = %signal.trade_uuid,
                trade_amount_sol = ?trade_amount_sol,
                "Position size computed by PositionSizer"
            );
        }
    }

    // Update signal payload amount with sized/capped value
    signal.payload.amount_sol = trade_amount_sol;

    // Check portfolio heat (if enabled)
    if let Some(ref portfolio_heat) = state.portfolio_heat {
        match portfolio_heat.can_open_position(trade_amount_sol).await {
            Ok(false) => {
                tracing::warn!(
                    trade_uuid = %signal.trade_uuid,
                    amount_sol = %trade_amount_sol,
                    "Signal rejected: portfolio heat limit reached"
                );

                // Log to dead letter queue
                let _ = state
                    .db
                    .insert_dlq(
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
                // Check strategy allocation heat
                match portfolio_heat
                    .can_open_strategy_position(
                        signal.payload.strategy,
                        trade_amount_sol,
                        state.shield_percent,
                        state.spear_percent,
                    )
                    .await
                {
                    Ok(false) => {
                        tracing::warn!(
                            trade_uuid = %signal.trade_uuid,
                            amount_sol = %trade_amount_sol,
                            strategy = ?signal.payload.strategy,
                            "Signal rejected: strategy allocation limit reached"
                        );

                        let _ = state
                            .db
                            .insert_dlq(
                                Some(&signal.trade_uuid),
                                &serde_json::to_string(&signal.payload).unwrap_or_default(),
                                "STRATEGY_HEAT_LIMIT",
                                Some("Strategy allocation limit reached"),
                                None,
                            )
                            .await;

                        return Ok((
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(WebhookResponse {
                                status: WebhookStatus::Rejected,
                                trade_uuid: signal.trade_uuid,
                                reason: Some(format!(
                                    "Strategy allocation limit reached for {:?}",
                                    signal.payload.strategy
                                )),
                            }),
                        ));
                    }
                    Ok(true) => {}
                    Err(e) => {
                        tracing::warn!(
                            trade_uuid = %signal.trade_uuid,
                            error = %e,
                            "Strategy heat check failed, allowing trade"
                        );
                    }
                }
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

    tracing::info!(
        trade_uuid = %signal.trade_uuid,
        strategy = %signal.payload.strategy,
        token = %signal.payload.token,
        amount_sol = trade_amount_sol.to_f64().unwrap_or(0.0),
        action = %signal.payload.action,
        "Signal received and validated"
    );

    // Use cached wallet data for queue routing
    let wallet_wqs: Option<f64> = wallet_data
        .as_ref()
        .and_then(|(_, wqs, _)| wqs.map(|d| d.to_f64().unwrap_or(0.0)));

    // Queue for execution
    match state.engine.queue_signal(signal.clone(), wallet_wqs).await {
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
