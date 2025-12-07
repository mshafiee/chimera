//! Notification service for Chimera Operator
//!
//! Provides push notifications via Telegram for system events:
//! - Circuit breaker triggered
//! - Wallet drained (emergency)
//! - Position exited
//! - RPC fallback activated
//! - Wallet promoted
//! - Daily summary

pub mod discord;
pub mod telegram;

pub use discord::DiscordNotifier;
pub use telegram::TelegramNotifier;

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Alert level for notifications
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertLevel {
    /// Critical alerts (circuit breaker, wallet drained, system crash)
    Critical,
    /// Important alerts (position exited, RPC fallback)
    Important,
    /// Informational alerts (wallet promoted, daily summary)
    Info,
}

impl std::fmt::Display for AlertLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertLevel::Critical => write!(f, "CRITICAL"),
            AlertLevel::Important => write!(f, "IMPORTANT"),
            AlertLevel::Info => write!(f, "INFO"),
        }
    }
}

/// Notification event types
#[derive(Debug, Clone)]
pub enum NotificationEvent {
    /// Circuit breaker was triggered
    CircuitBreakerTriggered { reason: String },
    /// Wallet balance dropped significantly
    WalletDrained { delta_sol: f64, timeframe: String },
    /// System component crashed
    SystemCrash { component: String },
    /// Position was exited
    PositionExited {
        token: String,
        strategy: String,
        pnl_percent: f64,
        pnl_sol: f64,
    },
    /// Switched to fallback RPC
    RpcFallback { reason: String },
    /// Wallet was promoted to active
    WalletPromoted { address: String, wqs_score: f64 },
    /// Daily trading summary
    DailySummary {
        pnl_usd: f64,
        trade_count: u32,
        win_rate: f64,
    },
}

impl NotificationEvent {
    /// Get the alert level for this event
    pub fn level(&self) -> AlertLevel {
        match self {
            NotificationEvent::CircuitBreakerTriggered { .. } => AlertLevel::Critical,
            NotificationEvent::WalletDrained { .. } => AlertLevel::Critical,
            NotificationEvent::SystemCrash { .. } => AlertLevel::Critical,
            NotificationEvent::PositionExited { .. } => AlertLevel::Important,
            NotificationEvent::RpcFallback { .. } => AlertLevel::Important,
            NotificationEvent::WalletPromoted { .. } => AlertLevel::Info,
            NotificationEvent::DailySummary { .. } => AlertLevel::Info,
        }
    }

    /// Format the event as a notification message
    pub fn format_message(&self) -> String {
        match self {
            NotificationEvent::CircuitBreakerTriggered { reason } => {
                format!("🚨 Circuit breaker triggered: {}", reason)
            }
            NotificationEvent::WalletDrained { delta_sol, timeframe } => {
                format!(
                    "🚨 EMERGENCY: Balance dropped {:.4} SOL in {}",
                    delta_sol, timeframe
                )
            }
            NotificationEvent::SystemCrash { component } => {
                format!("🚨 System down: {}", component)
            }
            NotificationEvent::PositionExited {
                token,
                strategy,
                pnl_percent,
                pnl_sol,
            } => {
                let emoji = if *pnl_percent >= 0.0 { "💰" } else { "📉" };
                format!(
                    "{} {} {}: {:+.2}% ({:+.4} SOL)",
                    emoji, token, strategy, pnl_percent, pnl_sol
                )
            }
            NotificationEvent::RpcFallback { reason } => {
                format!("⚠️ Switched to fallback RPC: {}", reason)
            }
            NotificationEvent::WalletPromoted { address, wqs_score } => {
                format!(
                    "📊 Wallet promoted: {}...{} (WQS: {:.2})",
                    &address[..4],
                    &address[address.len() - 4..],
                    wqs_score
                )
            }
            NotificationEvent::DailySummary {
                pnl_usd,
                trade_count,
                win_rate,
            } => {
                let emoji = if *pnl_usd >= 0.0 { "📈" } else { "📉" };
                format!(
                    "{} Daily: {:+.2} USD | Trades: {} | Win: {:.1}%",
                    emoji, pnl_usd, trade_count, win_rate
                )
            }
        }
    }
}

/// Notification service trait
#[async_trait::async_trait]
pub trait NotificationService: Send + Sync {
    /// Send a notification
    async fn notify(&self, event: NotificationEvent) -> anyhow::Result<()>;

    /// Check if the service is enabled
    fn is_enabled(&self) -> bool;
}

/// Composite notifier that can send to multiple services
pub struct CompositeNotifier {
    services: Vec<Arc<dyn NotificationService>>,
}

impl CompositeNotifier {
    /// Create a new composite notifier
    pub fn new() -> Self {
        Self {
            services: Vec::new(),
        }
    }

    /// Add a notification service
    pub fn add_service(&mut self, service: Arc<dyn NotificationService>) {
        self.services.push(service);
    }

    /// Send notification to all enabled services
    pub async fn notify(&self, event: NotificationEvent) {
        for service in &self.services {
            if service.is_enabled() {
                if let Err(e) = service.notify(event.clone()).await {
                    tracing::error!(
                        error = %e,
                        event = ?event.level(),
                        "Failed to send notification"
                    );
                }
            }
        }
    }
}

impl Default for CompositeNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alert_level_display() {
        assert_eq!(AlertLevel::Critical.to_string(), "CRITICAL");
        assert_eq!(AlertLevel::Important.to_string(), "IMPORTANT");
        assert_eq!(AlertLevel::Info.to_string(), "INFO");
    }

    #[test]
    fn test_event_format() {
        let event = NotificationEvent::CircuitBreakerTriggered {
            reason: "Max loss exceeded".to_string(),
        };
        assert!(event.format_message().contains("Circuit breaker"));
        assert!(event.format_message().contains("🚨"));
    }

    #[test]
    fn test_event_levels() {
        assert_eq!(
            NotificationEvent::CircuitBreakerTriggered {
                reason: "test".to_string()
            }
            .level(),
            AlertLevel::Critical
        );

        assert_eq!(
            NotificationEvent::PositionExited {
                token: "TEST".to_string(),
                strategy: "SHIELD".to_string(),
                pnl_percent: 10.0,
                pnl_sol: 0.1
            }
            .level(),
            AlertLevel::Important
        );

        assert_eq!(
            NotificationEvent::DailySummary {
                pnl_usd: 100.0,
                trade_count: 10,
                win_rate: 70.0
            }
            .level(),
            AlertLevel::Info
        );
    }
}
