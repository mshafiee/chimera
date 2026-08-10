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
use crate::db_abstraction::Database;
use crate::error::AppResult;
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
    db.update_circuit_breaker_state(state_str, tripped_at, trip_reason)
        .await
}

/// Load persisted circuit breaker state from the database.
/// A failed read is propagated (fail-closed at startup) rather than silently
/// treated as "no persisted state".
async fn load_cb_state(
    db: &dyn Database,
) -> AppResult<
    Option<(
        String,
        Option<String>,
        Option<String>,
        Option<DateTime<Utc>>,
    )>,
> {
    let state = db.get_circuit_breaker_state().await?;
    let updated_at = state
        .updated_at
        .parse::<DateTime<Utc>>()
        .or_else(|_| DateTime::parse_from_rfc3339(&state.updated_at).map(|d| d.with_timezone(&Utc)))
        .ok();
    Ok(Some((
        state.state,
        state.tripped_at,
        state.trip_reason,
        updated_at,
    )))
}

/// RAII guard that clears the `evaluation_in_progress` flag on drop.
///
/// Without this, an early return (e.g. `?` on a DB error, or a Tripped→Cooldown
/// transition) leaks the flag and every subsequent `evaluate()` call
/// short-circuits at the "already in progress" guard — leaving the breaker in
/// Cooldown forever with automatic recovery disabled.
struct EvaluationGuard<'a> {
    state: &'a Arc<RwLock<InternalState>>,
    armed: bool,
}

impl<'a> EvaluationGuard<'a> {
    /// Sets the flag unless an evaluation is already in progress.
    fn new(state: &'a Arc<RwLock<InternalState>>) -> Self {
        {
            let mut s = state.write();
            if s.evaluation_in_progress {
                return Self {
                    state,
                    armed: false,
                };
            }
            s.evaluation_in_progress = true;
        }
        Self { state, armed: true }
    }

    /// True if this guard set the flag (i.e. the caller may proceed).
    fn armed(&self) -> bool {
        self.armed
    }
}

