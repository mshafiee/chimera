//! Priority queue with load shedding
//!
//! Implements a priority queue where:
//! - EXIT signals have highest priority (protect capital)
//! - SHIELD signals have medium priority (conservative trades)
//! - SPEAR signals have lowest priority (aggressive trades)
//!
//! When queue depth exceeds threshold, SPEAR signals are dropped (load shedding).

use crate::models::{Signal, Strategy};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// Priority queue for trading signals
pub struct PriorityQueue {
    /// High priority queue (EXIT signals)
    high: Mutex<VecDeque<Signal>>,
    /// Medium priority queue (SHIELD signals)
    medium: Mutex<VecDeque<Signal>>,
    /// High-WQS SPEAR queue (SPEAR signals with WQS >= 70, smaller capacity to prevent starvation)
    spear_high_wqs: Mutex<VecDeque<Signal>>,
    /// Low priority queue (SPEAR signals with WQS < 70)
    low: Mutex<VecDeque<Signal>>,
    /// Atomic total length counter — updated on every push/pop so `len()` never
    /// needs to acquire all four locks simultaneously (which would give a non-atomic
    /// snapshot that may never have been true under concurrent access).
    total_len: AtomicUsize,
    /// Maximum capacity
    capacity: usize,
    /// Load shedding threshold (percentage)
    load_shed_threshold: u32,
    /// Maximum capacity for high-WQS SPEAR queue (smaller to prevent starvation)
    spear_high_wqs_capacity: usize,
    /// Wakes a waiting worker when a new signal is pushed
    push_notify: Arc<Notify>,
}

impl PriorityQueue {
    /// Create a new priority queue
    pub fn new(capacity: usize, load_shed_threshold_percent: u32) -> Self {
        // High-WQS SPEAR queue capacity is 10% of total capacity (minimum 1,
        // maximum 50), never exceeding the global capacity so a small queue
        // cannot grow past its configured size via the dedicated queue alone.
        let spear_high_wqs_capacity = ((capacity / 10).clamp(1, 50)).min(capacity);
        // A 0% threshold would reject every low-WQS push (current >= 0 always);
        // clamp to [1, 100] so the documented "percentage of capacity" holds.
        let load_shed_threshold = load_shed_threshold_percent.clamp(1, 100);

        Self {
            high: Mutex::new(VecDeque::new()),
            medium: Mutex::new(VecDeque::new()),
            spear_high_wqs: Mutex::new(VecDeque::new()),
            low: Mutex::new(VecDeque::new()),
            total_len: AtomicUsize::new(0),
            capacity,
            load_shed_threshold,
            spear_high_wqs_capacity,
            push_notify: Arc::new(Notify::new()),
        }
    }

