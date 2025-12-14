//! Trading engine for Chimera Operator
//!
//! Manages signal processing, priority queuing, and trade execution.

mod channel;
mod degradation;
pub mod executor;
mod jito_searcher;
pub mod recovery;
pub mod tips;
mod transaction_builder;
pub mod profit_targets;
pub mod stop_loss;
pub mod mev_protection;
pub mod position_sizer;
pub mod signal_quality;
pub mod kelly_sizer;
pub mod momentum_exit;
pub mod dex_comparator;
pub mod market_regime;
pub mod portfolio_heat;
pub mod rpc_cache;
pub mod volume_cache;

pub use channel::*;
pub use degradation::*;
pub use executor::*;
pub use recovery::RecoveryManager;
pub use tips::TipManager;
pub use profit_targets::{ProfitTargetManager, ProfitTargetAction};
pub use stop_loss::{StopLossManager, StopLossAction};
pub use mev_protection::MevProtection;
pub use position_sizer::PositionSizer;
pub use signal_quality::{SignalQuality, SignalFactors, QualityCategory};
pub use kelly_sizer::{KellySizer, KellyResult};
pub use momentum_exit::{MomentumExit, MomentumExitAction};
pub use dex_comparator::{DexComparator, DexComparisonResult};
pub use market_regime::{MarketRegimeDetector, MarketRegime};
pub use portfolio_heat::{PortfolioHeat, HeatResult};
pub use rpc_cache::{RpcCache, CacheStats};
pub use volume_cache::VolumeCache;

use crate::config::AppConfig;
use crate::db::DbPool;
use crate::handlers::{WsEvent, WsState, TradeUpdateData};
use crate::metrics::MetricsState;
use crate::models::{Signal, Action, Strategy};
use crate::notifications::CompositeNotifier;
use crate::price_cache::PriceCache;
use crate::token::TokenParser;
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
    pub async fn queue_signal(&self, signal: Signal) -> Result<(), String> {
        self.queue.push(signal).await
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
            config, db, notifier, metrics, ws_state, tip_manager, None,
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
            config, db, Some(notifier), metrics, ws_state, tip_manager, price_cache, None,
        )
    }

    /// Create a new engine instance with all optional extras including tip manager, price cache, and token parser
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
            config, db, Some(notifier), metrics, ws_state, tip_manager, price_cache, token_parser,
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
    async fn process_signal(&mut self, signal: Signal) {
        let trade_uuid = signal.trade_uuid.clone();
        let start_time = std::time::Instant::now();

        tracing::info!(
            trade_uuid = %trade_uuid,
            strategy = %signal.payload.strategy,
            token = %signal.payload.token,
            "Processing signal"
        );

        // Update status to EXECUTING
        if let Err(e) = crate::db::update_trade_status(
            &self.db,
            &trade_uuid,
            "EXECUTING",
            None,
            None,
        )
        .await
        {
            tracing::error!(error = %e, trade_uuid = %trade_uuid, "Failed to update status to EXECUTING");
            return;
        }

        // Slow-path token safety check (for BUY signals only, before execution)
        // EXIT signals don't need token validation, SELL signals already own the token
        if signal.payload.action == Action::Buy && signal.payload.strategy != Strategy::Exit {
            if let Some(ref token_parser) = self.token_parser {
                if let Some(ref token_address) = signal.payload.token_address {
                    match token_parser.slow_check(token_address, signal.payload.strategy).await {
                        Ok(result) => {
                            if !result.safe {
                                let reason = result
                                    .rejection_reason
                                    .unwrap_or_else(|| "Token failed slow-path safety check".to_string());

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

        // Execute the trade
        let result = {
            let mut executor = self.executor.write().await;
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

                // 1. Update Trade Status to ACTIVE (Confirmed on-chain)
                if let Err(e) = crate::db::update_trade_status(
                    &self.db,
                    &trade_uuid,
                    "ACTIVE",
                    Some(&tx_signature),
                    None,
                )
                .await
                {
                    tracing::error!(error = %e, "Failed to update trade status to ACTIVE");
                } else {
                    // Broadcast trade update via WebSocket
                    if let Some(ref ws) = self.ws_state {
                        ws.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
                            trade_uuid: trade_uuid.clone(),
                            status: "ACTIVE".to_string(),
                            token_symbol: Some(signal.payload.token.clone()),
                            strategy: signal.payload.strategy.to_string(),
                        }));
                    }
                }

                // 2. Manage Position Lifecycle
                if signal.payload.action == Action::Buy {
                    // Calculate entry price (from cache or default to 0.0)
                    let entry_price = self.price_cache.as_ref()
                        .and_then(|c| c.get_price_usd(signal.token_address()))
                        .unwrap_or(0.0);

                    // Open Position
                    if let Err(e) = crate::db::open_position(
                        &self.db,
                        &trade_uuid,
                        &signal.payload.wallet_address,
                        signal.token_address(),
                        Some(&signal.payload.token),
                        &signal.payload.strategy.to_string(),
                        signal.payload.amount_sol,
                        entry_price,
                        &tx_signature
                    ).await {
                         tracing::error!(error = %e, "Failed to open position");
                    }
                } else if signal.payload.action == Action::Sell {
                    let exit_price = self.price_cache.as_ref()
                        .and_then(|c| c.get_price_usd(signal.token_address()))
                        .unwrap_or(0.0);

                    // Close Position
                    if let Err(e) = crate::db::close_position(
                        &self.db,
                        signal.token_address(),
                        &signal.payload.wallet_address,
                        exit_price,
                        &tx_signature
                    ).await {
                         tracing::error!(error = %e, "Failed to close position");
                    }
                    
                    // Update trade status to CLOSED
                    if let Err(e) = crate::db::update_trade_status(
                        &self.db, &trade_uuid, "CLOSED", Some(&tx_signature), None
                    ).await {
                        tracing::error!(error = %e, "Failed to update trade status to CLOSED");
                    }
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
