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
    assert_eq!(TradeStatus::from_str("QUEUED").unwrap(), TradeStatus::Queued);
    assert_eq!(TradeStatus::from_str("EXECUTING").unwrap(), TradeStatus::Executing);
    assert_eq!(TradeStatus::from_str("ACTIVE").unwrap(), TradeStatus::Active);
    assert_eq!(TradeStatus::from_str("EXITING").unwrap(), TradeStatus::Exiting);
    assert_eq!(TradeStatus::from_str("CLOSED").unwrap(), TradeStatus::Closed);
    assert_eq!(TradeStatus::from_str("FAILED").unwrap(), TradeStatus::Failed);
    assert_eq!(TradeStatus::from_str("RETRY").unwrap(), TradeStatus::Retry);
    assert_eq!(TradeStatus::from_str("DEAD_LETTER").unwrap(), TradeStatus::DeadLetter);
}

#[test]
fn test_status_parsing_case_insensitive() {
    // Test that parsing is case-insensitive (implementation converts to uppercase)
    assert_eq!("pending".parse::<TradeStatus>().unwrap(), TradeStatus::Pending);
    assert_eq!("PENDING".parse::<TradeStatus>().unwrap(), TradeStatus::Pending);
    assert_eq!("Active".parse::<TradeStatus>().unwrap(), TradeStatus::Active);
    assert_eq!("ACTIVE".parse::<TradeStatus>().unwrap(), TradeStatus::Active);
    assert_eq!("dead_letter".parse::<TradeStatus>().unwrap(), TradeStatus::DeadLetter);
    assert_eq!("DEAD_LETTER".parse::<TradeStatus>().unwrap(), TradeStatus::DeadLetter);
}

#[test]
fn test_all_valid_transitions() {
    // Test all valid transitions from the state machine
    let valid_transitions = vec![
        // Forward flow
        (TradeStatus::Pending, TradeStatus::Queued),
        (TradeStatus::Queued, TradeStatus::Executing),
        (TradeStatus::Executing, TradeStatus::Active),
        (TradeStatus::Executing, TradeStatus::Failed),
        (TradeStatus::Active, TradeStatus::Exiting),
        (TradeStatus::Exiting, TradeStatus::Closed),
        // Retry flow
        (TradeStatus::Failed, TradeStatus::Retry),
        (TradeStatus::Retry, TradeStatus::Executing),
        (TradeStatus::Retry, TradeStatus::DeadLetter),
        // Recovery flows
        (TradeStatus::Exiting, TradeStatus::Active), // Stuck state recovery
        // Direct to dead letter
        (TradeStatus::Executing, TradeStatus::DeadLetter),
        (TradeStatus::Pending, TradeStatus::DeadLetter),
        (TradeStatus::Queued, TradeStatus::DeadLetter),
    ];
    
    for (from, to) in valid_transitions {
        assert!(
            from.can_transition_to(to),
            "{:?} -> {:?} should be valid",
            from,
            to
        );
    }
}

#[test]
fn test_all_invalid_transitions() {
    // Test invalid transitions that should not be allowed
    let invalid_transitions = vec![
        // Cannot skip states
        (TradeStatus::Pending, TradeStatus::Executing),
        (TradeStatus::Pending, TradeStatus::Active),
        (TradeStatus::Pending, TradeStatus::Closed),
        (TradeStatus::Queued, TradeStatus::Active),
        (TradeStatus::Queued, TradeStatus::Closed),
        (TradeStatus::Executing, TradeStatus::Closed), // Must go through ACTIVE first
        (TradeStatus::Executing, TradeStatus::Exiting), // Must go through ACTIVE first
        // Cannot go backwards (except recovery)
        (TradeStatus::Active, TradeStatus::Executing),
        (TradeStatus::Active, TradeStatus::Queued),
        (TradeStatus::Active, TradeStatus::Pending),
        (TradeStatus::Exiting, TradeStatus::Executing),
        (TradeStatus::Exiting, TradeStatus::Queued),
        // Terminal states cannot transition
        (TradeStatus::Closed, TradeStatus::Active),
        (TradeStatus::Closed, TradeStatus::Exiting),
        (TradeStatus::Closed, TradeStatus::Pending),
        (TradeStatus::DeadLetter, TradeStatus::Active),
        (TradeStatus::DeadLetter, TradeStatus::Retry),
        (TradeStatus::DeadLetter, TradeStatus::Executing),
        // Invalid retry flows
        (TradeStatus::Active, TradeStatus::Retry), // Only FAILED can go to RETRY
        (TradeStatus::Pending, TradeStatus::Retry),
        (TradeStatus::Queued, TradeStatus::Retry),
        // Invalid direct transitions
        (TradeStatus::Active, TradeStatus::Failed), // ACTIVE cannot fail directly
        (TradeStatus::Exiting, TradeStatus::Failed), // EXITING cannot fail
        (TradeStatus::Closed, TradeStatus::Failed),
    ];
    
    for (from, to) in invalid_transitions {
        assert!(
            !from.can_transition_to(to),
            "{:?} -> {:?} should be invalid",
            from,
            to
        );
    }
}