    /// Get total queue length.
    ///
    /// Reads a single atomic counter rather than acquiring all four sub-queue
    /// locks in sequence — the old approach produced a snapshot that may never
    /// have been true under concurrent push/pop.
    pub fn len(&self) -> usize {
        self.total_len.load(Ordering::Acquire)
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Check if we should shed load (drop Spear signals)
    fn should_shed_load(&self) -> bool {
        let current = self.len();
        let threshold = (self.capacity * self.load_shed_threshold as usize) / 100;
        current >= threshold
    }

    /// Atomically reserve a slot under the global capacity. Returns an error
    /// when the queue is already at capacity. The reservation and the
    /// `total_len` increment are one atomic operation, so concurrent pushes
    /// into different sub-queues can never jointly exceed `capacity`.
    fn try_reserve_slot(&self) -> Result<(), String> {
        self.total_len
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                if n >= self.capacity {
                    None
                } else {
                    Some(n + 1)
                }
            })
            .map(|_| ())
            .map_err(|_| "Queue is full".to_string())
    }

    /// Push a signal onto the appropriate queue
    ///
    /// # Arguments
    /// * `signal` - Signal to push
    /// * `wallet_wqs` - Optional wallet WQS score (used to route high-WQS SPEAR signals)
    pub async fn push(&self, signal: Signal, wallet_wqs: Option<f64>) -> Result<(), String> {
        // Push to appropriate queue — Exit signals bypass capacity checks (always allow exits)
        match signal.payload.strategy {
            Strategy::Exit => {
                self.high.lock().push_back(signal);
                self.total_len.fetch_add(1, Ordering::AcqRel);
                self.push_notify.notify_one();
                return Ok(());
            }
            Strategy::Shield => {
                let mut medium = self.medium.lock();
                self.try_reserve_slot()?;
                medium.push_back(signal);
                self.push_notify.notify_one();
            }
            Strategy::Spear => {
                // Route high-WQS SPEAR signals (WQS >= 70) to dedicated high-priority queue
                // This prevents starvation during high load
                if let Some(wqs) = wallet_wqs {
                    if wqs >= 70.0 {
                        let mut spear_high_wqs = self.spear_high_wqs.lock();
                        if spear_high_wqs.len() < self.spear_high_wqs_capacity {
                            // The global capacity still applies — the dedicated
                            // queue must not let the total exceed `capacity`.
                            self.try_reserve_slot()?;
                            // Add to high-WQS SPEAR queue
                            let trade_uuid = signal.trade_uuid.clone();
                            spear_high_wqs.push_back(signal);
                            drop(spear_high_wqs);
                            tracing::debug!(
                                trade_uuid = %trade_uuid,
                                wallet_wqs = wqs,
                                "Routed high-WQS SPEAR signal to dedicated queue"
                            );
                            self.push_notify.notify_one();
                            return Ok(());
                        }

                        // High-WQS SPEAR queue is full.
                        // Drop lock to avoid deadlock before checking self.should_shed_load()
                        drop(spear_high_wqs);

                        if self.should_shed_load() {
                            tracing::warn!(
                                trade_uuid = %signal.trade_uuid,
                                wallet_wqs = wqs,
                                queue_depth = self.len(),
                                "High-WQS SPEAR queue full and load shedding active, dropping signal"
                            );
                            return Err("Load shedding active: SPEAR signals temporarily rejected"
                                .to_string());
                        }
                        // Fall through to regular SPEAR queue
                    }
                }

                // Check load shedding for regular Spear signals (low WQS or no WQS data)
                if self.should_shed_load() {
                    tracing::warn!(
                        trade_uuid = %signal.trade_uuid,
                        queue_depth = self.len(),
                        capacity = self.capacity,
                        "Load shedding: dropping low-WQS Spear signal"
                    );
                    return Err(
                        "Load shedding active: Spear signals temporarily rejected".to_string()
                    );
                }

                let mut low = self.low.lock();
                self.try_reserve_slot()?;
                // Add to regular SPEAR queue
                low.push_back(signal);
                self.push_notify.notify_one();
            }
        }

        Ok(())
    }

    /// Pop the highest priority signal.
    ///
    /// Returns `None` if the queue is empty without waiting. Callers should
    /// subscribe to `push_notify` to avoid busy-waiting.
    pub async fn pop(&self) -> Option<Signal> {
        // Try high priority first (EXIT signals)
        if let Some(signal) = self.high.lock().pop_front() {
            self.total_len.fetch_sub(1, Ordering::AcqRel);
            return Some(signal);
        }

        // Then medium priority (SHIELD signals)
        if let Some(signal) = self.medium.lock().pop_front() {
            self.total_len.fetch_sub(1, Ordering::AcqRel);
            return Some(signal);
        }

        // Then high-WQS SPEAR signals (before regular SPEAR to prevent starvation)
        if let Some(signal) = self.spear_high_wqs.lock().pop_front() {
            self.total_len.fetch_sub(1, Ordering::AcqRel);
            return Some(signal);
        }

        // Finally low priority (regular SPEAR signals)
        if let Some(signal) = self.low.lock().pop_front() {
            self.total_len.fetch_sub(1, Ordering::AcqRel);
            return Some(signal);
        }

        None
    }

    /// Wait for the next signal, sleeping until one arrives.
    ///
    /// Uses the internal `Notify` to avoid busy-waiting when the queue is empty.
    pub async fn pop_wait(&self) -> Option<Signal> {
        loop {
            let notified = self.push_notify.notified();
            if let Some(signal) = self.pop().await {
                return Some(signal);
            }
            notified.await;
        }
    }

    /// Get queue depths by priority
    pub fn depths(&self) -> QueueDepths {
        let high = self.high.lock().len();
        let medium = self.medium.lock().len();
        let spear_high_wqs = self.spear_high_wqs.lock().len();
        let low = self.low.lock().len();
        let total = high + medium + spear_high_wqs + low;

        QueueDepths {
            high,
            medium,
            spear_high_wqs,
            low,
            total,
            capacity: self.capacity,
        }
    }
}

