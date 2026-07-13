//! Chimera Operator Library
//!
//! High-frequency copy-trading system for Solana.
//! This library exposes core modules for testing.

pub mod circuit_breaker;
pub mod config;
pub mod constants;
pub mod db_abstraction;
pub mod engine;
pub mod error;
pub mod experiment;
pub mod handlers;
pub mod jupiter;
pub mod jupiter_error_handling;
pub mod jupiter_http_client;
pub mod jupiter_monitoring;
pub mod jupiter_skills_integration;
pub mod metrics;
pub mod middleware;
pub mod models;
pub mod monitoring;
pub mod notifications;
pub mod price_cache;
pub mod retry;
pub mod roster;
pub mod state;
pub mod token;
pub mod utils;
pub mod vault;

// Re-export commonly used types for tests
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerState, TripReason};
pub use config::{AppConfig, CircuitBreakerConfig, JitoConfig};
pub use db_abstraction::*;
pub use engine::recovery::{RecoveryAction, DEFAULT_STUCK_THRESHOLD_SECS};
pub use engine::{Engine, EngineHandle, PriorityQueue, TipManager};
pub use error::{AppError, AppResult};
pub use middleware::{AuthState, HmacState, Role};
pub use models::{Action, Signal, SignalPayload, Strategy, Trade, TradeStatus};
pub use notifications::{CompositeNotifier, NotificationEvent};
pub use token::{TokenCache, TokenParser, TokenSafetyConfig, TokenSafetyResult};
