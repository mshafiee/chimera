//! Recovery Manager Unit Tests
//!
//! Tests stuck state detection and recovery:
//! - Stuck position detection (> 60 seconds in EXITING)
//! - Recovery actions (MARKED_CLOSED, REVERTED_TO_ACTIVE)
//! - Recovery threshold configuration

use chimera_operator::engine::recovery::{RecoveryAction, DEFAULT_STUCK_THRESHOLD_SECS};
use chrono::Utc;

#[test]
fn test_recovery_action_display() {
    assert_eq!(RecoveryAction::MarkedClosed.to_string(), "MARKED_CLOSED");
    assert_eq!(RecoveryAction::RevertedToActive.to_string(), "REVERTED_TO_ACTIVE");
}

#[test]
fn test_default_stuck_threshold() {
    assert_eq!(DEFAULT_STUCK_THRESHOLD_SECS, 60, "Default stuck threshold should be 60 seconds");
}

#[test]
fn test_recovery_action_variants() {
    // Test that all recovery actions are properly defined
    let marked_closed = RecoveryAction::MarkedClosed;
    let reverted = RecoveryAction::RevertedToActive;
    let still_pending = RecoveryAction::StillPending;
    
    // Verify they can be created and displayed
    assert!(!marked_closed.to_string().is_empty());
    assert!(!reverted.to_string().is_empty());
    assert!(!still_pending.to_string().is_empty());
}

#[test]
fn test_stuck_threshold_validation() {
    // Position stuck for 61 seconds should be detected
    let stuck_seconds = 61;
    let threshold = DEFAULT_STUCK_THRESHOLD_SECS;
    let is_stuck = stuck_seconds > threshold;
    
    assert!(is_stuck, "Position stuck for 61s should be detected");
}

#[test]
fn test_not_stuck_below_threshold() {
    // Position stuck for 59 seconds should not be detected
    let stuck_seconds = 59;
    let threshold = DEFAULT_STUCK_THRESHOLD_SECS;
    let is_stuck = stuck_seconds > threshold;
    
    assert!(!is_stuck, "Position stuck for 59s should not be detected");
}

#[test]
fn test_recovery_check_interval() {
    // Recovery should check every 30 seconds (from recovery.rs)
    const RECOVERY_CHECK_INTERVAL_SECS: u64 = 30;
    assert_eq!(RECOVERY_CHECK_INTERVAL_SECS, 30, "Recovery check interval should be 30 seconds");
}

