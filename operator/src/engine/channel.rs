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
}

impl PriorityQueue {
    /// Create a new priority queue
    pub fn new(capacity: usize, load_shed_threshold_percent: u32) -> Self {
        // High-WQS SPEAR queue capacity is 10% of total capacity (minimum 10, maximum 50)
        let spear_high_wqs_capacity = (capacity / 10).clamp(10, 50);

        Self {
            high: Mutex::new(VecDeque::new()),
            medium: Mutex::new(VecDeque::new()),
            spear_high_wqs: Mutex::new(VecDeque::new()),
            low: Mutex::new(VecDeque::new()),
            total_len: AtomicUsize::new(0),
            capacity,
            load_shed_threshold: load_shed_threshold_percent,
            spear_high_wqs_capacity,
        }
    }

    /// Get total queue length.
    ///
    /// Reads a single atomic counter rather than acquiring all four sub-queue
    /// locks in sequence — the old approach produced a snapshot that may never
    /// have been true under concurrent push/pop.
    pub fn len(&self) -> usize {
        self.total_len.load(Ordering::Relaxed)
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
                self.total_len.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
            Strategy::Shield => {
                if self.len() >= self.capacity {
                    return Err("Queue is full".to_string());
                }
                self.medium.lock().push_back(signal);
                self.total_len.fetch_add(1, Ordering::Relaxed);
            }
            Strategy::Spear => {
                // Route high-WQS SPEAR signals (WQS >= 70) to dedicated high-priority queue
                // This prevents starvation during high load
                if let Some(wqs) = wallet_wqs {
                    if wqs >= 70.0 {
                        let mut spear_high_wqs = self.spear_high_wqs.lock();
                        if spear_high_wqs.len() < self.spear_high_wqs_capacity {
                            // Add to high-WQS SPEAR queue
                            let trade_uuid = signal.trade_uuid.clone();
                            spear_high_wqs.push_back(signal);
                            drop(spear_high_wqs);
                            self.total_len.fetch_add(1, Ordering::Relaxed);
                            tracing::debug!(
                                trade_uuid = %trade_uuid,
                                wallet_wqs = wqs,
                                "Routed high-WQS SPEAR signal to dedicated queue"
                            );
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
                            return Err(
                                "Load shedding active: SPEAR signals temporarily rejected"
                                    .to_string(),
                            );
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

                if self.len() >= self.capacity {
                    return Err("Queue is full".to_string());
                }
                // Add to regular SPEAR queue
                self.low.lock().push_back(signal);
                self.total_len.fetch_add(1, Ordering::Relaxed);
            }
        }

        Ok(())
    }

    /// Pop the highest priority signal
    pub async fn pop(&self) -> Option<Signal> {
        // Try high priority first (EXIT signals)
        if let Some(signal) = self.high.lock().pop_front() {
            self.total_len.fetch_sub(1, Ordering::Relaxed);
            return Some(signal);
        }

        // Then medium priority (SHIELD signals)
        if let Some(signal) = self.medium.lock().pop_front() {
            self.total_len.fetch_sub(1, Ordering::Relaxed);
            return Some(signal);
        }

        // Then high-WQS SPEAR signals (before regular SPEAR to prevent starvation)
        if let Some(signal) = self.spear_high_wqs.lock().pop_front() {
            self.total_len.fetch_sub(1, Ordering::Relaxed);
            return Some(signal);
        }

        // Finally low priority (regular SPEAR signals)
        if let Some(signal) = self.low.lock().pop_front() {
            self.total_len.fetch_sub(1, Ordering::Relaxed);
            return Some(signal);
        }

        None
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
}
