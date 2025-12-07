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

pub use channel::*;
pub use degradation::*;
pub use executor::*;
pub use recovery::RecoveryManager;
pub use tips::TipManager;

use crate::config::AppConfig;
use crate::db::DbPool;
use crate::handlers::{WsEvent, WsState, TradeUpdateData};
use crate::metrics::MetricsState;
use crate::models::Signal;
use crate::notifications::CompositeNotifier;
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
        Self::new_with_optional_extras_and_tip_manager(
            config, db, Some(notifier), metrics, ws_state, tip_manager,
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
        Self::new_with_optional_extras_and_tip_manager(
            config, db, notifier, metrics, ws_state, None,
        )
    }

    /// Internal helper to create engine with optional extras including tip manager
    fn new_with_optional_extras_and_tip_manager(
        config: AppConfig,
        db: DbPool,
        notifier: Option<Arc<CompositeNotifier>>,
        metrics: Option<Arc<MetricsState>>,
        ws_state: Option<Arc<WsState>>,
        tip_manager: Option<Arc<TipManager>>,
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

                // Update status to ACTIVE with signature
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
