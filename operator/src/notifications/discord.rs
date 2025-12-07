//! Discord notification service
//!
//! Sends alerts via Discord Webhook API with rate limiting to prevent spam.

use super::{AlertLevel, NotificationEvent, NotificationService};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Discord notifier configuration
#[derive(Debug, Clone)]
pub struct DiscordConfig {
    /// Webhook URL from Discord channel settings
    pub webhook_url: String,
    /// Whether notifications are enabled
    pub enabled: bool,
    /// Minimum interval between messages of same type (seconds)
    pub rate_limit_seconds: u64,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            webhook_url: String::new(),
            enabled: false,
            rate_limit_seconds: 60, // 1 minute between similar messages
        }
    }
}

/// Rate limiter for notifications (reuse same logic as Telegram)
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

    /// Check if we can send a message of this type
    fn can_send(&self, key: &str) -> bool {
        let last_sent = self.last_sent.read();
        match last_sent.get(key) {
            Some(last) => last.elapsed() >= self.interval,
            None => true,
        }
    }

    /// Mark a message type as sent
    fn mark_sent(&self, key: &str) {
        let mut last_sent = self.last_sent.write();
        last_sent.insert(key.to_string(), Instant::now());
    }

    /// Get rate limit key for an event (same as Telegram)
    fn get_key(event: &NotificationEvent) -> String {
        match event {
            NotificationEvent::CircuitBreakerTriggered { .. } => "circuit_breaker".to_string(),
            NotificationEvent::WalletDrained { .. } => "wallet_drained".to_string(),
            NotificationEvent::SystemCrash { component } => format!("system_crash:{}", component),
            NotificationEvent::PositionExited { token, strategy, .. } => {
                format!("position:{}:{}", token, strategy)
            }
            NotificationEvent::RpcFallback { .. } => "rpc_fallback".to_string(),
            NotificationEvent::WalletPromoted { address, .. } => {
                format!("wallet_promoted:{}", address)
            }
            NotificationEvent::DailySummary { .. } => "daily_summary".to_string(),
        }
    }
}

/// Discord notification service
pub struct DiscordNotifier {
    /// Webhook URL
    webhook_url: String,
    /// HTTP client
    client: reqwest::Client,
    /// Whether enabled
    enabled: bool,
    /// Rate limiter
    rate_limiter: RateLimiter,
}

impl DiscordNotifier {
    /// Create a new Discord notifier
    pub fn new(config: DiscordConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            webhook_url: config.webhook_url,
            client,
            enabled: config.enabled,
            rate_limiter: RateLimiter::new(config.rate_limit_seconds),
        }
    }

    /// Create from environment variables
    pub fn from_env() -> Option<Self> {
        let webhook_url = std::env::var("DISCORD_WEBHOOK_URL").ok()?;

        if webhook_url.is_empty() {
            return None;
        }

        Some(Self::new(DiscordConfig {
            webhook_url,
            enabled: true,
            rate_limit_seconds: 60,
        }))
    }

    /// Send a message to Discord
    async fn send_message(&self, content: &str, level: AlertLevel) -> anyhow::Result<()> {
        // Discord webhook payload
        let color = match level {
            AlertLevel::Critical => 0xff0000, // Red
            AlertLevel::Important => 0xffaa00, // Orange
            AlertLevel::Info => 0x0099ff, // Blue
        };

        let payload = serde_json::json!({
            "embeds": [{
                "title": match level {
                    AlertLevel::Critical => "🚨 CRITICAL ALERT",
                    AlertLevel::Important => "⚠️ IMPORTANT ALERT",
                    AlertLevel::Info => "ℹ️ INFO",
                },
                "description": content,
                "color": color,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }]
        });

        let response = self.client.post(&self.webhook_url).json(&payload).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Discord API error: {} - {}", status, body);
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl NotificationService for DiscordNotifier {
    async fn notify(&self, event: NotificationEvent) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        // Check rate limit (skip for critical alerts)
        let rate_key = RateLimiter::get_key(&event);
        if event.level() != AlertLevel::Critical && !self.rate_limiter.can_send(&rate_key) {
            tracing::debug!(
                key = %rate_key,
                "Rate limited, skipping Discord notification"
            );
            return Ok(());
        }

        let level = event.level();
        let message = event.format_message();

        self.send_message(&message, level).await?;
        self.rate_limiter.mark_sent(&rate_key);

        tracing::info!(
            level = %level,
            "Sent Discord notification"
        );

        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.enabled && !self.webhook_url.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter() {
        let limiter = RateLimiter::new(1); // 1 second limit

        // First send should be allowed
        assert!(limiter.can_send("test"));
        limiter.mark_sent("test");

        // Immediate second send should be blocked
        assert!(!limiter.can_send("test"));

        // Different key should be allowed
        assert!(limiter.can_send("other"));
    }
}
