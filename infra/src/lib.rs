#![allow(clippy::too_many_arguments)]

//! Chimera Infra — adapters & concrete implementations.
//!
//! Implements the repository traits and domain services defined by
//! `chimera_core`. The dependency direction is strictly one-way:
//! `infra → core`, never the reverse. The `operator` crate re-exports these
//! modules so the legacy `chimera_operator::*` paths keep working during
//! incremental extraction.

pub mod db_abstraction;
pub mod engine;
pub mod keypair_utils;
pub mod notifications;
pub mod state;
pub mod vault;
pub mod monitoring {
    pub use helius::HeliusClient;
    pub mod dexscreener;
    pub mod exit_detector;
    pub mod helius;
    pub mod helius_wss_subscription;
    pub mod nav_snapshot;
    pub mod pre_validator;
    pub mod rate_limiter;
    pub mod signal_aggregator;
    pub mod transaction_parser;
    pub mod wallet_performance;
    pub mod webhook_health_task;
    pub mod webhook_lifecycle;
}
pub mod jupiter_http_client;
pub mod jupiter_monitoring;
pub mod jupiter_skills_integration;
pub mod token;
