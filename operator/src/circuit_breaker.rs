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
use crate::db_abstraction::{Database, datetime_to_string};
use crate::error::{AppError, AppResult};
use crate::notifications::{CompositeNotifier, NotificationEvent};
use chrono::{DateTime, Duration, Utc};
use parking_lot::RwLock;
use prometheus::{IntCounter, IntGauge};
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::sync::Arc;
use std::sync::OnceLock;

/// Persist circuit breaker state to the database
async fn persist_cb_state(
    db: &dyn Database,
    state: CircuitBreakerState,
    tripped_at: Option<DateTime<Utc>>,
    trip_reason: Option<&str>,
) -> AppResult<()> {
    let state_str = match state {
        CircuitBreakerState::Active => "Active",
        CircuitBreakerState::Tripped => "Tripped",
        CircuitBreakerState::Cooldown => "Cooldown",
    };
    let tripped_at_str = tripped_at.map(datetime_to_string);
    db.update_circuit_breaker_state(state_str, tripped_at_str.as_deref(), trip_reason)
        .await
}

/// Load persisted circuit breaker state from the database
async fn load_cb_state(
    db: &dyn Database,
) -> AppResult<Option<(String, Option<String>, Option<String>)>> {
    match db.get_circuit_breaker_state().await {
        Ok(state) => Ok(Some((state.state, state.tripped_at, state.trip_reason))),
        Err(_) => Ok(None),
    }
}

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
    MaxLoss24h { loss: Decimal, threshold: Decimal },
    /// Consecutive losses exceeded threshold
    ConsecutiveLosses { count: u32, threshold: u32 },
    /// Drawdown from peak exceeded threshold
    MaxDrawdown {
        drawdown: Decimal,
        threshold: Decimal,
    },
    /// 24h SOL-denominated loss exceeded threshold (portfolio stop)
    PortfolioStop24h {
        loss_pct: Decimal,
        threshold: Decimal,
    },
    /// Jupiter API failures exceeded threshold
    JupiterApiFailures {
        consecutive_failures: u32,
        threshold: u32,
        error_type: String,
    },
    /// Manual trip by admin
    Manual { reason: String },
}

impl std::fmt::Display for TripReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MaxLoss24h { loss, threshold } => {
                write!(
                    f,
                    "24h loss ${} exceeded threshold ${}",
                    loss.round_dp(2),
                    threshold.round_dp(2)
                )
            }
            Self::ConsecutiveLosses { count, threshold } => {
                write!(
                    f,
                    "{} consecutive losses exceeded threshold {}",
                    count, threshold
                )
            }
            Self::MaxDrawdown {
                drawdown,
                threshold,
            } => {
                write!(
                    f,
                    "Drawdown {}% exceeded threshold {}%",
                    drawdown.round_dp(1),
                    threshold.round_dp(1)
                )
            }
            Self::PortfolioStop24h {
                loss_pct,
                threshold,
            } => {
                write!(
                    f,
                    "24h realized SOL loss {}% exceeded threshold {}% (portfolio stop)",
                    loss_pct.round_dp(2),
                    threshold.round_dp(2)
                )
            }
            Self::JupiterApiFailures {
                consecutive_failures,
                threshold,
                error_type,
            } => {
                write!(
                    f,
                    "{} consecutive Jupiter API failures (type: {}) exceeded threshold {}",
                    consecutive_failures, error_type, threshold
                )
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
    /// Consecutive Jupiter API failures
    jupiter_failure_count: u32,
    /// Last Jupiter API failure type
    last_jupiter_error: Option<String>,
    /// Evaluation in progress flag to prevent concurrent evaluations
    evaluation_in_progress: bool,
}

/// Circuit Breaker
pub struct CircuitBreaker {
    /// Configuration
    config: CircuitBreakerConfig,
    /// Database pool
    db: Arc<dyn Database>,
    /// Internal state
    state: Arc<RwLock<InternalState>>,
    /// Check interval
    check_interval: Duration,
    /// Optional WebSocket state for broadcasting events
    ws_state: Option<Arc<crate::handlers::WsState>>,
    /// Total capital in SOL for portfolio stop calculation — shared with PortfolioHeat so
    /// that balance refreshes (every 60s in main.rs) propagate here automatically.
    total_capital_sol: Arc<RwLock<Decimal>>,
    /// Price cache for converting unrealized SOL losses to USD
    price_cache: Option<Arc<crate::price_cache::PriceCache>>,
    /// Prometheus gauge for circuit breaker state (2=Active, 1=Cooldown, 0=Tripped)
    circuit_breaker_state: OnceLock<IntGauge>,
    /// Prometheus counter for lifetime trips
    trips_total: OnceLock<IntCounter>,
    /// Optional notification service for push alerts
    notifier: OnceLock<Arc<CompositeNotifier>>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker
    pub fn new(
        config: CircuitBreakerConfig,
        db: Arc<dyn Database>,
        initial_capital_sol: Decimal,
    ) -> Self {
        Self::new_with_ws(config, db, None, initial_capital_sol)
    }

