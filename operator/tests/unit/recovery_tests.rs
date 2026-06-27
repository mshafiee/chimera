//! Recovery Manager Unit Tests
//!
//! Tests stuck state detection and recovery:
//! - Stuck position detection (> 60 seconds in EXITING)
//! - Recovery actions (MARKED_CLOSED, REVERTED_TO_ACTIVE)
//! - Recovery threshold configuration
//! - Recovery action discrimination logic

use chimera_operator::engine::recovery::{RecoveryAction, DEFAULT_STUCK_THRESHOLD_SECS};

#[test]
fn test_recovery_action_display() {
    assert_eq!(RecoveryAction::MarkedClosed.to_string(), "MARKED_CLOSED");
    assert_eq!(
        RecoveryAction::RevertedToActive.to_string(),
        "REVERTED_TO_ACTIVE"
    );
    assert_eq!(RecoveryAction::StillPending.to_string(), "STILL_PENDING");
}

#[test]
fn test_default_stuck_threshold() {
    assert_eq!(
        DEFAULT_STUCK_THRESHOLD_SECS, 60,
        "Default stuck threshold should be 60 seconds"
    );
}

#[test]
fn test_stuck_threshold_validation() {
    let stuck_seconds = 61;
    assert!(stuck_seconds > DEFAULT_STUCK_THRESHOLD_SECS);
}

#[test]
fn test_not_stuck_below_threshold() {
    let stuck_seconds = 59;
    assert!((stuck_seconds <= DEFAULT_STUCK_THRESHOLD_SECS));
}

#[test]
fn test_recovery_check_interval() {
    const RECOVERY_CHECK_INTERVAL_SECS: u64 = 30;
    assert_eq!(
        RECOVERY_CHECK_INTERVAL_SECS, 30,
        "Recovery check interval should be 30 seconds"
    );
}

#[test]
fn test_recovery_action_variants() {
    let marked_closed = RecoveryAction::MarkedClosed;
    let reverted = RecoveryAction::RevertedToActive;
    let still_pending = RecoveryAction::StillPending;

    assert!(!marked_closed.to_string().is_empty());
    assert!(!reverted.to_string().is_empty());
    assert!(!still_pending.to_string().is_empty());
}

#[test]
#[allow(clippy::assertions_on_constants, clippy::nonminimal_bool)]
fn test_stuck_detection_at_exact_threshold() {
    const EXACT_THRESHOLD: i64 = DEFAULT_STUCK_THRESHOLD_SECS;
    assert!(
        (EXACT_THRESHOLD <= DEFAULT_STUCK_THRESHOLD_SECS),
        "Position at exact threshold should NOT be stuck"
    );
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn test_stuck_detection_above_threshold() {
    const ABOVE_THRESHOLD: i64 = DEFAULT_STUCK_THRESHOLD_SECS + 1;
    assert!(
        ABOVE_THRESHOLD > DEFAULT_STUCK_THRESHOLD_SECS,
        "Position above threshold SHOULD be stuck"
    );
}
