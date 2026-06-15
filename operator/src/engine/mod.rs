//! Trading engine for Chimera Operator
//!
//! Manages signal processing, priority queuing, and trade execution.

mod channel;
mod degradation;
pub mod dex_comparator;
pub mod executor;
mod jito_searcher;
pub mod kelly_sizer;
pub mod market_regime;
pub mod mev_protection;
pub mod momentum_exit;
pub mod portfolio_heat;
pub mod position_sizer;
pub mod profit_targets;
pub mod recovery;
pub mod rpc_cache;
pub mod signal_quality;
pub mod stop_loss;
pub mod tips;
pub mod transaction_builder;
mod v0_reconstruction;
pub mod volume_cache;

pub use channel::*;
pub use degradation::*;
pub use dex_comparator::{DexComparator, DexComparisonResult};
pub use executor::*;
pub use kelly_sizer::{KellyResult, KellySizer};
pub use market_regime::{MarketRegime, MarketRegimeDetector};
pub use mev_protection::MevProtection;
pub use momentum_exit::{MomentumExit, MomentumExitAction};
pub use portfolio_heat::{HeatResult, PortfolioHeat};
pub use position_sizer::PositionSizer;
pub use profit_targets::{ProfitTargetAction, ProfitTargetManager};
pub use recovery::RecoveryManager;
pub use rpc_cache::{CacheStats, RpcCache};
pub use signal_quality::{QualityCategory, SignalFactors, SignalQuality};
pub use stop_loss::{StopLossAction, StopLossManager};
pub use tips::TipManager;
pub use volume_cache::VolumeCache;

use crate::config::AppConfig;
use crate::db::DbPool;
use crate::handlers::{TradeUpdateData, WsEvent, WsState};
use crate::metrics::MetricsState;
use crate::models::{Action, Signal, Strategy};
use crate::notifications::CompositeNotifier;
use crate::price_cache::PriceCache;
use crate::token::TokenParser;
use chrono::{Timelike, Utc};
use rust_decimal::prelude::*;
use sqlx;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Engine handle for external interaction
#[derive(Clone)]
pub struct EngineHandle {
    /// Sender for queueing signals
    #[allow(dead_code)] // Used for queueing signals
    tx: mpsc::Sender<Signal>,
    /// Priority queue for monitoring
    queue: Arc<PriorityQueue>,
    /// Executor for RPC state access
    executor: Option<Arc<tokio::sync::RwLock<crate::engine::executor::Executor>>>,
}

impl EngineHandle {
    /// Queue a signal for processing
    ///
    /// # Arguments
    /// * `signal` - Signal to queue
    /// * `wallet_wqs` - Optional wallet WQS score (used to route high-WQS SPEAR signals)
    pub async fn queue_signal(
        &self,
        signal: Signal,
        wallet_wqs: Option<f64>,
    ) -> Result<(), String> {
        self.queue.push(signal, wallet_wqs).await
    }

    /// Get current queue depth
    pub fn queue_depth(&self) -> usize {
        self.queue.len()
    }

    /// Get current RPC mode from executor (non-blocking)
    pub fn rpc_mode(&self) -> crate::engine::executor::RpcMode {
        if let Some(ref executor) = self.executor {
            // Use try_read to avoid blocking
            if let Ok(exec) = executor.try_read() {
                exec.rpc_mode()
            } else {
                // Default to Jito if lock is held
                crate::engine::executor::RpcMode::Jito
            }
        } else {
            // Default to Jito if executor not available
            crate::engine::executor::RpcMode::Jito
        }
    }