/// Queue depth information
#[derive(Debug, Clone)]
pub struct QueueDepths {
    /// High priority queue depth (EXIT)
    pub high: usize,
    /// Medium priority queue depth (SHIELD)
    pub medium: usize,
    /// High-WQS SPEAR queue depth
    pub spear_high_wqs: usize,
    /// Low priority queue depth (regular SPEAR)
    pub low: usize,
    /// Total depth
    pub total: usize,
    /// Maximum capacity
    pub capacity: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Action, SignalPayload};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn make_signal(strategy: Strategy) -> Signal {
        let payload = SignalPayload {
            strategy,
            token: "TEST".to_string(),
            token_address: None,
            action: Action::Buy,
            amount_sol: Decimal::from_str("0.1").unwrap(),
            wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            trade_uuid: None,
            exit_fraction: None,
        };
        Signal::new(payload, 12345, None)
    }

    #[tokio::test]
    async fn test_priority_ordering() {
        let queue = PriorityQueue::new(100, 80);

        // Push in reverse priority order
        queue
            .push(make_signal(Strategy::Spear), None)
            .await
            .unwrap();
        queue
            .push(make_signal(Strategy::Shield), None)
            .await
            .unwrap();
        queue.push(make_signal(Strategy::Exit), None).await.unwrap();

        // Should pop in priority order
        let s1 = queue.pop().await.unwrap();
        assert_eq!(s1.payload.strategy, Strategy::Exit);

        let s2 = queue.pop().await.unwrap();
        assert_eq!(s2.payload.strategy, Strategy::Shield);

        let s3 = queue.pop().await.unwrap();
        assert_eq!(s3.payload.strategy, Strategy::Spear);

        assert!(queue.pop().await.is_none());
    }

    #[tokio::test]
    async fn test_load_shedding() {
        // Small queue with 80% threshold = 8 items trigger shedding
        let queue = PriorityQueue::new(10, 80);

        // Fill up to threshold
        for _ in 0..8 {
            queue
                .push(make_signal(Strategy::Shield), None)
                .await
                .unwrap();
        }

        // Low-WQS Spear signals should be rejected now
        let result = queue.push(make_signal(Strategy::Spear), Some(50.0)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Load shedding"));

        // But high-WQS SPEAR should still work (routed to dedicated queue)
        assert!(queue
            .push(make_signal(Strategy::Spear), Some(75.0))
            .await
            .is_ok());

        // Shield and Exit should still work
        assert!(queue
            .push(make_signal(Strategy::Shield), None)
            .await
            .is_ok());
        assert!(queue.push(make_signal(Strategy::Exit), None).await.is_ok());
    }

    #[tokio::test]
    async fn test_capacity_limit() {
        let queue = PriorityQueue::new(2, 100); // No load shedding

        queue
            .push(make_signal(Strategy::Shield), None)
            .await
            .unwrap();
        queue
            .push(make_signal(Strategy::Shield), None)
            .await
            .unwrap();

        // Third should fail - queue full
        let result = queue.push(make_signal(Strategy::Shield), None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("full"));
    }

    #[tokio::test]
    async fn test_high_wqs_spear_routing() {
        let queue = PriorityQueue::new(100, 80);

        // High-WQS SPEAR should go to dedicated queue
        queue
            .push(make_signal(Strategy::Spear), Some(75.0))
            .await
            .unwrap();

        let depths = queue.depths();
        assert_eq!(depths.spear_high_wqs, 1);
        assert_eq!(depths.low, 0);

        // Low-WQS SPEAR should go to regular queue
        queue
            .push(make_signal(Strategy::Spear), Some(50.0))
            .await
            .unwrap();

        let depths = queue.depths();
        assert_eq!(depths.spear_high_wqs, 1);
        assert_eq!(depths.low, 1);

        // Pop should prioritize high-WQS SPEAR over regular SPEAR
        let s1 = queue.pop().await.unwrap();
        assert_eq!(s1.payload.strategy, Strategy::Spear);

        // Next pop should get regular SPEAR
        let s2 = queue.pop().await.unwrap();
        assert_eq!(s2.payload.strategy, Strategy::Spear);
    }

    #[tokio::test]
    async fn test_is_empty() {
        let queue = PriorityQueue::new(10, 80);
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        queue.push(make_signal(Strategy::Shield), None).await.unwrap();
        assert!(!queue.is_empty());
        assert_eq!(queue.len(), 1);
    }

    /// Minimal TRACE subscriber so `tracing::warn!` bodies execute.
    fn install_trace_subscriber() {
        use tracing::Subscriber;
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            struct TraceAll;
            impl Subscriber for TraceAll {
                fn enabled(&self, _m: &tracing::Metadata<'_>) -> bool {
                    true
                }
                fn new_span(&self, _s: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                    tracing::span::Id::from_u64(1)
                }
                fn record(&self, _s: &tracing::span::Id, _v: &tracing::span::Record<'_>) {}
                fn record_follows_from(&self, _s: &tracing::span::Id, _f: &tracing::span::Id) {}
                fn event(&self, _e: &tracing::Event<'_>) {}
                fn enter(&self, _s: &tracing::span::Id) {}
                fn exit(&self, _s: &tracing::span::Id) {}
            }
            let _ = tracing::subscriber::set_global_default(TraceAll);
        });
    }

    #[tokio::test]
    async fn test_high_wqs_spear_shed_when_dedicated_queue_full() {
        install_trace_subscriber();
        // capacity=10, threshold=80% => shed at 8 items. spear_high_wqs
        // capacity = min((10/10).clamp(1,50),10) = 1.
        let queue = PriorityQueue::new(10, 80);
        for _ in 0..8 {
            queue.push(make_signal(Strategy::Shield), None).await.unwrap();
        }
        // Fill the dedicated high-WQS queue (capacity 1).
        assert!(queue.push(make_signal(Strategy::Spear), Some(75.0)).await.is_ok());
        // Now the dedicated queue is full and load shedding is active → drop.
        let result = queue.push(make_signal(Strategy::Spear), Some(75.0)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Load shedding"));
    }

    #[tokio::test]
    async fn test_pop_wait_returns_after_push() {
        let queue = std::sync::Arc::new(PriorityQueue::new(10, 80));
        let q = queue.clone();
        let worker = tokio::spawn(async move { q.pop_wait().await });
        // Let the worker reach the empty-queue await on push_notify.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        queue.push(make_signal(Strategy::Exit), None).await.unwrap();
        let popped = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            worker,
        )
        .await
        .expect("pop_wait must return after a push")
        .expect("no panic")
        .expect("a signal");
        assert_eq!(popped.payload.strategy, Strategy::Exit);
    }
}
