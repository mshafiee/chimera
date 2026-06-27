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
pub mod webhook_health_task;
pub mod webhook_lifecycle;

pub use exit_detector::ExitDetector;
pub use helius::HeliusClient;
pub use polling_task::{start_polling_task, PollingConfig};
pub use pre_validator::PreValidator;
pub use rate_limiter::{RateLimitMetrics, RateLimiter, RequestPriority};
pub use rpc_polling::RpcPollingState;
pub use signal_aggregator::SignalAggregator;
pub use wallet_performance::WalletPerformanceTracker;
pub use webhook_health_task::{
    reconcile_helius_webhooks_async, run_startup_webhook_check, start_webhook_health_task,
    StartupWebhookResult, WebhookHealthConfig,
};
pub use webhook_lifecycle::{WebhookLifecycleConfig, WebhookLifecycleManager};

use crate::circuit_breaker::CircuitBreaker;
use crate::config::AppConfig;
use crate::db_abstraction::Database;
use crate::engine::{EngineHandle, PortfolioHeat};
use crate::token::{TokenMetadataFetcher, TokenParser};
use std::sync::Arc;

/// Main monitoring state
pub struct MonitoringState {
    pub db: Arc<dyn Database>,
    pub engine: EngineHandle,
    pub config: Arc<AppConfig>,
    pub webhook_rate_limiter: Arc<RateLimiter>,
    pub rpc_rate_limiter: Arc<RateLimiter>,
    pub helius_client: Arc<HeliusClient>,
    pub signal_aggregator: Arc<SignalAggregator>,
    pub pre_validator: Arc<PreValidator>,
    pub exit_detector: Arc<ExitDetector>,
    pub wallet_performance: Arc<WalletPerformanceTracker>,
    /// Circuit breaker — checked before queuing any signal from Helius webhooks
    pub circuit_breaker: Option<Arc<CircuitBreaker>>,
    /// Token parser — fast safety check before queuing
    pub token_parser: Option<Arc<TokenParser>>,
    /// Portfolio heat — checked before queuing new BUY signals
    pub portfolio_heat: Option<Arc<PortfolioHeat>>,
}

impl MonitoringState {
    pub fn new(
        db: Arc<dyn Database>,
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
        let exit_detector = Arc::new(ExitDetector::new().with_db(db.clone()));
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
            circuit_breaker: None,
            token_parser: None,
            portfolio_heat: None,
        })
    }

    /// Attach a circuit breaker (required for production use)
    pub fn with_circuit_breaker(mut self, cb: Arc<CircuitBreaker>) -> Self {
        self.circuit_breaker = Some(cb);
        self
    }

    /// Attach a token parser for fast safety checks
    pub fn with_token_parser(mut self, tp: Arc<TokenParser>) -> Self {
        self.token_parser = Some(tp);
        self
    }

    /// Attach a portfolio heat manager
    pub fn with_portfolio_heat(mut self, ph: Arc<PortfolioHeat>) -> Self {
        self.portfolio_heat = Some(ph);
        self
    }

    /// Attach an exit detector (for shared state with polling task)
    pub fn with_exit_detector(mut self, ed: Arc<ExitDetector>) -> Self {
        self.exit_detector = ed;
        self
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
