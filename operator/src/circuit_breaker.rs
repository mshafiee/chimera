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
use rust_decimal::prelude::*;
use std::sync::Arc;

/// Persist circuit breaker state to the database
async fn persist_cb_state(
    db: &DbPool,
    state: CircuitBreakerState,
    tripped_at: Option<DateTime<Utc>>,
    trip_reason: Option<&str>,
) -> AppResult<()> {
    let state_str = match state {
        CircuitBreakerState::Active => "Active",
        CircuitBreakerState::Tripped => "Tripped",
        CircuitBreakerState::Cooldown => "Cooldown",
    };
    let tripped_at_str = tripped_at.map(|t| t.to_rfc3339());
    sqlx::query(
        r#"UPDATE circuit_breaker_state
           SET state = ?, tripped_at = ?, trip_reason = ?, updated_at = datetime('now')
           WHERE id = 1"#,
    )
    .bind(state_str)
    .bind(tripped_at_str)
    .bind(trip_reason)
    .execute(db)
    .await?;
    Ok(())
}

/// Load persisted circuit breaker state from the database
async fn load_cb_state(
    db: &DbPool,
) -> AppResult<Option<(String, Option<String>, Option<String>)>> {
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT state, tripped_at, trip_reason FROM circuit_breaker_state WHERE id = 1",
    )
    .fetch_optional(db)
    .await?;
    Ok(row)
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
    MaxLoss24h { loss: f64, threshold: f64 },
    /// Consecutive losses exceeded threshold
    ConsecutiveLosses { count: u32, threshold: u32 },
    /// Drawdown from peak exceeded threshold
    MaxDrawdown { drawdown: f64, threshold: f64 },
    /// 24h SOL-denominated loss exceeded threshold (portfolio stop)
    PortfolioStop24h { loss_pct: f64, threshold: f64 },
    /// Manual trip by admin
    Manual { reason: String },
}