    /// Check if executor is in fallback mode (non-blocking)
    pub fn is_in_fallback(&self) -> bool {
        if let Some(ref executor) = self.executor {
            // Use try_read to avoid blocking
            if let Ok(exec) = executor.try_read() {
                exec.is_in_fallback()
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Get RPC health status from executor (async)
    pub async fn get_rpc_health(&self) -> Option<crate::engine::executor::RpcHealth> {
        if let Some(ref executor) = self.executor {
            executor.read().await.get_rpc_health().await
        } else {
            None
        }
    }

    /// Refresh RPC health status (async)
    pub async fn refresh_rpc_health(&self) {
        if let Some(ref executor) = self.executor {
            executor.read().await.refresh_rpc_health().await;
        }
    }
}

/// Main trading engine
pub struct Engine {
    /// Configuration
    #[allow(dead_code)] // Used for configuration access
    config: Arc<AppConfig>,
    /// Database pool
    db: DbPool,
    /// Priority queue
    queue: Arc<PriorityQueue>,
    /// Executor for trade submission (wrapped in RwLock for shared access)
    executor: Arc<tokio::sync::RwLock<Executor>>,
    /// Channel receiver for signals
    #[allow(dead_code)] // Used in run loop
    rx: mpsc::Receiver<Signal>,
    /// Notification service
    #[allow(dead_code)] // Used for notifications
    notifier: Option<Arc<CompositeNotifier>>,
    /// Metrics for monitoring
    metrics: Option<Arc<MetricsState>>,
    /// WebSocket state for real-time updates
    ws_state: Option<Arc<WsState>>,
    /// Token parser for slow-path safety checks
    token_parser: Option<Arc<TokenParser>>,
    /// Price cache for real-time pricing
    price_cache: Option<Arc<PriceCache>>,
}

impl Engine {
    /// Create a new engine instance
    pub fn new(config: AppConfig, db: DbPool) -> (Self, EngineHandle) {
        Self::new_with_optional_extras(config, db, None, None, None)
    }

    /// Create a new engine instance with notification support
    pub fn new_with_notifier(
        config: AppConfig,
        db: DbPool,
        notifier: Arc<CompositeNotifier>,
    ) -> (Self, EngineHandle) {
        Self::new_with_notifier_and_metrics(config, db, notifier, None)
    }

    /// Create a new engine instance with notification and metrics support
    pub fn new_with_notifier_and_metrics(
        config: AppConfig,
        db: DbPool,
        notifier: Arc<CompositeNotifier>,
        metrics: Option<Arc<MetricsState>>,
    ) -> (Self, EngineHandle) {
        Self::new_with_optional_extras(config, db, Some(notifier), metrics, None)
    }

    /// Create a new engine instance with all optional extras
    pub fn new_with_extras(
        config: AppConfig,
        db: DbPool,
        notifier: Arc<CompositeNotifier>,
        metrics: Option<Arc<MetricsState>>,
        ws_state: Option<Arc<WsState>>,
    ) -> (Self, EngineHandle) {
        Self::new_with_extras_and_tip_manager(config, db, notifier, metrics, ws_state, None)
    }

    /// Create a new engine instance with all optional extras including tip manager
    pub fn new_with_extras_and_tip_manager(
        config: AppConfig,
        db: DbPool,
        notifier: Arc<CompositeNotifier>,
        metrics: Option<Arc<MetricsState>>,
        ws_state: Option<Arc<WsState>>,
        tip_manager: Option<Arc<TipManager>>,
    ) -> (Self, EngineHandle) {
        Self::new_with_extras_tip_manager_and_price_cache(
            config,
            db,
            notifier,
            metrics,
            ws_state,
            tip_manager,
            None,
        )
    }

    /// Create a new engine instance with all optional extras including tip manager and price cache
    pub fn new_with_extras_tip_manager_and_price_cache(
        config: AppConfig,
        db: DbPool,
        notifier: Arc<CompositeNotifier>,
        metrics: Option<Arc<MetricsState>>,
        ws_state: Option<Arc<WsState>>,
        tip_manager: Option<Arc<TipManager>>,
        price_cache: Option<Arc<PriceCache>>,
    ) -> (Self, EngineHandle) {
        Self::new_with_optional_extras_tip_manager_and_price_cache(
            config,
            db,
            Some(notifier),
            metrics,
            ws_state,
            tip_manager,
            price_cache,
            None,
        )
    }

    /// Create a new engine instance with all optional extras including tip manager, price cache, and token parser
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_extras_tip_manager_price_cache_and_token_parser(
        config: AppConfig,
        db: DbPool,
        notifier: Arc<CompositeNotifier>,
        metrics: Option<Arc<MetricsState>>,
        ws_state: Option<Arc<WsState>>,
        tip_manager: Option<Arc<TipManager>>,
        price_cache: Option<Arc<PriceCache>>,
        token_parser: Option<Arc<TokenParser>>,
    ) -> (Self, EngineHandle) {
        Self::new_with_optional_extras_tip_manager_and_price_cache(
            config,
            db,
            Some(notifier),
            metrics,
            ws_state,
            tip_manager,
            price_cache,
            token_parser,
        )
    }

    /// Internal helper to create engine with optional extras
    fn new_with_optional_extras(
        config: AppConfig,
        db: DbPool,
        notifier: Option<Arc<CompositeNotifier>>,
        metrics: Option<Arc<MetricsState>>,
        ws_state: Option<Arc<WsState>>,
    ) -> (Self, EngineHandle) {
        Self::new_with_optional_extras_tip_manager_and_price_cache(
            config, db, notifier, metrics, ws_state, None, None, None,
        )
    }

    /// Internal helper to create engine with optional extras including tip manager and price cache
    #[allow(clippy::too_many_arguments)]
    fn new_with_optional_extras_tip_manager_and_price_cache(
        config: AppConfig,
        db: DbPool,
        notifier: Option<Arc<CompositeNotifier>>,
        metrics: Option<Arc<MetricsState>>,
        ws_state: Option<Arc<WsState>>,
        tip_manager: Option<Arc<TipManager>>,
        price_cache: Option<Arc<PriceCache>>,
        token_parser: Option<Arc<TokenParser>>,
    ) -> (Self, EngineHandle) {
        let config = Arc::new(config);
        let (tx, rx) = mpsc::channel(100); // Buffer for incoming signals

        let queue = Arc::new(PriorityQueue::new(
            config.queue.capacity,
            config.queue.load_shed_threshold_percent,
        ));

        let mut executor = Executor::new(config.clone(), db.clone());

        if let Some(ref notifier) = notifier {
            executor = executor.with_notifier(notifier.clone());
        }

        if let Some(ref tip_manager) = tip_manager {
            executor = executor.with_tip_manager(tip_manager.clone());
        }

        if let Some(ref price_cache) = price_cache {
            executor = executor.with_price_cache(price_cache.clone());
        }

        let executor_arc = Arc::new(tokio::sync::RwLock::new(executor));
        let handle = EngineHandle {
            tx,
            queue: queue.clone(),
            executor: Some(executor_arc.clone()),
        };

        let engine = Self {
            config,
            db,
            queue,
            executor: executor_arc,
            rx,
            notifier,
            metrics,
            ws_state,
            token_parser,
            price_cache,
        };

        (engine, handle)
    }

    /// Start the engine processing loop
    pub async fn run(mut self) {
        tracing::info!("Engine started");

        // Spawn metrics update task
        let metrics_clone = self.metrics.clone();
        let queue_clone = self.queue.clone();
        if let Some(metrics) = metrics_clone {
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    let depths = queue_clone.depths();
                    metrics.queue_depth.set(depths.total as i64);
                }
            });
        }

        loop {
            // Process signals from queue
            if let Some(signal) = self.queue.pop().await {
                self.process_signal(signal).await;
            } else {
                // No signals in queue, wait a bit
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
        }
    }

    /// Process a single signal
    async fn process_signal(&mut self, mut signal: Signal) {
        let trade_uuid = signal.trade_uuid.clone();
        let start_time = std::time::Instant::now();

        tracing::info!(
            trade_uuid = %trade_uuid,
            strategy = %signal.payload.strategy,
            token = %signal.payload.token,
            "Processing signal"
        );

        // Update status to EXECUTING
        if let Err(e) =
            crate::db::update_trade_status(&self.db, &trade_uuid, "EXECUTING", None, None).await
        {
            tracing::error!(error = %e, trade_uuid = %trade_uuid, "Failed to update status to EXECUTING");
            return;
        }

        // Slow-path token safety check (for BUY signals only, before execution)
        // EXIT signals don't need token validation, SELL signals already own the token
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

                                // Update trade status to DEAD_LETTER
                                if let Err(e) = crate::db::update_trade_status(
                                    &self.db,
                                    &trade_uuid,
                                    "DEAD_LETTER",
                                    None,
                                    Some(&reason),
                                )
                                .await
                                {
                                    tracing::error!(error = %e, "Failed to update trade status to DEAD_LETTER");
                                }

                                // Log to dead letter queue
                                let _ = crate::db::insert_dead_letter(
                                    &self.db,
                                    Some(&trade_uuid),
                                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                                    "TOKEN_SLOW_SAFETY_FAILED",
                                    Some(&reason),
                                    signal.source_ip.as_deref(),
                                )
                                .await;

                                // Broadcast update via WebSocket
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
                            // Fail closed: on slow-check error, reject the trade
                            let reason = format!("Slow-path token safety check failed: {}", e);
                            tracing::error!(
                                trade_uuid = %trade_uuid,
                                token = %token_address,
                                error = %e,
                                "Slow-path token check error, rejecting trade"
                            );

                            // Update trade status to DEAD_LETTER
                            if let Err(db_err) = crate::db::update_trade_status(
                                &self.db,
                                &trade_uuid,
                                "DEAD_LETTER",
                                None,
                                Some(&reason),
                            )
                            .await
                            {
                                tracing::error!(error = %db_err, "Failed to update trade status to DEAD_LETTER");
                            }

                            // Log to dead letter queue
                            let _ = crate::db::insert_dead_letter(
                                &self.db,
                                Some(&trade_uuid),
                                &serde_json::to_string(&signal.payload).unwrap_or_default(),
                                "TOKEN_SLOW_SAFETY_FAILED",
                                Some(&reason),
                                signal.source_ip.as_deref(),
                            )
                            .await;

                            // Broadcast update via WebSocket
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
                    // Missing token_address for BUY signal - reject
                    let reason = "Missing token_address for BUY signal".to_string();
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        "BUY signal missing token_address, rejecting"
                    );

                    // Update trade status to DEAD_LETTER
                    if let Err(e) = crate::db::update_trade_status(
                        &self.db,
                        &trade_uuid,
                        "DEAD_LETTER",
                        None,
                        Some(&reason),
                    )
                    .await
                    {
                        tracing::error!(error = %e, "Failed to update trade status to DEAD_LETTER");
                    }

                    // Log to dead letter queue
                    let _ = crate::db::insert_dead_letter(
                        &self.db,
                        Some(&trade_uuid),
                        &serde_json::to_string(&signal.payload).unwrap_or_default(),
                        "TOKEN_SLOW_SAFETY_FAILED",
                        Some(&reason),
                        signal.source_ip.as_deref(),
                    )
                    .await;

                    return;
                }
            }
        }

        // Re-check portfolio heat and strategy allocation before execution (for BUY signals only)
        if signal.payload.action == Action::Buy && signal.payload.strategy != Strategy::Exit {
            let portfolio_heat = PortfolioHeat::new(self.db.clone(), self.config.position_sizing.total_capital_sol);

            // 1. Portfolio Heat Check
            match portfolio_heat.can_open_position(signal.payload.amount_sol).await {
                Ok(false) => {
                    let reason = "Portfolio heat limit reached at execution time".to_string();
                    tracing::warn!(trade_uuid = %trade_uuid, "Signal rejected: portfolio heat limit reached");

                    // Reject trade and set status to DEAD_LETTER
                    let _ = crate::db::update_trade_status(&self.db, &trade_uuid, "DEAD_LETTER", None, Some(&reason)).await;
                    let _ = crate::db::insert_dead_letter(
                        &self.db,
                        Some(&trade_uuid),
                        &serde_json::to_string(&signal.payload).unwrap_or_default(),
                        "PORTFOLIO_HEAT_LIMIT",
                        Some(&reason),
                        signal.source_ip.as_deref(),
                    ).await;
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
            match portfolio_heat.can_open_strategy_position(
                signal.payload.strategy,
                signal.payload.amount_sol,
                self.config.strategy.shield_percent,
                self.config.strategy.spear_percent,
            ).await {
                Ok(false) => {
                    let reason = format!("Strategy allocation limit reached at execution time for {:?}", signal.payload.strategy);
                    tracing::warn!(trade_uuid = %trade_uuid, "Signal rejected: strategy allocation limit reached");

                    // Reject trade and set status to DEAD_LETTER
                    let _ = crate::db::update_trade_status(&self.db, &trade_uuid, "DEAD_LETTER", None, Some(&reason)).await;
                    let _ = crate::db::insert_dead_letter(
                        &self.db,
                        Some(&trade_uuid),
                        &serde_json::to_string(&signal.payload).unwrap_or_default(),
                        "STRATEGY_HEAT_LIMIT",
                        Some(&reason),
                        signal.source_ip.as_deref(),
                    ).await;
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

        // Duplicate-token guard: reject a second BUY for a token we already hold.
        // Two consensus signals arriving within the queue window both pass the heat check
        // before either is committed — doubling concentration in a single token.
        if signal.payload.action == Action::Buy && signal.payload.strategy != Strategy::Exit {
            if let Some(ref token_address) = signal.payload.token_address {
                let existing: i64 = match sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM positions WHERE token_address = ? AND state IN ('ACTIVE','EXITING')"
                )
                .bind(token_address)
                .fetch_one(&self.db)
                .await
                {
                    Ok(n) => n,
                    Err(e) => {
                        // Fail-closed: a DB error during the duplicate check could allow a
                        // duplicate position if we default to 0. Reject the signal instead.
                        let reason = format!("DB error during duplicate check — rejecting signal (fail-safe): {}", e);
                        tracing::error!(trade_uuid = %trade_uuid, error = %e, "DB error in duplicate position check — rejecting signal");
                        let _ = crate::db::update_trade_status(&self.db, &trade_uuid, "DEAD_LETTER", None, Some(&reason)).await;
                        let _ = crate::db::insert_dead_letter(
                            &self.db, Some(&trade_uuid),
                            &serde_json::to_string(&signal.payload).unwrap_or_default(),
                            "DB_ERROR_DUPLICATE_CHECK", Some(&reason), signal.source_ip.as_deref(),
                        ).await;
                        return;
                    }
                };

                if existing > 0 {
                    let reason = format!("Duplicate position rejected: already ACTIVE/EXITING in {}", token_address);
                    tracing::warn!(trade_uuid = %trade_uuid, token_address = %token_address, "Duplicate token position rejected");
                    let _ = crate::db::update_trade_status(&self.db, &trade_uuid, "DEAD_LETTER", None, Some(&reason)).await;
                    let _ = crate::db::insert_dead_letter(
                        &self.db, Some(&trade_uuid),
                        &serde_json::to_string(&signal.payload).unwrap_or_default(),
                        "DUPLICATE_TOKEN_POSITION", Some(&reason), signal.source_ip.as_deref(),
                    ).await;
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

        // Apply off-hours size reduction at execution time so signals queued just
        // before 01:00 UTC also get the multiplier when they actually execute.
        // The multiplier ramps linearly 01:00–02:00 (1.0 → base), holds flat 02:00–05:00,
        // then ramps back 05:00–06:00 (base → 1.0), avoiding the cliff effect of the old
        // binary step that applied full reduction at exactly 02:00.
        if signal.payload.action == Action::Buy {
            let now_time = Utc::now().time();
            let hour_utc = now_time.hour();
            let minute_utc = now_time.minute();
            let mins_since_midnight = (hour_utc * 60 + minute_utc) as i64;
            const RAMP_DOWN_START: i64 = 60;      // 01:00 UTC
            const FULL_REDUCTION_START: i64 = 120; // 02:00 UTC
            const FULL_REDUCTION_END: i64 = 300;   // 05:00 UTC
            const RAMP_UP_END: i64 = 360;           // 06:00 UTC
            let base_mult = self.config.position_sizing.off_hours_size_multiplier;
            let off_hours_mult = if mins_since_midnight < RAMP_DOWN_START
                || mins_since_midnight >= RAMP_UP_END
            {
                rust_decimal::Decimal::ONE
            } else if mins_since_midnight < FULL_REDUCTION_START {
                // linear ramp 1.0 → base_mult over 01:00–02:00
                let t = rust_decimal::Decimal::from(mins_since_midnight - RAMP_DOWN_START)
                    / rust_decimal::Decimal::from(60);
                rust_decimal::Decimal::ONE - t * (rust_decimal::Decimal::ONE - base_mult)
            } else if mins_since_midnight < FULL_REDUCTION_END {
                base_mult
            } else {
                // linear ramp base_mult → 1.0 over 05:00–06:00
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
                    "Off-hours window: reducing position size at execution time (gradual ramp)"
                );
                signal.payload.amount_sol = signal.payload.amount_sol * off_hours_mult;
            }
        }

        // Execute the trade
        let result = {
            let executor = self.executor.read().await;
            executor.execute(&signal).await
        };
        let latency_ms = start_time.elapsed().as_millis() as f64;

        // Update trade latency metric
        if let Some(ref metrics) = self.metrics {
            metrics.trade_latency.observe(latency_ms);
        }

        match result {
            Ok(tx_signature) => {
                tracing::info!(
                    trade_uuid = %trade_uuid,
                    tx_signature = %tx_signature,
                    "Trade executed successfully"
                );

                // 1 + 2a. For BUY signals, atomically mark trade ACTIVE and open position.
                // This prevents a dangling ACTIVE trade with no position row if open_position fails.
                if signal.payload.action == Action::Buy {
                    let fill_price_sol = {
                        let exec = self.executor.read().await;
                        exec.get_last_fill_price_sol_per_token()
                    };
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

                    // max_heat_sol = 20% of total capital — matched to PortfolioHeat::new default.
                    let max_heat_sol = self.config.position_sizing.total_capital_sol
                        * rust_decimal::Decimal::from_f64_retain(0.20)
                            .unwrap_or(rust_decimal::Decimal::ZERO);

                    match crate::db::activate_trade_and_open_position(
                        &self.db,
                        &trade_uuid,
                        &signal.payload.wallet_address,
                        signal.token_address(),
                        Some(&signal.payload.token),
                        &signal.payload.strategy.to_string(),
                        signal.payload.amount_sol,
                        entry_price,
                        &tx_signature,
                        Some(max_heat_sol),
                    )
                    .await
                    {
                        Ok(()) => {
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
                            tracing::error!(error = %e, "Failed to activate trade and open position — rolled back");
                        }
                    }
                } else if signal.payload.action == Action::Sell {
                    let fill_price_sol = {
                        let exec = self.executor.read().await;
                        exec.get_last_fill_price_sol_per_token()
                    };
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

                    // For SELL signals: mark ACTIVE first (state machine compliance), then EXITING.
                    if let Err(e) = crate::db::update_trade_status(
                        &self.db,
                        &trade_uuid,
                        "ACTIVE",
                        Some(&tx_signature),
                        None,
                    )
                    .await
                    {
                        tracing::error!(error = %e, "Failed to update sell trade status to ACTIVE");
                    } else if let Some(ref ws) = self.ws_state {
                        ws.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
                            trade_uuid: trade_uuid.clone(),
                            status: "ACTIVE".to_string(),
                            token_symbol: Some(signal.payload.token.clone()),
                            strategy: signal.payload.strategy.to_string(),
                        }));
                    }

                    // Transition to EXITING before closing so reconciliation can detect mid-close failures
                    if let Err(e) = crate::db::update_trade_status(
                        &self.db,
                        &trade_uuid,
                        "EXITING",
                        None,
                        None,
                    )
                    .await
                    {
                        tracing::error!(error = %e, "Failed to update trade status to EXITING");
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

                    // Close Position and write net PnL to trades table (full or partial exit)
                    if let Err(e) = crate::db::close_position(
                        &self.db,
                        signal.token_address(),
                        &signal.payload.wallet_address,
                        exit_price,
                        &tx_signature,
                        &trade_uuid,
                        sol_price_usd_opt,
                        exit_fraction,
                    )
                    .await
                    {
                        tracing::error!(error = %e, "Failed to close position");
                    }

                    // Transition to CLOSED after position is confirmed closed
                    if let Err(e) = crate::db::update_trade_status(
                        &self.db,
                        &trade_uuid,
                        "CLOSED",
                        Some(&tx_signature),
                        None,
                    )
                    .await
                    {
                        tracing::error!(error = %e, "Failed to update trade status to CLOSED");
                    }
                }
            }
            Err(crate::engine::executor::ExecutorError::MarketConditionsUnfavorable(ref reason)) => {
                if signal.payload.action == Action::Buy {
                    // BUY deferred due to market conditions: revert to PENDING for retry.
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        reason = %reason,
                        "BUY trade deferred — market conditions unfavorable, reverting to PENDING"
                    );
                    if let Err(db_err) = crate::db::update_trade_status(
                        &self.db,
                        &trade_uuid,
                        "PENDING",
                        None,
                        Some(reason),
                    )
                    .await
                    {
                        tracing::error!(error = %db_err, "Failed to revert trade status to PENDING");
                    }
                } else {
                    // EXIT/SELL deferred by market conditions — this is a critical failure because
                    // check_market_conditions should never block exits (see executor.rs). If we
                    // reach here something unexpected happened; fail visibly so it shows in DLQ.
                    tracing::error!(
                        trade_uuid = %trade_uuid,
                        reason = %reason,
                        action = %signal.payload.action,
                        "CRITICAL: EXIT signal deferred by market conditions — position may be stuck open"
                    );
                    if let Err(db_err) = crate::db::update_trade_status(
                        &self.db,
                        &trade_uuid,
                        "FAILED",
                        None,
                        Some(reason),
                    )
                    .await
                    {
                        tracing::error!(error = %db_err, "Failed to update exit trade status to FAILED");
                    }
                }
            }
            Err(crate::engine::executor::ExecutorError::ExecutionCostTooHigh { cost, cost_pct, limit_pct, strategy }) => {
                let reason = format!(
                    "Cost efficiency check failed: total cost {} SOL ({:.1}%) exceeds limit {:.1}% for strategy {:?}",
                    cost, cost_pct, limit_pct, strategy
                );
                tracing::warn!(trade_uuid = %trade_uuid, reason = %reason, "Trade rejected due to cost efficiency");

                // Update trade status to DEAD_LETTER
                let _ = crate::db::update_trade_status(&self.db, &trade_uuid, "DEAD_LETTER", None, Some(&reason)).await;

                // Log to dead letter queue
                let _ = crate::db::insert_dead_letter(
                    &self.db,
                    Some(&trade_uuid),
                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                    "EXECUTION_COST_TOO_HIGH",
                    Some(&reason),
                    signal.source_ip.as_deref(),
                )
                .await;

                // Broadcast update via WebSocket
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

                // Update status to FAILED
                if let Err(db_err) = crate::db::update_trade_status(
                    &self.db,
                    &trade_uuid,
                    "FAILED",
                    None,
                    Some(&e.to_string()),
                )
                .await
                {
                    tracing::error!(error = %db_err, "Failed to update trade status to FAILED");
                }
            }
        }
    }
}
