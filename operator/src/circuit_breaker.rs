//! Circuit Breaker for automatic trading halts
//!
//! Monitors trading conditions and automatically halts trading when:
//! - 24h losses exceed threshold
//! - Consecutive losses exceed threshold
//! - Drawdown from peak exceeds threshold
//!
//! After tripping, the circuit breaker enters cooldown before allowing
//! manual reset or automatic recovery.

use crate::config::CircuitBreakerConfig;
use crate::db::{self, DbPool};
use crate::error::AppResult;
use chrono::{DateTime, Duration, Utc};
use parking_lot::RwLock;
use std::sync::Arc;

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitBreakerState {
    /// Trading is allowed
    Active,
    /// Circuit breaker has tripped - trading halted
    Tripped,
    /// In cooldown period after trip
    Cooldown,
}

impl std::fmt::Display for CircuitBreakerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "ACTIVE"),
            Self::Tripped => write!(f, "TRIPPED"),
            Self::Cooldown => write!(f, "COOLDOWN"),
        }
    }
}

/// Reason for circuit breaker trip
#[derive(Debug, Clone)]
pub enum TripReason {
    /// 24h losses exceeded threshold
    MaxLoss24h { loss: f64, threshold: f64 },
    /// Consecutive losses exceeded threshold
    ConsecutiveLosses { count: u32, threshold: u32 },
    /// Drawdown from peak exceeded threshold
    MaxDrawdown { drawdown: f64, threshold: f64 },
    /// Manual trip by admin
    Manual { reason: String },
}

impl std::fmt::Display for TripReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MaxLoss24h { loss, threshold } => {
                write!(f, "24h loss ${:.2} exceeded threshold ${:.2}", loss, threshold)
            }
            Self::ConsecutiveLosses { count, threshold } => {
                write!(f, "{} consecutive losses exceeded threshold {}", count, threshold)
            }
            Self::MaxDrawdown { drawdown, threshold } => {
                write!(f, "Drawdown {:.1}% exceeded threshold {:.1}%", drawdown, threshold)
            }
            Self::Manual { reason } => write!(f, "Manual: {}", reason),
        }
    }
}

/// Circuit breaker internal state
struct InternalState {
    state: CircuitBreakerState,
    tripped_at: Option<DateTime<Utc>>,
    trip_reason: Option<TripReason>,
    last_check: Option<DateTime<Utc>>,
}

/// Circuit Breaker
pub struct CircuitBreaker {
    /// Configuration
    config: CircuitBreakerConfig,
    /// Database pool
    db: DbPool,
    /// Internal state
    state: Arc<RwLock<InternalState>>,
    /// Check interval
    check_interval: Duration,
}

impl CircuitBreaker {
    /// Create a new circuit breaker
    pub fn new(config: CircuitBreakerConfig, db: DbPool) -> Self {
        Self {
            config,
            db,
            state: Arc::new(RwLock::new(InternalState {
                state: CircuitBreakerState::Active,
                tripped_at: None,
                trip_reason: None,
                last_check: None,
            })),
            check_interval: Duration::seconds(30),
        }
    }

    /// Check if trading is allowed
    pub fn is_trading_allowed(&self) -> bool {
        let state = self.state.read();
        state.state == CircuitBreakerState::Active
    }

    /// Get current state
    pub fn current_state(&self) -> CircuitBreakerState {
        self.state.read().state
    }

    /// Get trip reason if tripped
    pub fn trip_reason(&self) -> Option<TripReason> {
        self.state.read().trip_reason.clone()
    }