impl std::fmt::Display for TripReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MaxLoss24h { loss, threshold } => {
                write!(
                    f,
                    "24h loss ${:.2} exceeded threshold ${:.2}",
                    loss, threshold
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
                    "Drawdown {:.1}% exceeded threshold {:.1}%",
                    drawdown, threshold
                )
            }
            Self::PortfolioStop24h { loss_pct, threshold } => {
                write!(
                    f,
                    "24h realized SOL loss {:.2}% exceeded threshold {:.2}% (portfolio stop)",
                    loss_pct, threshold
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
    /// Optional WebSocket state for broadcasting events
    ws_state: Option<Arc<crate::handlers::WsState>>,
    /// Total capital in SOL for portfolio stop calculation — shared with PortfolioHeat so
    /// that balance refreshes (every 60s in main.rs) propagate here automatically.
    total_capital_sol: Arc<RwLock<Decimal>>,
    /// Price cache for converting unrealized SOL losses to USD
    price_cache: Option<Arc<crate::price_cache::PriceCache>>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker
    pub fn new(config: CircuitBreakerConfig, db: DbPool) -> Self {
        Self::new_with_ws(config, db, None)
    }

    /// Create a new circuit breaker with WebSocket support
    pub fn new_with_ws(
        config: CircuitBreakerConfig,
        db: DbPool,
        ws_state: Option<Arc<crate::handlers::WsState>>,
    ) -> Self {
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
            ws_state,
            total_capital_sol: Arc::new(RwLock::new(Decimal::from(10))), // default to 10 SOL
            price_cache: None,
        }
    }

    /// Set price cache
    pub fn with_price_cache(mut self, price_cache: Arc<crate::price_cache::PriceCache>) -> Self {
        self.price_cache = Some(price_cache);
        self
    }

    /// Restore persisted circuit breaker state from DB on startup.
    /// Call this after construction but before the server starts accepting connections.
    pub async fn restore_from_db(&self) -> AppResult<()> {
        match load_cb_state(&self.db).await? {
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
                tracing::debug!("Circuit breaker persisted state is Active or absent — no restore needed");
            }
        }
        Ok(())
    }

    /// Set total capital in SOL (builder method, used at construction time)
    pub fn with_total_capital(self, total_capital_sol: Decimal) -> Self {
        *self.total_capital_sol.write() = total_capital_sol;
        self
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

    /// Evaluate trip conditions and update state
    pub async fn evaluate(&self) -> AppResult<()> {
        // FIX [R-M3]: Check interval under write lock but do NOT update last_check yet.
        // last_check is updated only after DB queries succeed (see below).
        {
            let state = self.state.write();
            if let Some(last_check) = state.last_check {
                if Utc::now().signed_duration_since(last_check) < self.check_interval {
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

        // Query unrealized SOL PnL for active/exiting positions
        let unrealized_sol_f64: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(unrealized_pnl_sol), 0.0)
            FROM positions
            WHERE state IN ('ACTIVE', 'EXITING')
            "#,
        )
        .fetch_one(&self.db)
        .await?;
        let unrealized_sol = Decimal::from_f64_retain(unrealized_sol_f64).unwrap_or(Decimal::ZERO);

        // FIX [R-H4]: Check 24h SOL portfolio stop using two clean separate queries to
        // avoid double-counting positions that are still ACTIVE/EXITING but also have a
        // closed_at timestamp (e.g. stuck positions).
        let total_capital = *self.total_capital_sol.read();
        if total_capital > Decimal::from_f64_retain(0.1).unwrap_or(Decimal::ZERO) {
            // Query 1: realized PnL from CLOSED positions in the last 24h
            let realized_pnl_sol_f64: f64 = sqlx::query_scalar::<_, f64>(
                r#"
                SELECT COALESCE(SUM(realized_pnl_sol), 0.0)
                FROM positions
                WHERE state = 'CLOSED'
                  AND closed_at >= datetime('now', '-24 hours')
                "#,
            )
            .fetch_one(&self.db)
            .await?;
            // Query 2: unrealized PnL from currently open positions (already fetched above)
            // unrealized_sol already holds SUM(unrealized_pnl_sol) WHERE state IN ('ACTIVE','EXITING')
            let realized_pnl_sol = Decimal::from_f64_retain(realized_pnl_sol_f64).unwrap_or(Decimal::ZERO);
            let total_loss_sol = realized_pnl_sol + unrealized_sol;
            let daily_loss_percent = (total_loss_sol / total_capital) * Decimal::from(100);
            let loss_threshold = Decimal::from_str("-5.0").expect("literal");
            if daily_loss_percent < loss_threshold {
                self.trip(TripReason::PortfolioStop24h {
                    loss_pct: daily_loss_percent.abs().to_f64().unwrap_or(0.0),
                    threshold: 5.0,
                })
                .await?;
                return Ok(());
            }
        }

        // Sum realized USD PnL in the last 24h — explicitly exclude NULL rows.
        // Positions closed when SOL price was unavailable have NULL realized_pnl_usd.
        // SUM() silently ignores those rows, undercounting losses. We recover them
        // by summing their SOL PnL separately and converting at the current price.
        let realized_usd_f64: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(realized_pnl_usd), 0.0)
            FROM positions
            WHERE state = 'CLOSED'
              AND closed_at >= datetime('now', '-24 hours')
              AND realized_pnl_usd IS NOT NULL
            "#,
        )
        .fetch_one(&self.db)
        .await?;

        let null_price_pnl_sol_f64: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(realized_pnl_sol), 0.0)
            FROM positions
            WHERE state = 'CLOSED'
              AND closed_at >= datetime('now', '-24 hours')
              AND realized_pnl_usd IS NULL
            "#,
        )
        .fetch_one(&self.db)
        .await?;

        if null_price_pnl_sol_f64 != 0.0 {
            tracing::warn!(
                null_price_pnl_sol = null_price_pnl_sol_f64,
                "Circuit breaker: positions closed without USD price data in 24h window — \
                 estimating their PnL from SOL-denominated value"
            );
        }

        let mut realized_usd = Decimal::from_f64_retain(realized_usd_f64).unwrap_or(Decimal::ZERO);

        // Get SOL price in USD from price cache
        let sol_price_usd = if let Some(ref cache) = self.price_cache {
            cache.get_price_usd(crate::constants::mints::SOL).unwrap_or(Decimal::ZERO)
        } else {
            Decimal::ZERO
        };

        // Add best-effort USD estimate for positions closed without price data
        if null_price_pnl_sol_f64 != 0.0 && sol_price_usd > Decimal::ZERO {
            let estimated = Decimal::from_f64_retain(null_price_pnl_sol_f64)
                .unwrap_or(Decimal::ZERO) * sol_price_usd;
            realized_usd += estimated;
        }

        let unrealized_usd = unrealized_sol * sol_price_usd;
        let total_pnl_usd = realized_usd + unrealized_usd;

        // Check 24h loss in USD (sum of realized USD + unrealized USD)
        if total_pnl_usd < Decimal::ZERO && total_pnl_usd.abs() >= self.config.max_loss_24h_usd {
            self.trip(TripReason::MaxLoss24h {
                loss: total_pnl_usd.abs().to_f64().unwrap_or(0.0),
                threshold: self.config.max_loss_24h_usd.to_f64().unwrap_or(0.0),
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
                drawdown: drawdown.to_f64().unwrap_or(0.0),
                threshold: self.config.max_drawdown_percent.to_f64().unwrap_or(0.0),
            })
            .await?;
            return Ok(());
        }

        // FIX [R-M3]: Update last_check only after all DB queries have succeeded.
        // If any query above fails (early return via ?), last_check is not advanced,
        // so the next call will retry immediately rather than silently skipping a cycle.
        {
            self.state.write().last_check = Some(Utc::now());
        }

        Ok(())
    }

    /// Trip the circuit breaker
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
            &self.db,
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
        db::log_config_change(
            &self.db,
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

        db::log_config_change(
            &self.db,
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
        // Query unrealized SOL PnL for active/exiting positions
        let unrealized_sol_f64: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(unrealized_pnl_sol), 0.0)
            FROM positions
            WHERE state IN ('ACTIVE', 'EXITING')
            "#,
        )
        .fetch_one(&self.db)
        .await?;
        let unrealized_sol = Decimal::from_f64_retain(unrealized_sol_f64).unwrap_or(Decimal::ZERO);

        // FIX [R-H4]: Re-check portfolio stop using two clean separate queries (same as evaluate()).
        let total_capital = *self.total_capital_sol.read();
        if total_capital > Decimal::from_f64_retain(0.1).unwrap_or(Decimal::ZERO) {
            // Query 1: realized PnL from CLOSED positions in the last 24h only
            let realized_pnl_sol_f64: f64 = sqlx::query_scalar::<_, f64>(
                r#"
                SELECT COALESCE(SUM(realized_pnl_sol), 0.0)
                FROM positions
                WHERE state = 'CLOSED'
                  AND closed_at >= datetime('now', '-24 hours')
                "#,
            )
            .fetch_one(&self.db)
            .await?;
            // Query 2: unrealized PnL from open positions (already in unrealized_sol)
            let realized_pnl_sol = Decimal::from_f64_retain(realized_pnl_sol_f64).unwrap_or(Decimal::ZERO);
            let total_loss_sol = realized_pnl_sol + unrealized_sol;
            let daily_loss_percent = (total_loss_sol / total_capital) * Decimal::from(100);
            let loss_threshold = Decimal::from_str("-5.0").expect("literal");
            if daily_loss_percent < loss_threshold {
                let trip_reason = TripReason::PortfolioStop24h {
                    loss_pct: daily_loss_percent.abs().to_f64().unwrap_or(0.0),
                    threshold: 5.0,
                };
                // FIX [R-M2]: Log re-trip event before calling trip().
                tracing::warn!(
                    reason = ?trip_reason,
                    original_tripped_at = ?self.state.read().tripped_at,
                    "Circuit breaker re-tripped during cooldown exit — clock reset"
                );
                self.trip(trip_reason).await?;
                tracing::warn!("Circuit breaker cooldown expired but daily SOL loss still breached — re-tripped");
                return Ok(());
            }
        }

        // Re-evaluate breach conditions before clearing cooldown.
        // Sum realized USD PnL in the last 24h — explicitly exclude NULL rows (see evaluate()).
        let realized_usd_f64: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(realized_pnl_usd), 0.0)
            FROM positions
            WHERE state = 'CLOSED'
              AND closed_at >= datetime('now', '-24 hours')
              AND realized_pnl_usd IS NOT NULL
            "#,
        )
        .fetch_one(&self.db)
        .await?;

        let null_price_pnl_sol_f64: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(realized_pnl_sol), 0.0)
            FROM positions
            WHERE state = 'CLOSED'
              AND closed_at >= datetime('now', '-24 hours')
              AND realized_pnl_usd IS NULL
            "#,
        )
        .fetch_one(&self.db)
        .await?;

        if null_price_pnl_sol_f64 != 0.0 {
            tracing::warn!(
                null_price_pnl_sol = null_price_pnl_sol_f64,
                "Circuit breaker cooldown: positions closed without USD price data in 24h window — \
                 estimating their PnL from SOL-denominated value"
            );
        }

        let mut realized_usd = Decimal::from_f64_retain(realized_usd_f64).unwrap_or(Decimal::ZERO);

        // Get SOL price in USD from price cache.
        // If the price cache is unavailable or stale (returns None/zero), skip the
        // USD threshold check entirely — computing unrealized_usd as 0 would
        // understate the loss and allow cooldown exit while the breach still holds.
        let sol_price_usd = if let Some(ref cache) = self.price_cache {
            cache.get_price_usd(crate::constants::mints::SOL)
        } else {
            None
        };

        let usd_check_result = if let Some(price) = sol_price_usd {
            // Add best-effort USD estimate for positions closed without price data
            if null_price_pnl_sol_f64 != 0.0 {
                let estimated = Decimal::from_f64_retain(null_price_pnl_sol_f64)
                    .unwrap_or(Decimal::ZERO) * price;
                realized_usd += estimated;
            }
            let unrealized_usd = unrealized_sol * price;
            let total_pnl_usd = realized_usd + unrealized_usd;
            Some(total_pnl_usd)
        } else {
            tracing::warn!(
                "SOL price unavailable (stale cache) — cannot verify USD loss threshold. \
                 Extending cooldown until price data is available."
            );
            // Do not exit cooldown when we cannot verify the loss threshold — a stale
            // cache could mask an ongoing breach. The caller will retry on the next tick.
            return Ok(());
        };

        if let Some(total_pnl_usd) = usd_check_result {
            if total_pnl_usd < Decimal::ZERO && total_pnl_usd.abs() >= self.config.max_loss_24h_usd {
                // Loss still breaches threshold — re-trip rather than resume.
                let trip_reason = TripReason::MaxLoss24h {
                    loss: total_pnl_usd.abs().to_f64().unwrap_or(0.0),
                    threshold: self.config.max_loss_24h_usd.to_f64().unwrap_or(0.0),
                };
                // FIX [R-M2]: Log re-trip event before calling trip().
                tracing::warn!(
                    reason = ?trip_reason,
                    original_tripped_at = ?self.state.read().tripped_at,
                    "Circuit breaker re-tripped during cooldown exit — clock reset"
                );
                self.trip(trip_reason).await?;
                tracing::warn!("Circuit breaker cooldown expired but loss threshold still breached — re-tripped");
                return Ok(());
            }
        }

        let consecutive = db::get_consecutive_losses(&self.db).await?;
        if consecutive >= self.config.max_consecutive_losses {
            let trip_reason = TripReason::ConsecutiveLosses {
                count: consecutive,
                threshold: self.config.max_consecutive_losses,
            };
            // FIX [R-M2]: Log re-trip event before calling trip().
            tracing::warn!(
                reason = ?trip_reason,
                original_tripped_at = ?self.state.read().tripped_at,
                "Circuit breaker re-tripped during cooldown exit — clock reset"
            );
            self.trip(trip_reason).await?;
            tracing::warn!("Circuit breaker cooldown expired but consecutive losses still breached — re-tripped");
            return Ok(());
        }

        let drawdown = db::get_max_drawdown_percent(&self.db).await?;
        if drawdown >= self.config.max_drawdown_percent {
            let trip_reason = TripReason::MaxDrawdown {
                drawdown: drawdown.to_f64().unwrap_or(0.0),
                threshold: self.config.max_drawdown_percent.to_f64().unwrap_or(0.0),
            };
            // FIX [R-M2]: Log re-trip event before calling trip().
            tracing::warn!(
                reason = ?trip_reason,
                original_tripped_at = ?self.state.read().tripped_at,
                "Circuit breaker re-tripped during cooldown exit — clock reset"
            );
            self.trip(trip_reason).await?;
            tracing::warn!("Circuit breaker cooldown expired but drawdown still breached — re-tripped");
            return Ok(());
        }

        {
            let mut state = self.state.write();
            state.state = CircuitBreakerState::Active;
            state.tripped_at = None;
            state.trip_reason = None;
        }

        tracing::info!("Circuit breaker exiting cooldown - trading resumed");

        // FIX [R-C1]: Persist Active state so restarts see cleared state.
        if let Err(e) = persist_cb_state(&self.db, CircuitBreakerState::Active, None, None).await {
            tracing::error!(error = %e, "Failed to persist circuit breaker ACTIVE state to DB after cooldown exit");
        }

        db::log_config_change(
            &self.db,
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

        tracing::warn!(
            admin = %admin,
            previous_state = %previous_state,
            "Circuit breaker manually reset"
        );

        // FIX [R-C1]: Persist Active state so restarts don't re-trip unnecessarily.
        if let Err(e) = persist_cb_state(&self.db, CircuitBreakerState::Active, None, None).await {
            tracing::error!(error = %e, "Failed to persist circuit breaker ACTIVE state to DB after reset");
        }

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
