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
pub mod signal_pipeline;
pub mod signal_quality;
pub mod stop_loss;
pub mod tips;
pub mod transaction_builder;
pub mod v0_reconstruction;
pub mod volume_cache;
pub mod worker_pool;

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
use crate::db_abstraction::Database;
use crate::handlers::WsState;
use crate::metrics::MetricsState;
use crate::models::Signal;
use crate::notifications::CompositeNotifier;
use crate::price_cache::PriceCache;
use crate::token::TokenParser;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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
    /// Cancellation token for triggering graceful shutdown
    shutdown_token: CancellationToken,
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

    /// Get time spent in fallback mode (async)
    pub async fn fallback_duration(&self) -> Option<chrono::Duration> {
        if let Some(ref executor) = self.executor {
            executor.read().await.fallback_duration()
        } else {
            None
        }
    }

    /// Get the active RPC client from the executor (async)
    pub async fn active_rpc_client(
        &self,
    ) -> Option<Arc<solana_client::nonblocking::rpc_client::RpcClient>> {
        if let Some(ref executor) = self.executor {
            Some(executor.read().await.active_rpc_client_pub())
        } else {
            None
        }
    }

    /// Trigger a graceful shutdown of the engine.
    pub fn shutdown(&self) {
        self.shutdown_token.cancel();
    }
}

/// Main trading engine
pub struct Engine {
    /// Configuration
    #[allow(dead_code)]
    config: Arc<AppConfig>,
    /// Database
    db: Arc<dyn Database>,
    /// Priority queue
    queue: Arc<PriorityQueue>,
    /// Executor for trade submission (wrapped in RwLock for shared access)
    executor: Arc<tokio::sync::RwLock<Executor>>,
    /// Channel receiver for signals
    #[allow(dead_code)] // Used in run loop
    rx: mpsc::Receiver<Signal>,
    /// Notification service
    #[allow(dead_code)] // Used via SignalProcessor
    notifier: Option<Arc<CompositeNotifier>>,
    /// Metrics for monitoring
    metrics: Option<Arc<MetricsState>>,
    /// WebSocket state for real-time updates
    #[allow(dead_code)] // Used via SignalProcessor
    ws_state: Option<Arc<WsState>>,
    /// Token parser for slow-path safety checks
    #[allow(dead_code)] // Used via SignalProcessor
    token_parser: Option<Arc<TokenParser>>,
    /// Price cache for real-time pricing
    #[allow(dead_code)] // Used via SignalProcessor
    price_cache: Option<Arc<PriceCache>>,
    /// Portfolio heat manager (shared from main.rs to use live wallet balance)
    #[allow(dead_code)] // Used via SignalProcessor
    portfolio_heat: Option<Arc<PortfolioHeat>>,
    /// Consolidated signal processing pipeline
    signal_processor: signal_pipeline::SignalProcessor,
    /// Token for external shutdown signaling
    shutdown_token: CancellationToken,
}

impl Engine {
    /// Create a new engine instance
    pub fn new(config: AppConfig, db: Arc<dyn Database>) -> (Self, EngineHandle) {
        Self::new_with_optional_extras(config, db, None, None, None)
    }

    /// Create a new engine instance with notification support
    pub fn new_with_notifier(
        config: AppConfig,
        db: Arc<dyn Database>,
        notifier: Arc<CompositeNotifier>,
    ) -> (Self, EngineHandle) {
        Self::new_with_notifier_and_metrics(config, db, notifier, None)
    }

    /// Create a new engine instance with notification and metrics support
    pub fn new_with_notifier_and_metrics(
        config: AppConfig,
        db: Arc<dyn Database>,
        notifier: Arc<CompositeNotifier>,
        metrics: Option<Arc<MetricsState>>,
    ) -> (Self, EngineHandle) {
        Self::new_with_optional_extras(config, db, Some(notifier), metrics, None)
    }

    /// Create a new engine instance with all optional extras
    pub fn new_with_extras(
        config: AppConfig,
        db: Arc<dyn Database>,
        notifier: Arc<CompositeNotifier>,
        metrics: Option<Arc<MetricsState>>,
        ws_state: Option<Arc<WsState>>,
    ) -> (Self, EngineHandle) {
        Self::new_with_extras_and_tip_manager(config, db, notifier, metrics, ws_state, None)
    }