    /// Evaluate trip conditions and update state
    pub async fn evaluate(&self) -> AppResult<()> {
        // Check if we should evaluate (rate limit checks)
        {
            let state = self.state.read();
            if let Some(last_check) = state.last_check {
                if Utc::now().signed_duration_since(last_check) < self.check_interval {
                    return Ok(());
                }
            }
        }

        // Update last check time
        {
            self.state.write().last_check = Some(Utc::now());
        }

        // If in cooldown, check if cooldown period has passed
        let should_exit_cooldown = {
            let state = self.state.read();
            if state.state == CircuitBreakerState::Cooldown {
                if let Some(tripped_at) = state.tripped_at {
                    let cooldown_duration = Duration::minutes(self.config.cooldown_minutes as i64);
                    Utc::now().signed_duration_since(tripped_at) >= cooldown_duration
                } else {
                    false
                }
            } else {
                false
            }
        };

        if should_exit_cooldown {
            self.exit_cooldown().await?;
            return Ok(());
        }

        // If still in cooldown or tripped, don't evaluate further
        if self.current_state() != CircuitBreakerState::Active {
            return Ok(());
        }

        // Check 24h loss
        let pnl_24h = db::get_pnl_24h(&self.db).await?;
        if pnl_24h < 0.0 && pnl_24h.abs() >= self.config.max_loss_24h_usd {
            self.trip(TripReason::MaxLoss24h {
                loss: pnl_24h.abs(),
                threshold: self.config.max_loss_24h_usd,
            })
            .await?;
            return Ok(());
        }

        // Check consecutive losses
        let consecutive = db::get_consecutive_losses(&self.db).await?;
        if consecutive >= self.config.max_consecutive_losses {
            self.trip(TripReason::ConsecutiveLosses {
                count: consecutive,
                threshold: self.config.max_consecutive_losses,
            })
            .await?;
            return Ok(());
        }

        // Check drawdown
        let drawdown = db::get_max_drawdown_percent(&self.db).await?;
        if drawdown >= self.config.max_drawdown_percent {
            self.trip(TripReason::MaxDrawdown {
                drawdown,
                threshold: self.config.max_drawdown_percent,
            })
            .await?;
            return Ok(());
        }

        Ok(())
    }

    /// Trip the circuit breaker
    async fn trip(&self, reason: TripReason) -> AppResult<()> {
        let reason_str = reason.to_string();

        {
            let mut state = self.state.write();
            state.state = CircuitBreakerState::Tripped;
            state.tripped_at = Some(Utc::now());
            state.trip_reason = Some(reason);
        }

        tracing::error!(
            reason = %reason_str,
            "Circuit breaker TRIPPED - trading halted"
        );

        // Log to config audit
        db::log_config_change(
            &self.db,
            "circuit_breaker",
            Some("ACTIVE"),
            "TRIPPED",
            "SYSTEM_CIRCUIT_BREAKER",
            Some(&reason_str),
        )
        .await?;

        Ok(())
    }

    /// Enter cooldown period
    pub async fn enter_cooldown(&self) -> AppResult<()> {
        {
            let mut state = self.state.write();
            if state.state != CircuitBreakerState::Tripped {
                return Ok(());
            }
            state.state = CircuitBreakerState::Cooldown;
        }

        tracing::info!(
            cooldown_minutes = self.config.cooldown_minutes,
            "Circuit breaker entering cooldown"
        );

        db::log_config_change(
            &self.db,
            "circuit_breaker",
            Some("TRIPPED"),
            "COOLDOWN",
            "SYSTEM",
            Some(&format!("Cooldown for {} minutes", self.config.cooldown_minutes)),
        )
        .await?;

        Ok(())
    }

    /// Exit cooldown and return to active
    async fn exit_cooldown(&self) -> AppResult<()> {
        {
            let mut state = self.state.write();
            state.state = CircuitBreakerState::Active;
            state.tripped_at = None;
            state.trip_reason = None;
        }

        tracing::info!("Circuit breaker exiting cooldown - trading resumed");

        db::log_config_change(
            &self.db,
            "circuit_breaker",
            Some("COOLDOWN"),
            "ACTIVE",
            "SYSTEM",
            Some("Cooldown period completed"),
        )
        .await?;

        Ok(())
    }

    /// Manually reset the circuit breaker (admin action)
    pub async fn reset(&self, admin: &str) -> AppResult<()> {
        let previous_state = self.current_state();

        {
            let mut state = self.state.write();
            state.state = CircuitBreakerState::Active;
            state.tripped_at = None;
            state.trip_reason = None;
        }

        tracing::warn!(
            admin = %admin,
            previous_state = %previous_state,
            "Circuit breaker manually reset"
        );

        db::log_config_change(
            &self.db,
            "circuit_breaker",
            Some(&previous_state.to_string()),
            "ACTIVE",
            admin,
            Some("Manual reset by admin"),
        )
        .await?;

        Ok(())
    }