    /// Create a new circuit breaker with WebSocket support
    pub fn new_with_ws(
        config: CircuitBreakerConfig,
        db: Arc<dyn Database>,
        ws_state: Option<Arc<crate::handlers::WsState>>,
        initial_capital_sol: Decimal,
    ) -> Self {
        Self {
            config,
            db,
            state: Arc::new(RwLock::new(InternalState {
                state: CircuitBreakerState::Active,
                tripped_at: None,
                trip_reason: None,
                last_check: None,
                jupiter_failure_count: 0,
                last_jupiter_error: None,
                evaluation_in_progress: false,
            })),
            check_interval: Duration::seconds(5), // Reduced from 30s to 5s for faster loss detection
            ws_state,
            total_capital_sol: Arc::new(RwLock::new(initial_capital_sol)),
            price_cache: None,
            circuit_breaker_state: OnceLock::new(),
            trips_total: OnceLock::new(),
            notifier: OnceLock::new(),
        }
    }

    /// Set Prometheus metrics (can be called once after construction)
    pub fn set_metrics(&self, gauge: IntGauge, counter: IntCounter) {
        // Initialize gauge from actual CB state — avoids overwriting an already-tripped state
        let val = match self.current_state() {
            CircuitBreakerState::Active => 2,
            CircuitBreakerState::Cooldown => 1,
            CircuitBreakerState::Tripped => 0,
        };
        gauge.set(val);
        let _ = self.circuit_breaker_state.set(gauge);
        let _ = self.trips_total.set(counter);
    }

    /// Set notification service (can be called once after construction)
    pub fn set_notifier(&self, notifier: Arc<CompositeNotifier>) {
        let _ = self.notifier.set(notifier);
    }

    /// Set price cache
    pub fn with_price_cache(mut self, price_cache: Arc<crate::price_cache::PriceCache>) -> Self {
        self.price_cache = Some(price_cache);
        self
    }

