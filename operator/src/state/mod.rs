//! In-memory state management for critical path operations
//!
//! This module provides thread-safe, in-memory storage for trade and position states,
//! eliminating database latency from the critical trading path.

pub mod coordinator;
pub mod registry;
pub mod write_queue;

// Re-export commonly used types
pub use coordinator::StateCoordinator;
pub use registry::{PortfolioHeatState, StateRegistry, TradeState, WalletState};
pub use write_queue::{AsyncWriteQueue, BatchConfig, RetryConfig, WriteOperation};
