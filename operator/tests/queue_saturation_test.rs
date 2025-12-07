//! Queue Saturation Load Test
//!
//! Tests priority queue load shedding from PDD Section 4.2:
//! - Fill queue to 80% capacity (800/1000)
//! - Verify Spear signals are dropped (load shedding)
//! - Verify Shield and Exit signals still accepted

use chimera_operator::engine::PriorityQueue;
use chimera_operator::models::{Action, Signal, SignalPayload, Strategy};

/// Create a test signal with the given strategy
fn make_signal(strategy: Strategy, id: u32) -> Signal {
    let payload = SignalPayload {
        strategy,
        token: format!("TEST{}", id),
        token_address: None,
        action: Action::Buy,
        amount_sol: 0.1,
        wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        trade_uuid: Some(format!("test-uuid-{}", id)),
    };
    Signal::new(payload, 12345, None)
}

// =============================================================================
// QUEUE CAPACITY TESTS
// =============================================================================

#[tokio::test]
async fn test_queue_accepts_signals_under_threshold() {
    let queue = PriorityQueue::new(100, 80); // 80 capacity for 80% threshold
    
    // Add 50 signals (under 80% threshold)
    for i in 0..50 {
        let result = queue.push(make_signal(Strategy::Shield, i)).await;
        assert!(result.is_ok(), "Signal {} should be accepted", i);
    }
    
    assert_eq!(queue.len(), 50);
}

#[tokio::test]
async fn test_queue_full_rejects_all() {
    let queue = PriorityQueue::new(10, 100); // No load shedding (100%)
    
    // Fill to capacity
    for i in 0..10 {
        let result = queue.push(make_signal(Strategy::Shield, i)).await;
        assert!(result.is_ok(), "Signal {} should be accepted", i);
    }
    
    // Next signal should be rejected (queue full)
    let result = queue.push(make_signal(Strategy::Shield, 999)).await;
    assert!(result.is_err(), "Signal should be rejected when queue is full");
    assert!(result.unwrap_err().contains("full"));
}

// =============================================================================
// LOAD SHEDDING TESTS (PDD Section 4.2)
// =============================================================================

#[tokio::test]
async fn test_load_shedding_at_80_percent() {
    // Queue with 10 capacity and 80% load shed threshold
    // Load shedding triggers at 8 items
    let queue = PriorityQueue::new(10, 80);
    
    // Fill to 80% (8 items)
    for i in 0..8 {
        let result = queue.push(make_signal(Strategy::Shield, i)).await;
        assert!(result.is_ok(), "Should accept signal {} before threshold", i);
    }
    
    assert_eq!(queue.len(), 8);
    
    // Now Spear should be rejected (load shedding active)
    let spear_result = queue.push(make_signal(Strategy::Spear, 100)).await;
    assert!(spear_result.is_err(), "Spear should be rejected at 80% capacity");
    assert!(spear_result.unwrap_err().contains("Load shedding"));
}

#[tokio::test]
async fn test_spear_dropped_shield_accepted_at_threshold() {
    let queue = PriorityQueue::new(10, 80);
    
    // Fill to threshold
    for i in 0..8 {
        queue.push(make_signal(Strategy::Shield, i)).await.unwrap();
    }
    
    // Spear should be rejected
    let spear = queue.push(make_signal(Strategy::Spear, 100)).await;
    assert!(spear.is_err(), "Spear should be dropped");
    
    // Shield should still be accepted
    let shield = queue.push(make_signal(Strategy::Shield, 101)).await;
    assert!(shield.is_ok(), "Shield should be accepted even during load shedding");
}

#[tokio::test]
async fn test_exit_accepted_during_load_shedding() {
    let queue = PriorityQueue::new(10, 80);
    
    // Fill to threshold
    for i in 0..8 {
        queue.push(make_signal(Strategy::Shield, i)).await.unwrap();
    }
    
    // Exit should be accepted (highest priority)
    let exit = queue.push(make_signal(Strategy::Exit, 102)).await;
    assert!(exit.is_ok(), "Exit should be accepted during load shedding");
}

#[tokio::test]
async fn test_only_spear_dropped() {
    let queue = PriorityQueue::new(10, 80);
    
    // Fill to threshold
    for i in 0..8 {
        queue.push(make_signal(Strategy::Shield, i)).await.unwrap();
    }
    
    let initial_len = queue.len();
    
    // Try to add all three strategies
    let spear_result = queue.push(make_signal(Strategy::Spear, 100)).await;
    let shield_result = queue.push(make_signal(Strategy::Shield, 101)).await;
    let exit_result = queue.push(make_signal(Strategy::Exit, 102)).await;
    
    // Spear dropped, others accepted
    assert!(spear_result.is_err());
    assert!(shield_result.is_ok());
    assert!(exit_result.is_ok());
    
    // Queue should only have 2 more items (Shield + Exit)
    assert_eq!(queue.len(), initial_len + 2);
}

// =============================================================================
// PRIORITY ORDERING TESTS
// =============================================================================

#[tokio::test]
async fn test_priority_order_exit_first() {
    let queue = PriorityQueue::new(100, 100);
    
    // Add in reverse priority order
    queue.push(make_signal(Strategy::Spear, 1)).await.unwrap();
    queue.push(make_signal(Strategy::Shield, 2)).await.unwrap();
    queue.push(make_signal(Strategy::Exit, 3)).await.unwrap();
    
    // Pop should return Exit first
    let first = queue.pop().await.unwrap();
    assert_eq!(first.payload.strategy, Strategy::Exit, "Exit should be popped first");
}

