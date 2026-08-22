//! Telegram notification service
//!
//! Sends alerts via Telegram Bot API with rate limiting to prevent spam.

use super::{AlertLevel, NotificationEvent, NotificationService};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Telegram notifier configuration
#[derive(Clone)]
pub struct TelegramConfig {
    /// Bot token from @BotFather
    pub bot_token: String,
    /// Chat ID to send messages to
    pub chat_id: String,
    /// Whether notifications are enabled
    pub enabled: bool,
    /// Minimum interval between messages of same type (seconds)
    pub rate_limit_seconds: u64,
}

impl std::fmt::Debug for TelegramConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramConfig")
            .field("bot_token", &"[REDACTED]")
            .field("chat_id", &"[REDACTED]")
            .field("enabled", &self.enabled)
            .field("rate_limit_seconds", &self.rate_limit_seconds)
            .finish()
    }
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            bot_token: String::new(),
            chat_id: String::new(),
            enabled: false,
            rate_limit_seconds: 60, // 1 minute between similar messages
        }
    }
}

/// Rate limiter for notifications
struct RateLimiter {
    /// Last sent time for each message type
    last_sent: RwLock<HashMap<String, Instant>>,
    /// Minimum interval between messages
    interval: Duration,
}

impl RateLimiter {
    fn new(interval_seconds: u64) -> Self {
        Self {
            last_sent: RwLock::new(HashMap::new()),
            interval: Duration::from_secs(interval_seconds),
        }
    }

    /// Atomically check the rate limit AND reserve the slot under a single
    /// write lock. The slot stays reserved even if the send later fails, so an
    /// API outage cannot turn into a request storm. Expired entries are pruned
    /// lazily to bound the map size.
    fn try_acquire(&self, key: &str) -> bool {
        let mut last_sent = self.last_sent.write();
        let now = Instant::now();

        if last_sent.len() > 1000 {
            last_sent.retain(|_, last| now.saturating_duration_since(*last) < self.interval);
        }

        match last_sent.get(key) {
            Some(last) if now.saturating_duration_since(*last) < self.interval => false,
            _ => {
                last_sent.insert(key.to_string(), now);
                true
            }
        }
    }

    /// Get rate limit key for an event
    fn get_key(event: &NotificationEvent) -> String {
        match event {
            NotificationEvent::CircuitBreakerTriggered { .. } => "circuit_breaker".to_string(),
            NotificationEvent::CircuitBreakerRecovered => "circuit_breaker_recovered".to_string(),
            NotificationEvent::WalletDrained { .. } => "wallet_drained".to_string(),
            NotificationEvent::SystemCrash { component } => format!("system_crash:{}", component),
            NotificationEvent::PositionExited {
                token, strategy, ..
            } => {
                format!("position:{}:{}", token, strategy)
            }
            NotificationEvent::RpcFallback { .. } => "rpc_fallback".to_string(),
            NotificationEvent::WalletPromoted { address, .. } => {
                format!("wallet_promoted:{}", address)
            }
            NotificationEvent::DailySummary { .. } => "daily_summary".to_string(),
            // Jito-specific notifications: use same routing as RPC fallback
            NotificationEvent::JitoFallbackTriggered { .. } => "jito_fallback".to_string(),
            NotificationEvent::JitoRecovered { .. } => "jito_recovered".to_string(),
            NotificationEvent::JitoHealthChanged { .. } => "jito_health".to_string(),
            NotificationEvent::ShadowRecordingGap { .. } => "shadow_recording_gap".to_string(),
        }
    }
}

/// Telegram notification service
pub struct TelegramNotifier {
    /// Bot token
    bot_token: String,
    /// Chat ID
    chat_id: String,
    /// HTTP client
    client: reqwest::Client,
    /// Whether enabled
    enabled: bool,
    /// Rate limiter
    rate_limiter: RateLimiter,
}

