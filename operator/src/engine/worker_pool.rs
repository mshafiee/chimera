//! Worker pool for parallel signal processing
//!
//! Spawns multiple worker tasks that process signals concurrently
//! while preserving priority ordering and respecting rate limits.

use crate::config::AppConfig;
use crate::engine::signal_pipeline::SignalProcessor;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{debug, error, info};

/// Worker pool configuration
#[derive(Debug, Clone)]
pub struct WorkerPoolConfig {
    /// Number of worker tasks (should match DB connection pool size)
    pub num_workers: usize,
    /// Maximum concurrent RPC requests
    pub max_concurrent_rpc: usize,
    /// RPC rate limiter (requests per second) - derived from config
    pub rpc_rate_limit: u32,
}

impl WorkerPoolConfig {
    /// Create worker pool config from app config
    pub fn from_app_config(config: &AppConfig) -> Self {
        let num_workers = config.queue.num_workers.unwrap_or(4);
        let max_concurrent_rpc = config.queue.max_concurrent_rpc.unwrap_or(8);

        Self {
            num_workers,
            max_concurrent_rpc,
            rpc_rate_limit: config.rpc.rate_limit_per_second,
        }
    }
}

/// Worker pool for parallel signal processing
pub struct WorkerPool {
    /// Priority queue for signal distribution
    queue: Arc<crate::engine::PriorityQueue>,
    /// Consolidated signal processing pipeline (shared across workers)
    signal_processor: SignalProcessor,
    /// Configuration
    config: WorkerPoolConfig,
    /// Worker tasks
    workers: JoinSet<Result<(), String>>,
    /// RPC semaphore for rate limiting
    rpc_semaphore: Arc<Semaphore>,
    /// Active worker count
    active_workers: Arc<std::sync::atomic::AtomicUsize>,
    /// Panic counter for circuit-breaker integration.
    #[allow(dead_code)] // Reserved for future panic-circuit-breaker wiring
    panic_count: Arc<std::sync::atomic::AtomicU32>,
    /// Cancellation token for graceful shutdown
    cancel_token: tokio_util::sync::CancellationToken,
}

impl WorkerPool {
    /// Create new worker pool
    pub fn new(
        queue: Arc<crate::engine::PriorityQueue>,
        signal_processor: SignalProcessor,
        config: WorkerPoolConfig,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Self {
        let rpc_semaphore = Arc::new(Semaphore::new(config.max_concurrent_rpc));

        Self {
            queue,
            signal_processor,
            config,
            workers: JoinSet::new(),
            rpc_semaphore,
            active_workers: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            panic_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            cancel_token,
        }
    }

    /// Expose the cancellation token for external shutdown triggering.
    pub fn cancel_token(&self) -> tokio_util::sync::CancellationToken {
        self.cancel_token.clone()
    }

    /// Start worker tasks
    pub async fn start(&mut self) {
        info!(
            num_workers = self.config.num_workers,
            max_concurrent_rpc = self.config.max_concurrent_rpc,
            "Starting worker pool"
        );

        for worker_id in 0..self.config.num_workers {
            let queue = Arc::clone(&self.queue);
            let signal_processor = self.signal_processor.clone();
            let rpc_semaphore = Arc::clone(&self.rpc_semaphore);
            let active_workers = Arc::clone(&self.active_workers);
            let cancel_token = self.cancel_token.clone();

            self.workers.spawn(async move {
                Self::worker_loop(
                    worker_id,
                    queue,
                    signal_processor,
                    rpc_semaphore,
                    active_workers,
                    cancel_token,
                )
                .await
            });
        }

        info!(
            workers = self.config.num_workers,
            "Worker pool started successfully"
        );
    }

    /// Worker processing loop
    async fn worker_loop(
        worker_id: usize,
        queue: Arc<crate::engine::PriorityQueue>,
        signal_processor: SignalProcessor,
        rpc_semaphore: Arc<Semaphore>,
        active_workers: Arc<std::sync::atomic::AtomicUsize>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<(), String> {
        debug!(worker_id = worker_id, "Worker started");

        loop {
            tokio::select! {
                biased;
                _ = cancel_token.cancelled() => {
                    info!(worker_id = worker_id, "Worker shutting down");
                    return Ok(());
                }
                signal = queue.pop_wait() => {
                    let signal = match signal {
                        Some(s) => s,
                        None => continue,
                    };

                    active_workers.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    let start_time = std::time::Instant::now();
                    let trade_uuid = signal.trade_uuid.clone();

                    debug!(
                        worker_id = worker_id,
                        trade_uuid = %trade_uuid,
                        strategy = %signal.payload.strategy,
                        "Worker processing signal"
                    );

                    let permit = match rpc_semaphore.acquire().await {
                        Ok(p) => p,
                        Err(e) => {
                            error!(worker_id = worker_id, error = %e, "Failed to acquire RPC permit");
                            active_workers.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                            continue;
                        }
                    };

                    let mut signal_clone = signal.clone();
                    signal_processor.process_signal(&mut signal_clone).await;

                    drop(permit);

                    let elapsed = start_time.elapsed();
                    active_workers.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);

                    info!(
                        worker_id = worker_id,
                        trade_uuid = %trade_uuid,
                        duration_ms = elapsed.as_millis(),
                        "Signal processed"
                    );
                }
            }
        }
    }

    /// Get worker pool statistics
    pub fn stats(&self) -> WorkerPoolStats {
        WorkerPoolStats {
            active_workers: self
                .active_workers
                .load(std::sync::atomic::Ordering::Relaxed),
            queue_depth: self.queue.len(),
            rpc_semaphore_available: self.rpc_semaphore.available_permits(),
        }
    }

    /// Wait for all workers to complete (for graceful shutdown)
    pub async fn shutdown(&mut self) {
        info!("Shutting down worker pool...");

        self.cancel_token.cancel();

        while let Some(result) = self.workers.join_next().await {
            match result {
                Ok(Ok(())) => {
                    debug!("Worker shut down successfully");
                }
                Ok(Err(e)) => {
                    error!(error = %e, "Worker shut down with error");
                }
                Err(e) => {
                    error!(error = %e, "Worker task panicked");
                }
            }
        }

        info!("Worker pool shutdown complete");
    }
}

/// Worker pool statistics
#[derive(Debug, Clone)]
pub struct WorkerPoolStats {
    /// Number of active workers currently processing signals
    pub active_workers: usize,
    /// Total queue depth across all priority levels
    pub queue_depth: usize,
    /// Number of available RPC permits
    pub rpc_semaphore_available: usize,
}

impl std::fmt::Display for WorkerPoolStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "WorkerPoolStats {{ active: {}, queue_depth: {}, rpc_permits: {} }}",
            self.active_workers, self.queue_depth, self.rpc_semaphore_available
        )
    }
}
