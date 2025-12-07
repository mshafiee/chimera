//! State Machine Unit Tests
//!
//! Tests all position state transitions from PDD diagram:
//! - PENDING->QUEUED->EXECUTING->ACTIVE->EXITING->CLOSED
//! - Retry paths
//! - Dead letter handling

use chimera_operator::models::{Trade, TradeStatus};

#[test]
fn test_pending_to_queued_valid() {
    assert!(
        TradeStatus::Pending.can_transition_to(TradeStatus::Queued),
        "PENDING -> QUEUED should be valid"
    );
}

#[test]
fn test_queued_to_executing_valid() {
    assert!(
        TradeStatus::Queued.can_transition_to(TradeStatus::Executing),
        "QUEUED -> EXECUTING should be valid"
    );
}

#[test]
fn test_executing_to_active_valid() {
    assert!(
        TradeStatus::Executing.can_transition_to(TradeStatus::Active),
        "EXECUTING -> ACTIVE should be valid"
    );
}

#[test]
fn test_executing_to_failed_valid() {
    assert!(
        TradeStatus::Executing.can_transition_to(TradeStatus::Failed),
        "EXECUTING -> FAILED should be valid"
    );
}

#[test]
fn test_active_to_exiting_valid() {
    assert!(
        TradeStatus::Active.can_transition_to(TradeStatus::Exiting),
        "ACTIVE -> EXITING should be valid"
    );
}

#[test]
fn test_exiting_to_closed_valid() {
    assert!(
        TradeStatus::Exiting.can_transition_to(TradeStatus::Closed),
        "EXITING -> CLOSED should be valid"
    );
}

#[test]
fn test_retry_flow() {
    // FAILED -> RETRY -> EXECUTING
    assert!(
        TradeStatus::Failed.can_transition_to(TradeStatus::Retry),
        "FAILED -> RETRY should be valid"
    );
    assert!(
        TradeStatus::Retry.can_transition_to(TradeStatus::Executing),
        "RETRY -> EXECUTING should be valid"
    );
}

#[test]
fn test_retry_to_dead_letter() {
    assert!(
        TradeStatus::Retry.can_transition_to(TradeStatus::DeadLetter),
        "RETRY -> DEAD_LETTER should be valid (max retries exceeded)"
    );
}

#[test]
fn test_recovery_flow() {
    // EXITING -> ACTIVE (stuck state recovery)
    assert!(
        TradeStatus::Exiting.can_transition_to(TradeStatus::Active),
        "EXITING -> ACTIVE should be valid (recovery)"
    );
}

#[test]
fn test_terminal_states() {
    assert!(TradeStatus::Closed.is_terminal(), "CLOSED should be terminal");
    assert!(TradeStatus::DeadLetter.is_terminal(), "DEAD_LETTER should be terminal");
    assert!(!TradeStatus::Active.is_terminal(), "ACTIVE should not be terminal");
    assert!(!TradeStatus::Pending.is_terminal(), "PENDING should not be terminal");
}

#[test]
fn test_active_position_states() {
    assert!(TradeStatus::Active.is_active_position(), "ACTIVE should be active position");
    assert!(TradeStatus::Exiting.is_active_position(), "EXITING should be active position");
    assert!(!TradeStatus::Closed.is_active_position(), "CLOSED should not be active position");
    assert!(!TradeStatus::Pending.is_active_position(), "PENDING should not be active position");
}

#[test]
fn test_invalid_transitions() {
    // PENDING cannot go directly to ACTIVE
    assert!(
        !TradeStatus::Pending.can_transition_to(TradeStatus::Active),
        "PENDING -> ACTIVE should be invalid"
    );
    
    // CLOSED cannot transition to anything
    assert!(
        !TradeStatus::Closed.can_transition_to(TradeStatus::Active),
        "CLOSED -> ACTIVE should be invalid"
    );
}

use std::str::FromStr;

#[test]
fn test_status_parsing() {
    assert_eq!(TradeStatus::from_str("PENDING").unwrap(), TradeStatus::Pending);
    assert_eq!(TradeStatus::from_str("ACTIVE").unwrap(), TradeStatus::Active);
    assert_eq!(TradeStatus::from_str("CLOSED").unwrap(), TradeStatus::Closed);
}

