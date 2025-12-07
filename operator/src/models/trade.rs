//! Trade models - represents trade state and lifecycle

use super::{Action, Signal, Strategy};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Trade status representing the state machine
///
/// State transitions:
/// ```text
/// PENDING -> QUEUED -> EXECUTING -> ACTIVE | FAILED
///                                     |
///                                     v
///                                  EXITING -> CLOSED
///                                     |
/// FAILED -> RETRY -> EXECUTING        |
///             |                       v
///             v                   DEAD_LETTER
///         DEAD_LETTER
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TradeStatus {
    /// Signal received, awaiting validation
    Pending,
    /// Validated, in priority buffer awaiting execution
    Queued,
    /// Transaction submitted to RPC, awaiting confirmation
    Executing,
    /// On-chain position confirmed, actively tracked
    Active,
    /// Exit signal received, selling transaction in flight
    Exiting,
    /// Position fully exited, PnL calculated
    Closed,
    /// Transaction rejected (insufficient funds, slippage, etc.)
    Failed,
    /// Failed transaction queued for retry
    Retry,
    /// Max retries exhausted, requires manual intervention
    DeadLetter,
}

impl TradeStatus {
    /// Check if transition to new status is valid
    pub fn can_transition_to(&self, new_status: TradeStatus) -> bool {
        use TradeStatus::*;

        matches!(
            (self, new_status),
            // Forward flow
            (Pending, Queued)
                | (Queued, Executing)
                | (Executing, Active)
                | (Executing, Failed)
                | (Active, Exiting)
                | (Exiting, Closed)
                // Retry flow
                | (Failed, Retry)
                | (Retry, Executing)
                | (Retry, DeadLetter)
                // Recovery flows
                | (Exiting, Active) // Stuck state recovery
                | (Executing, DeadLetter)
                // Direct to dead letter
                | (Pending, DeadLetter)
                | (Queued, DeadLetter)
        )
    }

    /// Check if this is a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(self, TradeStatus::Closed | TradeStatus::DeadLetter)
    }

    /// Check if this state represents an active position
    pub fn is_active_position(&self) -> bool {
        matches!(self, TradeStatus::Active | TradeStatus::Exiting)
    }
}

impl std::fmt::Display for TradeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradeStatus::Pending => write!(f, "PENDING"),
            TradeStatus::Queued => write!(f, "QUEUED"),
            TradeStatus::Executing => write!(f, "EXECUTING"),
            TradeStatus::Active => write!(f, "ACTIVE"),
            TradeStatus::Exiting => write!(f, "EXITING"),
            TradeStatus::Closed => write!(f, "CLOSED"),
            TradeStatus::Failed => write!(f, "FAILED"),
            TradeStatus::Retry => write!(f, "RETRY"),
            TradeStatus::DeadLetter => write!(f, "DEAD_LETTER"),
        }
    }
}

impl std::str::FromStr for TradeStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "PENDING" => Ok(TradeStatus::Pending),
            "QUEUED" => Ok(TradeStatus::Queued),
            "EXECUTING" => Ok(TradeStatus::Executing),
            "ACTIVE" => Ok(TradeStatus::Active),
            "EXITING" => Ok(TradeStatus::Exiting),
            "CLOSED" => Ok(TradeStatus::Closed),
            "FAILED" => Ok(TradeStatus::Failed),
            "RETRY" => Ok(TradeStatus::Retry),
            "DEAD_LETTER" => Ok(TradeStatus::DeadLetter),
            _ => Err(format!("Unknown trade status: {}", s)),
        }
    }
}

/// Trade record representing a complete trade lifecycle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    /// Database ID
    pub id: Option<i64>,
    /// Unique trade identifier
    pub trade_uuid: String,
    /// Wallet address being copied
    pub wallet_address: String,
    /// Token mint address
    pub token_address: String,
    /// Token symbol
    pub token_symbol: Option<String>,
    /// Trading strategy
    pub strategy: Strategy,
    /// Trade action
    pub side: Action,
    /// Amount in SOL
    pub amount_sol: f64,
    /// Current status
    pub status: TradeStatus,
    /// Number of retry attempts
    pub retry_count: u32,
    /// Transaction signature (if submitted)
    pub tx_signature: Option<String>,
    /// Error message (if failed)
    pub error_message: Option<String>,
    /// Realized PnL in SOL
    pub pnl_sol: Option<f64>,
    /// Realized PnL in USD
    pub pnl_usd: Option<f64>,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Last update timestamp
    pub updated_at: DateTime<Utc>,
}

