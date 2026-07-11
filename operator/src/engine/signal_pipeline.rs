//! Consolidated signal processing pipeline
//!
//! Single source of truth for all signal safety checks, trade execution,
//! and position management. Shared by both sequential (Engine) and
//! parallel (WorkerPool) processing paths.

use crate::config::AppConfig;
use crate::db_abstraction::Database;
use crate::engine::executor::{Executor, ExecutorError};
use crate::engine::portfolio_heat::PortfolioHeat;
use crate::handlers::{TradeUpdateData, WsEvent, WsState};
use crate::metrics::MetricsState;
use crate::models::{Action, Signal, Strategy};
use crate::notifications::CompositeNotifier;
use crate::price_cache::PriceCache;
use crate::token::TokenParser;
    use crate::state::PortfolioHeatState;
use crate::state::registry::TradeStatus;
use chrono::{Timelike, Utc};
use rust_decimal::prelude::*;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Holds all dependencies needed to process a single signal through
/// the full pipeline: validation, execution, and position management.
#[derive(Clone)]
pub struct SignalProcessor {
    db: Arc<dyn Database>,
    executor: Arc<RwLock<Executor>>,
    config: Arc<AppConfig>,
    metrics: Option<Arc<MetricsState>>,
    token_parser: Option<Arc<TokenParser>>,
    portfolio_heat: Option<Arc<PortfolioHeat>>,
    price_cache: Option<Arc<PriceCache>>,
    ws_state: Option<Arc<WsState>>,
    #[allow(dead_code)] // Reserved for future notification wiring
    notifier: Option<Arc<CompositeNotifier>>,
    /// State registry for in-memory trade/position tracking
    #[allow(dead_code)] // Used when available
    state_registry: Option<Arc<crate::state::StateRegistry>>,
    /// Async write queue for non-blocking database operations
    #[allow(dead_code)] // Used when available
    write_queue: Option<Arc<crate::state::AsyncWriteQueue>>,
    /// Execution lock for preventing concurrent processing of same trade_uuid
    #[allow(dead_code)] // Used when available
    execution_lock: Option<Arc<crate::engine::ExecutionLock>>,
    /// Worker ID for lock attribution (set by worker pool or engine)
    worker_id: String,
}

