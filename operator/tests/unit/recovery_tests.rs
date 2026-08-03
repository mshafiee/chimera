//! Recovery Manager Unit Tests
//!
//! Tests the production recovery constants and action vocabulary:
//! - Default stuck threshold (DEFAULT_STUCK_THRESHOLD_SECS)
//! - Recovery action display strings (MARKED_CLOSED, REVERTED_TO_ACTIVE, STILL_PENDING)
//!
//! NOTE: the stuck-detection predicate itself (get_stuck_positions) and the
//! action discrimination inside recover_position() live behind a Database
//! connection and an RPC checker, so they are covered by the DB-backed
//! integration tests (reconciliation_tests / chaos_tests), not here.

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
    // Pins the production constant: EXITING positions older than this are
    // reported stuck by get_stuck_positions.
    assert_eq!(
        DEFAULT_STUCK_THRESHOLD_SECS, 60,
        "Default stuck threshold should be 60 seconds"
    );
}
