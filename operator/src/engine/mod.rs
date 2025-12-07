//! Trading engine for Chimera Operator
//!
//! Manages signal processing, priority queuing, and trade execution.

mod channel;
mod executor;

pub use channel::*;
pub use executor::*;

use crate::config::AppConfig;
use crate::db::DbPool;
use crate::models::Signal;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Engine handle for external interaction
#[derive(Clone)]
pub struct EngineHandle {
    /// Sender for queueing signals
    tx: mpsc::Sender<Signal>,
    /// Priority queue for monitoring
    queue: Arc<PriorityQueue>,
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
}

/// Main trading engine
pub struct Engine {
    /// Configuration
    config: Arc<AppConfig>,
    /// Database pool
    db: DbPool,
    /// Priority queue
    queue: Arc<PriorityQueue>,
    /// Executor for trade submission
    executor: Executor,
    /// Channel receiver for signals
    rx: mpsc::Receiver<Signal>,
}

impl Engine {
    /// Create a new engine instance
    pub fn new(config: AppConfig, db: DbPool) -> (Self, EngineHandle) {
        let config = Arc::new(config);
        let (tx, rx) = mpsc::channel(100); // Buffer for incoming signals

        let queue = Arc::new(PriorityQueue::new(
            config.queue.capacity,
            config.queue.load_shed_threshold_percent,
        ));

        let executor = Executor::new(config.clone(), db.clone());

        let handle = EngineHandle {
            tx,
            queue: queue.clone(),
        };

        let engine = Self {
            config,
            db,
            queue,
            executor,
            rx,
        };

        (engine, handle)
    }

    /// Start the engine processing loop
    pub async fn run(mut self) {
        tracing::info!("Engine started");

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
        match self.executor.execute(&signal).await {
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
