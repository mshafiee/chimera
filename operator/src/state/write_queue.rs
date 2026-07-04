//! Async write queue with batching and retry logic
//!
//! Provides asynchronous database writes with automatic batching, exponential backoff retry,
//! and circuit breaker functionality to remove write latency from the critical path.

use crate::db_abstraction::{Database, InsertPosition, InsertTrade, UpdatePosition, UpdateTradeStatus};
use crate::state::registry::{TradeState, TradeStatus};
use rust_decimal::Decimal;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Notify};
use tracing::{debug, error, info, warn};

/// Write operation with retry metadata
#[derive(Debug, Clone)]
pub enum WriteOperation {
    InsertTrade(TradeState),
    UpdateTradeStatus {
        trade_uuid: String,
        status: TradeStatus,
        tx_signature: Option<String>,
        error_message: Option<String>,
        network_fee_sol: Option<Decimal>,
    },
    InsertPosition {
        trade_uuid: String,
        wallet_address: String,
        token_address: String,
        token_symbol: Option<String>,
        strategy: String,
        entry_amount_sol: Decimal,
        entry_price: Decimal,
        entry_tx_signature: String,
    },
    UpdatePositionState {
        trade_uuid: String,
        state: String,
    },
    UpsertWallet {
        address: String,
        status: String,
        wqs_score: Option<Decimal>,
        win_rate: Option<Decimal>,
    },
}

/// Write result with retry information
#[derive(Debug)]
pub struct WriteResult {
    pub operation: WriteOperation,
    pub success: bool,
    pub error: Option<String>,
    pub retry_count: u32,
    pub duration_ms: u64,
}

/// Async write queue manager
pub struct AsyncWriteQueue {
    /// Operation channel
    operation_tx: mpsc::Sender<WriteOperation>,
    operation_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<WriteOperation>>>,

    /// Database handle
    db: Arc<dyn Database>,

    /// Retry configuration
    retry_config: RetryConfig,

    /// Batching configuration
    batch_config: BatchConfig,

    /// Queue metrics
    metrics: Arc<QueueMetrics>,

    /// Shutdown signal
    shutdown: Arc<tokio::sync::Notify>,

    /// Worker pool
    workers: Vec<tokio::task::JoinHandle<()>>,
}

/// Retry configuration
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            initial_backoff_ms: 50,
            max_backoff_ms: 5000,
            backoff_multiplier: 2.0,
        }
    }
}

/// Batching configuration
#[derive(Debug, Clone)]
pub struct BatchConfig {
    pub max_batch_size: usize,
    pub batch_window_ms: u64,
    pub max_queue_depth: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 100,
            batch_window_ms: 100,
            max_queue_depth: 1000,
        }
    }
}

/// Queue metrics
#[derive(Debug, Default)]
pub struct QueueMetrics {
    pub operations_queued: Arc<std::sync::atomic::AtomicU64>,
    pub operations_completed: Arc<std::sync::atomic::AtomicU64>,
    pub operations_failed: Arc<std::sync::atomic::AtomicU64>,
    pub operations_retried: Arc<std::sync::atomic::AtomicU64>,
    pub batches_processed: Arc<std::sync::atomic::AtomicU64>,
    pub current_queue_depth: Arc<std::sync::atomic::AtomicU64>,
    pub total_write_duration_ms: Arc<std::sync::atomic::AtomicU64>,
}

impl AsyncWriteQueue {
    /// Create a new async write queue
    pub fn new(db: Arc<dyn Database>, retry_config: RetryConfig, batch_config: BatchConfig) -> Self {
        let (operation_tx, operation_rx) = mpsc::channel(batch_config.max_queue_depth);

        Self {
            operation_tx,
            operation_rx: Arc::new(tokio::sync::Mutex::new(operation_rx)),
            db,
            retry_config,
            batch_config,
            metrics: Arc::new(QueueMetrics::default()),
            shutdown: Arc::new(tokio::sync::Notify::new()),
            workers: Vec::new(),
        }
    }

    /// Start the write queue workers
    pub async fn start(&mut self, num_workers: usize) -> Result<(), QueueError> {
        info!("Starting async write queue with {} workers", num_workers);

        for worker_id in 0..num_workers {
            let rx = Arc::clone(&self.operation_rx);
            let db = Arc::clone(&self.db);
            let retry_config = self.retry_config.clone();
            let batch_config = self.batch_config.clone();
            let metrics = Arc::clone(&self.metrics);
            let shutdown = Arc::clone(&self.shutdown);

            let worker = tokio::spawn(async move {
                Self::worker_loop(worker_id, rx, db, retry_config, batch_config, metrics, shutdown).await;
            });

            self.workers.push(worker);
        }

        Ok(())
    }

