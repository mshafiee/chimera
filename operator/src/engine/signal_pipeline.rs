//! Consolidated signal processing pipeline
//!
//! Single source of truth for all signal safety checks, trade execution,
//! and position management. Shared by both sequential (Engine) and
//! parallel (WorkerPool) processing paths.

use crate::config::AppConfig;
use crate::db_abstraction::{Database, DbPool};
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
    /// Per-token BUY admission locks (A2): serialize duplicate pre-check +
    /// execution + position open per token so two concurrent BUYs for the
    /// same token cannot both pass pre-checks and submit. Shared across all
    /// SignalProcessor clones (workers) via Arc.
    admission_locks: Arc<dashmap::DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Worker ID for lock attribution (set by worker pool or engine)
    worker_id: String,
    /// Wallet copy-performance tracker (B3): called on every confirmed SELL
    /// to update WQS and trigger auto-demotion.
    wallet_performance: Option<Arc<crate::monitoring::WalletPerformanceTracker>>,
    /// Toxic-flow detector (B3): called on every confirmed SELL to record
    /// ROI-dropping wallet behaviour.
    toxic_detector: Option<Arc<crate::experiment::ToxicFlowDetector>>,
    /// Profitability verdict cache for live trading enforcement
    profitability_verdict: Option<Arc<tokio::sync::RwLock<Option<crate::handlers::CachedVerdict>>>>,
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
            admission_locks: Arc::new(dashmap::DashMap::new()),
            worker_id: "sequential".to_string(), // Default worker ID
            wallet_performance: None,
            toxic_detector: None,
            profitability_verdict: None, // Set via with_profitability_verdict()
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

    /// Attach the wallet copy-performance tracker (B3).
    pub fn with_wallet_performance(
        mut self,
        tracker: Arc<crate::monitoring::WalletPerformanceTracker>,
    ) -> Self {
        self.wallet_performance = Some(tracker);
        self
    }

    /// Attach the profitability verdict cache for live trading enforcement.
    pub fn with_profitability_verdict(
        mut self,
        verdict_cache: Arc<tokio::sync::RwLock<Option<crate::handlers::CachedVerdict>>>,
    ) -> Self {
        self.profitability_verdict = Some(verdict_cache);
        self
    }

    /// Attach the toxic-flow detector (B3).
    pub fn with_toxic_detector(
        mut self,
        detector: Arc<crate::experiment::ToxicFlowDetector>,
    ) -> Self {
        self.toxic_detector = Some(detector);
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
            token = %signal.payload.token,
            token_address = %signal.token_address().unwrap_or(""),
            wallet = %signal.payload.wallet_address,
            strategy = %signal.payload.strategy,
            side = %signal.payload.action,
            amount_sol = %signal.payload.amount_sol,
            timestamp = signal.timestamp,
            source_ip = signal.source_ip.as_deref(),
            "signal_pipeline: processing signal"
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

        // Signal-based exit management: skip wallet SELL signals when copy_wallet_sells
        // is disabled. The position remains ACTIVE and is managed by profit
        // targets, stop-loss, momentum exit, and time exit via the
        // position_monitor tick loop. This transforms the system from
        // copy-trading (follow both BUY and SELL) to signal-trading (use
        // wallet BUYs as entry signals only).
        //
        // Distinguishing wallet SELLs from internal EXITs:
        // The selection service labels ALL admitted SELLs as Strategy::Exit
        // (selection.rs:510), so strategy alone can't tell them apart.
        // However, wallet-originated SELLs always have exit_fraction=None
        // (monitoring.rs:344), while internal EXITs from build_exit_signal
        // always have exit_fraction=Some(fraction) (main.rs:3038).
        if skip_wallet_sell_signal(
            signal.payload.action,
            self.config.strategy.copy_wallet_sells,
            signal.payload.exit_fraction,
        ) {
            tracing::info!(
                trade_uuid = %trade_uuid,
                wallet = %signal.payload.wallet_address,
                token = %signal.token_address().unwrap_or(""),
                "Wallet SELL signal skipped (copy_wallet_sells=false) — position managed by exit system"
            );
            // Terminal bookkeeping (2026-08-28): the queue path already
            // created this trade row QUEUED; leaving it for the stale-trade
            // sweeper made every whale-SELL skip churn a sweeper cancel plus
            // a DLQ retry cycle for a skip that is deterministic. Mark it
            // DEAD_LETTER with a terminal-classified reason instead.
            let skip_reason = "WHALE_SELL_SKIP: position managed by exit system (copy_wallet_sells=false) — deterministic skip";
            if let Err(e) = self
                .db
                .mark_trade_dead_letter(
                    &trade_uuid,
                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                    skip_reason,
                )
                .await
            {
                // Fail-open: the sweeper still collects the row; do not
                // turn a bookkeeping failure into a trading failure.
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    error = %e,
                    "Failed to mark skipped whale SELL dead — sweeper will collect it"
                );
            }
            return;
        }

        // Update status to EXECUTING
        // First, update in-memory registry for immediate effect
        if let Some(ref registry) = self.state_registry {
            if let Err(e) = registry.update_trade_status(&trade_uuid, TradeStatus::Executing) {
                tracing::debug!(error = ?e, trade_uuid = %trade_uuid,
                              "Trade not in in-memory registry (expected after restart), proceeding with DB update");
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
                                    token_symbol = %signal.payload.token,
                                    strategy = %signal.payload.strategy,
                                    wallet = %signal.payload.wallet_address,
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
            let base_mult = self.config.position_sizing.off_hours_size_multiplier;
            let off_hours_mult = off_hours_multiplier(mins_since_midnight, base_mult);
            if off_hours_mult < rust_decimal::Decimal::ONE {
                let original_amount_sol = signal.payload.amount_sol;
                signal.payload.amount_sol *= off_hours_mult;
                tracing::debug!(
                    trade_uuid = %trade_uuid,
                    hour_utc = hour_utc,
                    minute_utc = minute_utc,
                    multiplier = %off_hours_mult,
                    original_amount_sol = %original_amount_sol,
                    reduced_amount_sol = %signal.payload.amount_sol,
                    "signal_pipeline: off-hours size reduction applied"
                );
            }

            // Minimum-size enforcement at the pipeline (2026-08-18):
            //
            // Legacy (skip_below_min_size = false): hard floor re-clamp —
            // off-hours reduction (or any prior shrinkage) may only bring a
            // position DOWN to the floor, never below it.
            //
            // Skip mode (default): the floor does NOT rescue — a sub-minimum
            // size here is either an off-hours shrink crossing the minimum
            // or a sizer output that already rejected upstream. Rescuing it
            // up means paying the fixed ~0.0006 SOL tip load on an entry the
            // sizer considered sub-economic — the exact uneconomical trade
            // the minimum exists to prevent. Reject observably instead.
            let pre_floor_sol = signal.payload.amount_sol;
            let min_size_sol = self.config.position_sizing.min_size_sol;
            // Trial-lane exemption (2026-09-01, Fix A): the 0.25 SOL trial
            // cap IS the risk bound for trial admissions — the off-hours
            // floor would otherwise kill every night trial (measured
            // 2026-08-29: 4 dead-letters; ~50% of the day lost for the lane,
            // whose shadow verdict on that flow was +9.9 SOL/12h). The flag
            // is authoritative: only the selection trial gate sets it, and
            // that gate requires the trial config enabled + the size already
            // clamped to the trial cap.
            let trial_exempt = signal.payload.trial_admission;
            if self.config.position_sizing.skip_below_min_size && !trial_exempt {
                if signal.payload.amount_sol < min_size_sol {
                    let reason = format!(
                        "OFF_HOURS_BELOW_MIN: size {} SOL below minimum {} SOL after off-hours multiplier — skipping (cost-uneconomical)",
                        signal.payload.amount_sol, min_size_sol
                    );
                    tracing::info!(
                        trade_uuid = %trade_uuid,
                        pre_floor_sol = %pre_floor_sol,
                        min_size_sol = %min_size_sol,
                        off_hours_mult = %off_hours_mult,
                        "signal_pipeline: sub-minimum size rejected (skip-below-min semantics)"
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
            } else {
                signal.payload.amount_sol = signal.payload.amount_sol.max(min_size_sol);
                tracing::info!(
                    trade_uuid = %trade_uuid,
                    final_amount_sol = %signal.payload.amount_sol,
                    pre_floor_sol = %pre_floor_sol,
                    off_hours_mult = %off_hours_mult,
                    min_size_sol = %min_size_sol,
                    strategy = ?signal.payload.strategy,
                    "signal_pipeline: final position size after floor re-clamp"
                );
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
            //
            // Self-exclusion (2026-08-18): by this point the queued trade's
            // own row exists in BOTH the in-memory registry (queue_signal
            // inserted it) and the DB (write queue flushed it). The re-check
            // must therefore EXCLUDE this trade's own exposure — otherwise
            // `current + own` charges it twice and every entry larger than
            // half the cap self-blocks (observed: all four 0.75 SOL entries
            // on 2026-08-18 dead-lettered this way).
            let can_open = if let Some(ref registry) = self.state_registry {
                // Fast path: check in-memory portfolio heat, minus this trade
                let heat = registry.get_portfolio_heat();
                let own_exposure = registry
                    .get_trade(&trade_uuid)
                    .map(|t| t.amount_sol)
                    .unwrap_or(Decimal::ZERO);
                let new_exposure =
                    heat.total_exposure_sol - own_exposure + signal.payload.amount_sol;
                let capital = self.config.position_sizing.total_capital_sol;
                let max_heat = capital * self.config.position_sizing.portfolio_heat_percent;
                tracing::debug!(
                    trade_uuid = %trade_uuid,
                    exposure_sol = %heat.total_exposure_sol,
                    own_exposure_sol = %own_exposure,
                    requested_amount_sol = %signal.payload.amount_sol,
                    new_exposure_sol = %new_exposure,
                    cap_sol = %max_heat,
                    portfolio_total_capital = %capital,
                    can_open = new_exposure <= max_heat,
                    "signal_pipeline: portfolio heat re-check (in-memory, self-excluded)"
                );
                new_exposure <= max_heat
            } else {
                // Fallback: database query via PortfolioHeat
                match portfolio_heat
                    .can_open_position(signal.payload.amount_sol)
                    .await
                {
                    Ok(result) => {
                        tracing::debug!(
                            trade_uuid = %trade_uuid,
                            requested_amount_sol = %signal.payload.amount_sol,
                            can_open = result,
                            "signal_pipeline: portfolio heat re-check (db fallback)"
                        );
                        result
                    }
                    Err(e) => {
                        tracing::error!(error = %e, trade_uuid = %trade_uuid, "Portfolio heat check failed");
                        true // Allow trade on error (fail-open)
                    }
                }
            };

            if !can_open {
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
                let capital = self.config.position_sizing.total_capital_sol;
                let max_heat = capital * self.config.position_sizing.portfolio_heat_percent;
                let reason = format!(
                    "Portfolio heat limit reached: {} SOL + {} SOL > {} SOL max ({} of capital)",
                    heat.total_exposure_sol,
                    signal.payload.amount_sol,
                    max_heat,
                    self.config.position_sizing.portfolio_heat_percent
                );
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    token = %signal.payload.token,
                    exposure_sol = %heat.total_exposure_sol,
                    cap_sol = %max_heat,
                    portfolio_total_capital = %capital,
                    requested_amount_sol = %signal.payload.amount_sol,
                    "Signal rejected: {}",
                    reason
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

            // 2. Strategy Allocation Check (self-excluding: the queued
            // trade's own rows are already flushed — see note above)
            tracing::debug!(
                trade_uuid = %trade_uuid,
                strategy = ?signal.payload.strategy,
                requested_amount_sol = %signal.payload.amount_sol,
                shield_percent = self.config.strategy.shield_percent,
                spear_percent = self.config.strategy.spear_percent,
                "signal_pipeline: strategy allocation re-check (self-excluded)"
            );
            match portfolio_heat
                .can_open_strategy_position_excluding(
                    signal.payload.strategy,
                    signal.payload.amount_sol,
                    self.config.strategy.shield_percent,
                    self.config.strategy.spear_percent,
                    Some(&trade_uuid),
                )
                .await
            {
                Ok(false) => {
                    let reason = format!(
                        "Strategy allocation limit reached at execution time for {:?}",
                        signal.payload.strategy
                    );
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        strategy = ?signal.payload.strategy,
                        token = %signal.payload.token,
                        amount = %signal.payload.amount_sol,
                        wallet = %signal.payload.wallet_address,
                        "[SIGNAL_PIPELINE] Allocation check failed: {}",
                        reason
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
                Ok(true) => {
                    tracing::debug!(
                        trade_uuid = %trade_uuid,
                        strategy = ?signal.payload.strategy,
                        "signal_pipeline: strategy allocation check passed"
                    );
                }
                Err(e) => {
                    let reason = format!(
                        "Strategy allocation check failed — rejecting signal (fail-safe): {}",
                        e
                    );
                    tracing::error!(trade_uuid = %trade_uuid, error = %e, "Strategy allocation check failed");
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

        // Duplicate-token guard
        if signal.payload.action == Action::Buy && signal.payload.strategy != Strategy::Exit {
            let token_address = signal.token_address().unwrap_or("");
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
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        token = %signal.payload.token,
                        token_address = %token_address,
                        wallet = %signal.payload.wallet_address,
                        existing_positions = existing,
                        "Signal rejected: duplicate token ({} active position(s))",
                        existing
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
        }
        }

        // Per-token loss cooldown: skip re-entry if this token had a >3% loss
        // within the last 30 minutes (prevents chasing a dumping token).
        if signal.payload.action == Action::Buy {
            if let Some(ref token_addr) = signal.payload.token_address {
                match self.db.has_recent_token_loss(token_addr, 30).await {
                    Ok(true) => {
                        let reason = "Token recently lost >3% — 30min cooldown".to_string();
                        tracing::info!(
                            trade_uuid = %trade_uuid,
                            token = %signal.payload.token,
                            token_address = %token_addr,
                            "Signal rejected: per-token loss cooldown"
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
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "Cooldown check failed — proceeding");
                    }
                }
            }
        }

        // Token shadow blacklist: reject BUY signals for tokens whose shadow
        // performance under our own exits (mirror_main, any wallet, rolling
        // window) is consistently negative. Complements the cooldown with a
        // durable, data-backed ban — e.g. 6GmAFSYs4g averaged -13%/h over 40
        // shadow signals and kept getting re-entered.
        // 2026-08-06: the former `NOT LIKE '%pump'` exclusion was removed —
        // pump.fun tokens are the losing class (all 134 historical closed
        // trades) and were exempt from this filter.
        if signal.payload.action == Action::Buy {
            if let Some(ref token_addr) = signal.payload.token_address {
                let blacklist = &self.config.shadow_blacklist;
                if blacklist.enabled {
                    let banned: bool = {
                        let DbPool::PostgreSQL(pool) = self.db.pool();
                        sqlx::query_scalar(
                            // Dedup (2026-08-14): one exit per (wallet, hour)
                            // so repeat whale BUY signals on a token cannot
                            // multiply its sample count toward min_samples
                            // with the same round-trip PnL. no_price exits
                            // book zero PnL and are excluded.
                            r#"
                            SELECT EXISTS(
                                WITH dedup AS (
                                    SELECT DISTINCT ON (sp.wallet_address, date_trunc('hour', sp.opened_at))
                                           se.pnl_pct
                                    FROM shadow_exits se
                                    JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id
                                    WHERE sp.token_address = $1
                                      AND se.exit_strategy = 'mirror_main'
                                      AND se.exit_reason IS DISTINCT FROM 'no_price'
                                      AND sp.opened_at > NOW() - ($2 || ' hours')::interval
                                    ORDER BY sp.wallet_address, date_trunc('hour', sp.opened_at), sp.opened_at
                                )
                                SELECT 1
                                FROM dedup
                                HAVING COUNT(*) >= $3 AND AVG(pnl_pct) < $4 + $5
                            )
                            "#,
                        )
                        .bind(token_addr)
                        .bind(blacklist.window_hours)
                        .bind(blacklist.min_samples)
                        .bind(blacklist.threshold_pct)
                        .bind(blacklist.cost_adjustment_pct)
                        .fetch_one(&pool)
                        .await
                        .unwrap_or(false)
                    };
                    if banned {
                        let reason = format!(
                            "Token shadow blacklist: {} shadow exits avg < {:.1}% over {}h",
                            blacklist.min_samples,
                            blacklist.threshold_pct,
                            blacklist.window_hours
                        );
                        tracing::info!(
                            trade_uuid = %trade_uuid,
                            token = %signal.payload.token,
                            token_address = %token_addr,
                            "Signal rejected: token shadow blacklist"
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
                }
            }
        }

        // Ensure token_decimals is populated — required by executor's convert_fill_price().
        // The monitoring handler and polling task may not have set it.
        if signal.token_decimals.is_none() {
            if let Some(ref token_parser) = self.token_parser {
                if let Some(ref token_address) = signal.payload.token_address {
                    if let Some(decimals) = token_parser.get_token_decimals(token_address).await {
                        signal.token_decimals = Some(decimals);
                    }
                }
            }
        }

        // A2: serialized BUY admission per token. Two concurrent BUYs for the
        // same wallet/token must not both pass pre-checks and submit — the
        // atomic write-time check in activate_trade_and_open_position is the
        // backstop; this lock + pre-check prevents the wasted submission.
        let admission_lock: Option<Arc<tokio::sync::Mutex<()>>> =
            if signal.payload.action == Action::Buy {
                Some(
                    self.admission_locks
                        .entry(signal.token_address().unwrap_or("").to_string())
                        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                        .value()
                        .clone(),
                )
            } else {
                None
            };

        let _admission_guard = if let Some(ref lock) = admission_lock {
            let guard = lock.lock().await;

            // Pre-check both an existing ACTIVE/EXITING position AND any
            // unresolved trade (PENDING/QUEUED/EXECUTING/PENDING_CONFIRMATION).
            // An unconfirmed BUY never inserts a position row, so the
            // position-only check is blind to an in-flight first BUY for the
            // same wallet/token — without the trade check a second concurrent
            // BUY would pass and submit another on-chain order.
            match self
                .db
                .get_unresolved_trade_by_wallet_token(
                    &signal.payload.wallet_address,
                    signal.token_address().unwrap_or(""),
                )
                .await
            {
                // Self-match guard: the trade row is created by the webhook
                // handler (monitoring.rs) BEFORE this pre-check runs, so a
                // first-delivery BUY always finds its OWN just-created PENDING
                // row. That is the current trade, not a different concurrent
                // one — fall through to execution. Only a genuinely different
                // trade_uuid (different on-chain trade for the same
                // wallet/token) should be rejected. (Redeliveries of an
                // already-completed trade are caught by the monitoring-signal
                // dedup upstream; a duplicate ACTIVE position is caught by the
                // position-open guard.)
                Ok(Some(existing_uuid)) if existing_uuid == trade_uuid => {
                    tracing::debug!(
                        trade_uuid = %trade_uuid,
                        wallet = %signal.payload.wallet_address,
                        "pre-check: unresolved trade is the BUY's own in-flight row — proceeding"
                    );
                }
                Ok(Some(existing_uuid)) => {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        existing_trade_uuid = %existing_uuid,
                        wallet = %signal.payload.wallet_address,
                        token = %signal.payload.token,
                        token_address = %signal.token_address().unwrap_or(""),
                        "BUY rejected at pre-execution admission: unresolved trade already exists for wallet/token"
                    );
                    if let Err(e) = self
                        .db
                        .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                            trade_uuid: trade_uuid.clone(),
                            status: "REJECTED".to_string(),
                            tx_signature: None,
                            error_message: Some(
                                "Duplicate admission: unresolved trade already exists for wallet/token"
                                    .to_string(),
                            ),
                            network_fee_sol: None,
                        })
                        .await
                    {
                        tracing::error!(error = %e, "Failed to mark duplicate BUY as REJECTED");
                    }
                    return;
                }
                Ok(None) => {}
                Err(e) => {
                    // Fail-closed: an unresolved-trade lookup error must not
                    // let a duplicate BUY through.
                    tracing::warn!(
                        error = %e,
                        trade_uuid = %trade_uuid,
                        "Admission unresolved-trade pre-check failed; rejecting signal (fail-safe)"
                    );
                    let _ = self
                        .db
                        .mark_trade_dead_letter(
                            &trade_uuid,
                            &serde_json::to_string(&signal.payload).unwrap_or_default(),
                            &format!("Admission pre-check failed: {}", e),
                        )
                        .await;
                    return;
                }
            }

            match self
                .db
                .get_active_position_by_wallet_token(
                    &signal.payload.wallet_address,
                    signal.token_address().unwrap_or(""),
                )
                .await
            {
                Ok(Some(_)) => {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        wallet = %signal.payload.wallet_address,
                        token = %signal.payload.token,
                        token_address = %signal.token_address().unwrap_or(""),
                        "BUY rejected at pre-execution admission: active position already exists for wallet/token"
                    );
                    if let Err(e) = self
                        .db
                        .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                            trade_uuid: trade_uuid.clone(),
                            status: "REJECTED".to_string(),
                            tx_signature: None,
                            error_message: Some(
                                "Duplicate position detected at pre-execution admission"
                                    .to_string(),
                            ),
                            network_fee_sol: None,
                        })
                        .await
                    {
                        tracing::error!(error = %e, "Failed to mark duplicate BUY as REJECTED");
                    }
                    return;
                }
                Ok(None) => {
                    tracing::debug!(
                        trade_uuid = %trade_uuid,
                        wallet = %signal.payload.wallet_address,
                        token = %signal.payload.token,
                        "Admission pre-check passed: no active position or unresolved trade for wallet/token"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        trade_uuid = %trade_uuid,
                        "Admission duplicate pre-check failed; proceeding (atomic write-time check is backstop)"
                    );
                }
            }
            Some(AdmissionGuard::new(
                &self.admission_locks,
                signal.token_address().unwrap_or("").to_string(),
                guard,
            ))
        } else {
            None
        };

        // ── Profitability gate: LIVE fail-closed enforcement ──────────────
        // Live entry BUYs require a GO verdict (sample ≥ 60, 95%-CI net return
        // > 0, drawdown ≤ 20%, completeness ≥ 99% — see docs/profitability-gates.md).
        // Anything else dead-letters the trade. Paper/Devnet and exits always
        // proceed. See `profitability_gate_blocks` for the decision table.
        let verdict_str: String = match self.profitability_verdict.as_ref() {
            Some(cache) => cache
                .read()
                .await
                .as_ref()
                .map_or(String::new(), |c| c.verdict.clone()),
            None => String::new(), // no cache → cannot be GO → fail-closed
        };
        if let Some(reason) = profitability_gate_blocks(
            self.config.profitability_gate.enforce_on_live,
            self.config.trade_mode,
            signal.payload.action,
            signal.payload.strategy,
            &verdict_str,
        ) {
            let _ = self
                .db
                .mark_trade_dead_letter(
                    &signal.trade_uuid,
                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                    reason,
                )
                .await;
            if let Some(ref ws) = self.ws_state {
                ws.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
                    trade_uuid: signal.trade_uuid.clone(),
                    status: "DEAD_LETTER".to_string(),
                    token_symbol: Some(signal.payload.token.clone()),
                    strategy: signal.payload.strategy.to_string(),
                }));
            }
            tracing::warn!(
                trade_uuid = %signal.trade_uuid,
                verdict = %verdict_str,
                "Profitability gate: live entry BUY blocked (fail-closed)"
            );
            return;
        }

        // Execute the trade
        // ── Price-feed gate (BUY only) ─────────────────────────────────────
        // Tokens with no continuous price feed cannot be exit-managed: the
        // position monitor skips stop-loss/trailing/time exits when the price
        // is unavailable (observed: 8-hour bleeds on tokens missing from
        // Jupiter's price API). Reject here instead of entering blind.
        if signal.payload.action == Action::Buy {
            if let Some(ref pc) = self.price_cache {
                let token_addr = signal.token_address().unwrap_or_default();
                pc.track_token(token_addr);
                pc.eager_fetch_token(token_addr).await;
                if pc.get_price_usd(token_addr).is_none() {
                    let reason = "Token has no price feed — cannot monitor exits".to_string();
                    tracing::warn!(
                        trade_uuid = signal.trade_uuid,
                        token = signal.payload.token,
                        token_address = token_addr,
                        "Signal rejected: no price feed for token"
                    );
                    let _ = self
                        .db
                        .mark_trade_dead_letter(
                            &signal.trade_uuid,
                            &serde_json::to_string(&signal.payload).unwrap_or_default(),
                            &reason,
                        )
                        .await;
                    return;
                }
            }
        }

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
                    confirmed = outcome.confirmed,
                    fill_price_sol_per_token = ?outcome.fill_price_sol_per_token,
                    price_impact_pct = ?outcome.price_impact_pct,
                    token_amount = ?outcome.token_amount,
                    executed_output_sol = ?outcome.executed_output_sol,
                    estimated_fee_sol = ?outcome.estimated_fee_sol,
                    route_fee_sol = ?outcome.route_fee_sol,
                    "Trade executed successfully - checking position lifecycle"
                );

                // Handle BUY signals — activate trade and open position
                if signal.payload.action == Action::Buy {
                    tracing::info!(
                        trade_uuid = %trade_uuid,
                        is_paper_trade = is_paper_trade,
                        "BUY signal detected - opening position"
                    );

                    if !outcome.confirmed {
                        // A2: unconfirmed BUY — never open a position on an
                        // unresolved submission. Mark PENDING_CONFIRMATION;
                        // recovery reconciliation finalizes (opens) or fails
                        // the trade after re-checking the signature.
                        tracing::warn!(
                            trade_uuid = %trade_uuid,
                            tx_signature = %outcome.signature,
                            "BUY submitted but unconfirmed — deferring position open to recovery"
                        );
                        if let Err(e) = self
                            .db
                            .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                                trade_uuid: trade_uuid.clone(),
                                status: "PENDING_CONFIRMATION".to_string(),
                                tx_signature: Some(outcome.signature.clone()),
                                error_message: None,
                                network_fee_sol: outcome.estimated_fee_sol,
                            })
                            .await
                        {
                            tracing::error!(error = %e, "Failed to mark BUY as PENDING_CONFIRMATION");
                        }
                        return;
                    }

                    let fill_price_sol = outcome.fill_price_sol_per_token;
                    let sol_price_usd = self
                        .price_cache
                        .as_ref()
                        .and_then(|c| c.get_sol_price_usd_fallback())
                        .unwrap_or(Decimal::ZERO);

                    // A1 canonical unit: `entry_price` is ALWAYS USD per whole
                    // token. `fill_price_sol_per_token` is SOL per whole token —
                    // it may only be persisted as USD after multiplication by a
                    // valid SOL/USD price. Storing the raw SOL fill in the USD
                    // field corrupts every downstream PnL calculation.
                    let token_price_usd = self
                        .price_cache
                        .as_ref()
                        .and_then(|c| c.get_price_usd(signal.token_address().unwrap_or("")))
                        .unwrap_or(Decimal::ZERO);

                    let entry_price = if let Some(fps) = fill_price_sol {
                        if !fps.is_zero() && !sol_price_usd.is_zero() {
                            fps * sol_price_usd
                        } else {
                            token_price_usd
                        }
                    } else {
                        token_price_usd
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
                        * self.config.position_sizing.portfolio_heat_percent;

                    match self
                        .db
                        .atomic_portfolio_heat_check_and_open_position(
                            &trade_uuid,
                            &signal.payload.wallet_address,
                            signal.token_address().unwrap_or(""),
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
                                amount_sol = %signal.payload.amount_sol,
                                fill_price_sol_per_token = ?outcome.fill_price_sol_per_token,
                                tx_signature = %outcome.signature,
                                "Position opened successfully for BUY signal"
                            );

                            // Track the token in the price cache so the position
                            // monitor and PnL refresh tasks can fetch live prices.
                            // Without this, stop-loss/profit-target/time-exit checks
                            // silently no-op because get_price_usd returns None.
                            if let Some(ref pc) = self.price_cache {
                                pc.track_token(signal.token_address().unwrap_or(""));
                                // Eagerly prime a live price so the position monitor
                                // sees current data on its very next 5s tick. Without
                                // this the first price can arrive 15-60s late — by
                                // then pump tokens are already 5-8% below entry and
                                // stop-loss/momentum exits fire at a guaranteed loss.
                                let pc_fetch = pc.clone();
                                let token_fetch = signal.token_address().unwrap_or("").to_string();
                                tokio::spawn(async move {
                                    pc_fetch.eager_fetch_token(&token_fetch).await;
                                });
                            }

                            // Update in-memory registry with the new position
                            if let Some(ref registry) = self.state_registry {
                                let position_state = crate::state::registry::PositionState {
                                    trade_uuid: trade_uuid.clone(),
                                    wallet_address: signal.payload.wallet_address.clone(),
                                    token_address: signal.token_address().unwrap_or("").to_string(),
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
                                tracing::info!(
                                    trade_uuid = %trade_uuid,
                                    token_amount = token_amount,
                                    "Persisting token_amount to position"
                                );
                                if let Err(e) = self
                                    .db
                                    .update_position_token_amount(&trade_uuid, token_amount)
                                    .await
                                {
                                    tracing::warn!(error = %e, "Failed to set token_amount on position");
                                }
                            } else {
                                // Defense of last resort: a successful BUY
                                // without a usable token_amount can never be
                                // sold (paper SELL requires it). Force-close the
                                // just-opened position rather than leaving an
                                // unsellable slot that blocks
                                // max_concurrent_positions and spams EXIT
                                // failures every few seconds.
                                tracing::warn!(
                                    trade_uuid = %trade_uuid,
                                    token = %signal.token_address().unwrap_or(""),
                                    "outcome.token_amount is None after BUY — force-closing orphaned position to free slot"
                                );
                                if let Err(e) = self
                                    .db
                                    .force_close_orphan_position(
                                        &trade_uuid,
                                        "orphan_null_token_amount_buy",
                                    )
                                    .await
                                {
                                    tracing::error!(
                                        error = %e,
                                        trade_uuid = %trade_uuid,
                                        "Failed to force-close orphaned position"
                                    );
                                }
                                if let Some(ref registry) = self.state_registry {
                                    let _ =
                                        registry.update_position_state(&trade_uuid, "CLOSED");
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
                    // A1: use the last-known SOL price fallback so a stale
                    // primary entry does not force a zero exit price.
                    let sol_price_usd = self
                        .price_cache
                        .as_ref()
                        .and_then(|c| c.get_sol_price_usd_fallback())
                        .unwrap_or(Decimal::ZERO);

                    let exit_price = if let Some(fps) = fill_price_sol {
                        if !fps.is_zero() && !sol_price_usd.is_zero() {
                            fps * sol_price_usd
                        } else {
                            self.price_cache
                                .as_ref()
                                .and_then(|c| c.get_price_usd(signal.token_address().unwrap_or("")))
                                .unwrap_or(Decimal::ZERO)
                        }
                    } else {
                        self.price_cache
                            .as_ref()
                            .and_then(|c| c.get_price_usd(signal.token_address().unwrap_or("")))
                            .unwrap_or(Decimal::ZERO)
                    };

                    let sol_price_usd_opt = self
                        .price_cache
                        .as_ref()
                        .and_then(|c| c.get_sol_price_usd_fallback());

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

                    match self
                        .db
                        .close_position_full(
                            &trade_uuid,
                            &signal.payload.wallet_address,
                            signal.token_address().unwrap_or(""),
                            exit_price,
                            &outcome.signature,
                            sol_price_usd_opt,
                            exit_fraction,
                            outcome.confirmed,
                        )
                        .await
                    {
                        Ok(position_closed) => {
                            // A2: an unconfirmed SELL submission stays EXITING
                            // (position row also stays EXITING via confirmed=false)
                            // until recovery reconciles the signature. Only a
                            // confirmed close finalizes the trade as CLOSED.
                            let final_status = if !position_closed {
                                "REJECTED"
                            } else if !outcome.confirmed {
                                "EXITING"
                            } else {
                                "CLOSED"
                            };
                            let err_msg = if position_closed { None } else { Some("Skipped: no active position found to close".to_string()) };

                            tracing::info!(
                                trade_uuid = %trade_uuid,
                                exit_price = %exit_price,
                                exit_fraction = %exit_fraction,
                                position_closed = position_closed,
                                final_status = final_status,
                                tx_signature = %outcome.signature,
                                "SELL executed - position close resolved"
                            );

                            if let Err(e) = self
                                .db
                                .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                                    trade_uuid: trade_uuid.clone(),
                                    status: final_status.to_string(),
                                    tx_signature: Some(outcome.signature.clone()),
                                    error_message: err_msg,
                                    network_fee_sol: None,
                                })
                                .await
                            {
                                tracing::error!(error = %e, "Failed to update sell trade status");
                            }

                            // B3: Wire trade-close outcome to WalletPerformanceTracker
                            // and ToxicFlowDetector (only on confirmed CLOSED).
                            if final_status == "CLOSED" {
                                // B3b: Keep the in-memory registry consistent.
                                // The duplicate-token guard reads
                                // `token_position_counts`, and a position closed
                                // in the DB stayed ACTIVE in the registry —
                                // every re-BUY for the token was then
                                // DEAD_LETTERed as a duplicate (observed
                                // 2026-08-08: 9p84TE2Z… closed +12.9% at 01:43,
                                // re-entries blocked 02:39/03:23/03:33).
                                if let Some(ref registry) = self.state_registry {
                                    let _ = registry.update_position_state(&trade_uuid, "CLOSED");
                                }
                                // Untrack price polling (2026-08-23): a fully
                                // closed position must not keep its token in
                                // the 15s background price-poll set forever.
                                // Shadow streams on the same token still get
                                // prices via their per-tick eager fetches.
                                // (Known residual gap: recovery/reconciliation
                                // closes bypass this path and leave the token
                                // tracked until process restart.)
                                if let Some(ref pc) = self.price_cache {
                                    pc.untrack_token(signal.token_address().unwrap_or(""));
                                }
                                let wallet = &signal.payload.wallet_address;
                                // Query the trade for its net PnL.
                                let pnl_sol = self
                                    .db
                                    .get_trade_by_uuid(&trade_uuid)
                                    .await
                                    .ok()
                                    .flatten()
                                    .and_then(|t| t.net_pnl_sol)
                                    .unwrap_or(Decimal::ZERO);

                                tracing::info!(
                                    trade_uuid = %trade_uuid,
                                    wallet = %wallet,
                                    exit_price = %exit_price,
                                    pnl_sol = %pnl_sol,
                                    "Position closed - realized PnL recorded"
                                );

                                if let Some(ref wp) = self.wallet_performance {
                                    if let Err(e) =
                                        wp.record_trade_result(wallet, pnl_sol).await
                                    {
                                        tracing::warn!(
                                            wallet = %wallet,
                                            error = %e,
                                            "WalletPerformanceTracker: record_trade_result failed"
                                        );
                                    }
                                }

                                if let Some(ref td) = self.toxic_detector {
                                    // is_local_top is unknown without price-history
                                    // analysis; conservatively set false. The ROI-drop
                                    // detection (the primary toxic signal) still works.
                                    //
                                    // CRITICAL FIX (2026-08-05): current_roi must be a
                                    // ROI RATIO, consistent with selection_roi (the
                                    // Dune/on-chain promotion ROI). Previously pnl_sol
                                    // (absolute SOL PnL of ONE trade) was passed — e.g.
                                    // selection_roi 3.56 (356%) vs post_promotion_roi
                                    // -0.055 (SOL) → deterioration 3.6 > 0.3 threshold
                                    // → every promoted wallet flagged toxic after its
                                    // first losing trade. 6 wallets stuck toxic, the
                                    // only active trader blocked → zero trades.
                                    // Use the wallet's 30d ROI ratio; skip recording
                                    // when unavailable (never flag without data).
                                    let roi_ratio: Option<f64> = {
                                        use crate::db_abstraction::DbPool;
                                        match self.db.pool() {
                                            DbPool::PostgreSQL(pool) => {
                                                sqlx::query_scalar::<_, f64>(
                                                    "SELECT roi_30d FROM wallets WHERE address = $1",
                                                )
                                                .bind(wallet)
                                                .fetch_optional(&pool)
                                                .await
                                                .unwrap_or(None)
                                            }
                                        }
                                    };
                                    match roi_ratio {
                                        Some(roi) => {
                                            match td
                                                .record_entry(wallet.clone(), false, roi)
                                                .await
                                            {
                                                Ok(Some(reason)) => {
                                                    tracing::warn!(
                                                        wallet = %wallet,
                                                        roi_30d = roi,
                                                        ?reason,
                                                        "ToxicFlowDetector: wallet flagged as toxic"
                                                    );
                                                    // Persist immediately on detection
                                                    use crate::db_abstraction::DbPool;
                                                    let DbPool::PostgreSQL(pool) = self.db.pool();
                                                    let run_id = format!(
                                                        "v{}",
                                                        env!("CARGO_PKG_VERSION")
                                                    );
                                                    let _ = td
                                                        .persist_to_database(&pool, &run_id)
                                                        .await;
                                                }
                                                Err(e) => {
                                                    tracing::warn!(
                                                        wallet = %wallet,
                                                        error = %e,
                                                        "ToxicFlowDetector: record_entry failed"
                                                    );
                                                }
                                                _ => {}
                                            }
                                        }
                                        None => {
                                            tracing::debug!(
                                                wallet = %wallet,
                                                "ToxicFlowDetector: no roi_30d available — skipping entry record"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to close position");
                        }
                    }
                }
            }
            Err(ExecutorError::MarketConditionsUnfavorable(reason)) => {
                if signal.payload.action == Action::Buy {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        token = %signal.payload.token,
                        side = %signal.payload.action,
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
                    if let Some(ref registry) = self.state_registry {
                        let _ = registry.update_trade_status(&trade_uuid, TradeStatus::Pending);
                    }
                } else {
                    tracing::error!(
                        trade_uuid = %trade_uuid,
                        token = %signal.payload.token,
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
                    if let Some(ref registry) = self.state_registry {
                        let _ = registry.update_trade_status(&trade_uuid, TradeStatus::Failed);
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
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    token = %signal.payload.token,
                    side = %signal.payload.action,
                    reason = %reason,
                    "Trade rejected due to cost efficiency"
                );

                if signal.payload.action == Action::Sell {
                    // For a SELL the on-chain exit was never submitted —
                    // dead-lettering would abandon the exit and leave the
                    // position ACTIVE with no retry. Mark FAILED so the
                    // position monitor re-attempts the exit (same distinction
                    // as the MarketConditionsUnfavorable arm).
                    if let Err(db_err) = self
                        .db
                        .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                            trade_uuid: trade_uuid.clone(),
                            status: "FAILED".to_string(),
                            tx_signature: None,
                            error_message: Some(reason.clone()),
                            network_fee_sol: None,
                        })
                        .await
                    {
                        tracing::error!(error = %db_err, "Failed to mark exit trade as FAILED");
                    }
                    if let Some(ref registry) = self.state_registry {
                        let _ = registry.update_trade_status(&trade_uuid, TradeStatus::Failed);
                    }
                    return;
                }

                let _ = self
                    .db
                    .mark_trade_dead_letter(
                        &trade_uuid,
                        &serde_json::to_string(&signal.payload).unwrap_or_default(),
                        &reason,
                    )
                    .await;

                if let Some(ref registry) = self.state_registry {
                    let _ = registry.update_trade_status(&trade_uuid, TradeStatus::DeadLetter);
                }

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
                    token = %signal.payload.token,
                    side = %signal.payload.action,
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
                // Keep the in-memory registry in sync with the DB status.
                if let Some(ref registry) = self.state_registry {
                    let _ = registry.update_trade_status(&trade_uuid, TradeStatus::Failed);
                }
            }
        }
    }
}

/// Off-hours BUY size multiplier (01:00–06:00 UTC ramp down/up).
///
/// Pure function extracted from `process_signal` so the ramp arithmetic is
/// unit-testable at any wall-clock time; the caller computes
/// `mins_since_midnight` from `Utc::now()`.
/// Profitability gate decision table (pure, no I/O — unit-tested).
///
/// Returns `Some(reason)` when a signal must be dead-lettered by the
/// profitability gate, or `None` when it may proceed. The gate is **live
/// entry-BUY only**: Paper/Devnet and all exits (sells) always proceed, so
/// shadow evidence keeps accumulating and protective exits are never blocked.
///
/// Live entries require a `"GO"` verdict; anything else (no verdict yet,
/// INCONCLUSIVE, STOP, unknown) fails closed. Enforcement is gated by
/// `enforce`: when false, live entries are NOT blocked and behave identically
/// to paper/devnet (the "live == paper" policy). See
/// docs/profitability-gates.md.
pub fn profitability_gate_blocks(
    enforce: bool,
    trade_mode: crate::config::TradeMode,
    action: Action,
    strategy: Strategy,
    verdict: &str,
) -> Option<&'static str> {
    use crate::config::TradeMode;
    if !enforce || trade_mode != TradeMode::Live || action != Action::Buy || strategy == Strategy::Exit {
        return None;
    }
    match verdict {
        "GO" => None,
        "" => Some("Profitability verdict not computed — fail-closed (live) until edge is proven"),
        "STOP" => Some("Profitability verdict STOP: integrity/completeness failure"),
        "INCONCLUSIVE" => Some("Profitability verdict INCONCLUSIVE: edge not statistically proven (live)"),
        _ => Some("Profitability verdict unknown: fail-closed (live)"),
    }
}

/// Whether a wallet-originated SELL signal should be skipped.
///
/// A wallet SELL is identifiable by `exit_fraction == None`; internal EXITs
/// always carry a fraction. In signal-trading mode (`copy_wallet_sells = false`)
/// the wallet's own SELL is ignored and the position is managed by the internal
/// exit system (profit targets/stop-loss/time). In copy-trading mode
/// (`copy_wallet_sells = true`) the wallet SELL is followed and closes the
/// position — the exit that the shadow backtest showed nets +13.20 SOL vs
/// the exit system's −0.16 SOL on the same admitted positions. Internal EXITs
/// are never skipped.
fn skip_wallet_sell_signal(
    action: Action,
    copy_wallet_sells: bool,
    exit_fraction: Option<rust_decimal::Decimal>,
) -> bool {
    action == Action::Sell && !copy_wallet_sells && exit_fraction.is_none()
}

pub fn off_hours_multiplier(mins_since_midnight: i64, base_mult: Decimal) -> Decimal {
    const RAMP_DOWN_START: i64 = 60;
    const FULL_REDUCTION_START: i64 = 120;
    const FULL_REDUCTION_END: i64 = 300;
    const RAMP_UP_END: i64 = 360;
    if !(RAMP_DOWN_START..RAMP_UP_END).contains(&mins_since_midnight) {
        Decimal::ONE
    } else if mins_since_midnight < FULL_REDUCTION_START {
        let t = Decimal::from(mins_since_midnight - RAMP_DOWN_START) / Decimal::from(60);
        Decimal::ONE - t * (Decimal::ONE - base_mult)
    } else if mins_since_midnight < FULL_REDUCTION_END {
        base_mult
    } else {
        let t = Decimal::from(mins_since_midnight - FULL_REDUCTION_END) / Decimal::from(60);
        base_mult + t * (Decimal::ONE - base_mult)
    }
}

/// RAII guard for the per-token BUY admission lock.
///
/// Also bounds `admission_locks` growth: when the guard drops and no other
/// worker holds or waits on the token's mutex (the map entry is then the only
/// remaining reference besides the caller's), the entry is removed so memory
/// does not grow with the number of distinct tokens ever traded.
pub struct AdmissionGuard<'a> {
    locks: &'a dashmap::DashMap<String, Arc<tokio::sync::Mutex<()>>>,
    key: String,
    _guard: Option<tokio::sync::MutexGuard<'a, ()>>,
}