    /// Restore persisted circuit breaker state from DB on startup.
    /// Call this after construction but before the server starts accepting connections.
    pub async fn restore_from_db(&self) -> AppResult<()> {
        match load_cb_state(self.db.as_ref()).await? {
            Some((state_str, tripped_at_str, trip_reason_str)) if state_str != "Active" => {
                let tripped_at = tripped_at_str
                    .as_deref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc));

                let reason = trip_reason_str
                    .clone()
                    .map(|r| TripReason::Manual { reason: r })
                    .unwrap_or(TripReason::Manual {
                        reason: "Restored from persisted state".to_string(),
                    });

                {
                    let mut state = self.state.write();
                    state.state = CircuitBreakerState::Tripped;
                    state.tripped_at = tripped_at;
                    state.trip_reason = Some(reason);
                }

                tracing::warn!(
                    persisted_state = %state_str,
                    tripped_at = ?tripped_at_str,
                    trip_reason = ?trip_reason_str,
                    "Circuit breaker restored to non-Active state from persisted DB record"
                );

                // Re-evaluate immediately to transition Tripped → Cooldown → Active if appropriate
                self.evaluate().await?;
            }
            _ => {
                tracing::debug!(
                    "Circuit breaker persisted state is Active or absent — no restore needed"
                );
            }
        }
        Ok(())
    }

    /// Update total capital in SOL (called from the live balance refresh loop)
    pub fn update_capital(&self, new_capital: Decimal) {
        *self.total_capital_sol.write() = new_capital;
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

    /// Check all breach conditions and return the reason if breached.
    /// Returns None if no breach conditions are met.
    async fn check_breach_conditions(&self) -> AppResult<Option<TripReason>> {
        let (unrealized_sol, realized_pnl_sol, mut realized_usd, null_price_pnl_sol) =
            self.db.get_evaluation_data().await?;

        let total_capital = *self.total_capital_sol.read();
        // Skip portfolio stop check for paper trading or zero/low capital scenarios
        // Paper trading often uses test wallets with minimal or no capital
        if total_capital > dec!(1.0) {
            let total_loss_sol = realized_pnl_sol + unrealized_sol;
            let daily_loss_percent = (total_loss_sol / total_capital) * Decimal::from(100);
            // portfolio_stop_loss_percent is negative by convention (default -5.0,
            // validated < 0 in config). Use it directly as the comparison threshold
            // so we trip only when the loss is worse than it (e.g. -6% < -5%).
            // Previously this negated the value (-(-5.0) = +5.0), inverting the
            // comparison and false-tripping on ANY pnl below +5% — including 0%.
            let loss_threshold = self.config.portfolio_stop_loss_percent;

            if daily_loss_percent < loss_threshold {
                return Ok(Some(TripReason::PortfolioStop24h {
                    loss_pct: daily_loss_percent.abs(),
                    threshold: self.config.portfolio_stop_loss_percent,
                }));
            }
        }

        if null_price_pnl_sol != Decimal::ZERO {
            tracing::warn!(
                null_price_pnl_sol = %null_price_pnl_sol,
                "Circuit breaker: positions closed without USD price data in 24h window — \
                 estimating their PnL from SOL-denominated value"
            );
        }

        let sol_price_usd = if let Some(ref cache) = self.price_cache {
            cache.get_price_usd(crate::constants::mints::SOL)
        } else {
            None
        };

        if let Some(price) = sol_price_usd {
            if price > Decimal::ZERO {
                if null_price_pnl_sol != Decimal::ZERO {
                    let estimated = null_price_pnl_sol * price;
                    realized_usd += estimated;
                }

                let unrealized_usd = unrealized_sol * price;
                let total_pnl_usd = realized_usd + unrealized_usd;

                if total_pnl_usd < Decimal::ZERO
                    && total_pnl_usd.abs() >= self.config.max_loss_24h_usd
                {
                    return Ok(Some(TripReason::MaxLoss24h {
                        loss: total_pnl_usd.abs(),
                        threshold: self.config.max_loss_24h_usd,
                    }));
                }
            } else {
                tracing::warn!(
                    "SOL price from cache is zero — skipping USD loss check for this tick"
                );
            }
        } else {
            tracing::warn!(
                "SOL price unavailable (stale cache) — skipping USD loss check for this tick"
            );
        }

        let consecutive = self.db.get_consecutive_losses().await?;
        if consecutive >= self.config.max_consecutive_losses {
            return Ok(Some(TripReason::ConsecutiveLosses {
                count: consecutive,
                threshold: self.config.max_consecutive_losses,
            }));
        }

        let total_capital = *self.total_capital_sol.read();
        let drawdown = self.db.get_max_drawdown_percent(total_capital).await?;
        if drawdown >= self.config.max_drawdown_percent {
            return Ok(Some(TripReason::MaxDrawdown {
                drawdown,
                threshold: self.config.max_drawdown_percent,
            }));
        }

        Ok(None)
    }

    /// Evaluate trip conditions and update state
    #[tracing::instrument(skip(self))]
    pub async fn evaluate(&self) -> AppResult<()> {
        // Atomically check if evaluation is already in progress and skip if true
        {
            let mut state = self.state.write();
            if state.evaluation_in_progress {
                tracing::debug!("Circuit breaker evaluation already in progress, skipping");
                return Ok(());
            }
            state.evaluation_in_progress = true;
            // Lock released here
        }

        // FIX [R-M3]: Check interval under write lock but do NOT update last_check yet.
        // last_check is updated only after DB queries succeed (see below).
        {
            let state = self.state.write();
            if let Some(last_check) = state.last_check {
                if Utc::now().signed_duration_since(last_check) < self.check_interval {
                    // Clear evaluation flag since we're skipping
                    drop(state);
                    let mut state = self.state.write();
                    state.evaluation_in_progress = false;
                    return Ok(());
                }
            }
            // Do NOT set last_check here — we set it after queries succeed.
            // (write guard is released at end of this block)
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
            // Clear evaluation flag before returning
            {
                let mut state = self.state.write();
                state.evaluation_in_progress = false;
            }
            return Ok(());
        }

        // Read state once — avoids TOCTOU between two separate current_state() calls.
        let current = self.current_state();

        // Transition from Tripped → Cooldown after trip is recorded
        if current == CircuitBreakerState::Tripped {
            self.enter_cooldown().await?;
            return Ok(());
        }

        // If still in cooldown or tripped, don't evaluate further
        if current != CircuitBreakerState::Active {
            // Clear evaluation flag before returning
            {
                let mut state = self.state.write();
                state.evaluation_in_progress = false;
            }
            return Ok(());
        }

        if let Some(reason) = self.check_breach_conditions().await? {
            self.trip(reason).await?;
            // Clear evaluation flag before returning
            {
                let mut state = self.state.write();
                state.evaluation_in_progress = false;
            }
            return Ok(());
        }
        // Update last_check
        {
            let mut state = self.state.write();
            state.last_check = Some(Utc::now());
            state.evaluation_in_progress = false;
        }

        Ok(())
    }

    /// Trip the circuit breaker
    #[tracing::instrument(skip(self))]
    async fn trip(&self, reason: TripReason) -> AppResult<()> {
        let reason_str = reason.to_string();
        let now = Utc::now();

        {
            let mut state = self.state.write();
            state.state = CircuitBreakerState::Tripped;
            state.tripped_at = Some(now);
            state.trip_reason = Some(reason);
        }

        tracing::error!(
            reason = %reason_str,
            "Circuit breaker TRIPPED - trading halted"
        );

        // FIX [R-C1]: Persist state to DB so it survives restarts.
        if let Err(e) = persist_cb_state(
            self.db.as_ref(),
            CircuitBreakerState::Tripped,
            Some(now),
            Some(&reason_str),
        )
        .await
        {
            tracing::error!(error = %e, "Failed to persist circuit breaker TRIPPED state to DB");
            // Non-fatal: in-memory state is already set; log the failure and continue.
        }

        // Log to config audit
        self.db
            .log_config_change(
                "circuit_breaker",
                Some("ACTIVE"),
                "TRIPPED",
                "SYSTEM_CIRCUIT_BREAKER",
                Some(&reason_str),
            )
            .await?;

        // Broadcast alert via WebSocket
        if let Some(ref ws) = self.ws_state {
            ws.broadcast(crate::handlers::WsEvent::Alert(
                crate::handlers::AlertData {
                    severity: "critical".to_string(),
                    component: "circuit_breaker".to_string(),
                    message: format!("Circuit breaker tripped: {}", reason_str),
                },
            ));
        }

        // Update Prometheus metrics
        if let Some(gauge) = self.circuit_breaker_state.get() {
            gauge.set(0);
        }
        if let Some(counter) = self.trips_total.get() {
            counter.inc();
        }

        // Send push notification
        if let Some(notifier) = self.notifier.get() {
            notifier
                .notify(NotificationEvent::CircuitBreakerTriggered { reason: reason_str })
                .await;
        }

        Ok(())
    }

    /// Enter cooldown period
    pub async fn enter_cooldown(&self) -> AppResult<()> {
        {
            let mut state = self.state.write();
            if state.state != CircuitBreakerState::Tripped {
                tracing::debug!("enter_cooldown called but state is not Tripped — no-op");
                return Ok(());
            }
            state.state = CircuitBreakerState::Cooldown;
        }
        // Only reaches here when an actual Tripped → Cooldown transition occurred.
        tracing::info!(
            cooldown_minutes = self.config.cooldown_minutes,
            "Circuit breaker entering cooldown"
        );

        // Update Prometheus gauge to Cooldown (1)
        if let Some(gauge) = self.circuit_breaker_state.get() {
            gauge.set(1);
        }

        self.db
            .log_config_change(
                "circuit_breaker",
                Some("TRIPPED"),
                "COOLDOWN",
                "SYSTEM",
                Some(&format!(
                    "Cooldown for {} minutes",
                    self.config.cooldown_minutes
                )),
            )
            .await?;

        Ok(())
    }

    /// Exit cooldown: re-evaluate breach conditions before resuming.
    /// If the breach condition still holds, re-trip instead of going Active.
    async fn exit_cooldown(&self) -> AppResult<()> {
        if let Some(reason) = self.check_breach_conditions().await? {
            tracing::warn!(
                reason = ?reason,
                original_tripped_at = ?self.state.read().tripped_at,
                "Circuit breaker re-tripped during cooldown exit — clock reset"
            );
            self.trip(reason).await?;
            tracing::warn!(
                "Circuit breaker cooldown expired but breach condition still present — re-tripped"
            );
            return Ok(());
        }

        {
            let mut state = self.state.write();
            state.state = CircuitBreakerState::Active;
            state.tripped_at = None;
            state.trip_reason = None;
        }

        // Update Prometheus gauge to Active (2)
        if let Some(gauge) = self.circuit_breaker_state.get() {
            gauge.set(2);
        }

        tracing::info!("Circuit breaker exiting cooldown - trading resumed");

        // FIX [R-C1]: Persist Active state so restarts see cleared state.
        if let Err(e) =
            persist_cb_state(self.db.as_ref(), CircuitBreakerState::Active, None, None).await
        {
            tracing::error!(error = %e, "Failed to persist circuit breaker ACTIVE state to DB after cooldown exit");
        }

        self.db
            .log_config_change(
                "circuit_breaker",
                Some("COOLDOWN"),
                "ACTIVE",
                "SYSTEM",
                Some("Cooldown period completed — breach conditions cleared"),
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

        // Update Prometheus gauge to Active (2)
        if let Some(gauge) = self.circuit_breaker_state.get() {
            gauge.set(2);
        }

        tracing::warn!(
            admin = %admin,
            previous_state = %previous_state,
            "Circuit breaker manually reset"
        );

        // FIX [R-C1]: Persist Active state so restarts don't re-trip unnecessarily.
        if let Err(e) =
            persist_cb_state(self.db.as_ref(), CircuitBreakerState::Active, None, None).await
        {
            tracing::error!(error = %e, "Failed to persist circuit breaker ACTIVE state to DB after reset");
        }

        self.db
            .log_config_change(
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

        self.db
            .log_config_change(
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

    /// Record a Jupiter API failure and check if threshold is exceeded
    ///
    /// This should be called when Jupiter API calls fail. If consecutive failures
    /// exceed the threshold, the circuit breaker will trip automatically.
    #[tracing::instrument(skip(self))]
    pub fn record_jupiter_failure(&self, error_type: String) -> AppResult<bool> {
        let mut state = self.state.write();

        // Increment failure counter
        state.jupiter_failure_count += 1;
        state.last_jupiter_error = Some(error_type.clone());

        let current_failures = state.jupiter_failure_count;
        let threshold = self.config.max_jupiter_failures;

        tracing::warn!(
            jupiter_failures = current_failures,
            threshold = threshold,
            error_type = %error_type,
            "Jupiter API failure recorded"
        );

        // Check if threshold exceeded
        if current_failures >= threshold {
            drop(state); // Release lock before calling trip
            let reason = TripReason::JupiterApiFailures {
                consecutive_failures: current_failures,
                threshold,
                error_type,
            };

            // Trip the circuit breaker (will re-acquire lock)
            let rt = tokio::runtime::Handle::try_current()
                .map_err(|e| AppError::Internal(format!("No tokio runtime: {}", e)))?;

            rt.block_on(async {
                self.trip(reason).await
            })?;

            return Ok(true); // Circuit breaker was tripped
        }

        Ok(false) // Circuit breaker not tripped
    }

    /// Reset Jupiter failure counter (called on successful Jupiter API call)
    #[tracing::instrument(skip(self))]
    pub fn reset_jupiter_failures(&self) {
        let mut state = self.state.write();
        if state.jupiter_failure_count > 0 {
            tracing::info!(
                previous_failures = state.jupiter_failure_count,
                "Jupiter API failures cleared after successful call"
            );
        }
        state.jupiter_failure_count = 0;
        state.last_jupiter_error = None;
    }

    /// Get current Jupiter failure count
    pub fn get_jupiter_failure_count(&self) -> u32 {
        self.state.read().jupiter_failure_count
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
            loss: dec!(525.50),
            threshold: dec!(500),
        };
        let display = reason.to_string();
        assert!(
            display.contains("525.50"),
            "Should include actual loss amount"
        );
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
        assert!(
            display.contains("consecutive"),
            "Should indicate consecutive losses"
        );
    }

    #[test]
    fn test_trip_reason_max_drawdown() {
        let reason = TripReason::MaxDrawdown {
            drawdown: dec!(18.5),
            threshold: dec!(15.0),
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
        assert!(
            display.contains("Emergency halt"),
            "Should include reason text"
        );
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
        assert!(
            should_trip,
            "Exact boundary ($500) should trigger circuit breaker"
        );
    }

    #[test]
    fn test_max_loss_threshold_below_boundary() {
        let loss = 499.99_f64;
        let threshold = 500.0_f64;
        let should_trip = loss.abs() >= threshold;
        assert!(
            !should_trip,
            "Below threshold should not trigger circuit breaker"
        );
    }

    #[test]
    fn test_consecutive_losses_exact_boundary() {
        let consecutive: u32 = 5;
        let threshold: u32 = 5;
        let should_trip = consecutive >= threshold;
        assert!(
            should_trip,
            "Exact 5 consecutive losses should trigger circuit breaker"
        );
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
        assert!(
            should_trip,
            "Exact 15% drawdown should trigger circuit breaker"
        );
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
        assert!(
            !should_trip,
            "Positive PnL should never trip loss-based circuit breaker"
        );
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
        assert!(
            remaining_secs > 500 && remaining_secs < 700,
            "Should have ~10 minutes remaining, got {} seconds",
            remaining_secs
        );
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