impl TelegramNotifier {
    /// Create a new Telegram notifier
    pub fn new(config: TelegramConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "Failed to create Telegram HTTP client — using default client");
                reqwest::Client::new()
            });

        Self {
            bot_token: config.bot_token,
            chat_id: config.chat_id,
            client,
            enabled: config.enabled,
            rate_limiter: RateLimiter::new(config.rate_limit_seconds),
        }
    }

    /// Create from environment variables
    pub fn from_env() -> Option<Self> {
        let bot_token = std::env::var("TELEGRAM_BOT_TOKEN").ok()?;
        let chat_id = std::env::var("TELEGRAM_CHAT_ID").ok()?;

        if bot_token.is_empty() || chat_id.is_empty() {
            return None;
        }

        Some(Self::new(TelegramConfig {
            bot_token,
            chat_id,
            enabled: true,
            rate_limit_seconds: 60,
        }))
    }

    /// Send a message to Telegram
    async fn send_message(&self, text: &str) -> anyhow::Result<()> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);

        let payload = serde_json::json!({
            "chat_id": self.chat_id,
            "text": text,
            "parse_mode": "HTML",
            "disable_web_page_preview": true,
        });

        let response = self.client.post(&url).json(&payload).send().await.map_err(|e| {
            // reqwest errors embed the request URL, which contains the bot
            // token — sanitize before the error can reach logs/metrics.
            let sanitized = e.to_string().replace(&self.bot_token, "***");
            anyhow::anyhow!("Telegram request failed: {}", sanitized)
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Telegram API error: {} - {}", status, body);
        }

        Ok(())
    }

    /// Format message with level prefix
    fn format_with_level(&self, level: AlertLevel, message: &str) -> String {
        let level_prefix = match level {
            AlertLevel::Critical => "🔴 <b>CRITICAL</b>",
            AlertLevel::Important => "🟡 <b>IMPORTANT</b>",
            AlertLevel::Info => "🔵 <b>INFO</b>",
        };

        // Event-derived text is inserted with parse_mode=HTML — escape markup
        // so `<`, `>`, `&` in reasons/symbols/addresses cannot corrupt the
        // message or inject formatting.
        format!("{}\n\n{}", level_prefix, escape_html(message))
    }
}

/// Escape HTML markup characters for Telegram's HTML parse mode.
fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[async_trait::async_trait]
impl NotificationService for TelegramNotifier {
    async fn notify(&self, event: &NotificationEvent, trade_mode: &str) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        // Atomically check + reserve the rate-limit slot (skip for critical
        // alerts). The slot is reserved even if the send later fails, so an
        // API outage cannot produce a request storm.
        let rate_key = RateLimiter::get_key(event);
        if event.level() != AlertLevel::Critical && !self.rate_limiter.try_acquire(&rate_key) {
            tracing::debug!(
                key = %rate_key,
                "Rate limited, skipping notification"
            );
            return Ok(());
        }

        let level = event.level();
        let message = event.format_message(trade_mode);
        let formatted = self.format_with_level(level, &message);

        self.send_message(&formatted).await?;

        tracing::info!(
            level = %level,
            "Sent Telegram notification"
        );

        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.enabled && !self.bot_token.is_empty() && !self.chat_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_rate_limiter() {
        let limiter = RateLimiter::new(1); // 1 second limit

        // First send should be allowed (and reserves the slot)
        assert!(limiter.try_acquire("test"));

        // Immediate second send should be blocked
        assert!(!limiter.try_acquire("test"));

        // Different key should be allowed
        assert!(limiter.try_acquire("other"));
    }

    #[test]
    fn test_message_formatting() {
        let notifier = TelegramNotifier::new(TelegramConfig::default());

        let formatted = notifier.format_with_level(AlertLevel::Critical, "Test message");
        assert!(formatted.contains("CRITICAL"));
        assert!(formatted.contains("Test message"));
    }

    #[test]
    fn test_rate_limit_keys() {
        let key1 = RateLimiter::get_key(&NotificationEvent::CircuitBreakerTriggered {
            reason: "test".to_string(),
        });
        assert_eq!(key1, "circuit_breaker");

        let key2 = RateLimiter::get_key(&NotificationEvent::PositionExited {
            token: "BONK".to_string(),
            strategy: "SHIELD".to_string(),
            pnl_percent: rust_decimal::Decimal::from(10u32),
            pnl_sol: rust_decimal::Decimal::from_str("0.1").unwrap(),
        });
        assert_eq!(key2, "position:BONK:SHIELD");
    }
}
