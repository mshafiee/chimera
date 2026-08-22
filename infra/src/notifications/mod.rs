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

use rust_decimal::prelude::*;
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
    /// Circuit breaker recovered (returned to active)
    CircuitBreakerRecovered,
    /// Wallet balance dropped significantly
    WalletDrained {
        delta_sol: Decimal,
        timeframe: String,
    },
    /// System component crashed
    SystemCrash { component: String },
    /// Position was exited
    PositionExited {
        token: String,
        strategy: String,
        pnl_percent: Decimal,
        pnl_sol: Decimal,
    },
    /// Switched to fallback RPC
    RpcFallback { reason: String },
    /// Wallet was promoted to active
    WalletPromoted { address: String, wqs_score: f64 },
    /// Daily trading summary
    DailySummary {
        pnl_usd: Decimal,
        trade_count: u32,
        win_rate: f64,
    },
    /// Jito fallback was triggered
    JitoFallbackTriggered {
        reason: String,
        failure_count: u32,
        threshold: u32,
    },
    /// Jito recovered (switched back from Standard)
    JitoRecovered { latency_ms: u64 },
    /// Jito health changed status
    JitoHealthChanged {
        healthy: bool,
        latency_ms: Option<u64>,
        success_rate: f64,
    },
    /// Admitted decisions recorded without a linked shadow position — the
    /// shadow-measurement loop is silently losing admitted signals.
    ShadowRecordingGap { missing: i64 },
}

impl NotificationEvent {
    /// Get the alert level for this event
    pub fn level(&self) -> AlertLevel {
        match self {
            NotificationEvent::CircuitBreakerTriggered { .. } => AlertLevel::Critical,
            NotificationEvent::CircuitBreakerRecovered => AlertLevel::Info,
            NotificationEvent::WalletDrained { .. } => AlertLevel::Critical,
            NotificationEvent::SystemCrash { .. } => AlertLevel::Critical,
            NotificationEvent::PositionExited { .. } => AlertLevel::Important,
            NotificationEvent::RpcFallback { .. } => AlertLevel::Important,
            NotificationEvent::WalletPromoted { .. } => AlertLevel::Info,
            NotificationEvent::DailySummary { .. } => AlertLevel::Info,
            NotificationEvent::JitoFallbackTriggered { .. } => AlertLevel::Important,
            NotificationEvent::JitoRecovered { .. } => AlertLevel::Info,
            NotificationEvent::JitoHealthChanged { healthy, .. } => {
                if *healthy {
                    AlertLevel::Info
                } else {
                    AlertLevel::Important
                }
            }
            NotificationEvent::ShadowRecordingGap { .. } => AlertLevel::Important,
        }
    }

    /// Format the event as a notification message
    pub fn format_message(&self, trade_mode: &str) -> String {
        let prefix = match trade_mode.trim().to_lowercase().as_str() {
            "paper" => "[PAPER] ",
            "devnet" => "[DEVNET] ",
            "live" | "" => "",
            other => {
                tracing::warn!(mode = other, "Unknown trade mode, treating as live");
                ""
            }
        };
        match self {
            NotificationEvent::CircuitBreakerTriggered { reason } => {
                format!("{prefix}🚨 Circuit breaker triggered: {}", reason)
            }
            NotificationEvent::CircuitBreakerRecovered => {
                format!("{prefix}✅ Circuit breaker recovered - trading resumed")
            }
            NotificationEvent::WalletDrained {
                delta_sol,
                timeframe,
            } => {
                format!(
                    "{prefix}🚨 EMERGENCY: Balance dropped {:.4} SOL in {}",
                    delta_sol, timeframe
                )
            }
            NotificationEvent::SystemCrash { component } => {
                format!("{prefix}🚨 System down: {}", component)
            }
            NotificationEvent::PositionExited {
                token,
                strategy,
                pnl_percent,
                pnl_sol,
            } => {
                let pnl_percent_f64 = pnl_percent.to_f64().unwrap_or(0.0);
                let emoji = if *pnl_percent >= Decimal::ZERO {
                    "💰"
                } else {
                    "📉"
                };
                format!(
                    "{prefix}{} {} {}: {:+.2}% ({:+.4} SOL)",
                    emoji, token, strategy, pnl_percent_f64, pnl_sol
                )
            }
            NotificationEvent::RpcFallback { reason } => {
                format!("{prefix}⚠️ Switched to fallback RPC: {}", reason)
            }
            NotificationEvent::WalletPromoted { address, wqs_score } => {
                // Guard against short/malformed addresses — slicing by raw byte
                // offsets panics when the address is shorter than 8 bytes or
                // contains multi-byte characters.
                let (head, tail) = if address.len() > 8 {
                    (&address[..4], &address[address.len() - 4..])
                } else {
                    (address.as_str(), "")
                };
                format!(
                    "{prefix}📊 Wallet promoted: {}...{} (WQS: {:.2})",
                    head, tail, wqs_score
                )
            }
            NotificationEvent::DailySummary {
                pnl_usd,
                trade_count,
                win_rate,
            } => {
                let pnl_usd_f64 = pnl_usd.to_f64().unwrap_or(0.0);
                let emoji = if *pnl_usd >= Decimal::ZERO {
                    "📈"
                } else {
                    "📉"
                };
                format!(
                    "{prefix}{} Daily: {:+.2} USD | Trades: {} | Win: {:.1}%",
                    emoji, pnl_usd_f64, trade_count, win_rate
                )
            }
            NotificationEvent::JitoFallbackTriggered {
                reason,
                failure_count,
                threshold,
            } => {
                format!(
                    "{prefix}⚠️ Jito fallback triggered after {}/{} failures: {}",
                    failure_count, threshold, reason
                )
            }
            NotificationEvent::JitoRecovered { latency_ms } => {
                format!("{prefix}✅ Jito recovered (latency: {}ms)", latency_ms)
            }
            NotificationEvent::JitoHealthChanged {
                healthy,
                latency_ms,
                success_rate,
            } => {
                let status = if *healthy { "healthy" } else { "unhealthy" };
                // A missing latency must read as "unknown", not a perfect 0ms.
                let latency = match latency_ms {
                    Some(ms) => format!("{}ms", ms),
                    None => "N/A".to_string(),
                };
                format!(
                    "{prefix}🔄 Jito health: {} (latency: {}, success_rate: {:.1}%)",
                    status,
                    latency,
                    success_rate * 100.0
                )
            }
            NotificationEvent::ShadowRecordingGap { missing } => {
                format!(
                    "{prefix}🕳️ Shadow recording gap: {} admitted decision(s) in the last 24h have no shadow position — measurement loop is blind",
                    missing
                )
            }
        }
    }
}

