//! Chimera Operator Library
//!
//! High-frequency copy-trading system for Solana.
//! This library exposes core modules for testing.

// Long-running pipeline functions legitimately take many contextual
// parameters; allow only this narrow, noisy lint.
#![allow(clippy::too_many_arguments)]

pub mod circuit_breaker;
pub use chimera_core::{config, constants, error, retry, utils};
pub use chimera_infra::db_abstraction;
pub mod engine;
pub use chimera_core::experiment;
pub mod handlers;
pub use chimera_core::{jupiter, price_cache};
pub mod jupiter_error_handling;
pub use chimera_infra::jupiter_http_client;
pub use chimera_infra::jupiter_monitoring;
pub use chimera_infra::jupiter_skills_integration;
pub use chimera_infra::keypair_utils;
pub mod metrics;
pub mod middleware;
pub use chimera_core::models;
pub mod monitoring;
pub use chimera_infra::notifications;

pub use chimera_core::roster;
pub use chimera_infra::state;
pub use chimera_infra::token;
#[allow(dead_code)]
pub mod tools;
pub use chimera_infra::vault;

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
pub use chimera_core::models::{Action, Signal, SignalPayload, Strategy, Trade, TradeStatus};
pub use notifications::{CompositeNotifier, NotificationEvent};
pub use token::{TokenCache, TokenParser, TokenSafetyConfig, TokenSafetyResult};