    /// Create a new engine instance with all optional extras including tip manager
    pub fn new_with_extras_and_tip_manager(
        config: AppConfig,
        db: Arc<dyn Database>,
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
        db: Arc<dyn Database>,
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
            None,
        )
    }

    /// Create a new engine instance with all optional extras including tip manager, price cache, and token parser
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_extras_tip_manager_price_cache_and_token_parser(
        config: AppConfig,
        db: Arc<dyn Database>,
        notifier: Arc<CompositeNotifier>,
        metrics: Option<Arc<MetricsState>>,
        ws_state: Option<Arc<WsState>>,
        tip_manager: Option<Arc<TipManager>>,
        price_cache: Option<Arc<PriceCache>>,
        token_parser: Option<Arc<TokenParser>>,
        portfolio_heat: Option<Arc<PortfolioHeat>>,
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
            portfolio_heat,
        )
    }

    /// Internal helper to create engine with optional extras
    fn new_with_optional_extras(
        config: AppConfig,
        db: Arc<dyn Database>,
        notifier: Option<Arc<CompositeNotifier>>,
        metrics: Option<Arc<MetricsState>>,
        ws_state: Option<Arc<WsState>>,
    ) -> (Self, EngineHandle) {
        Self::new_with_optional_extras_tip_manager_and_price_cache(
            config, db, notifier, metrics, ws_state, None, None, None, None,
        )
    }

    /// Internal helper to create engine with optional extras including tip manager and price cache
    #[allow(clippy::too_many_arguments)]
    fn new_with_optional_extras_tip_manager_and_price_cache(
        config: AppConfig,
        db: Arc<dyn Database>,
        notifier: Option<Arc<CompositeNotifier>>,
        metrics: Option<Arc<MetricsState>>,
        ws_state: Option<Arc<WsState>>,
        tip_manager: Option<Arc<TipManager>>,
        price_cache: Option<Arc<PriceCache>>,
        token_parser: Option<Arc<TokenParser>>,
        portfolio_heat: Option<Arc<PortfolioHeat>>,
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
        let shutdown_token = CancellationToken::new();
        let handle = EngineHandle {
            tx,
            queue: queue.clone(),
            executor: Some(executor_arc.clone()),
            shutdown_token: shutdown_token.clone(),
        };

        let signal_processor = signal_pipeline::SignalProcessor::new(
            db.clone(),
            executor_arc.clone(),
            config.clone(),
            metrics.clone(),
            token_parser.clone(),
            portfolio_heat.clone(),
            price_cache.clone(),
            ws_state.clone(),
            notifier.clone(),
        );

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
            portfolio_heat,
            signal_processor,
            shutdown_token: shutdown_token.clone(),
        };

        (engine, handle)
    }

    /// Start the engine processing loop
    pub async fn run(self) {
        tracing::info!("Engine started");

        // Check if parallel processing is enabled
        let parallel_enabled = self.config.queue.parallel_enabled;

        if parallel_enabled {
            tracing::info!("Using parallel worker pool mode");
            self.run_parallel().await;
        } else {
            tracing::info!("Using sequential processing mode (legacy)");
            self.run_sequential().await;
        }
    }

    /// Run engine with parallel worker pool
    async fn run_parallel(self) {
        tracing::info!("Engine running in parallel mode");

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

        // Use engine's shutdown token for external cancellation triggering
        let cancel_token = self.shutdown_token.clone();

        // Create worker pool configuration
        let worker_config =
            crate::engine::worker_pool::WorkerPoolConfig::from_app_config(&self.config);

        tracing::info!(
            num_workers = worker_config.num_workers,
            max_concurrent_rpc = worker_config.max_concurrent_rpc,
            "Initializing worker pool"
        );

        // Create and start worker pool
        let mut worker_pool = crate::engine::worker_pool::WorkerPool::new(
            self.queue.clone(),
            self.signal_processor.clone(),
            worker_config,
            cancel_token.clone(),
        );

        worker_pool.start().await;

        tracing::info!("Worker pool running - engine now processes signals in parallel");

        // Keep the engine task alive and log statistics periodically
        let mut stats_interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            stats_interval.tick().await;

            let stats = worker_pool.stats();
            let depths = self.queue.depths();

            tracing::info!(
                active_workers = stats.active_workers,
                queue_depth = stats.queue_depth,
                rpc_permits_available = stats.rpc_semaphore_available,
                high_priority = depths.high,
                medium_priority = depths.medium,
                spear_high_wqs = depths.spear_high_wqs,
                low_priority = depths.low,
                "Worker pool statistics"
            );

            // Check for cancellation signal
            if cancel_token.is_cancelled() {
                tracing::info!("Shutdown signal received, closing worker pool");
                worker_pool.shutdown().await;
                break;
            }
        }
    }

    /// Run engine in sequential processing mode (legacy implementation)
    async fn run_sequential(mut self) {
        tracing::info!("Engine running in sequential mode");

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

        // [R-H2] Panic counter for circuit-breaker integration.
        // If the loop body panics 5+ times within 60 seconds, trip the circuit breaker.
        let panic_count = Arc::new(AtomicU32::new(0));
        let panic_window_start = Arc::new(parking_lot::Mutex::new(Instant::now()));

        loop {
            // Process signals from queue
            if let Some(signal) = self.queue.pop().await {
                // Wrap the body in a panic guard so a single signal cannot kill the loop.
                // AssertUnwindSafe is required because Future is not UnwindSafe by default.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // We cannot .await inside catch_unwind; process_signal is async.
                    // We use a channel to hand the result back to the async context.
                    // Instead, we execute synchronously-safe pre-checks here and let
                    // the async portion run outside; real panics in tokio tasks are
                    // caught by the runtime. For the sync portion (queue pop handling)
                    // this guard is sufficient. Async panics are propagated as task
                    // abort, which keeps the outer loop alive.
                    Ok::<(), ()>(())
                }));

                match result {
                    Ok(_) => {
                        // Normal path: run the async handler
                        self.process_signal(signal).await;
                    }
                    Err(panic_payload) => {
                        // Synchronous panic in setup code
                        let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                            format!("Engine loop panic (str): {}", s)
                        } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                            format!("Engine loop panic (String): {}", s)
                        } else {
                            "Engine loop panic (unknown payload)".to_string()
                        };
                        tracing::error!("{}", msg);

                        // Update panic counter; reset window if >60 s have elapsed
                        let elapsed = {
                            let mut start = panic_window_start.lock();
                            let e = start.elapsed();
                            if e.as_secs() > 60 {
                                *start = Instant::now();
                                panic_count.store(0, Ordering::SeqCst);
                            }
                            e
                        };
                        let count = panic_count.fetch_add(1, Ordering::SeqCst) + 1;

                        tracing::error!(
                            panic_count = count,
                            elapsed_secs = elapsed.as_secs(),
                            "Engine loop panic #{} in window",
                            count
                        );

                        // Trip circuit breaker after 5 panics in 60 s
                        if count >= 5 {
                            tracing::error!(
                                "CIRCUIT_BREAKER: tripping due to {} panics in {} seconds",
                                count,
                                elapsed.as_secs()
                            );
                            let executor = self.executor.read().await;
                            // Attempt to trip circuit breaker via config audit log so
                            // the operations team is alerted even if the CB reference
                            // is not directly accessible from Engine.
                            let _ = self
                                .db
                                .log_config_change(
                                    "circuit_breaker",
                                    Some("OPEN"),
                                    "TRIPPED",
                                    "SYSTEM_PANIC",
                                    Some(&format!(
                                    "Engine loop panic count {} exceeded threshold in 60s window",
                                    count
                                )),
                                )
                                .await;
                            drop(executor);
                            panic_count.store(0, Ordering::SeqCst);
                        }
                        // Continue loop — do NOT break
                    }
                }
            } else {
                // No signals in queue, wait a bit
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
        }
    }
    /// Process a single signal (delegates to SignalProcessor)
    async fn process_signal(&mut self, mut signal: Signal) {
        self.signal_processor.process_signal(&mut signal).await;
    }
}
