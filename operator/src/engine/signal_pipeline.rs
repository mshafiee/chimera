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
        }
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

        // Update status to EXECUTING
        if let Err(e) = self
            .db
            .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                trade_uuid: trade_uuid.clone(),
                status: "EXECUTING".to_string(),
                tx_signature: None,
                error_message: None,
            })
            .await
        {
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
            match portfolio_heat
                .can_open_position(signal.payload.amount_sol)
                .await
            {
                Ok(false) => {
                    let reason = "Portfolio heat limit reached at execution time".to_string();
                    tracing::warn!(trade_uuid = %trade_uuid, "Signal rejected: portfolio heat limit reached");

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
                    tracing::error!(trade_uuid = %trade_uuid, error = %e, "Portfolio heat check failed at execution time");
                }
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
        }

        // Duplicate-token guard
        if signal.payload.action == Action::Buy && signal.payload.strategy != Strategy::Exit {
            if let Some(ref token_address) = signal.payload.token_address {
                let existing: i64 = match self.db.get_active_positions().await {
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
                };

                if existing > 0 {
                    let reason = format!(
                        "Duplicate position rejected: already ACTIVE/EXITING in {}",
                        token_address
                    );
                    tracing::warn!(trade_uuid = %trade_uuid, token_address = %token_address, "Duplicate token position rejected");
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
                tracing::info!(
                    trade_uuid = %trade_uuid,
                    tx_signature = %outcome.signature,
                    "Trade executed successfully"
                );

                // Handle BUY signals — activate trade and open position
                if signal.payload.action == Action::Buy {
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
                    }

                    if let Err(e) = self
                        .db
                        .update_trade_status(&crate::db_abstraction::UpdateTradeStatus {
                            trade_uuid: trade_uuid.clone(),
                            status: "CLOSED".to_string(),
                            tx_signature: Some(outcome.signature.clone()),
                            error_message: None,
                        })
                        .await
                    {
                        tracing::error!(error = %e, "Failed to update trade status to CLOSED");
                    }
                }
            }
            Err(ExecutorError::MarketConditionsUnfavorable(ref reason)) => {
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
                    })
                    .await
                {
                    tracing::error!(error = %db_err, "Failed to update trade status to FAILED");
                }
            }
        }
    }
}
