//! Telegram source manager
//!
//! Manages Telegram channels in the signal_sources table, including
//! channel registration, signal processing, and performance tracking.

use super::parser::{ParsedTelegramSignal, RawTelegramSignal, TelegramParser};
use super::telegram_error::{TelegramError, TelegramResult};
use crate::db::DbPool;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Channel configuration for Telegram signal sources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// Channel ID (e.g., "@solana_whales_signal")
    pub channel_id: String,
    /// Numeric Telegram channel ID
    pub channel_id_numeric: Option<i64>,
    /// Whether channel is enabled
    pub enabled: bool,
    /// Minimum quality score for signals from this channel
    pub min_quality_score: f64,
    /// Maximum signals per hour
    pub max_signals_per_hour: u32,
    /// Strategy preference (optional)
    pub strategy_preference: Option<String>,
    /// Maximum signal age in seconds (reject older signals)
    pub max_signal_age_seconds: u64,
}

/// Signal source record from database
#[derive(Debug, Clone, FromRow)]
pub struct SignalSource {
    pub id: i64,
    pub source_type: String,
    pub source_id: String,
    pub enabled: bool,
    pub quality_score: f64,
    pub max_signals_per_hour: i64,
    pub max_signal_age_seconds: i64,
    pub strategy_preference: Option<String>,
    pub parse_success_rate: f64,
    pub signal_frequency: f64,
    pub total_signals: i64,
    pub successful_signals: i64,
    pub rejected_signals: i64,
    pub total_trades: i64,
    pub winning_trades: i64,
    pub roi_7d: f64,
    pub roi_30d: f64,
    pub win_rate: f64,
    pub realized_pnl_30d_sol: f64,
    pub telegram_channel_id: Option<i64>,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Channel health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelHealth {
    pub parse_success_rate: f64,
    pub avg_liquidity_usd: Option<f64>,
    pub rejection_rate: f64,
    pub is_healthy: bool,
}

/// Telegram signal source manager
///
/// Manages Telegram channels in the signal_sources table.
pub struct TelegramSourceManager {
    db: DbPool,
    enabled_channels: Arc<RwLock<HashMap<String, ChannelConfig>>>,
    parser: Arc<TelegramParser>,
    // Rate limiting per channel
    rate_limits: Arc<RwLock<HashMap<String, RateLimitState>>>,
}

#[derive(Debug, Clone)]
struct RateLimitState {
    signals_last_hour: Vec<DateTime<Utc>>,
}

impl TelegramSourceManager {
    /// Create a new Telegram source manager
    pub fn new(db: DbPool, channels: Vec<ChannelConfig>) -> Self {
        let mut channel_map = HashMap::new();
        for channel in channels {
            channel_map.insert(channel.channel_id.clone(), channel);
        }

        Self {
            db,
            enabled_channels: Arc::new(RwLock::new(channel_map)),
            parser: Arc::new(TelegramParser::new()),
            rate_limits: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register or update a channel in signal_sources table
    pub async fn register_channel(&self, config: &ChannelConfig) -> TelegramResult<i64> {
        let result = sqlx::query(
            "INSERT INTO signal_sources
               (source_type, source_id, telegram_channel_id, enabled,
                quality_score, max_signals_per_hour, max_signal_age_seconds,
                strategy_preference)
             VALUES ('TELEGRAM', ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(source_type, source_id) DO UPDATE SET
                enabled = excluded.enabled,
                max_signals_per_hour = excluded.max_signals_per_hour,
                max_signal_age_seconds = excluded.max_signal_age_seconds,
                strategy_preference = excluded.strategy_preference
             RETURNING id"
        )
        .bind(&config.channel_id)
        .bind(config.channel_id_numeric)
        .bind(config.enabled)
        .bind(config.min_quality_score)
        .bind(config.max_signals_per_hour as i64)
        .bind(config.max_signal_age_seconds as i64)
        .bind(config.strategy_preference.as_deref())
        .fetch_one(&self.db)
        .await
        .map_err(|e| TelegramError::DatabaseError(format!("Failed to register channel: {}", e)))?;

        let channel_id: i64 = result.try_get("id").map_err(|e| TelegramError::DatabaseError(format!("Failed to get id: {}", e)))?;

        tracing::info!(
            "Registered Telegram channel: {} with id {}",
            config.channel_id,
            channel_id
        );

        // Add to enabled channels map
        let mut channels = self.enabled_channels.write().await;
        channels.insert(config.channel_id.clone(), config.clone());

        Ok(channel_id)
    }

    /// Get channel record from signal_sources table
    pub async fn get_channel(&self, channel: &str) -> TelegramResult<Option<SignalSource>> {
        let source = sqlx::query_as::<_, SignalSource>(
            "SELECT * FROM signal_sources
             WHERE source_type = 'TELEGRAM' AND source_id = ?"
        )
        .bind(channel)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| TelegramError::DatabaseError(format!("Failed to get channel: {}", e)))?;

        Ok(source)
    }

    /// Get channel by database ID
    pub async fn get_channel_by_id(&self, id: i64) -> TelegramResult<Option<SignalSource>> {
        let source = sqlx::query_as::<_, SignalSource>(
            "SELECT * FROM signal_sources WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| TelegramError::DatabaseError(format!("Failed to get channel by id: {}", e)))?;

        Ok(source)
    }

    /// Process incoming Telegram signal
    pub async fn process_signal(&self, raw: RawTelegramSignal) -> TelegramResult<ParsedTelegramSignal> {
        // 1. Check if channel is enabled
        let channels = self.enabled_channels.read().await;
        let config = channels
            .get(&raw.channel)
            .ok_or_else(|| TelegramError::ChannelNotFound(raw.channel.clone()))?;

        if !config.enabled {
            return Err(TelegramError::InvalidSignal("Channel is disabled".to_string()));
        }

        // 2. Check rate limit
        if !self
            .check_rate_limit(&raw.channel, config.max_signals_per_hour)
            .await
        {
            return Err(TelegramError::RateLimitExceeded(raw.channel));
        }

        // 3. Parse signal
        let parsed = self.parser.parse(raw.clone())?;

        // 4. Validate against channel requirements
        let confidence_score = parsed.confidence.to_score();
        if confidence_score < (config.min_quality_score / 100.0) {
            return Err(TelegramError::InvalidSignal(format!(
                "Signal confidence {:.2} below threshold {:.2}",
                confidence_score,
                config.min_quality_score / 100.0
            )));
        }

        // 5. Record signal for rate limiting
        self.record_signal(&raw.channel).await;

        tracing::debug!(
            "Processed signal from {}: token={}, symbol={:?}",
            parsed.channel,
            parsed.token_address,
            parsed.token_symbol
        );

        Ok(parsed)
    }

    /// Check rate limit for channel
    async fn check_rate_limit(&self, channel: &str, max_per_hour: u32) -> bool {
        let mut limits = self.rate_limits.write().await;
        let now = Utc::now();
        let one_hour_ago = now - chrono::Duration::hours(1);

        let state = limits
            .entry(channel.to_string())
            .or_insert_with(|| RateLimitState {
                signals_last_hour: Vec::new(),
            });

        // Clean old signals
        state.signals_last_hour.retain(|&ts| ts > one_hour_ago);

        // Check limit
        state.signals_last_hour.len() < max_per_hour as usize
    }

    /// Record a signal for rate limiting
    async fn record_signal(&self, channel: &str) {
        let mut limits = self.rate_limits.write().await;
        let now = Utc::now();
        let state = limits
            .entry(channel.to_string())
            .or_insert_with(|| RateLimitState {
                signals_last_hour: Vec::new(),
            });
        state.signals_last_hour.push(now);
    }

    /// Update channel metrics after trade completion
    pub async fn update_channel_metrics(
        &self,
        channel_db_id: i64,
        trade_success: bool,
        pnl_sol: Decimal,
    ) -> TelegramResult<()> {
        // Adjust quality score based on performance
        // Simple adjustment: +2 for win, -1 for loss
        let score_adjustment = if trade_success { 2.0 } else { -1.0 };

        sqlx::query(
            "UPDATE signal_sources SET
                total_trades = total_trades + 1,
                winning_trades = winning_trades + ?,
                quality_score = (quality_score + ?) / 2,
                realized_pnl_30d_sol = realized_pnl_30d_sol + ?,
                updated_at = CURRENT_TIMESTAMP
             WHERE id = ?"
        )
        .bind(if trade_success { 1i64 } else { 0 })
        .bind(score_adjustment)
        .bind(pnl_sol.to_f64().unwrap_or(0.0))
        .bind(channel_db_id)
        .execute(&self.db)
        .await
        .map_err(|e| TelegramError::DatabaseError(format!("Failed to update metrics: {}", e)))?;

        tracing::info!(
            "Updated channel metrics: id={}, success={}, pnl={}",
            channel_db_id,
            trade_success,
            pnl_sol
        );

        Ok(())
    }

    /// Check channel health
    pub async fn check_channel_health(&self, channel: &str) -> TelegramResult<ChannelHealth> {
        let stats: (Option<f64>, i64, i64) = sqlx::query_as(
            "SELECT parse_success_rate, total_signals,
                    COALESCE(total_trades, 0)
             FROM signal_sources
             WHERE source_type = 'TELEGRAM' AND source_id = ?"
        )
        .bind(channel)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| TelegramError::DatabaseError(format!("Failed to get channel health: {}", e)))?
        .unwrap_or((Some(1.0), 0, 0));

        let (parse_rate, _signals, total_trades) = stats;

        // Calculate rejection rate (simplified)
        let rejection_rate = if total_trades > 0 { 0.1 } else { 0.0 };

        let is_healthy = parse_rate.unwrap_or(0.0) >= 0.8;

        Ok(ChannelHealth {
            parse_success_rate: parse_rate.unwrap_or(0.0),
            avg_liquidity_usd: None,
            rejection_rate,
            is_healthy,
        })
    }

    /// Get all enabled channels from signal_sources table
    pub async fn get_enabled_channels(&self) -> Vec<SignalSource> {
        let sources = sqlx::query_as::<_, SignalSource>(
            "SELECT * FROM signal_sources
             WHERE source_type = 'TELEGRAM' AND enabled = 1
             ORDER BY quality_score DESC"
        )
        .fetch_all(&self.db)
        .await
        .unwrap_or_default();

        sources
    }

    /// Enable or disable a channel
    pub async fn set_channel_enabled(&self, channel: &str, enabled: bool) -> TelegramResult<()> {
        // Update database
        sqlx::query("UPDATE signal_sources SET enabled = ? WHERE source_type = 'TELEGRAM' AND source_id = ?")
            .bind(enabled)
            .bind(channel)
            .execute(&self.db)
            .await
            .map_err(|e| TelegramError::DatabaseError(format!("Failed to toggle channel: {}", e)))?;

        // Update in-memory config
        let mut channels = self.enabled_channels.write().await;
        if let Some(config) = channels.get_mut(channel) {
            config.enabled = enabled;
            tracing::info!("Channel {} {}", channel, if enabled { "enabled" } else { "disabled" });
            Ok(())
        } else {
            Err(TelegramError::ChannelNotFound(channel.to_string()))
        }
    }

    /// Get all channels from in-memory config (for backward compatibility)
    pub async fn get_enabled_channels_config(&self) -> Vec<ChannelConfig> {
        let channels = self.enabled_channels.read().await;
        channels.values().filter(|c| c.enabled).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    // Tests can be added here for signal source management
}