impl<'a> AdmissionGuard<'a> {
    pub fn new(
        locks: &'a dashmap::DashMap<String, Arc<tokio::sync::Mutex<()>>>,
        key: String,
        guard: tokio::sync::MutexGuard<'a, ()>,
    ) -> Self {
        Self {
            locks,
            key,
            _guard: Some(guard),
        }
    }
}

impl Drop for AdmissionGuard<'_> {
    fn drop(&mut self) {
        // Release the mutex before the reference-count check (try_lock while
        // holding would always fail).
        drop(self._guard.take());
        // Map entry (1) + the caller's Arc clone (1) = 2 references when
        // nobody else holds or waits on this mutex. A queued worker holds
        // another clone, so the entry is kept for it — it removes the entry
        // when it finishes.
        let entry = self.locks.get(&self.key);
        if let Some(entry) = entry {
            let keep = Arc::strong_count(entry.value()) > 2;
            drop(entry);
            if !keep {
                self.locks.remove(&self.key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    #[test]
    fn off_hours_multiplier_is_one_outside_ramp_window() {
        // Before 01:00 (min 60) and at/after 06:00 (min 360) → full size.
        assert_eq!(off_hours_multiplier(0, dec!(0.5)), Decimal::ONE);
        assert_eq!(off_hours_multiplier(59, dec!(0.5)), Decimal::ONE);
        assert_eq!(off_hours_multiplier(360, dec!(0.5)), Decimal::ONE);
        assert_eq!(off_hours_multiplier(1000, dec!(0.5)), Decimal::ONE);
    }

    #[test]
    fn off_hours_multiplier_ramps_down_01_02() {
        // mins in [60,120): linear ramp from 1.0 down to base_mult.
        assert_eq!(off_hours_multiplier(60, dec!(0.5)), Decimal::ONE);
        // t = (90-60)/60 = 0.5 → 1 - 0.5*0.5 = 0.75
        assert_eq!(off_hours_multiplier(90, dec!(0.5)), dec!(0.75));
        // just before full reduction: t = (119-60)/60 → 1 - (59/60)*0.5
        let expected = Decimal::ONE - dec!(59) / dec!(60) * dec!(0.5);
        assert_eq!(off_hours_multiplier(119, dec!(0.5)), expected);
    }

    #[test]
    fn off_hours_multiplier_full_reduction_02_05() {
        // mins in [120,300): fully reduced to base_mult.
        assert_eq!(off_hours_multiplier(120, dec!(0.5)), dec!(0.5));
        assert_eq!(off_hours_multiplier(200, dec!(0.5)), dec!(0.5));
        assert_eq!(off_hours_multiplier(299, dec!(0.5)), dec!(0.5));
    }

    #[test]
    fn off_hours_multiplier_ramps_up_05_06() {
        // mins in [300,360): linear ramp from base_mult back up to 1.0.
        assert_eq!(off_hours_multiplier(300, dec!(0.5)), dec!(0.5));
        // t = (330-300)/60 = 0.5 → 0.5 + 0.5*0.5 = 0.75
        assert_eq!(off_hours_multiplier(330, dec!(0.5)), dec!(0.75));
        // just before 06:00: t = (359-300)/60 = 59/60
        let expected = dec!(0.5) + dec!(59) / dec!(60) * dec!(0.5);
        assert_eq!(off_hours_multiplier(359, dec!(0.5)), expected);
    }

    #[test]
    fn off_hours_multiplier_base_mult_is_one_is_noop() {
        // A base multiplier of 1.0 means no reduction anywhere in the window.
        assert_eq!(off_hours_multiplier(90, Decimal::ONE), Decimal::ONE);
        assert_eq!(off_hours_multiplier(180, Decimal::ONE), Decimal::ONE);
        assert_eq!(off_hours_multiplier(330, Decimal::ONE), Decimal::ONE);
    }

    #[test]
    fn off_hours_multiplier_never_exceeds_one() {
        // The ramp formula interpolates within [base_mult, 1.0]; verify no
        // input produces a multiplier above 1.0 (which would upsell size).
        for mins in 0..=400 {
            let m = off_hours_multiplier(mins, dec!(0.4));
            assert!(m <= Decimal::ONE, "mins={mins} produced {m}");
            assert!(m >= dec!(0.4), "mins={mins} produced {m}");
        }
    }

    // ── wallet-sell follow/skip decision (copy_trader exit mode) ──

    #[test]
    fn wallet_sell_skipped_when_copy_disabled() {
        // signal-trading (copy_wallet_sells=false): a wallet SELL (no fraction)
        // is skipped — the position is managed by the internal exit system.
        assert!(skip_wallet_sell_signal(Action::Sell, false, None));
    }

    #[test]
    fn wallet_sell_followed_when_copy_enabled() {
        // copy-trading (copy_wallet_sells=true): the wallet SELL is followed
        // and closes the position — NOT skipped.
        assert!(!skip_wallet_sell_signal(Action::Sell, true, None));
    }

    #[test]
    fn internal_exit_never_skipped() {
        // Internal EXITs always carry a fraction and must never be skipped,
        // regardless of copy_wallet_sells.
        assert!(!skip_wallet_sell_signal(Action::Sell, false, Some(Decimal::ONE)));
        assert!(!skip_wallet_sell_signal(Action::Sell, true, Some(Decimal::ONE)));
    }

    #[test]
    fn non_sell_action_never_skipped() {
        assert!(!skip_wallet_sell_signal(Action::Buy, false, None));
    }
}
