//! Monitoring module for automatic copy trading
//!
//! Handles on-chain transaction monitoring via Helius webhooks and RPC polling,
//! signal processing, and intelligent trade detection.

pub mod rate_limiter;
pub mod helius;
pub mod rpc_polling;
pub mod polling_task;
pub mod transaction_parser;
pub mod signal_aggregator;
pub mod pre_validator;
pub mod exit_detector;
pub mod wallet_performance;

pub use rate_limiter::{RateLimiter, RateLimitMetrics, RequestPriority};
pub use helius::HeliusClient;
pub use rpc_polling::RpcPollingState;
pub use polling_task::{start_polling_task, PollingConfig};
pub use signal_aggregator::SignalAggregator;
pub use pre_validator::PreValidator;
pub use exit_detector::ExitDetector;
pub use wallet_performance::WalletPerformanceTracker;

use std::sync::Arc;
use crate::config::AppConfig;
use crate::db::DbPool;
use crate::engine::EngineHandle;

/// Main monitoring state
pub struct MonitoringState {
    pub db: DbPool,
    pub engine: EngineHandle,
    pub config: Arc<AppConfig>,
    pub webhook_rate_limiter: Arc<RateLimiter>,
    pub rpc_rate_limiter: Arc<RateLimiter>,
    pub helius_client: Arc<HeliusClient>,
    pub signal_aggregator: Arc<SignalAggregator>,
    pub pre_validator: Arc<PreValidator>,
    pub exit_detector: Arc<ExitDetector>,
    pub wallet_performance: Arc<WalletPerformanceTracker>,
}

impl MonitoringState {
    pub fn new(
        db: DbPool,
        engine: EngineHandle,
        config: Arc<AppConfig>,
    ) -> anyhow::Result<Self> {
        let webhook_rate_limiter = Arc::new(RateLimiter::new(
            config.monitoring.as_ref()
                .map(|m| m.webhook_processing_rate_limit)
                .unwrap_or(40),
            1,
        ));
        
        let rpc_rate_limiter = Arc::new(RateLimiter::new(
            config.monitoring.as_ref()
                .map(|m| m.rpc_poll_rate_limit)
                .unwrap_or(40),
            1,
        ));

        let helius_client = Arc::new(HeliusClient::new(
            config.monitoring.as_ref()
                .and_then(|m| m.helius_api_key.clone())
                .unwrap_or_default(),
        )?);

        let signal_aggregator = Arc::new(SignalAggregator::new(db.clone()));
        let pre_validator = Arc::new(PreValidator::new(config.clone()));
        let exit_detector = Arc::new(ExitDetector::new());
        let auto_demote_enabled = config.monitoring.as_ref()
            .map(|m| m.auto_demote_wallets)
            .unwrap_or(false);
        let wallet_performance = Arc::new(WalletPerformanceTracker::with_auto_demotion(
            db.clone(),
            auto_demote_enabled,
        ));

        Ok(Self {
            db,
            engine,
            config,
            webhook_rate_limiter,
            rpc_rate_limiter,
            helius_client,
            signal_aggregator,
            pre_validator,
            exit_detector,
            wallet_performance,
        })
    }
}

// Re-export types for convenience
pub use helius::HeliusWebhookPayload;
pub use rpc_polling::WalletTransaction;
pub use transaction_parser::{TransactionInfo, ParsedSwap, SwapDirection};
pub use signal_aggregator::ConsensusSignal;
pub use pre_validator::ValidationResult;
pub use exit_detector::{ExitSignal, ExitType};
pub use wallet_performance::WalletCopyMetrics;