impl Trade {
    /// Create a new trade from a signal
    pub fn from_signal(signal: &Signal) -> Self {
        let now = Utc::now();
        Self {
            id: None,
            trade_uuid: signal.trade_uuid.clone(),
            wallet_address: signal.payload.wallet_address.clone(),
            token_address: signal.token_address().to_string(),
            token_symbol: Some(signal.payload.token.clone()),
            strategy: signal.payload.strategy,
            side: signal.payload.action,
            amount_sol: signal.payload.amount_sol,
            status: TradeStatus::Pending,
            retry_count: 0,
            tx_signature: None,
            error_message: None,
            pnl_sol: None,
            pnl_usd: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Transition to a new status with validation
    pub fn transition_to(&mut self, new_status: TradeStatus) -> Result<(), String> {
        if !self.status.can_transition_to(new_status) {
            return Err(format!(
                "Invalid state transition: {} -> {}",
                self.status, new_status
            ));
        }
        self.status = new_status;
        self.updated_at = Utc::now();
        Ok(())
    }

    /// Mark as failed with error message
    pub fn mark_failed(&mut self, error: String) {
        self.error_message = Some(error);
        self.status = TradeStatus::Failed;
        self.updated_at = Utc::now();
    }

    /// Increment retry count and transition to Retry status
    pub fn queue_retry(&mut self) -> Result<(), String> {
        if self.status != TradeStatus::Failed {
            return Err("Can only retry from Failed state".to_string());
        }
        self.retry_count += 1;
        self.status = TradeStatus::Retry;
        self.updated_at = Utc::now();
        Ok(())
    }

    /// Check if max retries exceeded
    pub fn max_retries_exceeded(&self, max_retries: u32) -> bool {
        self.retry_count >= max_retries
    }
}

/// Maximum number of retry attempts before moving to dead letter queue
pub const MAX_RETRY_ATTEMPTS: u32 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    // ==========================================================================
    // VALID STATE TRANSITIONS (PDD Section 4.5)
    // ==========================================================================

    #[test]
    fn test_pending_to_queued_valid() {
        assert!(
            TradeStatus::Pending.can_transition_to(TradeStatus::Queued),
            "PENDING -> QUEUED should be valid (validation passed)"
        );
    }

    #[test]
    fn test_queued_to_executing_valid() {
        assert!(
            TradeStatus::Queued.can_transition_to(TradeStatus::Executing),
            "QUEUED -> EXECUTING should be valid (dequeued for execution)"
        );
    }

    #[test]
    fn test_executing_to_active_valid() {
        assert!(
            TradeStatus::Executing.can_transition_to(TradeStatus::Active),
            "EXECUTING -> ACTIVE should be valid (TX confirmed)"
        );
    }

    #[test]
    fn test_executing_to_failed_valid() {
        assert!(
            TradeStatus::Executing.can_transition_to(TradeStatus::Failed),
            "EXECUTING -> FAILED should be valid (TX rejected)"
        );
    }

    #[test]
    fn test_active_to_exiting_valid() {
        assert!(
            TradeStatus::Active.can_transition_to(TradeStatus::Exiting),
            "ACTIVE -> EXITING should be valid (exit signal received)"
        );
    }

    #[test]
    fn test_exiting_to_closed_valid() {
        assert!(
            TradeStatus::Exiting.can_transition_to(TradeStatus::Closed),
            "EXITING -> CLOSED should be valid (exit confirmed)"
        );
    }

    // ==========================================================================
    // RETRY FLOW
    // ==========================================================================

    #[test]
    fn test_failed_to_retry_valid() {
        assert!(
            TradeStatus::Failed.can_transition_to(TradeStatus::Retry),
            "FAILED -> RETRY should be valid (auto-retry)"
        );
    }

    #[test]
    fn test_retry_to_executing_valid() {
        assert!(
            TradeStatus::Retry.can_transition_to(TradeStatus::Executing),
            "RETRY -> EXECUTING should be valid (retry attempt)"
        );
    }

    #[test]
    fn test_retry_to_dead_letter_valid() {
        assert!(
            TradeStatus::Retry.can_transition_to(TradeStatus::DeadLetter),
            "RETRY -> DEAD_LETTER should be valid (max retries exceeded)"
        );
    }

    // ==========================================================================
    // RECOVERY FLOWS
    // ==========================================================================

    #[test]
    fn test_exiting_to_active_recovery() {
        assert!(
            TradeStatus::Exiting.can_transition_to(TradeStatus::Active),
            "EXITING -> ACTIVE should be valid (stuck state recovery)"
        );
    }

    #[test]
    fn test_executing_to_dead_letter_valid() {
        assert!(
            TradeStatus::Executing.can_transition_to(TradeStatus::DeadLetter),
            "EXECUTING -> DEAD_LETTER should be valid (unrecoverable failure)"
        );
    }

    #[test]
    fn test_pending_to_dead_letter_valid() {
        assert!(
            TradeStatus::Pending.can_transition_to(TradeStatus::DeadLetter),
            "PENDING -> DEAD_LETTER should be valid (validation failure)"
        );
    }

    #[test]
    fn test_queued_to_dead_letter_valid() {
        assert!(
            TradeStatus::Queued.can_transition_to(TradeStatus::DeadLetter),
            "QUEUED -> DEAD_LETTER should be valid (queue timeout)"
        );
    }

    // ==========================================================================
    // INVALID STATE TRANSITIONS
    // ==========================================================================

    #[test]
    fn test_pending_to_active_invalid() {
        assert!(
            !TradeStatus::Pending.can_transition_to(TradeStatus::Active),
            "PENDING -> ACTIVE should be invalid (must go through QUEUED, EXECUTING)"
        );
    }

    #[test]
    fn test_active_to_queued_invalid() {
        assert!(
            !TradeStatus::Active.can_transition_to(TradeStatus::Queued),
            "ACTIVE -> QUEUED should be invalid (backwards flow)"
        );
    }

    #[test]
    fn test_closed_to_any_invalid() {
        assert!(!TradeStatus::Closed.can_transition_to(TradeStatus::Active));
        assert!(!TradeStatus::Closed.can_transition_to(TradeStatus::Pending));
        assert!(!TradeStatus::Closed.can_transition_to(TradeStatus::Exiting));
    }

    #[test]
    fn test_dead_letter_to_any_invalid() {
        assert!(!TradeStatus::DeadLetter.can_transition_to(TradeStatus::Pending));
        assert!(!TradeStatus::DeadLetter.can_transition_to(TradeStatus::Retry));
        assert!(!TradeStatus::DeadLetter.can_transition_to(TradeStatus::Active));
    }

    #[test]
    fn test_active_to_closed_invalid() {
        assert!(
            !TradeStatus::Active.can_transition_to(TradeStatus::Closed),
            "ACTIVE -> CLOSED should be invalid (must go through EXITING)"
        );
    }

    // ==========================================================================
    // TERMINAL STATE TESTS
    // ==========================================================================

    #[test]
    fn test_closed_is_terminal() {
        assert!(TradeStatus::Closed.is_terminal(), "CLOSED should be terminal");
    }

    #[test]
    fn test_dead_letter_is_terminal() {
        assert!(TradeStatus::DeadLetter.is_terminal(), "DEAD_LETTER should be terminal");
    }

    #[test]
    fn test_active_not_terminal() {
        assert!(!TradeStatus::Active.is_terminal(), "ACTIVE should not be terminal");
    }

    #[test]
    fn test_pending_not_terminal() {
        assert!(!TradeStatus::Pending.is_terminal(), "PENDING should not be terminal");
    }

    // ==========================================================================
    // ACTIVE POSITION TESTS
    // ==========================================================================

    #[test]
    fn test_active_is_active_position() {
        assert!(TradeStatus::Active.is_active_position(), "ACTIVE is an active position");
    }

    #[test]
    fn test_exiting_is_active_position() {
        assert!(TradeStatus::Exiting.is_active_position(), "EXITING is an active position (still holding)");
    }

    #[test]
    fn test_closed_not_active_position() {
        assert!(!TradeStatus::Closed.is_active_position(), "CLOSED is not an active position");
    }

    // ==========================================================================
    // STRING PARSING TESTS
    // ==========================================================================

    #[test]
    fn test_status_from_string_uppercase() {
        assert_eq!("PENDING".parse::<TradeStatus>().unwrap(), TradeStatus::Pending);
        assert_eq!("QUEUED".parse::<TradeStatus>().unwrap(), TradeStatus::Queued);
        assert_eq!("EXECUTING".parse::<TradeStatus>().unwrap(), TradeStatus::Executing);
        assert_eq!("ACTIVE".parse::<TradeStatus>().unwrap(), TradeStatus::Active);
        assert_eq!("EXITING".parse::<TradeStatus>().unwrap(), TradeStatus::Exiting);
        assert_eq!("CLOSED".parse::<TradeStatus>().unwrap(), TradeStatus::Closed);
        assert_eq!("FAILED".parse::<TradeStatus>().unwrap(), TradeStatus::Failed);
        assert_eq!("RETRY".parse::<TradeStatus>().unwrap(), TradeStatus::Retry);
        assert_eq!("DEAD_LETTER".parse::<TradeStatus>().unwrap(), TradeStatus::DeadLetter);
    }

    #[test]
    fn test_status_from_string_lowercase() {
        assert_eq!("pending".parse::<TradeStatus>().unwrap(), TradeStatus::Pending);
        assert_eq!("active".parse::<TradeStatus>().unwrap(), TradeStatus::Active);
        assert_eq!("dead_letter".parse::<TradeStatus>().unwrap(), TradeStatus::DeadLetter);
    }

    #[test]
    fn test_status_from_string_invalid() {
        assert!("INVALID".parse::<TradeStatus>().is_err());
        assert!("".parse::<TradeStatus>().is_err());
    }

    // ==========================================================================
    // RETRY COUNT TESTS
    // ==========================================================================

    #[test]
    fn test_max_retry_attempts_constant() {
        assert_eq!(MAX_RETRY_ATTEMPTS, 3, "Max retry attempts should be 3 per PDD");
    }

    #[test]
    fn test_max_retries_exceeded() {
        let retry_count: u32 = 3;
        assert!(retry_count >= MAX_RETRY_ATTEMPTS, "3 retries should exceed max of 3");
    }

    #[test]
    fn test_max_retries_not_exceeded() {
        let retry_count: u32 = 2;
        assert!(retry_count < MAX_RETRY_ATTEMPTS, "2 retries should not exceed max");
    }

    // ==========================================================================
    // FULL FLOW TESTS
    // ==========================================================================

    #[test]
    fn test_happy_path_flow() {
        // PENDING -> QUEUED -> EXECUTING -> ACTIVE -> EXITING -> CLOSED
        assert!(TradeStatus::Pending.can_transition_to(TradeStatus::Queued));
        assert!(TradeStatus::Queued.can_transition_to(TradeStatus::Executing));
        assert!(TradeStatus::Executing.can_transition_to(TradeStatus::Active));
        assert!(TradeStatus::Active.can_transition_to(TradeStatus::Exiting));
        assert!(TradeStatus::Exiting.can_transition_to(TradeStatus::Closed));
    }

    #[test]
    fn test_failure_with_retry_flow() {
        // PENDING -> QUEUED -> EXECUTING -> FAILED -> RETRY -> EXECUTING -> ACTIVE
        assert!(TradeStatus::Pending.can_transition_to(TradeStatus::Queued));
        assert!(TradeStatus::Queued.can_transition_to(TradeStatus::Executing));
        assert!(TradeStatus::Executing.can_transition_to(TradeStatus::Failed));
        assert!(TradeStatus::Failed.can_transition_to(TradeStatus::Retry));
        assert!(TradeStatus::Retry.can_transition_to(TradeStatus::Executing));
        assert!(TradeStatus::Executing.can_transition_to(TradeStatus::Active));
    }

    #[test]
    fn test_stuck_state_recovery_flow() {
        // ACTIVE -> EXITING -> ACTIVE (recovery) -> EXITING -> CLOSED
        assert!(TradeStatus::Active.can_transition_to(TradeStatus::Exiting));
        assert!(TradeStatus::Exiting.can_transition_to(TradeStatus::Active)); // Recovery
        assert!(TradeStatus::Active.can_transition_to(TradeStatus::Exiting));
        assert!(TradeStatus::Exiting.can_transition_to(TradeStatus::Closed));
    }
}
