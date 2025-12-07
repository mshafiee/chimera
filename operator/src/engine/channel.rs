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

/// Priority queue for trading signals
pub struct PriorityQueue {
    /// High priority queue (EXIT signals)
    high: Mutex<VecDeque<Signal>>,
    /// Medium priority queue (SHIELD signals)
    medium: Mutex<VecDeque<Signal>>,
    /// Low priority queue (SPEAR signals)
    low: Mutex<VecDeque<Signal>>,
    /// Maximum capacity
    capacity: usize,
    /// Load shedding threshold (percentage)
    load_shed_threshold: u32,
}

impl PriorityQueue {
    /// Create a new priority queue
    pub fn new(capacity: usize, load_shed_threshold_percent: u32) -> Self {
        Self {
            high: Mutex::new(VecDeque::new()),
            medium: Mutex::new(VecDeque::new()),
            low: Mutex::new(VecDeque::new()),
            capacity,
            load_shed_threshold: load_shed_threshold_percent,
        }
    }

    /// Get total queue length
    pub fn len(&self) -> usize {
        self.high.lock().len() + self.medium.lock().len() + self.low.lock().len()
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
    pub async fn push(&self, signal: Signal) -> Result<(), String> {
        // Check capacity
        if self.len() >= self.capacity {
            return Err("Queue is full".to_string());
        }

        // Check load shedding for Spear signals
        if signal.payload.strategy.is_sheddable() && self.should_shed_load() {
            tracing::warn!(
                trade_uuid = %signal.trade_uuid,
                queue_depth = self.len(),
                capacity = self.capacity,
                "Load shedding: dropping Spear signal"
            );
            return Err("Load shedding active: Spear signals temporarily rejected".to_string());
        }

        // Push to appropriate queue
        match signal.payload.strategy {
            Strategy::Exit => {
                self.high.lock().push_back(signal);
            }
            Strategy::Shield => {
                self.medium.lock().push_back(signal);
            }
            Strategy::Spear => {
                self.low.lock().push_back(signal);
            }
        }

        Ok(())
    }

    /// Pop the highest priority signal
    pub async fn pop(&self) -> Option<Signal> {
        // Try high priority first
        if let Some(signal) = self.high.lock().pop_front() {
            return Some(signal);
        }

        // Then medium priority
        if let Some(signal) = self.medium.lock().pop_front() {
            return Some(signal);
        }

        // Finally low priority
        self.low.lock().pop_front()
    }

    /// Get queue depths by priority
    pub fn depths(&self) -> QueueDepths {
        QueueDepths {
            high: self.high.lock().len(),
            medium: self.medium.lock().len(),
            low: self.low.lock().len(),
            total: self.len(),
            capacity: self.capacity,
        }
    }
}

/// Queue depth information
#[derive(Debug, Clone)]
pub struct QueueDepths {
    /// High priority queue depth
    pub high: usize,
    /// Medium priority queue depth
    pub medium: usize,
    /// Low priority queue depth
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

    fn make_signal(strategy: Strategy) -> Signal {
        let payload = SignalPayload {
            strategy,
            token: "TEST".to_string(),
            token_address: None,
            action: Action::Buy,
            amount_sol: 0.1,
            wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            trade_uuid: None,
        };
        Signal::new(payload, 12345, None)
    }

    #[tokio::test]
    async fn test_priority_ordering() {
        let queue = PriorityQueue::new(100, 80);

        // Push in reverse priority order
        queue.push(make_signal(Strategy::Spear)).await.unwrap();
        queue.push(make_signal(Strategy::Shield)).await.unwrap();
        queue.push(make_signal(Strategy::Exit)).await.unwrap();

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
            queue.push(make_signal(Strategy::Shield)).await.unwrap();
        }

        // Spear signals should be rejected now
        let result = queue.push(make_signal(Strategy::Spear)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Load shedding"));

        // But Shield and Exit should still work
        assert!(queue.push(make_signal(Strategy::Shield)).await.is_ok());
        assert!(queue.push(make_signal(Strategy::Exit)).await.is_ok());
    }

    #[tokio::test]
    async fn test_capacity_limit() {
        let queue = PriorityQueue::new(2, 100); // No load shedding

        queue.push(make_signal(Strategy::Shield)).await.unwrap();
        queue.push(make_signal(Strategy::Shield)).await.unwrap();

        // Third should fail - queue full
        let result = queue.push(make_signal(Strategy::Shield)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("full"));
    }
}