#[test]
fn test_complete_forward_flow() {
    // Test the complete happy path: PENDING -> QUEUED -> EXECUTING -> ACTIVE -> EXITING -> CLOSED
    let flow = vec![
        TradeStatus::Pending,
        TradeStatus::Queued,
        TradeStatus::Executing,
        TradeStatus::Active,
        TradeStatus::Exiting,
        TradeStatus::Closed,
    ];
    
    for i in 0..flow.len() - 1 {
        let from = flow[i];
        let to = flow[i + 1];
        assert!(
            from.can_transition_to(to),
            "Step {}: {:?} -> {:?} should be valid in forward flow",
            i,
            from,
            to
        );
    }
}

#[test]
fn test_complete_retry_flow() {
    // Test the retry path: EXECUTING -> FAILED -> RETRY -> EXECUTING -> ACTIVE
    assert!(TradeStatus::Executing.can_transition_to(TradeStatus::Failed));
    assert!(TradeStatus::Failed.can_transition_to(TradeStatus::Retry));
    assert!(TradeStatus::Retry.can_transition_to(TradeStatus::Executing));
    assert!(TradeStatus::Executing.can_transition_to(TradeStatus::Active));
}

#[test]
fn test_retry_to_dead_letter_flow() {
    // Test that RETRY can go to DEAD_LETTER (max retries exceeded)
    assert!(TradeStatus::Retry.can_transition_to(TradeStatus::DeadLetter));
}

#[test]
fn test_stuck_state_recovery_flow() {
    // Test recovery flow: EXITING -> ACTIVE (when transaction expires)
    assert!(
        TradeStatus::Exiting.can_transition_to(TradeStatus::Active),
        "EXITING -> ACTIVE should be valid for stuck state recovery"
    );
    
    // After recovery, should be able to exit again
    assert!(
        TradeStatus::Active.can_transition_to(TradeStatus::Exiting),
        "After recovery, ACTIVE -> EXITING should be valid"
    );
}

#[test]
fn test_direct_to_dead_letter() {
    // Test that certain states can go directly to DEAD_LETTER
    assert!(
        TradeStatus::Pending.can_transition_to(TradeStatus::DeadLetter),
        "PENDING -> DEAD_LETTER should be valid"
    );
    assert!(
        TradeStatus::Queued.can_transition_to(TradeStatus::DeadLetter),
        "QUEUED -> DEAD_LETTER should be valid"
    );
    assert!(
        TradeStatus::Executing.can_transition_to(TradeStatus::DeadLetter),
        "EXECUTING -> DEAD_LETTER should be valid"
    );
}

#[test]
fn test_terminal_state_checks() {
    // Test all states for terminal status
    assert!(!TradeStatus::Pending.is_terminal(), "PENDING should not be terminal");
    assert!(!TradeStatus::Queued.is_terminal(), "QUEUED should not be terminal");
    assert!(!TradeStatus::Executing.is_terminal(), "EXECUTING should not be terminal");
    assert!(!TradeStatus::Active.is_terminal(), "ACTIVE should not be terminal");
    assert!(!TradeStatus::Exiting.is_terminal(), "EXITING should not be terminal");
    assert!(!TradeStatus::Failed.is_terminal(), "FAILED should not be terminal");
    assert!(!TradeStatus::Retry.is_terminal(), "RETRY should not be terminal");
    assert!(TradeStatus::Closed.is_terminal(), "CLOSED should be terminal");
    assert!(TradeStatus::DeadLetter.is_terminal(), "DEAD_LETTER should be terminal");
}

