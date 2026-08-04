//! Trading engine for Chimera Operator
//!
//! Manages signal processing, priority queuing, and trade execution.

mod channel;
mod degradation;
pub mod decision_recorder;
pub mod dex_comparator;
pub mod dune_monitor;
pub mod executor;
mod execution_lock;
pub mod jito_searcher;
pub mod kelly_sizer;
pub mod market_regime;
pub mod mev_protection;
pub mod momentum_exit;
pub mod portfolio_heat;
pub mod position_sizer;
pub mod profit_targets;
pub mod recovery;
pub mod rejection_mute;
pub mod reconciliation;
mod rent_scavenger;
pub mod rpc_cache;
pub mod run_context;
pub mod selection;
pub mod shadow_fill;
pub mod shadow_trader;
pub mod signal_pipeline;
pub mod signal_quality;
pub mod slippage;
pub mod stop_loss;
pub mod tip_inlining;
pub mod tips;
pub mod transaction_builder;
pub mod v0_reconstruction;
pub mod volume_cache;
pub mod worker_pool;

pub use channel::*;
pub use degradation::*;
pub use decision_recorder::DecisionRecorder;
pub use dex_comparator::{DexComparator, RouteSelection};
pub use executor::*;
pub use execution_lock::{ExecutionLock, ExecutionLockConfig, LockGuard, LockInfo};
pub use kelly_sizer::{KellyResult, KellySizer};
pub use market_regime::{MarketRegime, MarketRegimeDetector};
pub use mev_protection::MevProtection;
pub use momentum_exit::{MomentumExit, MomentumExitAction};
pub use portfolio_heat::{HeatResult, PortfolioHeat};
pub use position_sizer::PositionSizer;
pub use profit_targets::{ProfitTargetAction, ProfitTargetManager};
pub use recovery::RecoveryManager;
pub use rent_scavenger::{RentScavenger, RentScavengerConfig};
pub use rpc_cache::{CacheStats, RpcCache};
pub use run_context::RunContext;
pub use selection::{BuyDecision, Ingress, SelectionConfig, SelectionRequest, SelectionService};
pub use shadow_fill::LatencyTracker;
pub use shadow_trader::{ShadowConfig, ShadowTrader};
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
use crate::state::{StateRegistry, AsyncWriteQueue};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// Engine handle for external interaction
#[derive(Clone)]
pub struct EngineHandle {
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
    /// State registry for in-memory trade/position tracking
    #[allow(dead_code)] // Used via SignalProcessor
    state_registry: Option<Arc<crate::state::StateRegistry>>,
    /// Async write queue for database operations
    #[allow(dead_code)] // Used via SignalProcessor
    write_queue: Option<Arc<crate::state::AsyncWriteQueue>>,
    /// Execution lock for preventing concurrent processing
    #[allow(dead_code)] // Used via SignalProcessor
    execution_lock: Option<Arc<ExecutionLock>>,
    /// Profitability verdict cache for live trading enforcement
    #[allow(dead_code)] // Used via SignalProcessor
    verdict_cache: Option<Arc<tokio::sync::RwLock<Option<crate::handlers::CachedVerdict>>>>,
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
        Self::new_with_extras_tip_manager_price_cache_and_token_parser(
            config,
            db,
            notifier,
            metrics,
            ws_state,
            tip_manager,
            price_cache,
            None, // token_parser
            None, // portfolio_heat
            None, // state_registry
            None, // write_queue
            None, // wallet_performance
            None, // toxic_detector
            None, // verdict_cache
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
        state_registry: Option<Arc<StateRegistry>>,
        write_queue: Option<Arc<AsyncWriteQueue>>,
        wallet_performance: Option<Arc<crate::monitoring::WalletPerformanceTracker>>,
        toxic_detector: Option<Arc<crate::experiment::ToxicFlowDetector>>,
        verdict_cache: Option<Arc<tokio::sync::RwLock<Option<crate::handlers::CachedVerdict>>>>,
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
            state_registry,
            write_queue,
            wallet_performance,
            toxic_detector,
            verdict_cache,
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
            config, db, notifier, metrics, ws_state, None, None, None, None, None, None, None, None, None,
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
        state_registry: Option<Arc<StateRegistry>>,
        write_queue: Option<Arc<AsyncWriteQueue>>,
        wallet_performance: Option<Arc<crate::monitoring::WalletPerformanceTracker>>,
        toxic_detector: Option<Arc<crate::experiment::ToxicFlowDetector>>,
        verdict_cache: Option<Arc<tokio::sync::RwLock<Option<crate::handlers::CachedVerdict>>>>,
    ) -> (Self, EngineHandle) {
        let config = Arc::new(config);
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

        if let Some(ref metrics) = metrics {
            executor = executor.with_metrics(metrics.clone());
        }

        let executor_arc = Arc::new(tokio::sync::RwLock::new(executor));
        let shutdown_token = CancellationToken::new();
        let handle = EngineHandle {
            queue: queue.clone(),
            executor: Some(executor_arc.clone()),
            shutdown_token: shutdown_token.clone(),
        };

        // Create execution lock if enabled in configuration
        let execution_lock_config = config.execution_lock.clone();
        let execution_lock = if execution_lock_config.enabled {
            let lock_metrics = metrics.as_ref().map(|m| {
                // Register the execution-lock collectors with the shared registry
                // so they are actually scraped at /metrics.
                Arc::new(crate::metrics::ExecutionLockMetrics::new(m.registry()))
            });
            Some(Arc::new(ExecutionLock::new(execution_lock_config, lock_metrics)))
        } else {
            None
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
            state_registry.clone(),
            write_queue.clone(),
        )
        .with_worker_id("sequential".to_string()); // Set worker ID for sequential processing

        // B3: Wire wallet performance tracker and toxic detector
        let signal_processor = if let Some(ref wp) = wallet_performance {
            signal_processor.with_wallet_performance(wp.clone())
        } else {
            signal_processor
        };
        let signal_processor = if let Some(ref td) = toxic_detector {
            signal_processor.with_toxic_detector(td.clone())
        } else {
            signal_processor
        };

        // Wire profitability verdict cache
        let signal_processor = if let Some(ref verdict_cache) = verdict_cache {
            signal_processor.with_profitability_verdict(verdict_cache.clone())
        } else {
            signal_processor
        };

        // Add execution lock to signal processor if enabled
        let signal_processor = if let Some(ref lock) = execution_lock {
            signal_processor.with_execution_lock(lock.clone())
        } else {
            signal_processor
        };

        let engine = Self {
            config,
            db,
            queue,
            executor: executor_arc,
            notifier,
            metrics,
            ws_state,
            token_parser,
            price_cache,
            portfolio_heat,
            signal_processor,
            shutdown_token: shutdown_token.clone(),
            state_registry: state_registry.clone(),
            write_queue: write_queue.clone(),
            execution_lock,
            verdict_cache,
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
        let metrics_shutdown = self.shutdown_token.clone();
        if let Some(metrics) = metrics_clone {
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    tokio::select! {
                        _ = metrics_shutdown.cancelled() => break,
                        _ = interval.tick() => {
                            let depths = queue_clone.depths();
                            metrics.queue_depth.set(depths.total as i64);
                        }
                    }
                }
            });
        }

        // Spawn execution lock cleanup task
        if let Some(ref execution_lock) = self.execution_lock {
            let lock_clone = execution_lock.clone();
            let cleanup_token = self.shutdown_token.clone();
            let cleanup_interval = std::time::Duration::from_secs(self.config.execution_lock.cleanup_interval_seconds);

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(cleanup_interval);
                loop {
                    tokio::select! {
                        _ = cleanup_token.cancelled() => {
                            tracing::info!("Shutting down execution lock cleanup task");
                            break;
                        }
                        _ = interval.tick() => {
                            let cleaned = lock_clone.cleanup_expired();
                            if cleaned > 0 {
                                tracing::debug!(cleaned = cleaned, "Execution lock cleanup completed");
                            }
                        }
                    }
                }
            });
        }

        // Spawn Jito health check task
        let executor_clone = self.executor.clone();
        let jito_health_token = self.shutdown_token.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = jito_health_token.cancelled() => {
                        tracing::info!("Shutting down Jito health check task");
                        break;
                    }
                    _ = interval.tick() => {
                        let executor = executor_clone.read().await;
                        match executor.check_jito_health().await {
                            Ok(_) => {
                                tracing::debug!("Jito health check completed");
                            }
                            Err(crate::engine::executor::ExecutorError::JitoDisabled) => {
                                tracing::debug!("Jito client not configured, skipping health check");
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Jito health check failed");
                            }
                        }
                    }
                }
            }
        });

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

        // Keep the engine task alive and log statistics periodically.
        // Cancellation is handled immediately via select! — waiting for the
        // 60s stats tick would delay shutdown by up to a minute.
        let mut stats_interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("Shutdown signal received, closing worker pool");
                    worker_pool.shutdown().await;
                    break;
                }
                _ = stats_interval.tick() => {
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
                }
            }
        }
    }

    /// Run engine in sequential processing mode (legacy implementation)
    async fn run_sequential(self) {
        tracing::info!("Engine running in sequential mode");

        // Spawn metrics update task
        let metrics_clone = self.metrics.clone();
        let queue_clone = self.queue.clone();
        let metrics_shutdown = self.shutdown_token.clone();
        if let Some(metrics) = metrics_clone {
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    tokio::select! {
                        _ = metrics_shutdown.cancelled() => break,
                        _ = interval.tick() => {
                            let depths = queue_clone.depths();
                            metrics.queue_depth.set(depths.total as i64);
                        }
                    }
                }
            });
        }

        // Spawn execution lock cleanup task
        if let Some(ref execution_lock) = self.execution_lock {
            let lock_clone = execution_lock.clone();
            let cleanup_token = self.shutdown_token.clone();
            let cleanup_interval = std::time::Duration::from_secs(self.config.execution_lock.cleanup_interval_seconds);

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(cleanup_interval);
                loop {
                    tokio::select! {
                        _ = cleanup_token.cancelled() => {
                            tracing::info!("Shutting down execution lock cleanup task");
                            break;
                        }
                        _ = interval.tick() => {
                            let cleaned = lock_clone.cleanup_expired();
                            if cleaned > 0 {
                                tracing::debug!(cleaned = cleaned, "Execution lock cleanup completed");
                            }
                        }
                    }
                }
            });
        }

        // [R-H2] Panic counter for circuit-breaker integration.
        // If processing panics 5+ times within 60 seconds, trip the circuit breaker.
        let panic_count = Arc::new(AtomicU32::new(0));
        let panic_window_start = Arc::new(parking_lot::Mutex::new(Instant::now()));

        loop {
            // Graceful shutdown: the sequential loop only pops from the queue,
            // so observe the token explicitly — cancellation must be able to
            // stop the engine even while the queue is idle.
            if self.shutdown_token.is_cancelled() {
                tracing::info!("Shutdown signal received, exiting sequential loop");
                break;
            }

            // Process signals from queue
            if let Some(signal) = self.queue.pop().await {
                // Real panic boundary: process_signal runs in a spawned task so
                // a panic inside it is caught here via the JoinHandle instead of
                // unwinding and killing the engine loop.
                let processor = self.signal_processor.clone();
                let handle = tokio::spawn(async move {
                    let mut signal = signal;
                    processor.process_signal(&mut signal).await;
                });

                let result = handle.await;
                match result {
                    Ok(_) => {
                        // Normal path: processing completed
                    }
                    Err(join_err) => {
                        // Panic (or cancellation) in the processing task
                        let msg = if join_err.is_panic() {
                            let payload = join_err.into_panic();
                            if let Some(s) = payload.downcast_ref::<&str>() {
                                format!("Engine task panic (str): {}", s)
                            } else if let Some(s) = payload.downcast_ref::<String>() {
                                format!("Engine task panic (String): {}", s)
                            } else {
                                "Engine task panic (unknown payload)".to_string()
                            }
                        } else {
                            "Engine task cancelled".to_string()
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
                            // Do not hold the executor read lock across the async
                            // DB write below — the guard is unused (the circuit
                            // breaker reference is not accessible from Engine).
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
}
