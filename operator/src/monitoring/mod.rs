//! Monitoring module for automatic copy trading
//!
//! Handles on-chain transaction monitoring via Helius webhooks and RPC polling,
//! signal processing, and intelligent trade detection.

pub mod exit_detector;
pub mod helius;
pub mod polling_task;
pub mod pre_validator;
pub mod rate_limiter;
pub mod rpc_polling;
pub mod signal_aggregator;
pub mod transaction_parser;
pub mod wallet_performance;

pub use exit_detector::ExitDetector;
pub use helius::HeliusClient;
pub use polling_task::{start_polling_task, PollingConfig};
pub use pre_validator::PreValidator;
pub use rate_limiter::{RateLimitMetrics, RateLimiter, RequestPriority};
pub use rpc_polling::RpcPollingState;
pub use signal_aggregator::SignalAggregator;
pub use wallet_performance::WalletPerformanceTracker;

use crate::config::AppConfig;
use crate::db::DbPool;
use crate::engine::EngineHandle;
use crate::token::TokenMetadataFetcher;
use std::sync::Arc;

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
        token_fetcher: Option<Arc<TokenMetadataFetcher>>,
    ) -> anyhow::Result<Self> {
        let webhook_rate_limiter = Arc::new(RateLimiter::new(
            config
                .monitoring
                .as_ref()
                .map(|m| m.webhook_processing_rate_limit)
                .unwrap_or(40),
            1,
        ));

        let rpc_rate_limiter = Arc::new(RateLimiter::new(
            config
                .monitoring
                .as_ref()
                .map(|m| m.rpc_poll_rate_limit)
                .unwrap_or(40),
            1,
        ));

        let helius_client = Arc::new(HeliusClient::new(
            config
                .monitoring
                .as_ref()
                .and_then(|m| m.helius_api_key.clone())
                .unwrap_or_default(),
        )?);

        let signal_aggregator = Arc::new(SignalAggregator::new(db.clone()));
        let mut pv = PreValidator::new(config.clone()).with_helius(helius_client.clone());
        if let Some(tf) = token_fetcher {
            pv = pv.with_token_fetcher(tf);
        }
        let pre_validator = Arc::new(pv);
        let exit_detector = Arc::new(ExitDetector::new());
        let auto_demote_enabled = config
            .monitoring
            .as_ref()
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
pub use exit_detector::{ExitSignal, ExitType};
pub use helius::HeliusWebhookPayload;
pub use pre_validator::ValidationResult;
pub use rpc_polling::WalletTransaction;
pub use signal_aggregator::ConsensusSignal;
pub use transaction_parser::{ParsedSwap, SwapDirection, TransactionInfo};
pub use wallet_performance::WalletCopyMetrics;