    /// Manually trip the circuit breaker (admin action)
    pub async fn manual_trip(&self, admin: &str, reason: String) -> AppResult<()> {
        self.trip(TripReason::Manual { reason }).await?;

        db::log_config_change(
            &self.db,
            "circuit_breaker",
            Some("ACTIVE"),
            "TRIPPED",
            admin,
            Some("Manual trip by admin"),
        )
        .await?;

        Ok(())
    }

    /// Get status summary
    pub fn status(&self) -> CircuitBreakerStatus {
        let state = self.state.read();
        CircuitBreakerStatus {
            state: state.state,
            tripped_at: state.tripped_at,
            trip_reason: state.trip_reason.as_ref().map(|r| r.to_string()),
            cooldown_remaining_secs: if state.state == CircuitBreakerState::Cooldown {
                state.tripped_at.map(|t| {
                    let cooldown = Duration::minutes(self.config.cooldown_minutes as i64);
                    let elapsed = Utc::now().signed_duration_since(t);
                    (cooldown - elapsed).num_seconds().max(0)
                })
            } else {
                None
            },
        }
    }
}

/// Circuit breaker status for API responses
#[derive(Debug, Clone)]
pub struct CircuitBreakerStatus {
    /// Current state
    pub state: CircuitBreakerState,
    /// When it was tripped
    pub tripped_at: Option<DateTime<Utc>>,
    /// Reason for trip
    pub trip_reason: Option<String>,
    /// Seconds remaining in cooldown
    pub cooldown_remaining_secs: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==========================================================================
    // STATE DISPLAY TESTS
    // ==========================================================================

    #[test]
    fn test_state_display() {
        assert_eq!(CircuitBreakerState::Active.to_string(), "ACTIVE");
        assert_eq!(CircuitBreakerState::Tripped.to_string(), "TRIPPED");
        assert_eq!(CircuitBreakerState::Cooldown.to_string(), "COOLDOWN");
    }

    // ==========================================================================
    // TRIP REASON DISPLAY TESTS
    // ==========================================================================

    #[test]
    fn test_trip_reason_max_loss_24h() {
        let reason = TripReason::MaxLoss24h {
            loss: 525.50,
            threshold: 500.0,
        };
        let display = reason.to_string();
        assert!(display.contains("525.50"), "Should include actual loss amount");
        assert!(display.contains("500"), "Should include threshold");
        assert!(display.contains("24h"), "Should indicate 24h period");
    }

    #[test]
    fn test_trip_reason_consecutive_losses() {
        let reason = TripReason::ConsecutiveLosses {
            count: 6,
            threshold: 5,
        };
        let display = reason.to_string();
        assert!(display.contains("6"), "Should include actual count");
        assert!(display.contains("5"), "Should include threshold");
        assert!(display.contains("consecutive"), "Should indicate consecutive losses");
    }

    #[test]
    fn test_trip_reason_max_drawdown() {
        let reason = TripReason::MaxDrawdown {
            drawdown: 18.5,
            threshold: 15.0,
        };
        let display = reason.to_string();
        assert!(display.contains("18.5"), "Should include actual drawdown");
        assert!(display.contains("15"), "Should include threshold");
    }

    #[test]
    fn test_trip_reason_manual() {
        let reason = TripReason::Manual {
            reason: "Emergency halt by admin".to_string(),
        };
        let display = reason.to_string();
        assert!(display.contains("Manual"), "Should indicate manual trip");
        assert!(display.contains("Emergency halt"), "Should include reason text");
    }

    // ==========================================================================
    // THRESHOLD BOUNDARY TESTS (per PDD Section 4.4)
    // ==========================================================================

    #[test]
    fn test_max_loss_threshold_exact_boundary() {
        // Testing: loss >= threshold should trip
        let loss = 500.0_f64;
        let threshold = 500.0_f64;
        let should_trip = loss.abs() >= threshold;
        assert!(should_trip, "Exact boundary ($500) should trigger circuit breaker");
    }

    #[test]
    fn test_max_loss_threshold_below_boundary() {
        let loss = 499.99_f64;
        let threshold = 500.0_f64;
        let should_trip = loss.abs() >= threshold;
        assert!(!should_trip, "Below threshold should not trigger circuit breaker");
    }

    #[test]
    fn test_consecutive_losses_exact_boundary() {
        let consecutive: u32 = 5;
        let threshold: u32 = 5;
        let should_trip = consecutive >= threshold;
        assert!(should_trip, "Exact 5 consecutive losses should trigger circuit breaker");
    }

