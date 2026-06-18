//! Telegram signal handler
//!
//! Handles incoming signals from the Python Telegram collector service.

use crate::circuit_breaker::CircuitBreaker;
use crate::engine::EngineHandle;
use crate::error::AppError;
use crate::models::{Action, Signal, SignalPayload, Strategy};
use crate::telegram::parser::{RawTelegramSignal, TelegramParser};
use crate::telegram::source_manager::{ChannelConfig, TelegramSourceManager};
use axum::{
    extract::{Path, State},
    response::Json,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use tracing;

/// Telegram signal request from Python collector
#[derive(Debug, Deserialize)]
pub struct TelegramSignalRequest {
    pub channel: String,
    pub channel_id: i64,
    pub message_id: i32,
    pub timestamp: i64,
    pub text: String,
}

/// Telegram signal response
#[derive(Debug, Serialize)]
pub struct TelegramSignalResponse {
    pub success: bool,
    pub message: String,
    pub signal_uuid: Option<String>,
}

/// Telegram signal endpoint
/// POST /api/v1/telegram/signal
pub async fn telegram_signal_handler(
    State(state): State<Arc<TelegramHandlerState>>,
    Json(req): Json<TelegramSignalRequest>,
) -> Result<Json<TelegramSignalResponse>, AppError> {
    tracing::info!(
        channel = %req.channel,
        message_id = req.message_id,
        "Received Telegram signal"
    );

    // Convert to RawTelegramSignal
    let raw_signal = RawTelegramSignal {
        channel: req.channel.clone(),
        channel_id: req.channel_id,
        message_id: req.message_id,
        timestamp: req.timestamp,
        text: req.text.clone(),
    };

    // Parse signal
    let parsed = state.parser.parse(raw_signal).map_err(|e| {
        AppError::Signal(format!("Parse error: {}", e))
    })?;

    // Check signal staleness (age validation)
    let now = chrono::Utc::now().timestamp();
    let signal_age = now - parsed.timestamp;

    // Get or create channel in signal_sources table
    let signal_source = match state.source_manager.get_channel(&parsed.channel).await {
        Ok(Some(source)) => source,
        Ok(None) => {
            // Auto-register channel if not exists
            tracing::warn!(
                channel = %parsed.channel,
                "Channel not registered, attempting auto-registration"
            );

            let config = ChannelConfig {
                channel_id: parsed.channel.clone(),
                channel_id_numeric: Some(req.channel_id),
                enabled: true,
                min_quality_score: 50.0,
                max_signals_per_hour: 30,
                strategy_preference: None,
                max_signal_age_seconds: 300, // Default 5 minutes
            };

            let source_id = state.source_manager.register_channel(&config).await.map_err(|e| {
                AppError::Internal(format!("Failed to auto-register channel: {}", e))
            })?;

            // Fetch the newly created channel
            state.source_manager.get_channel_by_id(source_id).await
                .map_err(|e| AppError::Internal(format!("Failed to fetch new channel: {}", e)))?
                .unwrap()
        }
        Err(e) => {
            return Err(AppError::Internal(format!("Database error: {}", e)));
        }
    };

    // Check if channel is enabled
    if !signal_source.enabled {
        return Ok(Json(TelegramSignalResponse {
            success: false,
            message: format!("Channel {} is disabled", parsed.channel),
            signal_uuid: None,
        }));
    }

    // Check max signal age from channel config
    let max_age = signal_source.max_signal_age_seconds as i64;

    if signal_age > max_age {
        tracing::debug!(
            channel = %parsed.channel,
            signal_age_sec = signal_age,
            max_age_sec = max_age,
            "Signal too old, rejecting"
        );
        return Ok(Json(TelegramSignalResponse {
            success: false,
            message: format!("Signal too old: {} seconds (max: {}s)", signal_age, max_age),
            signal_uuid: None,
        }));
    }

    // Apply age decay to confidence score
    let age_decay_multiplier = if signal_age < 60 {
        1.0 // Fresh: 0-60 seconds = 100%
    } else if signal_age < 180 {
        0.7 // 1-3 minutes = 70%
    } else if signal_age < 300 {
        0.4 // 3-5 minutes = 40%
    } else {
        0.1 // Should be rejected by max_age check, but apply strong decay
    };

    tracing::debug!(
        channel = %parsed.channel,
        signal_age_sec = signal_age,
        age_decay = age_decay_multiplier,
        "Signal staleness check passed with age decay"
    );

    // Check circuit breaker
    if !state.circuit_breaker.is_trading_allowed() {
        let reason = state
            .circuit_breaker
            .trip_reason()
            .map(|r| r.to_string())
            .unwrap_or_else(|| "Circuit breaker tripped".to_string());
        tracing::warn!(
            channel = %parsed.channel,
            reason = %reason,
            "Telegram signal blocked by circuit breaker"
        );
        return Ok(Json(TelegramSignalResponse {
            success: false,
            message: format!("Circuit breaker tripped: {}", reason),
            signal_uuid: None,
        }));
    }

    // Get channel quality score
    let channel_quality = signal_source.quality_score;
    let confidence_score = parsed.confidence.to_score();

    // Strategy selection: High confidence + High quality = Spear, else Shield
    let strategy = if confidence_score >= 0.8 && channel_quality >= 70.0 {
        Strategy::Spear
    } else {
        Strategy::Shield
    };

    // Determine action (default to BUY for signals)
    let action = Action::Buy;

    // Default amount for Shield is 0.1 SOL, Spear is 0.05 SOL
    let amount_sol = if matches!(strategy, Strategy::Shield) {
        Decimal::from_str("0.1").unwrap_or(Decimal::ZERO)
    } else {
        Decimal::from_str("0.05").unwrap_or(Decimal::ZERO)
    };

    // Create signal payload with source attribution
    let payload = SignalPayload {
        strategy,
        token: parsed.token_symbol.clone().unwrap_or_else(|| parsed.token_address.clone()),
        token_address: Some(parsed.token_address.clone()),
        action,
        amount_sol,
        wallet_address: String::new(), // Empty for non-wallet signals
        trade_uuid: None,
        exit_fraction: None,
        signal_source_id: Some(signal_source.id),
        signal_source: "TELEGRAM".to_string(),
    };

    // Create signal with original signal ID for attribution
    let signal_uuid = payload.generate_trade_uuid(parsed.timestamp);
    let signal = Signal {
        trade_uuid: signal_uuid.clone(),
        payload: payload.clone(),
        timestamp: parsed.timestamp,
        source_ip: Some("telegram_collector".to_string()),
        liquidity_usd: None,
        force_slow_path: false,
        token_decimals: None,
        original_signal_id: Some(format!("{}:{}", parsed.channel, req.message_id)),
    };

    // Queue signal to engine with channel quality score
    state.engine.queue_signal(signal, Some(channel_quality)).await.map_err(|e| {
        tracing::error!(
            channel = %parsed.channel,
            error = %e,
            "Failed to queue Telegram signal"
        );
        AppError::Internal(format!("Failed to queue signal: {}", e))
    })?;

    tracing::info!(
        channel = %parsed.channel,
        token = %parsed.token_address,
        strategy = ?strategy,
        signal_uuid = %signal_uuid,
        "Telegram signal queued successfully"
    );

    Ok(Json(TelegramSignalResponse {
        success: true,
        message: "Signal queued".to_string(),
        signal_uuid: Some(signal_uuid),
    }))
}

/// Telegram handler state
#[derive(Clone)]
pub struct TelegramHandlerState {
    pub db: sqlx::SqlitePool,
    pub engine: EngineHandle,
    pub circuit_breaker: Arc<CircuitBreaker>,
    pub source_manager: Arc<TelegramSourceManager>,
    pub parser: Arc<TelegramParser>,
}

/// Get Telegram signal status
/// GET /api/v1/telegram/status
pub async fn telegram_status_handler(
    State(state): State<Arc<TelegramHandlerState>>,
) -> Result<Json<TelegramStatus>, AppError> {
    let channels = state.source_manager.get_enabled_channels().await;

    let mut channel_statuses = Vec::new();
    for c in channels.iter() {
        let health = state
            .source_manager
            .check_channel_health(&c.source_id)
            .await
            .unwrap_or_else(|_| crate::telegram::source_manager::ChannelHealth {
                parse_success_rate: 0.0,
                avg_liquidity_usd: None,
                rejection_rate: 0.0,
                is_healthy: false,
            });

        channel_statuses.push(ChannelStatus {
            channel_id: c.source_id.clone(),
            enabled: c.enabled,
            quality_score: c.quality_score,
            max_signals_per_hour: c.max_signals_per_hour as u32,
            max_signal_age_seconds: c.max_signal_age_seconds as u64,
            is_healthy: health.is_healthy,
            parse_success_rate: health.parse_success_rate,
            rejection_rate: health.rejection_rate,
        });
    }

    Ok(Json(TelegramStatus {
        enabled_channels: channel_statuses.len() as u32,
        channels: channel_statuses,
    }))
}

#[derive(Debug, Serialize)]
pub struct TelegramStatus {
    pub enabled_channels: u32,
    pub channels: Vec<ChannelStatus>,
}

#[derive(Debug, Serialize)]
pub struct ChannelStatus {
    pub channel_id: String,
    pub enabled: bool,
    pub quality_score: f64,
    pub max_signals_per_hour: u32,
    pub max_signal_age_seconds: u64,
    pub is_healthy: bool,
    pub parse_success_rate: f64,
    pub rejection_rate: f64,
}

/// Enable or disable a Telegram channel
/// POST /api/v1/telegram/channel/:channel_id/enable
/// POST /api/v1/telegram/channel/:channel_id/disable
pub async fn telegram_channel_toggle_handler(
    State(state): State<Arc<TelegramHandlerState>>,
    Path((channel_id, action)): Path<(String, String)>,
    axum::Extension(auth): axum::Extension<crate::middleware::AuthExtension>,
) -> Result<Json<serde_json::Value>, AppError> {
    use crate::middleware::Role;

    if !auth.0.role.has_permission(Role::Operator) {
        return Err(AppError::Forbidden("Insufficient permissions".to_string()));
    }

    let enabled = match action.as_str() {
        "enable" => true,
        "disable" => false,
        _ => {
            return Err(AppError::Signal("Invalid action, use 'enable' or 'disable'".to_string()));
        }
    };

    state
        .source_manager
        .set_channel_enabled(&channel_id, enabled)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to toggle channel: {}", e)))?;

    tracing::info!(
        channel = %channel_id,
        enabled = enabled,
        "Channel {}abled",
        if enabled { "en" } else { "dis" }
    );

    Ok(Json(serde_json::json!({
        "channel_id": channel_id,
        "enabled": enabled
    })))
}