impl SignalProcessor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<dyn Database>,
        executor: Arc<RwLock<Executor>>,
        config: Arc<AppConfig>,
        metrics: Option<Arc<MetricsState>>,
        token_parser: Option<Arc<TokenParser>>,
        portfolio_heat: Option<Arc<PortfolioHeat>>,
        price_cache: Option<Arc<PriceCache>>,
        ws_state: Option<Arc<WsState>>,
        notifier: Option<Arc<CompositeNotifier>>,
        state_registry: Option<Arc<crate::state::StateRegistry>>,
        write_queue: Option<Arc<crate::state::AsyncWriteQueue>>,
    ) -> Self {
        Self {
            db,
            executor,
            config,
            metrics,
            token_parser,
            portfolio_heat,
            price_cache,
            ws_state,
            notifier,
            state_registry,
            write_queue,
            execution_lock: None, // Set via with_execution_lock()
            worker_id: "sequential".to_string(), // Default worker ID
        }
    }

    /// Set the execution lock for this signal processor
    pub fn with_execution_lock(mut self, execution_lock: Arc<crate::engine::ExecutionLock>) -> Self {
        self.execution_lock = Some(execution_lock);
        self
    }

    /// Set the worker ID for this signal processor
    pub fn with_worker_id(mut self, worker_id: String) -> Self {
        self.worker_id = worker_id;
        self
    }

    /// Run the full signal processing pipeline.
    ///
    /// All signal processing converges here — this is the single path
    /// for token safety, off-hours sizing, portfolio heat, duplicate
    /// protection, execution, and position management.
    pub async fn process_signal(&self, signal: &mut Signal) {
        let trade_uuid = signal.trade_uuid.clone();
        let start_time = std::time::Instant::now();

        tracing::info!(
            trade_uuid = %trade_uuid,
            strategy = %signal.payload.strategy,
            token = %signal.payload.token,
            "Processing signal"
        );

        // ACQUIRE EXECUTION LOCK - must happen before any state changes
        // This prevents concurrent processing of the same trade_uuid
        let _lock_guard = if let Some(ref execution_lock) = self.execution_lock {
            match execution_lock.try_acquire(&trade_uuid, &self.worker_id) {
                Some(guard) => {
                    tracing::debug!(
                        trade_uuid = %trade_uuid,
                        worker_id = %self.worker_id,
                        "Execution lock acquired"
                    );
                    Some(guard)
                }
                None => {
                    tracing::debug!(
                        trade_uuid = %trade_uuid,
                        worker_id = %self.worker_id,
                        "Trade already being processed by another worker, skipping"
                    );
                    // Early exit - no processing occurs
                    return;
                }
            }
        } else {
            // No execution lock configured, proceed without locking
            tracing::trace!(
                trade_uuid = %trade_uuid,
                "No execution lock configured, proceeding without locking"
            );
            None
        };

        // Update status to EXECUTING
        // First, update in-memory registry for immediate effect
        if let Some(ref registry) = self.state_registry {
            if let Err(e) = registry.update_trade_status(&trade_uuid, TradeStatus::Executing) {
                tracing::error!(error = ?e, trade_uuid = %trade_uuid,
                              "Failed to update trade status in registry, continuing with DB update");
                // Continue anyway - DB is the source of truth
            }
        }

        // Queue async DB write for persistence
        if let Some(ref queue) = self.write_queue {
            if let Err(e) = queue.enqueue(crate::state::WriteOperation::UpdateTradeStatus {
                trade_uuid: trade_uuid.clone(),
                status: TradeStatus::Executing,
                tx_signature: None,
                error_message: None,
                network_fee_sol: None,
            }).await {
                tracing::error!(error = %e, trade_uuid = %trade_uuid, "Failed to queue EXECUTING status update");
            }
        } else {
            // Fallback to synchronous DB write
            if let Err(e) = self.db.update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                trade_uuid: trade_uuid.clone(),
                status: "EXECUTING".to_string(),
                tx_signature: None,
                error_message: None,
                network_fee_sol: None,
            }).await {
                tracing::error!(error = %e, trade_uuid = %trade_uuid, "Failed to update status to EXECUTING — marking FAILED to prevent phantom-QUEUED state");
                if let Err(e2) = self
                .db
                .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                    trade_uuid: trade_uuid.clone(),
                    status: "FAILED".to_string(),
                    tx_signature: None,
                    error_message: Some(
                        "DB error: failed to transition QUEUED->EXECUTING".to_string(),
                    ),
                    network_fee_sol: None,
                })
                .await
            {
                tracing::error!(error = %e2, trade_uuid = %trade_uuid, "Failed to mark trade FAILED after EXECUTING transition failed — trade is stuck in QUEUED");
            }
            return;
        }

        // Slow-path token safety check (for BUY signals only, before execution)
        if signal.payload.action == Action::Buy && signal.payload.strategy != Strategy::Exit {
            if let Some(ref token_parser) = self.token_parser {
                if let Some(ref token_address) = signal.payload.token_address {
                    match token_parser
                        .slow_check(token_address, signal.payload.strategy)
                        .await
                    {
                        Ok(result) => {
                            if !result.safe {
                                let reason = result.rejection_reason.unwrap_or_else(|| {
                                    "Token failed slow-path safety check".to_string()
                                });

                                tracing::warn!(
                                    trade_uuid = %trade_uuid,
                                    token = %token_address,
                                    reason = %reason,
                                    "Token rejected by slow-path safety check"
                                );

                                let _ = self
                                    .db
                                    .mark_trade_dead_letter(
                                        &trade_uuid,
                                        &serde_json::to_string(&signal.payload).unwrap_or_default(),
                                        &reason,
                                    )
                                    .await;

                                if let Some(ref ws) = self.ws_state {
                                    ws.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
                                        trade_uuid: trade_uuid.clone(),
                                        status: "DEAD_LETTER".to_string(),
                                        token_symbol: Some(signal.payload.token.clone()),
                                        strategy: signal.payload.strategy.to_string(),
                                    }));
                                }

                                return;
                            }
                        }
                        Err(e) => {
                            let reason = format!("Slow-path token safety check failed: {}", e);
                            tracing::error!(
                                trade_uuid = %trade_uuid,
                                token = %token_address,
                                error = %e,
                                "Slow-path token check error, rejecting trade"
                            );

                            let _ = self
                                .db
                                .mark_trade_dead_letter(
                                    &trade_uuid,
                                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                                    &reason,
                                )
                                .await;

                            if let Some(ref ws) = self.ws_state {
                                ws.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
                                    trade_uuid: trade_uuid.clone(),
                                    status: "DEAD_LETTER".to_string(),
                                    token_symbol: Some(signal.payload.token.clone()),
                                    strategy: signal.payload.strategy.to_string(),
                                }));
                            }

                            return;
                        }
                    }
                } else {
                    let reason = "Missing token_address for BUY signal".to_string();
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        "BUY signal missing token_address, rejecting"
                    );

                    let _ = self
                        .db
                        .mark_trade_dead_letter(
                            &trade_uuid,
                            &serde_json::to_string(&signal.payload).unwrap_or_default(),
                            &reason,
                        )
                        .await;

                    return;
                }
            } else if signal.force_slow_path {
                let reason = "Token parser unavailable; slow-path required by force_slow_path flag but cannot run — trade blocked".to_string();
                tracing::error!(
                    trade_uuid = %trade_uuid,
                    "force_slow_path is set but token_parser is None — rejecting trade to prevent unchecked token execution"
                );

                if let Err(e) = self
                    .db
                    .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                        trade_uuid: trade_uuid.clone(),
                        status: "DEAD_LETTER".to_string(),
                        tx_signature: None,
                        error_message: Some(reason.clone()),
                        network_fee_sol: None,
                    })
                    .await
                {
                    tracing::error!(error = %e, "Failed to update trade status to DEAD_LETTER");
                }

                let _ = self
                    .db
                    .insert_dlq(
                        Some(&trade_uuid),
                        &serde_json::to_string(&signal.payload).unwrap_or_default(),
                        "TOKEN_SLOW_SAFETY_UNAVAILABLE",
                        Some(&reason),
                        signal.source_ip.as_deref(),
                    )
                    .await;

                return;
            }
        }

        // Apply off-hours size reduction BEFORE heat/allocation checks
        if signal.payload.action == Action::Buy {
            let now_time = Utc::now().time();
            let hour_utc = now_time.hour();
            let minute_utc = now_time.minute();
            let mins_since_midnight = (hour_utc * 60 + minute_utc) as i64;
            const RAMP_DOWN_START: i64 = 60;
            const FULL_REDUCTION_START: i64 = 120;
            const FULL_REDUCTION_END: i64 = 300;
            const RAMP_UP_END: i64 = 360;
            let base_mult = self.config.position_sizing.off_hours_size_multiplier;
            let off_hours_mult = if !(RAMP_DOWN_START..RAMP_UP_END).contains(&mins_since_midnight) {
                rust_decimal::Decimal::ONE
            } else if mins_since_midnight < FULL_REDUCTION_START {
                let t = rust_decimal::Decimal::from(mins_since_midnight - RAMP_DOWN_START)
                    / rust_decimal::Decimal::from(60);
                rust_decimal::Decimal::ONE - t * (rust_decimal::Decimal::ONE - base_mult)
            } else if mins_since_midnight < FULL_REDUCTION_END {
                base_mult
            } else {
                let t = rust_decimal::Decimal::from(mins_since_midnight - FULL_REDUCTION_END)
                    / rust_decimal::Decimal::from(60);
                base_mult + t * (rust_decimal::Decimal::ONE - base_mult)
            };
            if off_hours_mult < rust_decimal::Decimal::ONE {
                tracing::info!(
                    trade_uuid = %trade_uuid,
                    hour_utc = hour_utc,
                    minute_utc = minute_utc,
                    multiplier = %off_hours_mult,
                    original_amount_sol = %signal.payload.amount_sol,
                    "Off-hours window: reducing position size before heat check (gradual ramp)"
                );
                signal.payload.amount_sol *= off_hours_mult;
            }
        }

        // Re-check portfolio heat and strategy allocation before execution (for BUY signals)
        if signal.payload.action == Action::Buy && signal.payload.strategy != Strategy::Exit {
            let portfolio_heat = if let Some(ref ph) = self.portfolio_heat {
                Arc::clone(ph)
            } else {
                Arc::new(PortfolioHeat::new(
                    self.db.clone(),
                    self.config.position_sizing.total_capital_sol,
                ))
            };

            // 1. Portfolio Heat Check
            let can_open = if let Some(ref registry) = self.state_registry {
                // Fast path: check in-memory portfolio heat
                let heat = registry.get_portfolio_heat();
                let new_exposure = heat.total_exposure_sol + signal.payload.amount_sol;
                let capital = self.config.position_sizing.total_capital_sol;
                let max_heat = capital * Decimal::from(20u32) / Decimal::from(100u32);
                new_exposure <= max_heat
            } else {
                // Fallback: database query via PortfolioHeat
                match portfolio_heat
                    .can_open_position(signal.payload.amount_sol)
                    .await
                {
                    Ok(result) => result,
                    Err(e) => {
                        tracing::error!(error = %e, trade_uuid = %trade_uuid, "Portfolio heat check failed");
                        true // Allow trade on error (fail-open)
                    }
                }
            };

            if !can_open {
                let reason = format!(
                    "Portfolio heat limit reached: {} SOL + {} SOL > {} SOL max (20% of capital)",
                    {
                        let heat = if let Some(ref registry) = self.state_registry {
                            registry.get_portfolio_heat()
                        } else {
                            // This shouldn't happen as we have portfolio_heat above, but handle gracefully
                            PortfolioHeatState {
                                total_exposure_sol: Decimal::ZERO,
                                shield_exposure_sol: Decimal::ZERO,
                                spear_exposure_sol: Decimal::ZERO,
                                pending_heat_sol: Decimal::ZERO,
                                last_updated: std::time::SystemTime::now(),
                            }
                        };
                        heat.total_exposure_sol
                    },
                    signal.payload.amount_sol,
                    {
                        let capital = self.config.position_sizing.total_capital_sol;
                        capital * Decimal::from(20u32) / Decimal::from(100u32)
                    }
                );
                tracing::warn!(trade_uuid = %trade_uuid, "Signal rejected: {}", reason);

                let _ = self
                    .db
                    .mark_trade_dead_letter(
                        &trade_uuid,
                        &serde_json::to_string(&signal.payload).unwrap_or_default(),
                        &reason,
                    )
                    .await;
                if let Some(ref ws) = self.ws_state {
                    ws.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
                        trade_uuid: trade_uuid.clone(),
                        status: "DEAD_LEAD_LETTER".to_string(),
                        token_symbol: Some(signal.payload.token.clone()),
                        strategy: signal.payload.strategy.to_string(),
                    }));
                }
                    return;
                }

            // 2. Strategy Allocation Check
            match portfolio_heat
                .can_open_strategy_position(
                    signal.payload.strategy,
                    signal.payload.amount_sol,
                    self.config.strategy.shield_percent,
                    self.config.strategy.spear_percent,
                )
                .await
            {
                Ok(false) => {
                    let reason = format!(
                        "Strategy allocation limit reached at execution time for {:?}",
                        signal.payload.strategy
                    );
                    tracing::warn!(trade_uuid = %trade_uuid, "Signal rejected: strategy allocation limit reached");

                    let _ = self
                        .db
                        .mark_trade_dead_letter(
                            &trade_uuid,
                            &serde_json::to_string(&signal.payload).unwrap_or_default(),
                            &reason,
                        )
                        .await;
                    if let Some(ref ws) = self.ws_state {
                        ws.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
                            trade_uuid: trade_uuid.clone(),
                            status: "DEAD_LETTER".to_string(),
                            token_symbol: Some(signal.payload.token.clone()),
                            strategy: signal.payload.strategy.to_string(),
                        }));
                    }
                    return;
                }
                Ok(true) => {}
                Err(e) => {
                    tracing::error!(trade_uuid = %trade_uuid, error = %e, "Strategy allocation check failed at execution time");
                }
            }

        // Duplicate-token guard
        if signal.payload.action == Action::Buy && signal.payload.strategy != Strategy::Exit {
            let token_address = signal.token_address();
            let existing: i64 = if let Some(ref registry) = self.state_registry {
                // Fast path: check in-memory registry
                registry.has_active_position_for_token(token_address) as i64
            } else {
                // Fallback: database query
                match self.db.get_active_positions().await {
                    Ok(positions) => positions
                        .iter()
                        .filter(|p| p.token_address == *token_address)
                        .count() as i64,
                    Err(e) => {
                        let reason = format!(
                                "DB error during duplicate check — rejecting signal (fail-safe): {}",
                                e
                            );
                            tracing::error!(trade_uuid = %trade_uuid, error = %e, "DB error in duplicate position check — rejecting signal");
                            let _ = self
                                .db
                                .mark_trade_dead_letter(
                                    &trade_uuid,
                                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                                    &reason,
                                )
                                .await;
                            return;
                        }
                    }
                };

                if existing > 0 {
                    let reason = format!(
                        "Duplicate token: {} already has {} active position(s)",
                        token_address, existing
                    );
                    tracing::warn!(trade_uuid = %trade_uuid, "Signal rejected: duplicate token");
                    let _ = self
                        .db
                        .mark_trade_dead_letter(
                            &trade_uuid,
                            &serde_json::to_string(&signal.payload).unwrap_or_default(),
                            &reason,
                        )
                        .await;
                    if let Some(ref ws) = self.ws_state {
                        ws.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
                            trade_uuid: trade_uuid.clone(),
                            status: "DEAD_LETTER".to_string(),
                            token_symbol: Some(signal.payload.token.clone()),
                            strategy: signal.payload.strategy.to_string(),
                        }));
                    }
                    return;
                }
            }
        }
        }

        // Execute the trade
        let result = {
            let executor = self.executor.read().await;
            executor.execute(signal).await
        };
        let latency_ms = start_time.elapsed().as_millis() as f64;

        if let Some(ref metrics) = self.metrics {
            metrics.trade_latency.observe(latency_ms);
        }

        match result {
            Ok(outcome) => {
                let is_paper_trade = outcome.signature.starts_with("simulated_");

                tracing::info!(
                    trade_uuid = %trade_uuid,
                    tx_signature = %outcome.signature,
                    is_paper_trade = is_paper_trade,
                    action = ?signal.payload.action,
                    "Trade executed successfully - checking position lifecycle"
                );

                // Handle BUY signals — activate trade and open position
                if signal.payload.action == Action::Buy {
                    tracing::info!(
                        trade_uuid = %trade_uuid,
                        is_paper_trade = is_paper_trade,
                        "BUY signal detected - opening position"
                    );
                    let fill_price_sol = outcome.fill_price_sol_per_token;
                    let sol_price_usd = self
                        .price_cache
                        .as_ref()
                        .and_then(|c| c.get_price_usd(crate::constants::mints::SOL))
                        .unwrap_or(Decimal::ZERO);

                    let entry_price = if let Some(fps) = fill_price_sol {
                        if !sol_price_usd.is_zero() {
                            fps * sol_price_usd
                        } else {
                            self.price_cache
                                .as_ref()
                                .and_then(|c| c.get_price_usd(signal.token_address()))
                                .unwrap_or(Decimal::ZERO)
                        }
                    } else {
                        self.price_cache
                            .as_ref()
                            .and_then(|c| c.get_price_usd(signal.token_address()))
                            .unwrap_or(Decimal::ZERO)
                    };

                    if entry_price.is_zero() {
                        tracing::warn!(
                            trade_uuid = %trade_uuid,
                            token = %signal.payload.token,
                            "BUY executed on-chain but entry price unavailable (entry_price=0); \
                             opening position with zero cost basis so stop-loss monitor will force-exit it"
                        );
                    }

                    let max_heat_sol = self.config.position_sizing.total_capital_sol
                        * rust_decimal::Decimal::from_f64_retain(0.20)
                            .unwrap_or(rust_decimal::Decimal::ZERO);

                    match self
                        .db
                        .atomic_portfolio_heat_check_and_open_position(
                            &trade_uuid,
                            &signal.payload.wallet_address,
                            signal.token_address(),
                            Some(&signal.payload.token),
                            &signal.payload.strategy.to_string(),
                            signal.payload.amount_sol,
                            entry_price,
                            &outcome.signature,
                            Some(max_heat_sol),
                            Some(sol_price_usd),
                        )
                        .await
                    {
                        Ok(()) => {
                            tracing::info!(
                                trade_uuid = %trade_uuid,
                                is_paper_trade = is_paper_trade,
                                entry_price = %entry_price,
                                "Position opened successfully for BUY signal"
                            );

                            // Update in-memory registry with the new position
                            if let Some(ref registry) = self.state_registry {
                                let position_state = crate::state::registry::PositionState {
                                    trade_uuid: trade_uuid.clone(),
                                    wallet_address: signal.payload.wallet_address.clone(),
                                    token_address: signal.token_address().to_string(),
                                    token_symbol: Some(signal.payload.token.clone()),
                                    state: "ACTIVE".to_string(),
                                    strategy: signal.payload.strategy.to_string(),
                                    entry_amount_sol: signal.payload.amount_sol,
                                    current_price: Some(sol_price_usd),
                                    unrealized_pnl_sol: None,
                                    updated_at: std::time::SystemTime::now(),
                                };
                                if let Err(e) = registry.insert_position(position_state) {
                                    tracing::warn!(error = ?e, trade_uuid = %trade_uuid,
                                                  "Failed to insert position into registry");
                                }
                            }

                            if let Some(token_amount) = outcome.token_amount {
                                if let Err(e) = self
                                    .db
                                    .update_position_token_amount(&trade_uuid, token_amount)
                                    .await
                                {
                                    tracing::warn!(error = %e, "Failed to set token_amount on position");
                                }
                            }

                            if let Some(ref ws) = self.ws_state {
                                ws.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
                                    trade_uuid: trade_uuid.clone(),
                                    status: "ACTIVE".to_string(),
                                    token_symbol: Some(signal.payload.token.clone()),
                                    strategy: signal.payload.strategy.to_string(),
                                }));
                            }
                        }
                        Err(e) => {
                            let reason =
                                format!("Position row insert failed after on-chain BUY: {}", e);
                            tracing::error!(error = %e, trade_uuid = %trade_uuid, "Failed to activate trade and open position — DEAD_LETTER-ing");
                            let _ = self
                                .db
                                .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                                    trade_uuid: trade_uuid.clone(),
                                    status: "DEAD_LETTER".to_string(),
                                    tx_signature: None,
                                    error_message: Some(reason.clone()),
                                    network_fee_sol: None,
                                })
                                .await;
                            let _ = self
                                .db
                                .insert_dlq(
                                    Some(&trade_uuid),
                                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                                    "POSITION_ROW_INSERT_FAILED",
                                    Some(&reason),
                                    signal.source_ip.as_deref(),
                                )
                                .await;
                        }
                    }
                } else if signal.payload.action == Action::Sell {
                    let is_paper_trade = outcome.signature.starts_with("simulated_");

                    tracing::info!(
                        trade_uuid = %trade_uuid,
                        is_paper_trade = is_paper_trade,
                        "SELL signal detected - closing position"
                    );

                    let fill_price_sol = outcome.fill_price_sol_per_token;
                    let sol_price_usd = self
                        .price_cache
                        .as_ref()
                        .and_then(|c| c.get_price_usd(crate::constants::mints::SOL))
                        .unwrap_or(Decimal::ZERO);

                    let exit_price = if let Some(fps) = fill_price_sol {
                        if !sol_price_usd.is_zero() {
                            fps * sol_price_usd
                        } else {
                            self.price_cache
                                .as_ref()
                                .and_then(|c| c.get_price_usd(signal.token_address()))
                                .unwrap_or(Decimal::ZERO)
                        }
                    } else {
                        self.price_cache
                            .as_ref()
                            .and_then(|c| c.get_price_usd(signal.token_address()))
                            .unwrap_or(Decimal::ZERO)
                    };

                    let sol_price_usd_opt = self
                        .price_cache
                        .as_ref()
                        .and_then(|c| c.get_price_usd(crate::constants::mints::SOL));

                    if let Err(e) = self
                        .db
                        .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                            trade_uuid: trade_uuid.clone(),
                            status: "EXITING".to_string(),
                            tx_signature: Some(outcome.signature.clone()),
                            error_message: None,
                            network_fee_sol: outcome.estimated_fee_sol,
                        })
                        .await
                    {
                        tracing::error!(error = %e, "Failed to update sell trade status to EXITING");
                    } else if let Some(ref ws) = self.ws_state {
                        ws.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
                            trade_uuid: trade_uuid.clone(),
                            status: "EXITING".to_string(),
                            token_symbol: Some(signal.payload.token.clone()),
                            strategy: signal.payload.strategy.to_string(),
                        }));
                    }

                    let exit_fraction = {
                        let raw = signal.payload.exit_fraction.unwrap_or(Decimal::ONE);
                        if raw <= Decimal::ZERO || raw > Decimal::ONE {
                            tracing::warn!(
                                trade_uuid = %trade_uuid,
                                exit_fraction = %raw,
                                "Invalid exit_fraction (must be in (0, 1]) — clamping to 1.0 (full exit)"
                            );
                            Decimal::ONE
                        } else {
                            raw
                        }
                    };

                    tracing::info!(
                        trade_uuid = %trade_uuid,
                        is_paper_trade = is_paper_trade,
                        exit_price = %exit_price,
                        exit_fraction = %exit_fraction,
                        "Calling close_position_full for SELL signal"
                    );

                    if let Err(e) = self
                        .db
                        .close_position_full(
                            &trade_uuid,
                            &signal.payload.wallet_address,
                            signal.token_address(),
                            exit_price,
                            &outcome.signature,
                            sol_price_usd_opt,
                            exit_fraction,
                            outcome.confirmed,
                        )
                        .await
                    {
                        tracing::error!(error = %e, "Failed to close position");
                    } else {
                        tracing::info!(
                            trade_uuid = %trade_uuid,
                            is_paper_trade = is_paper_trade,
                            "Position closed successfully for SELL signal"
                        );
                    }

                    if let Err(e) = self
                        .db
                        .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                            trade_uuid: trade_uuid.clone(),
                            status: "CLOSED".to_string(),
                            tx_signature: Some(outcome.signature.clone()),
                            error_message: None,
                            network_fee_sol: None,
                        })
                        .await
                    {
                        tracing::error!(error = %e, "Failed to update trade status to CLOSED");
                    }
                }
            }
            Err(ExecutorError::MarketConditionsUnfavorable(reason)) => {
                if signal.payload.action == Action::Buy {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        reason = %reason,
                        "BUY trade deferred — market conditions unfavorable, reverting to PENDING"
                    );
                    if let Err(db_err) = self
                        .db
                        .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                            trade_uuid: trade_uuid.clone(),
                            status: "PENDING".to_string(),
                            tx_signature: None,
                            error_message: Some(reason.to_string()),
                            network_fee_sol: None,
                        })
                        .await
                    {
                        tracing::error!(error = %db_err, "Failed to revert trade status to PENDING");
                    }
                } else {
                    tracing::error!(
                        trade_uuid = %trade_uuid,
                        reason = %reason,
                        action = %signal.payload.action,
                        "CRITICAL: EXIT signal deferred by market conditions — position may be stuck open"
                    );
                    if let Err(db_err) = self
                        .db
                        .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                            trade_uuid: trade_uuid.clone(),
                            status: "FAILED".to_string(),
                            tx_signature: None,
                            error_message: Some(reason.to_string()),
                            network_fee_sol: None,
                        })
                        .await
                    {
                        tracing::error!(error = %db_err, "Failed to update exit trade status to FAILED");
                    }
                }
            }
            Err(ExecutorError::ExecutionCostTooHigh {
                cost,
                cost_pct,
                limit_pct,
                strategy,
            }) => {
                let reason = format!(
                    "Cost efficiency check failed: total cost {} SOL ({:.1}%) exceeds limit {:.1}% for strategy {:?}",
                    cost, cost_pct, limit_pct, strategy
                );
                tracing::warn!(trade_uuid = %trade_uuid, reason = %reason, "Trade rejected due to cost efficiency");

                let _ = self
                    .db
                    .mark_trade_dead_letter(
                        &trade_uuid,
                        &serde_json::to_string(&signal.payload).unwrap_or_default(),
                        &reason,
                    )
                    .await;

                if let Some(ref ws) = self.ws_state {
                    ws.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
                        trade_uuid: trade_uuid.clone(),
                        status: "DEAD_LETTER".to_string(),
                        token_symbol: Some(signal.payload.token.clone()),
                        strategy: signal.payload.strategy.to_string(),
                    }));
                }
            }
            Err(e) => {
                tracing::error!(
                    trade_uuid = %trade_uuid,
                    error = %e,
                    "Trade execution failed"
                );

                if let Err(db_err) = self
                    .db
                    .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                        trade_uuid: trade_uuid.clone(),
                        status: "FAILED".to_string(),
                        tx_signature: None,
                        error_message: Some(e.to_string()),
                        network_fee_sol: None,
                    })
                    .await
                {
                    tracing::error!(error = %db_err, "Failed to update trade status to FAILED");
                }
            }
        }
    }
}