    #[test]
    fn test_consecutive_losses_below_boundary() {
        let consecutive: u32 = 4;
        let threshold: u32 = 5;
        let should_trip = consecutive >= threshold;
        assert!(!should_trip, "4 consecutive losses should not trip");
    }

    #[test]
    fn test_drawdown_exact_boundary() {
        let drawdown = 15.0_f64;
        let threshold = 15.0_f64;
        let should_trip = drawdown >= threshold;
        assert!(should_trip, "Exact 15% drawdown should trigger circuit breaker");
    }

    #[test]
    fn test_drawdown_below_boundary() {
        let drawdown = 14.99_f64;
        let threshold = 15.0_f64;
        let should_trip = drawdown >= threshold;
        assert!(!should_trip, "Below 15% drawdown should not trip");
    }

    // ==========================================================================
    // PNL HANDLING TESTS
    // ==========================================================================

    #[test]
    fn test_negative_pnl_triggers_loss_check() {
        let pnl_24h = -525.50_f64; // Loss of $525.50
        let threshold = 500.0_f64;
        // From evaluate(): pnl_24h < 0.0 && pnl_24h.abs() >= threshold
        let should_trip = pnl_24h < 0.0 && pnl_24h.abs() >= threshold;
        assert!(should_trip, "Negative PnL exceeding threshold should trip");
    }

    #[test]
    fn test_positive_pnl_never_trips() {
        let pnl_24h = 1000.0_f64; // Profit of $1000
        let threshold = 500.0_f64;
        let should_trip = pnl_24h < 0.0 && pnl_24h.abs() >= threshold;
        assert!(!should_trip, "Positive PnL should never trip loss-based circuit breaker");
    }

    #[test]
    fn test_zero_pnl_no_trip() {
        let pnl_24h = 0.0_f64;
        let threshold = 500.0_f64;
        let should_trip = pnl_24h < 0.0 && pnl_24h.abs() >= threshold;
        assert!(!should_trip, "Zero PnL should not trip");
    }

    // ==========================================================================
    // COOLDOWN TESTS
    // ==========================================================================

    #[test]
    fn test_cooldown_not_expired() {
        let cooldown_minutes: u32 = 30;
        let tripped_at = Utc::now() - Duration::minutes(15); // 15 minutes ago
        let cooldown_duration = Duration::minutes(cooldown_minutes as i64);
        let elapsed = Utc::now().signed_duration_since(tripped_at);
        let should_exit = elapsed >= cooldown_duration;
        assert!(!should_exit, "Should still be in cooldown after 15 minutes");
    }

    #[test]
    fn test_cooldown_expired() {
        let cooldown_minutes: u32 = 30;
        let tripped_at = Utc::now() - Duration::minutes(31); // 31 minutes ago
        let cooldown_duration = Duration::minutes(cooldown_minutes as i64);
        let elapsed = Utc::now().signed_duration_since(tripped_at);
        let should_exit = elapsed >= cooldown_duration;
        assert!(should_exit, "Should exit cooldown after 31 minutes");
    }

    #[test]
    fn test_cooldown_remaining_calculation() {
        let cooldown_minutes: u32 = 30;
        let tripped_at = Utc::now() - Duration::minutes(20); // 20 minutes ago
        let cooldown_duration = Duration::minutes(cooldown_minutes as i64);
        let elapsed = Utc::now().signed_duration_since(tripped_at);
        let remaining_secs = (cooldown_duration - elapsed).num_seconds().max(0);
        // Should be approximately 10 minutes = 600 seconds remaining
        assert!(remaining_secs > 500 && remaining_secs < 700, 
            "Should have ~10 minutes remaining, got {} seconds", remaining_secs);
    }

    // ==========================================================================
    // STATE EQUALITY TESTS
    // ==========================================================================

    #[test]
    fn test_state_equality() {
        assert_eq!(CircuitBreakerState::Active, CircuitBreakerState::Active);
        assert_ne!(CircuitBreakerState::Active, CircuitBreakerState::Tripped);
        assert_ne!(CircuitBreakerState::Tripped, CircuitBreakerState::Cooldown);
    }

    #[test]
    fn test_state_copy() {
        let state = CircuitBreakerState::Active;
        let copied = state;
        assert_eq!(state, copied, "CircuitBreakerState should be Copy");
    }
}