/// Notification service trait
#[async_trait::async_trait]
pub trait NotificationService: Send + Sync {
    async fn notify(&self, event: &NotificationEvent, trade_mode: &str) -> anyhow::Result<()>;

    fn is_enabled(&self) -> bool;
}

/// Composite notifier that can send to multiple services
pub struct CompositeNotifier {
    services: Vec<Arc<dyn NotificationService>>,
    trade_mode: String,
}

impl CompositeNotifier {
    /// Create a new composite notifier
    pub fn new() -> Self {
        Self {
            services: Vec::new(),
            trade_mode: "live".to_string(),
        }
    }

    pub fn set_trade_mode(&mut self, mode: &str) {
        self.trade_mode = mode.to_lowercase();
    }

    /// Add a notification service
    pub fn add_service(&mut self, service: Arc<dyn NotificationService>) {
        self.services.push(service);
    }

    /// Send notification to all enabled services.
    ///
    /// Services are dispatched CONCURRENTLY with an individual timeout so a
    /// slow or hung Telegram/Discord client can never stall critical alerts
    /// behind it.
    pub async fn notify(&self, event: NotificationEvent) {
        let mode = self.trade_mode.as_str();
        let mut tasks = Vec::new();
        for service in &self.services {
            if service.is_enabled() {
                let service = service.clone();
                let event = event.clone();
                let mode = mode.to_string();
                tasks.push(tokio::spawn(async move {
                    let result = tokio::time::timeout(
                        tokio::time::Duration::from_secs(15),
                        service.notify(&event, &mode),
                    )
                    .await;
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            tracing::error!(
                                error = %e,
                                event = ?event.level(),
                                "Failed to send notification"
                            );
                        }
                        Err(_) => {
                            tracing::error!(
                                event = ?event.level(),
                                "Notification service timed out"
                            );
                        }
                    }
                }));
            }
        }
        for task in tasks {
            let _ = task.await;
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
        assert!(event.format_message("live").contains("Circuit breaker"));
        assert!(event.format_message("live").contains("🚨"));
    }

    #[test]
    fn test_circuit_breaker_recovered_event() {
        let event = NotificationEvent::CircuitBreakerRecovered;
        assert!(event.format_message("live").contains("recovered"));
        assert!(event.format_message("live").contains("trading resumed"));
        assert_eq!(event.level(), AlertLevel::Info);
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
                pnl_percent: Decimal::from(10),
                pnl_sol: Decimal::from_str("0.1").unwrap()
            }
            .level(),
            AlertLevel::Important
        );

        assert_eq!(
            NotificationEvent::DailySummary {
                pnl_usd: Decimal::from(100),
                trade_count: 10,
                win_rate: 70.0
            }
            .level(),
            AlertLevel::Info
        );
    }
}
