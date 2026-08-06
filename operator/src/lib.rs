//! Chimera Operator Library
//!
//! High-frequency copy-trading system for Solana.
//! This library exposes core modules for testing.

// Long-running pipeline functions legitimately take many contextual
// parameters; allow only this narrow, noisy lint.
#![allow(clippy::too_many_arguments)]

pub mod circuit_breaker;
pub use chimera_core::{config, constants, error, retry, utils};
pub mod db_abstraction;
pub mod engine;
pub mod experiment;
pub mod handlers;
pub mod jupiter;
pub mod jupiter_error_handling;
pub mod jupiter_http_client;
pub mod jupiter_monitoring;
pub mod jupiter_skills_integration;
pub mod keypair_utils;
pub mod metrics;
pub mod middleware;
pub mod models;
pub mod monitoring;
pub mod notifications;
pub mod price_cache;

pub mod roster;
pub mod state;
pub mod token;
pub mod vault;

// Re-export commonly used types for tests
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerState, TripReason};
pub use config::{AppConfig, CircuitBreakerConfig, JitoConfig};
// Explicit re-export list (NOT a glob): `db_abstraction` defines its own `Trade`
// and `CircuitBreakerState` which would silently collide with the explicit
// `models::Trade` / `circuit_breaker::CircuitBreakerState` re-exports above.
// Those types remain reachable via `chimera_operator::db_abstraction::*`.
pub use db_abstraction::{
    create_database, dec_to_text, datetime_to_string, opt_dec_to_text, opt_text_to_dec,
    string_to_datetime, text_to_dec, timed_query, trades_to_csv, trades_to_pdf, ActivePositionEntry,
    ActivePositionSummary, ConfigAuditItem, Database, DatabaseBackend, DatabaseConfig, DbPool,
    DeadLetterItem, DiscrepancyRow, DiscrepancyTypeStats, ExitTargetData, InsertPosition,
    InsertTrade, KillSwitchState, LatencyBucket, PoolStats, Position, PositionDetail,
    PositionRecord, ReconciliationRun, ReconciliationStats, ReconciliationStatus,
    RetryableDlqItem, TradeDetail, TradeLatencyStats, TradeStatistics, UpdateDlqItemParams,
    UpdatePosition, UpdateTradeStatus, Wallet, WalletCopyPerformance, WalletDetail,
    WalletMonitoring, WalletMonitoringExtended, WalletPerformance, WebhookAuditLog,
    WebhookEligibility, WebhookStats, DatabaseMode,
};
pub use engine::recovery::{RecoveryAction, DEFAULT_STUCK_THRESHOLD_SECS};
pub use engine::{Engine, EngineHandle, PriorityQueue, TipManager};
pub use error::{AppError, AppResult};
pub use middleware::{AuthState, HmacState, Role};
pub use models::{Action, Signal, SignalPayload, Strategy, Trade, TradeStatus};
pub use notifications::{CompositeNotifier, NotificationEvent};
pub use token::{TokenCache, TokenParser, TokenSafetyConfig, TokenSafetyResult};
