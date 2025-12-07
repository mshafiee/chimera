//! Chimera Operator Library
//!
//! High-frequency copy-trading system for Solana.
//! This library exposes core modules for testing.

pub mod circuit_breaker;
pub mod config;
pub mod db;
pub mod engine;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod metrics;
pub mod notifications;
pub mod price_cache;
pub mod roster;
pub mod token;
pub mod vault;

// Re-export commonly used types for tests
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerState, TripReason};
pub use config::{AppConfig, CircuitBreakerConfig, JitoConfig};
pub use db::DbPool;
pub use engine::{Engine, EngineHandle, PriorityQueue, TipManager};
pub use engine::recovery::{RecoveryAction, DEFAULT_STUCK_THRESHOLD_SECS};
pub use error::{AppError, AppResult};
pub use middleware::{AuthState, HmacState, Role};
pub use models::{Action, Signal, SignalPayload, Strategy, Trade, TradeStatus};
pub use notifications::{CompositeNotifier, NotificationEvent};
pub use token::{TokenCache, TokenParser, TokenSafetyConfig, TokenSafetyResult};