impl Drop for EvaluationGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.state.write().evaluation_in_progress = false;
        }
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
    /// State restored from a persisted DB record (not an admin action)
    Restored { reason: String },
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
            Self::Restored { reason } => write!(f, "Restored: {}", reason),
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
    /// Baseline timestamp for the consecutive-loss counter. Set on manual reset
    /// (and on startup from the persisted Active-state `updated_at`). The
    /// consecutive-loss check only counts losing trades closed AFTER this
    /// moment, so a reset actually clears the streak instead of re-tripping on
    /// the next tick because the historical losses are still in the DB.
    last_reset_at: Option<DateTime<Utc>>,
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
                last_reset_at: None,
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
        // A failed read propagates here (fail-closed): silently resuming trading
        // when the persisted state said Tripped/Cooldown would be unsafe.
        match load_cb_state(self.db.as_ref()).await? {
            Some((state_str, tripped_at_str, trip_reason_str, updated_at))
                if state_str != "Active" =>
            {
                // A missing/unparseable persisted timestamp must not strand the
                // breaker in Cooldown forever (the cooldown-expiry check returns
                // false when tripped_at is None) — fall back to now so the
                // cooldown clock can still expire.
                let tripped_at = tripped_at_str
                    .as_deref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc))
                    .or_else(|| {
                        tracing::warn!(
                            "Persisted circuit breaker tripped_at is missing/unparseable — \
                             defaulting to now so the cooldown can expire"
                        );
                        Some(Utc::now())
                    });

                let reason = trip_reason_str
                    .clone()
                    .map(|r| TripReason::Restored { reason: r })
                    .unwrap_or(TripReason::Restored {
                        reason: "Restored from persisted state".to_string(),
                    });

                {
                    let mut state = self.state.write();
                    state.state = CircuitBreakerState::Tripped;
                    state.tripped_at = tripped_at;
                    state.trip_reason = Some(reason);
                    // Baseline the consecutive-loss counter at the persisted
                    // state timestamp so the immediate re-evaluate() below does
                    // not re-trip on the historical losing streak. A manual
                    // reset updates this baseline to the reset moment.
                    state.last_reset_at = updated_at.or(tripped_at);
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
            Some((_state_str, _tripped_at, _trip_reason, updated_at)) => {
                // Persisted state is Active (or absent). Baseline the
                // consecutive-loss counter at the persisted Active-state
                // timestamp so a restart right after a manual reset does not
                // re-trip on the historical losing streak still in the DB.
                if let Some(dt) = updated_at {
                    self.state.write().last_reset_at = Some(dt);
                }
                tracing::debug!(
                    "Circuit breaker persisted state is Active or absent — no restore needed"
                );
            }
            None => {
                tracing::debug!("No persisted circuit breaker state — starting Active");
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
            match self.db.get_evaluation_data().await {
                Ok(data) => data,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Circuit breaker: failed to fetch evaluation data (price/DB) — \
                         skipping evaluation this tick"
                    );
                    return Err(e);
                }
            };

        let total_capital = *self.total_capital_sol.read();
        // Skip portfolio stop check for paper trading or zero/low capital scenarios
        // Paper trading often uses test wallets with minimal or no capital
        let portfolio_stop_check_active = total_capital > dec!(1.0);
        let total_loss_sol = realized_pnl_sol + unrealized_sol;
        let daily_loss_percent = if portfolio_stop_check_active {
            (total_loss_sol / total_capital) * Decimal::from(100)
        } else {
            Decimal::ZERO
        };
        // portfolio_stop_loss_percent is negative by convention (default -5.0,
        // validated < 0 in config). Use it directly as the comparison threshold
        // so we trip only when the loss is worse than it (e.g. -6% < -5%).
        // Previously this negated the value (-(-5.0) = +5.0), inverting the
        // comparison and false-tripping on ANY pnl below +5% — including 0%.
        let loss_threshold = self.config.portfolio_stop_loss_percent;
        let portfolio_stop_breached =
            portfolio_stop_check_active && daily_loss_percent < loss_threshold;

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

        let mut unrealized_usd = Decimal::ZERO;
        let mut total_pnl_usd = Decimal::ZERO;
        let mut max_loss_breached = false;
        if let Some(price) = sol_price_usd {
            if price > Decimal::ZERO {
                if null_price_pnl_sol != Decimal::ZERO {
                    let estimated = null_price_pnl_sol * price;
                    realized_usd += estimated;
                }

                unrealized_usd = unrealized_sol * price;
                total_pnl_usd = realized_usd + unrealized_usd;

                max_loss_breached = total_pnl_usd < Decimal::ZERO
                    && total_pnl_usd.abs() >= self.config.max_loss_24h_usd;
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

        // Count consecutive losses only since the last reset baseline (if any),
        // so a manual reset clears the streak instead of re-tripping on the
        // historical losing trades still in the DB.
        let reset_baseline = self.state.read().last_reset_at;
        let consecutive = match self.db.get_consecutive_losses_since(reset_baseline).await {
            Ok(count) => count,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Circuit breaker: failed to fetch consecutive losses — \
                     skipping evaluation this tick"
                );
                return Err(e);
            }
        };
        let consecutive_breached = consecutive >= self.config.max_consecutive_losses;

        let total_capital = *self.total_capital_sol.read();
        let (drawdown, historical_max_drawdown) =
            match self.db.get_max_drawdown_percent(total_capital).await {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Circuit breaker: failed to fetch max drawdown — \
                         skipping evaluation this tick"
                    );
                    return Err(e);
                }
            };
        let drawdown_breached = drawdown >= self.config.max_drawdown_percent;

        tracing::debug!(
            realized_pnl_sol = %realized_pnl_sol,
            unrealized_pnl_sol = %unrealized_sol,
            total_loss_sol = %total_loss_sol,
            total_capital_sol = %total_capital,
            daily_loss_percent = %daily_loss_percent,
            portfolio_stop_threshold_percent = %loss_threshold,
            portfolio_stop_breached = portfolio_stop_breached,
            unrealized_usd = %unrealized_usd,
            total_pnl_usd = %total_pnl_usd,
            max_loss_24h_threshold_usd = %self.config.max_loss_24h_usd,
            max_loss_24h_breached = max_loss_breached,
            consecutive_losses = consecutive,
            consecutive_losses_threshold = self.config.max_consecutive_losses,
            consecutive_losses_breached = consecutive_breached,
            max_drawdown_percent = %drawdown,
            historical_max_drawdown_percent = %historical_max_drawdown,
            max_drawdown_threshold_percent = %self.config.max_drawdown_percent,
            max_drawdown_breached = drawdown_breached,
            sol_price_usd = ?sol_price_usd,
            "circuit_breaker: evaluation"
        );

        if portfolio_stop_breached {
            return Ok(Some(TripReason::PortfolioStop24h {
                loss_pct: daily_loss_percent.abs(),
                threshold: self.config.portfolio_stop_loss_percent,
            }));
        }

        if max_loss_breached {
            return Ok(Some(TripReason::MaxLoss24h {
                loss: total_pnl_usd.abs(),
                threshold: self.config.max_loss_24h_usd,
            }));
        }

        if consecutive_breached {
            return Ok(Some(TripReason::ConsecutiveLosses {
                count: consecutive,
                threshold: self.config.max_consecutive_losses,
            }));
        }

        if drawdown_breached {
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
        // Atomically check if evaluation is already in progress and skip if true.
        // The guard clears the flag on EVERY exit path (including `?` on the
        // DB errors in check_breach_conditions) — see EvaluationGuard.
        let guard = EvaluationGuard::new(&self.state);
        if !guard.armed() {
            tracing::debug!("Circuit breaker evaluation already in progress, skipping");
            return Ok(());
        }

        // FIX [R-M3]: Check interval under write lock but do NOT update last_check yet.
        // last_check is updated only after DB queries succeed (see below).
        {
            let state = self.state.write();
            if let Some(last_check) = state.last_check {
                if Utc::now().signed_duration_since(last_check) < self.check_interval {
                    // Guard clears evaluation flag on drop.
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
            return Ok(());
        }

        if let Some(reason) = self.check_breach_conditions().await? {
            self.trip(reason).await?;
            return Ok(());
        }
        // Update last_check
        {
            let mut state = self.state.write();
            state.last_check = Some(Utc::now());
        }

        Ok(())
    }

    /// Trip the circuit breaker
    #[tracing::instrument(skip(self))]
    async fn trip(&self, reason: TripReason) -> AppResult<()> {
        // Guard against duplicate trips: concurrent Jupiter failures (or failures
        // while already tripped) must not re-log, re-notify, re-increment
        // `trips_total`, or reset the cooldown clock. A Tripped breaker stays
        // Tripped until evaluate()/exit_cooldown() advances it. (A Cooldown→Tripped
        // re-trip from exit_cooldown is still allowed — it is not yet Tripped.)
        {
            let state = self.state.read();
            if state.state == CircuitBreakerState::Tripped {
                tracing::debug!(
                    reason = %reason,
                    "Circuit breaker already TRIPPED — ignoring duplicate trip"
                );
                return Ok(());
            }
        }

        let reason_str = reason.to_string();
        let now = Utc::now();

        {
            let mut state = self.state.write();
            state.state = CircuitBreakerState::Tripped;
            state.tripped_at = Some(now);
            state.trip_reason = Some(reason);
        }

        let trip_reason = self.state.read().trip_reason.clone();

        // Structured trip log with actual numbers vs thresholds for each condition type
        match trip_reason.as_ref() {
            Some(TripReason::MaxLoss24h { loss, threshold }) => {
                tracing::error!(
                    loss_usd = %loss,
                    threshold_usd = %threshold,
                    reason = %reason_str,
                    "Circuit breaker TRIPPED - trading halted"
                );
                tracing::info!(
                    trading_allowed = false,
                    loss_usd = %loss,
                    threshold_usd = %threshold,
                    reason = %reason_str,
                    "Circuit breaker: trading halted (is_trading_allowed=false)"
                );
            }
            Some(TripReason::ConsecutiveLosses { count, threshold }) => {
                tracing::error!(
                    consecutive_losses = count,
                    consecutive_losses_threshold = threshold,
                    reason = %reason_str,
                    "Circuit breaker TRIPPED - trading halted"
                );
                tracing::info!(
                    trading_allowed = false,
                    consecutive_losses = count,
                    consecutive_losses_threshold = threshold,
                    reason = %reason_str,
                    "Circuit breaker: trading halted (is_trading_allowed=false)"
                );
            }
            Some(TripReason::MaxDrawdown {
                drawdown,
                threshold,
            }) => {
                tracing::error!(
                    drawdown_percent = %drawdown,
                    drawdown_threshold_percent = %threshold,
                    reason = %reason_str,
                    "Circuit breaker TRIPPED - trading halted"
                );
                tracing::info!(
                    trading_allowed = false,
                    drawdown_percent = %drawdown,
                    drawdown_threshold_percent = %threshold,
                    reason = %reason_str,
                    "Circuit breaker: trading halted (is_trading_allowed=false)"
                );
            }
            Some(TripReason::PortfolioStop24h {
                loss_pct,
                threshold,
            }) => {
                tracing::error!(
                    loss_percent = %loss_pct,
                    loss_threshold_percent = %threshold,
                    reason = %reason_str,
                    "Circuit breaker TRIPPED - trading halted"
                );
                tracing::info!(
                    trading_allowed = false,
                    loss_percent = %loss_pct,
                    loss_threshold_percent = %threshold,
                    reason = %reason_str,
                    "Circuit breaker: trading halted (is_trading_allowed=false)"
                );
            }
            Some(TripReason::JupiterApiFailures {
                consecutive_failures,
                threshold,
                error_type,
            }) => {
                tracing::error!(
                    jupiter_failures = consecutive_failures,
                    jupiter_failures_threshold = threshold,
                    error_type = %error_type,
                    reason = %reason_str,
                    "Circuit breaker TRIPPED - trading halted"
                );
                tracing::info!(
                    trading_allowed = false,
                    jupiter_failures = consecutive_failures,
                    jupiter_failures_threshold = threshold,
                    error_type = %error_type,
                    reason = %reason_str,
                    "Circuit breaker: trading halted (is_trading_allowed=false)"
                );
            }
            _ => {
                tracing::error!(
                    reason = %reason_str,
                    "Circuit breaker TRIPPED - trading halted"
                );
                tracing::info!(
                    trading_allowed = false,
                    reason = %reason_str,
                    "Circuit breaker: trading halted (is_trading_allowed=false)"
                );
            }
        }

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

        let cleared_trip_reason = self
            .state
            .read()
            .trip_reason
            .as_ref()
            .map(ToString::to_string);
        {
            let mut state = self.state.write();
            state.state = CircuitBreakerState::Active;
            state.tripped_at = None;
            state.trip_reason = None;
            // Fresh start after cooldown: clear the Jupiter failure accumulation
            // so the next outage gets a full threshold window.
            state.jupiter_failure_count = 0;
            state.last_jupiter_error = None;
        }

        // Update Prometheus gauge to Active (2)
        if let Some(gauge) = self.circuit_breaker_state.get() {
            gauge.set(2);
        }

        tracing::info!(
            trading_allowed = true,
            reason = ?cleared_trip_reason,
            "Circuit breaker exiting cooldown - trading resumed (is_trading_allowed=true)"
        );

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
            // Clear Jupiter failure accumulation on manual reset.
            state.jupiter_failure_count = 0;
            state.last_jupiter_error = None;
            // Baseline the consecutive-loss counter at the reset moment so the
            // historical losing streak doesn't re-trip the breaker on the next
            // evaluation tick.
            state.last_reset_at = Some(Utc::now());
        }

        // Update Prometheus gauge to Active (2)
        if let Some(gauge) = self.circuit_breaker_state.get() {
            gauge.set(2);
        }

        tracing::warn!(
            admin = %admin,
            previous_state = %previous_state,
            trading_allowed = true,
            "Circuit breaker manually reset - trading resumed (is_trading_allowed=true)"
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
    ///
    /// Async (not blocking): `trip()` awaits DB/notification work, so this must
    /// never be called via `Handle::block_on` from inside an async context
    /// (that panics on a tokio worker thread and blocks the runtime).
    #[tracing::instrument(skip(self))]
    pub async fn record_jupiter_failure(&self, error_type: String) -> AppResult<bool> {
        // Scope the parking_lot write guard to a block: holding the (non-Send)
        // guard across an await would make this future !Send.
        let (current_failures, threshold) = {
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

            (current_failures, threshold)
        };

        // Check if threshold exceeded
        if current_failures >= threshold {
            let reason = TripReason::JupiterApiFailures {
                consecutive_failures: current_failures,
                threshold,
                error_type,
            };

            // Trip the circuit breaker (will re-acquire lock)
            self.trip(reason).await?;

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

    // ==========================================================================
    // ADDITIONAL COVERAGE: full state machine against the in-memory MockDb
    // ==========================================================================

    use crate::monitoring::test_db::MockDb;
    use rust_decimal::Decimal;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    fn cfg() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            max_loss_24h_usd: Decimal::from(500),
            max_consecutive_losses: 3,
            max_drawdown_percent: Decimal::from(15),
            portfolio_stop_loss_percent: Decimal::from(-5),
            cooldown_minutes: 30,
            max_jupiter_failures: 3,
        }
    }

    fn make_cb(db: Arc<MockDb>) -> CircuitBreaker {
        CircuitBreaker::new(cfg(), db.clone(), Decimal::from(1000))
    }

    /// Force-evaluate without the 5s check-interval gate.
    async fn evaluate_now(cb: &CircuitBreaker) {
        cb.state.write().last_check = Some(Utc::now() - Duration::seconds(60));
        cb.evaluate().await.unwrap();
    }

    // ── basic accessors ────────────────────────────────────────────────────

    #[test]
    fn test_accessors_and_capital() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db);
        assert!(cb.is_trading_allowed());
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);
        assert!(cb.trip_reason().is_none());

        cb.update_capital(Decimal::from(2000));
        assert_eq!(*cb.total_capital_sol.read(), Decimal::from(2000));

        let status = cb.status();
        assert_eq!(status.state, CircuitBreakerState::Active);
        assert!(status.cooldown_remaining_secs.is_none());
    }

    #[test]
    fn test_set_metrics_and_notifier() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db);
        let gauge = prometheus::IntGauge::new("cb_test_gauge", "h").unwrap();
        let counter = prometheus::IntCounter::new("cb_test_counter", "h").unwrap();
        cb.set_metrics(gauge, counter);
        cb.circuit_breaker_state.get().unwrap().set(9);
        assert_eq!(cb.circuit_breaker_state.get().unwrap().get(), 9);
        let _ = cb.trips_total.get().unwrap().get();

        let notifier = Arc::new(crate::notifications::CompositeNotifier::new());
        cb.set_notifier(notifier);
        assert!(cb.notifier.get().is_some());
    }

    #[tokio::test]
    async fn test_with_price_cache_builder() {
        let db = Arc::new(MockDb::new());
        let cache = Arc::new(crate::price_cache::PriceCache::new().unwrap());
        let cb = make_cb(db).with_price_cache(cache.clone());
        assert!(cb.price_cache.is_some());
    }

    #[test]
    fn test_new_with_ws_sets_ws_state() {
        let db = Arc::new(MockDb::new());
        let ws = Arc::new(crate::handlers::WsState::new(
            std::collections::HashMap::new(),
            "secret".to_string(),
            false,
        ));
        let cb = CircuitBreaker::new_with_ws(cfg(), db.clone(), Some(ws), Decimal::from(100));
        assert!(cb.ws_state.is_some());
    }

    // ── evaluation guard ───────────────────────────────────────────────────

    #[test]
    fn test_evaluation_guard_prevents_concurrent() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db);
        let guard1 = EvaluationGuard::new(&cb.state);
        assert!(guard1.armed());
        let guard2 = EvaluationGuard::new(&cb.state);
        assert!(!guard2.armed(), "second guard must not arm");
        drop(guard1);
        let guard3 = EvaluationGuard::new(&cb.state);
        assert!(guard3.armed(), "flag cleared on drop");
    }

    // ── evaluate() state machine ───────────────────────────────────────────

    #[tokio::test]
    async fn test_evaluate_skips_when_in_progress() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db);
        cb.state.write().evaluation_in_progress = true;
        cb.evaluate().await.unwrap();
        // still active, nothing ran
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);
        cb.state.write().evaluation_in_progress = false;
    }

    #[tokio::test]
    async fn test_evaluate_rate_limited_within_interval() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db.clone());
        cb.state.write().last_check = Some(Utc::now());
        // Evaluation data would trip, but the interval gate skips it.
        db.evaluation_data.lock().unwrap().replace((
            Decimal::ZERO,
            Decimal::from(-1000),
            Decimal::from(-1000),
            Decimal::ZERO,
        ));
        cb.evaluate().await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);
    }

    #[tokio::test]
    async fn test_evaluate_trips_on_each_condition() {
        // Portfolio stop (needs total_capital > 1 and daily loss below -5%).
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db.clone());
        db.evaluation_data.lock().unwrap().replace((
            Decimal::from(-10),  // unrealized
            Decimal::from(-100), // realized
            Decimal::ZERO,
            Decimal::ZERO,
        ));
        evaluate_now(&cb).await;
        assert_eq!(cb.current_state(), CircuitBreakerState::Tripped);
        assert!(matches!(
            cb.trip_reason(),
            Some(TripReason::PortfolioStop24h { .. })
        ));

        // Reset and trip on max loss (USD) — requires a SOL price.
        cb.reset("t").await.unwrap();
        let cache = Arc::new(crate::price_cache::PriceCache::new().unwrap());
        cache.set_price(
            crate::constants::mints::SOL,
            Decimal::ONE,
            crate::price_cache::PriceSource::Jupiter,
            Some(9),
        );
        let cb2 = make_cb(db.clone()).with_price_cache(cache.clone());
        cb2.update_capital(Decimal::from(100000));
        db.evaluation_data.lock().unwrap().replace((
            Decimal::from(-600),
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
        ));
        evaluate_now(&cb2).await;
        assert!(matches!(
            cb2.trip_reason(),
            Some(TripReason::MaxLoss24h { .. })
        ));
        cb2.reset("t").await.unwrap();

        // Consecutive losses (clear the prior evaluation data first so the
        // portfolio stop cannot pre-empt the consecutive check).
        db.evaluation_data.lock().unwrap().replace((
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
        ));
        let cb3 = make_cb(db.clone());
        db.consecutive_losses.lock().unwrap().replace(5);
        evaluate_now(&cb3).await;
        assert!(matches!(
            cb3.trip_reason(),
            Some(TripReason::ConsecutiveLosses { count: 5, .. })
        ));
        cb3.reset("t").await.unwrap();

        // Drawdown (clear the consecutive-loss counter first).
        db.consecutive_losses.lock().unwrap().replace(0);
        let cb4 = make_cb(db.clone());
        db.drawdown
            .lock()
            .unwrap()
            .replace((Decimal::from(20), Decimal::from(50)));
        evaluate_now(&cb4).await;
        assert!(matches!(
            cb4.trip_reason(),
            Some(TripReason::MaxDrawdown { .. })
        ));
    }

    #[tokio::test]
    async fn test_evaluate_no_breach_updates_last_check() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db);
        cb.state.write().last_check = Some(Utc::now() - Duration::seconds(60));
        cb.evaluate().await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);
        assert!(cb.state.read().last_check.is_some());
    }

    #[tokio::test]
    async fn test_evaluate_portfolio_stop_inactive_low_capital() {
        let db = Arc::new(MockDb::new());
        // total_capital = 0.5 SOL → portfolio stop check disabled.
        let cb = CircuitBreaker::new(cfg(), db.clone(), Decimal::from_str("0.5").unwrap());
        db.evaluation_data.lock().unwrap().replace((
            Decimal::from(-10),
            Decimal::from(-100),
            Decimal::ZERO,
            Decimal::ZERO,
        ));
        evaluate_now(&cb).await;
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);
    }

    #[tokio::test]
    async fn test_evaluate_error_paths_skip_tick() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db.clone());
        db.evaluation_error.store(true, Ordering::Relaxed);
        let result = cb.evaluate().await;
        assert!(result.is_err(), "evaluation errors propagate");
        assert!(
            !cb.state.read().evaluation_in_progress,
            "guard cleared after error"
        );

        db.evaluation_error.store(false, Ordering::Relaxed);
        db.consecutive_error.store(true, Ordering::Relaxed);
        let result = cb.evaluate().await;
        assert!(result.is_err());
        assert!(!cb.state.read().evaluation_in_progress);

        db.consecutive_error.store(false, Ordering::Relaxed);
        db.drawdown_error.store(true, Ordering::Relaxed);
        let result = cb.evaluate().await;
        assert!(result.is_err());
        assert!(!cb.state.read().evaluation_in_progress);
    }

    #[tokio::test]
    async fn test_evaluate_sol_price_variants() {
        // No price cache → USD checks skipped, no trip. (Capital is large so
        // the -600 SOL loss stays far above the -5% portfolio stop.)
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db.clone());
        cb.update_capital(Decimal::from(100000));
        db.evaluation_data.lock().unwrap().replace((
            Decimal::from(-600),
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
        ));
        evaluate_now(&cb).await;
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);

        // Price cache with ZERO price → warn path.
        let cache = Arc::new(crate::price_cache::PriceCache::new().unwrap());
        cache.set_price(
            crate::constants::mints::SOL,
            Decimal::ZERO,
            crate::price_cache::PriceSource::Jupiter,
            Some(9),
        );
        let cb2 = make_cb(db.clone()).with_price_cache(cache.clone());
        cb2.update_capital(Decimal::from(100000));
        evaluate_now(&cb2).await;
        assert_eq!(cb2.current_state(), CircuitBreakerState::Active);

        // Null-price PnL estimation path with a real price.
        let db3 = Arc::new(MockDb::new());
        let cache3 = Arc::new(crate::price_cache::PriceCache::new().unwrap());
        cache3.set_price(
            crate::constants::mints::SOL,
            Decimal::from(2),
            crate::price_cache::PriceSource::Jupiter,
            Some(9),
        );
        let cb3 = make_cb(db3.clone()).with_price_cache(cache3.clone());
        db3.evaluation_data.lock().unwrap().replace((
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::from(-700), // realized USD
            Decimal::from(50),   // null-price SOL pnl → estimated 100 USD
        ));
        evaluate_now(&cb3).await;
        // realized 500 total → trips MaxLoss24h (>= 500).
        assert!(matches!(
            cb3.trip_reason(),
            Some(TripReason::MaxLoss24h { .. })
        ));
    }

    // ── trip() ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_trip_ignores_duplicate_when_already_tripped() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db.clone());
        cb.manual_trip("admin", "first".into()).await.unwrap();
        let trips_before = cb.trips_total.get().map(|c| c.get()).unwrap_or(0);

        cb.trip(TripReason::Manual {
            reason: "second".into(),
        })
        .await
        .unwrap();
        assert_eq!(
            cb.trips_total.get().map(|c| c.get()).unwrap_or(0),
            trips_before,
            "duplicate trip must not re-increment"
        );
        assert_eq!(cb.trip_reason().unwrap().to_string(), "Manual: first");
    }

    #[tokio::test]
    async fn test_trip_all_reason_logging_branches() {
        for reason in [
            TripReason::MaxLoss24h {
                loss: dec!(100),
                threshold: dec!(500),
            },
            TripReason::ConsecutiveLosses {
                count: 4,
                threshold: 3,
            },
            TripReason::MaxDrawdown {
                drawdown: dec!(20),
                threshold: dec!(15),
            },
            TripReason::PortfolioStop24h {
                loss_pct: dec!(6),
                threshold: dec!(-5),
            },
            TripReason::JupiterApiFailures {
                consecutive_failures: 4,
                threshold: 3,
                error_type: "http".into(),
            },
            TripReason::Manual {
                reason: "manual".into(),
            },
            TripReason::Restored {
                reason: "restored".into(),
            },
        ] {
            let db = Arc::new(MockDb::new());
            let cb = make_cb(db.clone());
            let gauge = prometheus::IntGauge::new("cb_trip_gauge", "h").unwrap();
            let counter = prometheus::IntCounter::new("cb_trip_counter", "h").unwrap();
            cb.set_metrics(gauge.clone(), counter.clone());
            cb.trip(reason.clone()).await.unwrap();
            assert_eq!(cb.current_state(), CircuitBreakerState::Tripped);
            assert_eq!(cb.trip_reason().unwrap().to_string(), reason.to_string());
            assert_eq!(gauge.get(), 0, "gauge set to Tripped");
            assert_eq!(counter.get(), 1);
            // Persisted to MockDb.
            let persisted = db.circuit_breaker_state.lock().unwrap().clone();
            assert_eq!(persisted.unwrap().state, "Tripped");
        }
    }

    #[tokio::test]
    async fn test_trip_with_ws_and_notifier() {
        let db = Arc::new(MockDb::new());
        let ws = Arc::new(crate::handlers::WsState::new(
            std::collections::HashMap::new(),
            "secret".to_string(),
            false,
        ));
        let cb = CircuitBreaker::new_with_ws(cfg(), db.clone(), Some(ws.clone()), dec!(1000));
        let mut rx = ws.tx.subscribe();
        cb.set_notifier(Arc::new(crate::notifications::CompositeNotifier::new()));
        cb.trip(TripReason::Manual {
            reason: "ws".into(),
        })
        .await
        .unwrap();
        let event = rx.try_recv().expect("ws broadcast");
        assert!(matches!(event, crate::handlers::WsEvent::Alert(_)));
    }

    #[tokio::test]
    async fn test_trip_persist_failure_is_non_fatal() {
        let db = Arc::new(MockDb::new());
        db.cb_state_error.store(true, Ordering::Relaxed);
        let cb = make_cb(db);
        cb.trip(TripReason::Manual { reason: "x".into() })
            .await
            .unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Tripped);
    }

    // ── cooldown / exit ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_enter_cooldown_requires_tripped_state() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db);
        // No-op while Active.
        cb.enter_cooldown().await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);

        // Tripped → Cooldown.
        cb.trip(TripReason::Manual { reason: "x".into() })
            .await
            .unwrap();
        cb.enter_cooldown().await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Cooldown);
        let status = cb.status();
        assert_eq!(status.state, CircuitBreakerState::Cooldown);
        assert!(status.cooldown_remaining_secs.is_some());
    }

    #[tokio::test]
    async fn test_exit_cooldown_retrips_when_breach_persists() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db.clone());
        db.evaluation_data.lock().unwrap().replace((
            Decimal::ZERO,
            Decimal::from(-1000),
            Decimal::ZERO,
            Decimal::ZERO,
        ));
        cb.trip(TripReason::Manual { reason: "x".into() })
            .await
            .unwrap();
        cb.enter_cooldown().await.unwrap();

        // Cooldown expired → exit_cooldown re-checks → re-trips.
        cb.state.write().tripped_at = Some(Utc::now() - Duration::hours(1));
        cb.evaluate().await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Tripped);
    }

    #[tokio::test]
    async fn test_exit_cooldown_resumes_when_cleared() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db.clone());
        let gauge = prometheus::IntGauge::new("cb_exit_gauge", "h").unwrap();
        let counter = prometheus::IntCounter::new("cb_exit_counter", "h").unwrap();
        cb.set_metrics(gauge.clone(), counter.clone());

        cb.trip(TripReason::Manual { reason: "x".into() })
            .await
            .unwrap();
        cb.enter_cooldown().await.unwrap();
        cb.state.write().tripped_at = Some(Utc::now() - Duration::hours(1));

        // No breach data → cooldown exits to Active.
        cb.evaluate().await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);
        assert_eq!(gauge.get(), 2, "gauge set to Active");
        assert_eq!(cb.get_jupiter_failure_count(), 0);
        let persisted = db.circuit_breaker_state.lock().unwrap().clone();
        assert_eq!(persisted.unwrap().state, "Active");
    }

    #[tokio::test]
    async fn test_cooldown_not_expired_stays_in_cooldown() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db);
        cb.trip(TripReason::Manual { reason: "x".into() })
            .await
            .unwrap();
        cb.enter_cooldown().await.unwrap();
        // tripped_at is "now" → cooldown not expired → stays Cooldown.
        cb.evaluate().await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Cooldown);
    }

    // ── reset / manual trip / jupiter failures ─────────────────────────────

    #[tokio::test]
    async fn test_reset_clears_everything() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db.clone());
        cb.record_jupiter_failure("http".into()).await.unwrap();
        cb.trip(TripReason::Manual { reason: "x".into() })
            .await
            .unwrap();
        let gauge = prometheus::IntGauge::new("cb_reset_gauge", "h").unwrap();
        let counter = prometheus::IntCounter::new("cb_reset_counter", "h").unwrap();
        cb.set_metrics(gauge.clone(), counter.clone());

        cb.reset("admin").await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);
        assert!(cb.trip_reason().is_none());
        assert_eq!(cb.get_jupiter_failure_count(), 0);
        assert_eq!(gauge.get(), 2);
        assert!(cb.state.read().last_reset_at.is_some());
    }

    #[tokio::test]
    async fn test_manual_trip_logs_config_change() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db.clone());
        cb.manual_trip("admin", "because".into()).await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Tripped);
        assert!(
            matches!(cb.trip_reason(), Some(TripReason::Manual { reason }) if reason == "because")
        );
    }

    #[tokio::test]
    async fn test_record_jupiter_failure_below_threshold() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db);
        assert!(!cb.record_jupiter_failure("http".into()).await.unwrap());
        assert!(!cb.record_jupiter_failure("http".into()).await.unwrap());
        assert_eq!(cb.get_jupiter_failure_count(), 2);
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);
        assert_eq!(cb.state.read().last_jupiter_error.as_deref(), Some("http"));
    }

    #[tokio::test]
    async fn test_record_jupiter_failure_trips_at_threshold() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db);
        assert!(!cb.record_jupiter_failure("timeout".into()).await.unwrap());
        assert!(!cb.record_jupiter_failure("timeout".into()).await.unwrap());
        let tripped = cb.record_jupiter_failure("timeout".into()).await.unwrap();
        assert!(tripped, "third failure at threshold=3 trips");
        assert_eq!(cb.current_state(), CircuitBreakerState::Tripped);
        assert!(matches!(
            cb.trip_reason(),
            Some(TripReason::JupiterApiFailures {
                consecutive_failures: 3,
                ..
            })
        ));
    }

    #[test]
    fn test_reset_jupiter_failures() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db);
        {
            let mut state = cb.state.write();
            state.jupiter_failure_count = 4;
            state.last_jupiter_error = Some("http".into());
        }
        cb.reset_jupiter_failures();
        assert_eq!(cb.get_jupiter_failure_count(), 0);
        assert!(cb.state.read().last_jupiter_error.is_none());
    }

    // ── restore_from_db ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_restore_from_db_no_persisted_state() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db);
        cb.restore_from_db().await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);
    }

    #[tokio::test]
    async fn test_restore_from_db_active_state_sets_baseline() {
        let db = Arc::new(MockDb::new());
        db.update_circuit_breaker_state("Active", None, None)
            .await
            .unwrap();
        // Ensure updated_at is parseable (rfc3339 from MockDb).
        let cb = make_cb(db);
        cb.restore_from_db().await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);
        assert!(cb.state.read().last_reset_at.is_some());
    }

    #[tokio::test]
    async fn test_restore_from_db_tripped_state_with_reason() {
        let db = Arc::new(MockDb::new());
        db.update_circuit_breaker_state(
            "Tripped",
            Some(Utc::now() - Duration::hours(2)),
            Some("crash".into()),
        )
        .await
        .unwrap();
        let cb = make_cb(db);
        cb.restore_from_db().await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Cooldown);
        assert!(matches!(
            cb.trip_reason(),
            Some(TripReason::Restored { .. })
        ));
    }

    #[tokio::test]
    async fn test_restore_from_db_tripped_unparseable_timestamp() {
        let db = Arc::new(MockDb::new());
        db.circuit_breaker_state.lock().unwrap().replace(
            crate::db_abstraction::CircuitBreakerState {
                state: "Cooldown".to_string(),
                tripped_at: Some("not-a-date".to_string()),
                trip_reason: None,
                updated_at: "also-not-a-date".to_string(),
            },
        );
        let cb = make_cb(db);
        cb.restore_from_db().await.unwrap();
        // Unparseable timestamp falls back to now; still Tripped/Cooldown path.
        assert!(matches!(
            cb.current_state(),
            CircuitBreakerState::Cooldown | CircuitBreakerState::Tripped
        ));
        assert!(cb.state.read().tripped_at.is_some());
    }

    #[tokio::test]
    async fn test_restore_from_db_load_error_propagates() {
        let db = Arc::new(MockDb::new());
        db.cb_state_error.store(true, Ordering::Relaxed);
        let cb = make_cb(db);
        let result = cb.restore_from_db().await;
        assert!(result.is_err(), "fail-closed on unreadable state");
    }

    #[test]
    fn test_trip_reason_display_remaining_variants() {
        let portfolio = TripReason::PortfolioStop24h {
            loss_pct: dec!(6.5),
            threshold: dec!(-5),
        };
        assert!(portfolio.to_string().contains("6.5"));
        assert!(portfolio.to_string().contains("portfolio stop"));

        let jup = TripReason::JupiterApiFailures {
            consecutive_failures: 4,
            threshold: 3,
            error_type: "timeout".into(),
        };
        assert!(jup.to_string().contains("4"));
        assert!(jup.to_string().contains("timeout"));

        let restored = TripReason::Restored {
            reason: "from db".into(),
        };
        assert!(restored.to_string().contains("Restored: from db"));
    }

    #[test]
    fn test_set_metrics_cooldown_state() {
        let db = Arc::new(MockDb::new());
        let cb = make_cb(db);
        cb.state.write().state = CircuitBreakerState::Cooldown;
        let gauge = prometheus::IntGauge::new("cb_cd_gauge", "h").unwrap();
        let counter = prometheus::IntCounter::new("cb_cd_counter", "h").unwrap();
        cb.set_metrics(gauge.clone(), counter.clone());
        assert_eq!(gauge.get(), 1, "Cooldown maps to gauge value 1");
    }

    #[tokio::test]
    async fn test_exit_cooldown_persist_failure_is_non_fatal() {
        let db = Arc::new(MockDb::new());
        db.cb_update_error.store(true, Ordering::Relaxed);
        let cb = make_cb(db.clone());
        cb.trip(TripReason::Manual { reason: "x".into() })
            .await
            .unwrap();
        cb.enter_cooldown().await.unwrap();
        cb.state.write().tripped_at = Some(Utc::now() - Duration::hours(1));
        // Exit succeeds in memory even though the persist write fails.
        cb.evaluate().await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);
    }

    #[tokio::test]
    async fn test_reset_persist_failure_is_non_fatal() {
        let db = Arc::new(MockDb::new());
        db.cb_update_error.store(true, Ordering::Relaxed);
        let cb = make_cb(db.clone());
        cb.trip(TripReason::Manual { reason: "x".into() })
            .await
            .unwrap();
        cb.reset("admin").await.unwrap();
        assert_eq!(cb.current_state(), CircuitBreakerState::Active);
    }

    // The `persist_cb_state` helper is exercised through every trip/reset path
    // above (Tripped + Active strings; the Cooldown arm is unreachable because
    // enter_cooldown never persists).
}