#[test]
fn test_active_position_checks() {
    // Test all states for active position status
    assert!(!TradeStatus::Pending.is_active_position(), "PENDING should not be active position");
    assert!(!TradeStatus::Queued.is_active_position(), "QUEUED should not be active position");
    assert!(!TradeStatus::Executing.is_active_position(), "EXECUTING should not be active position");
    assert!(TradeStatus::Active.is_active_position(), "ACTIVE should be active position");
    assert!(TradeStatus::Exiting.is_active_position(), "EXITING should be active position");
    assert!(!TradeStatus::Closed.is_active_position(), "CLOSED should not be active position");
    assert!(!TradeStatus::Failed.is_active_position(), "FAILED should not be active position");
    assert!(!TradeStatus::Retry.is_active_position(), "RETRY should not be active position");
    assert!(!TradeStatus::DeadLetter.is_active_position(), "DEAD_LETTER should not be active position");
}

#[test]
fn test_status_display() {
    // Test Display implementation
    assert_eq!(format!("{}", TradeStatus::Pending), "PENDING");
    assert_eq!(format!("{}", TradeStatus::Queued), "QUEUED");
    assert_eq!(format!("{}", TradeStatus::Executing), "EXECUTING");
    assert_eq!(format!("{}", TradeStatus::Active), "ACTIVE");
    assert_eq!(format!("{}", TradeStatus::Exiting), "EXITING");
    assert_eq!(format!("{}", TradeStatus::Closed), "CLOSED");
    assert_eq!(format!("{}", TradeStatus::Failed), "FAILED");
    assert_eq!(format!("{}", TradeStatus::Retry), "RETRY");
    assert_eq!(format!("{}", TradeStatus::DeadLetter), "DEAD_LETTER");
}

#[test]
fn test_status_equality() {
    // Test equality
    assert_eq!(TradeStatus::Pending, TradeStatus::Pending);
    assert_eq!(TradeStatus::Active, TradeStatus::Active);
    assert_ne!(TradeStatus::Pending, TradeStatus::Active);
    assert_ne!(TradeStatus::Active, TradeStatus::Closed);
}

#[test]
fn test_status_clone() {
    // Test that status can be cloned
    let status = TradeStatus::Active;
    let cloned = status;
    assert_eq!(status, cloned);
}

#[test]
fn test_self_transitions() {
    // Test that states cannot transition to themselves (except maybe in edge cases)
    let states = vec![
        TradeStatus::Pending,
        TradeStatus::Queued,
        TradeStatus::Executing,
        TradeStatus::Active,
        TradeStatus::Exiting,
        TradeStatus::Closed,
        TradeStatus::Failed,
        TradeStatus::Retry,
        TradeStatus::DeadLetter,
    ];
    
    for state in states {
        assert!(
            !state.can_transition_to(state),
            "{:?} should not be able to transition to itself",
            state
        );
    }
}

#[test]
fn test_multiple_paths_to_same_state() {
    // Test that multiple states can transition to the same target
    // EXECUTING and RETRY can both go to EXECUTING
    assert!(TradeStatus::Retry.can_transition_to(TradeStatus::Executing));
    // EXECUTING can go to itself via RETRY -> EXECUTING (but not directly)
    assert!(!TradeStatus::Executing.can_transition_to(TradeStatus::Executing));
    
    // Multiple states can go to DEAD_LETTER
    assert!(TradeStatus::Pending.can_transition_to(TradeStatus::DeadLetter));
    assert!(TradeStatus::Queued.can_transition_to(TradeStatus::DeadLetter));
    assert!(TradeStatus::Executing.can_transition_to(TradeStatus::DeadLetter));
    assert!(TradeStatus::Retry.can_transition_to(TradeStatus::DeadLetter));
}

#[test]
fn test_state_machine_completeness() {
    // Test that all states have at least one valid transition (except terminal states)
    let non_terminal_states = vec![
        TradeStatus::Pending,
        TradeStatus::Queued,
        TradeStatus::Executing,
        TradeStatus::Active,
        TradeStatus::Exiting,
        TradeStatus::Failed,
        TradeStatus::Retry,
    ];
    
    for state in non_terminal_states {
        let has_valid_transition = vec![
            TradeStatus::Pending,
            TradeStatus::Queued,
            TradeStatus::Executing,
            TradeStatus::Active,
            TradeStatus::Exiting,
            TradeStatus::Closed,
            TradeStatus::Failed,
            TradeStatus::Retry,
            TradeStatus::DeadLetter,
        ]
        .into_iter()
        .any(|target| state.can_transition_to(target));
        
        assert!(
            has_valid_transition,
            "{:?} should have at least one valid transition",
            state
        );
    }
}