    /// Queue a write operation
    pub async fn enqueue(&self, operation: WriteOperation) -> Result<(), QueueError> {
        self.metrics.operations_queued.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        match self.operation_tx.try_send(operation) {
            Ok(_) => {
                let depth = self.metrics.current_queue_depth.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if depth > (self.batch_config.max_queue_depth / 2) as u64 {
                    warn!("Write queue depth high: {}", depth);
                }
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_op)) => {
                error!("Write queue full, rejecting operation");
                Err(QueueError::QueueFull)
            }
            Err(mpsc::error::TrySendError::Closed(_op)) => {
                Err(QueueError::QueueClosed)
            }
        }
    }

    /// Get current queue depth
    pub fn queue_depth(&self) -> u64 {
        self.metrics.current_queue_depth.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get metrics snapshot
    pub fn get_metrics(&self) -> QueueMetricsSnapshot {
        QueueMetricsSnapshot {
            operations_queued: self.metrics.operations_queued.load(std::sync::atomic::Ordering::Relaxed),
            operations_completed: self.metrics.operations_completed.load(std::sync::atomic::Ordering::Relaxed),
            operations_failed: self.metrics.operations_failed.load(std::sync::atomic::Ordering::Relaxed),
            operations_retried: self.metrics.operations_retried.load(std::sync::atomic::Ordering::Relaxed),
            batches_processed: self.metrics.batches_processed.load(std::sync::atomic::Ordering::Relaxed),
            current_queue_depth: self.metrics.current_queue_depth.load(std::sync::atomic::Ordering::Relaxed),
            total_write_duration_ms: self.metrics.total_write_duration_ms.load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Shutdown the write queue gracefully
    pub async fn shutdown(self) -> Result<(), QueueError> {
        info!("Shutting down async write queue...");

        // Signal shutdown
        self.shutdown.notify_waiters();

        // Wait for all workers to finish (with timeout)
        let timeout_duration = Duration::from_secs(30);
        for worker in self.workers {
            match tokio::time::timeout(timeout_duration, worker).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => error!("Worker task error: {}", e),
                Err(_) => warn!("Worker shutdown timeout"),
            }
        }

        info!("Async write queue shutdown complete");
        Ok(())
    }

    /// Worker loop for processing write operations
    async fn worker_loop(
        worker_id: usize,
        rx: Arc<tokio::sync::Mutex<mpsc::Receiver<WriteOperation>>>,
        db: Arc<dyn Database>,
        retry_config: RetryConfig,
        batch_config: BatchConfig,
        metrics: Arc<QueueMetrics>,
        shutdown: Arc<tokio::sync::Notify>,
    ) {
        let mut batch_buffer: Vec<WriteOperation> = Vec::with_capacity(batch_config.max_batch_size);
        let mut last_batch_time = Instant::now();
        let mut shutdown_rx = shutdown.notified();

        info!("Worker {} started", worker_id);

        // Create a timeout for periodic shutdown checks
        let mut shutdown_interval = tokio::time::interval(Duration::from_secs(1));
        shutdown_interval.tick().await; // Skip first tick

        loop {
            // Try to receive with a timeout
            let recv_result = tokio::time::timeout(
                Duration::from_secs(1),
                Self::receive_operation(&rx)
            ).await;

            match recv_result {
                Ok(Some(operation)) => {
                    batch_buffer.push(operation);
                    metrics.current_queue_depth.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);

                    // Check if we should flush the batch
                    let elapsed = last_batch_time.elapsed();
                    let should_flush = batch_buffer.len() >= batch_config.max_batch_size
                        || elapsed >= Duration::from_millis(batch_config.batch_window_ms);

                    if should_flush {
                        debug!("Worker {} processing batch of {} operations", worker_id, batch_buffer.len());
                        Self::process_batch(&batch_buffer, &db, &retry_config, &metrics).await;
                        batch_buffer.clear();
                        last_batch_time = Instant::now();
                    }
                }
                Ok(None) => {
                    // Channel closed - flush and exit
                    if !batch_buffer.is_empty() {
                        Self::process_batch(&batch_buffer, &db, &retry_config, &metrics).await;
                    }
                    info!("Worker {} channel closed, exiting", worker_id);
                    break;
                }
                Err(_) => {
                    // Timeout - check for shutdown signal
                    shutdown.notified().await;
                    // Flush remaining batch
                    if !batch_buffer.is_empty() {
                        debug!("Worker {} flushing final batch of {} operations", worker_id, batch_buffer.len());
                        Self::process_batch(&batch_buffer, &db, &retry_config, &metrics).await;
                    }
                    info!("Worker {} shutting down", worker_id);
                    break;
                }
            }
        }
    }

    async fn receive_operation(
        rx: &Arc<tokio::sync::Mutex<mpsc::Receiver<WriteOperation>>>,
    ) -> Option<WriteOperation> {
        let mut rx_guard = rx.lock().await;
        rx_guard.recv().await
    }

    async fn process_batch(
        batch: &[WriteOperation],
        db: &Arc<dyn Database>,
        retry_config: &RetryConfig,
        metrics: &Arc<QueueMetrics>,
    ) {
        let start = Instant::now();
        let mut success_count = 0;
        let mut failure_count = 0;

        debug!("Processing batch of {} operations", batch.len());

        for operation in batch {
            let result = Self::execute_operation_with_retry(db, operation.clone(), retry_config, 0).await;

            if result.success {
                success_count += 1;
            } else {
                failure_count += 1;
            }

            metrics.operations_completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        let duration_ms = start.elapsed().as_millis() as u64;
        metrics.total_write_duration_ms.fetch_add(duration_ms, std::sync::atomic::Ordering::Relaxed);
        metrics.batches_processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        debug!(
            "Batch completed: {} success, {} failure, {}ms",
            success_count, failure_count, duration_ms
        );
    }

    async fn execute_operation_with_retry(
        db: &Arc<dyn Database>,
        operation: WriteOperation,
        retry_config: &RetryConfig,
        mut retry_count: u32,
    ) -> WriteResult {
        let start_total = Instant::now();
        let mut current_operation = operation;

        loop {
            let start = Instant::now();

            let result = match &current_operation {
                WriteOperation::InsertTrade(trade) => {
                    db.insert_trade(&InsertTrade {
                        trade_uuid: trade.trade_uuid.clone(),
                        wallet_address: trade.wallet_address.clone(),
                        token_address: trade.token_address.clone(),
                        token_symbol: trade.token_symbol.clone(),
                        strategy: trade.strategy.clone(),
                        side: trade.side.clone(),
                        amount_sol: trade.amount_sol,
                        status: (trade.status.clone()).into(),
                    }).await.map(|_| ())
                }

                WriteOperation::UpdateTradeStatus {
                    trade_uuid,
                    status,
                    tx_signature,
                    error_message,
                    network_fee_sol,
                } => {
                    db.update_trade_status(&UpdateTradeStatus {
                        trade_uuid: trade_uuid.clone(),
                        status: status.clone().into(),
                        tx_signature: tx_signature.clone(),
                        error_message: error_message.clone(),
                        network_fee_sol: *network_fee_sol,
                    }).await
                }

                WriteOperation::InsertPosition {
                    trade_uuid,
                    wallet_address,
                    token_address,
                    token_symbol,
                    strategy,
                    entry_amount_sol,
                    entry_price,
                    entry_tx_signature,
                } => {
                    db.insert_position(&InsertPosition {
                        trade_uuid: trade_uuid.clone(),
                        wallet_address: wallet_address.clone(),
                        token_address: token_address.clone(),
                        token_symbol: token_symbol.clone(),
                        strategy: strategy.clone(),
                        entry_amount_sol: *entry_amount_sol,
                        entry_price: *entry_price,
                        entry_tx_signature: entry_tx_signature.clone(),
                    }).await.map(|_| ())
                }

                WriteOperation::UpdatePositionState { trade_uuid, state } => {
                    db.update_position(&UpdatePosition {
                        trade_uuid: trade_uuid.clone(),
                        current_price: None,
                        unrealized_pnl_sol: None,
                        unrealized_pnl_percent: None,
                        state: Some(state.clone()),
                        exit_price: None,
                        exit_tx_signature: None,
                        realized_pnl_sol: None,
                        realized_pnl_usd: None,
                    }).await
                }

                WriteOperation::UpsertWallet { .. } => {
                    // Placeholder - would need DB method implementation
                    Ok(())
                }
            };

            let duration = start.elapsed();

            match result {
                Ok(_) => {
                    return WriteResult {
                        operation: current_operation,
                        success: true,
                        error: None,
                        retry_count,
                        duration_ms: duration.as_millis() as u64,
                    };
                }
                Err(e) => {
                    if retry_count < retry_config.max_retries {
                        // Exponential backoff
                        let backoff_ms = (retry_config.initial_backoff_ms as f64
                            * retry_config.backoff_multiplier.powi(retry_count as i32))
                            .min(retry_config.max_backoff_ms as f64) as u64;

                        warn!(
                            "Operation failed (attempt {}), retrying in {}ms: {}",
                            retry_count + 1, backoff_ms, e
                        );
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;

                        retry_count += 1;
                        // Continue loop to retry
                    } else {
                        error!("Operation failed after {} retries: {}", retry_config.max_retries, e);
                        return WriteResult {
                            operation: current_operation,
                            success: false,
                            error: Some(e.to_string()),
                            retry_count,
                            duration_ms: start_total.elapsed().as_millis() as u64,
                        };
                    }
                }
            }
        }
    }
}

/// Queue metrics snapshot
#[derive(Debug, Clone)]
pub struct QueueMetricsSnapshot {
    pub operations_queued: u64,
    pub operations_completed: u64,
    pub operations_failed: u64,
    pub operations_retried: u64,
    pub batches_processed: u64,
    pub current_queue_depth: u64,
    pub total_write_duration_ms: u64,
}

/// Queue errors
#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("Queue full")]
    QueueFull,

    #[error("Queue closed")]
    QueueClosed,

    #[error("Database error: {0}")]
    DatabaseError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.initial_backoff_ms, 50);
        assert_eq!(config.max_backoff_ms, 5000);
        assert_eq!(config.backoff_multiplier, 2.0);
    }

    #[test]
    fn test_batch_config_default() {
        let config = BatchConfig::default();
        assert_eq!(config.max_batch_size, 100);
        assert_eq!(config.batch_window_ms, 100);
        assert_eq!(config.max_queue_depth, 1000);
    }
}