#[tokio::test]
async fn test_priority_order_shield_before_spear() {
    let queue = PriorityQueue::new(100, 100);
    
    // Add Spear first, then Shield
    queue.push(make_signal(Strategy::Spear, 1)).await.unwrap();
    queue.push(make_signal(Strategy::Shield, 2)).await.unwrap();
    
    // Pop should return Shield first
    let first = queue.pop().await.unwrap();
    assert_eq!(first.payload.strategy, Strategy::Shield, "Shield should be popped before Spear");
}

#[tokio::test]
async fn test_full_priority_ordering() {
    let queue = PriorityQueue::new(100, 100);
    
    // Add multiple of each strategy in mixed order
    queue.push(make_signal(Strategy::Spear, 1)).await.unwrap();
    queue.push(make_signal(Strategy::Exit, 2)).await.unwrap();
    queue.push(make_signal(Strategy::Shield, 3)).await.unwrap();
    queue.push(make_signal(Strategy::Spear, 4)).await.unwrap();
    queue.push(make_signal(Strategy::Exit, 5)).await.unwrap();
    queue.push(make_signal(Strategy::Shield, 6)).await.unwrap();
    
    // Should pop: Exit, Exit, Shield, Shield, Spear, Spear
    let s1 = queue.pop().await.unwrap();
    let s2 = queue.pop().await.unwrap();
    let s3 = queue.pop().await.unwrap();
    let s4 = queue.pop().await.unwrap();
    let s5 = queue.pop().await.unwrap();
    let s6 = queue.pop().await.unwrap();
    
    assert_eq!(s1.payload.strategy, Strategy::Exit);
    assert_eq!(s2.payload.strategy, Strategy::Exit);
    assert_eq!(s3.payload.strategy, Strategy::Shield);
    assert_eq!(s4.payload.strategy, Strategy::Shield);
    assert_eq!(s5.payload.strategy, Strategy::Spear);
    assert_eq!(s6.payload.strategy, Strategy::Spear);
}

// =============================================================================
// HIGH LOAD SIMULATION
// =============================================================================

#[tokio::test]
async fn test_high_load_spear_rejection_rate() {
    let queue = PriorityQueue::new(1000, 80); // 80% = 800 threshold
    
    // Fill to 80%
    for i in 0..800 {
        queue.push(make_signal(Strategy::Shield, i)).await.unwrap();
    }
    
    // Try to push 100 Spear signals - all should be rejected
    let mut rejected = 0;
    for i in 0..100 {
        let result = queue.push(make_signal(Strategy::Spear, 1000 + i)).await;
        if result.is_err() {
            rejected += 1;
        }
    }
    
    assert_eq!(rejected, 100, "All 100 Spear signals should be rejected");
}

#[tokio::test]
async fn test_high_load_shield_acceptance_rate() {
    let queue = PriorityQueue::new(1000, 80);
    
    // Fill to 80%
    for i in 0..800 {
        queue.push(make_signal(Strategy::Shield, i)).await.unwrap();
    }
    
    // Try to push 100 Shield signals - all should be accepted (until full)
    let mut accepted = 0;
    for i in 0..100 {
        let result = queue.push(make_signal(Strategy::Shield, 1000 + i)).await;
        if result.is_ok() {
            accepted += 1;
        }
    }
    
    // Should accept up to 200 more (1000 - 800 = 200)
    assert_eq!(accepted, 100, "All 100 Shield signals should be accepted");
}

#[tokio::test]
async fn test_queue_empty_after_drain() {
    let queue = PriorityQueue::new(100, 80);
    
    // Add signals
    for i in 0..50 {
        queue.push(make_signal(Strategy::Shield, i)).await.unwrap();
    }
    
    // Drain queue
    while queue.pop().await.is_some() {}
    
    assert_eq!(queue.len(), 0, "Queue should be empty after drain");
    assert!(queue.pop().await.is_none(), "Pop on empty queue should return None");
}

// =============================================================================
// EDGE CASES
// =============================================================================

#[tokio::test]
async fn test_zero_capacity_queue() {
    let queue = PriorityQueue::new(0, 80);
    
    // Should immediately reject
    let result = queue.push(make_signal(Strategy::Shield, 1)).await;
    assert!(result.is_err(), "Zero capacity queue should reject all signals");
}

#[tokio::test]
async fn test_100_percent_threshold_no_load_shedding() {
    let queue = PriorityQueue::new(10, 100); // 100% = no load shedding
    
    // Fill almost full
    for i in 0..9 {
        queue.push(make_signal(Strategy::Shield, i)).await.unwrap();
    }
    
    // Spear should be accepted (no load shedding at 100%)
    let spear = queue.push(make_signal(Strategy::Spear, 100)).await;
    assert!(spear.is_ok(), "Spear should be accepted with 100% threshold");
}

#[tokio::test]
async fn test_very_low_threshold() {
    let queue = PriorityQueue::new(100, 10); // 10% = load shed at 10 items
    
    // Fill to 10%
    for i in 0..10 {
        queue.push(make_signal(Strategy::Shield, i)).await.unwrap();
    }
    
    // Spear should be rejected even though queue is mostly empty
    let spear = queue.push(make_signal(Strategy::Spear, 100)).await;
    assert!(spear.is_err(), "Spear should be rejected at 10% threshold");
}

