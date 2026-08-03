//! Monitoring module for automatic copy trading
//!
//! Handles on-chain transaction monitoring via Helius webhooks, WebSocket, and RPC polling,
//! signal processing, and intelligent trade detection.

pub mod exit_detector;
pub mod dexscreener;
pub mod helius;
pub mod helius_wss;
pub mod helius_wss_health;
pub mod nav_snapshot;
pub mod helius_wss_subscription;
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
pub use dexscreener::{DexScreenerClient, TokenMarketData};
pub use helius::HeliusClient;
pub use helius_wss::{ConnectionState, LaserStreamClient, LaserStreamConfig, ReconnectConfig};
pub use helius_wss_health::{HealthMetrics, WebSocketHealth};
pub use helius_wss_subscription::SubscriptionManager;
pub use polling_task::{start_polling_task, PollingConfig};
pub use pre_validator::PreValidator;
pub use rate_limiter::{RateLimitMetrics, RateLimiter, RequestPriority};
pub use rpc_polling::RpcPollingState;
pub use signal_aggregator::SignalAggregator;
pub use wallet_performance::WalletPerformanceTracker;
pub use wallet_performance::{classify_copy_tier, CopyTier};
pub use webhook_health_task::{
    reconcile_helius_webhooks_async, run_startup_webhook_check, start_webhook_health_task,
    StartupWebhookResult, WebhookHealthConfig,
};
pub use webhook_lifecycle::{WebhookLifecycleConfig, WebhookLifecycleManager};

use crate::circuit_breaker::CircuitBreaker;
use crate::config::AppConfig;
use crate::db_abstraction::Database;
use crate::engine::{EngineHandle, PortfolioHeat};
use crate::token::{TokenMetadataFetcher, TokenParser, is_non_speculative};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Record speculative activity for a wallet (non-stablecoin swaps)
pub async fn record_speculative_activity(db: std::sync::Arc<dyn Database>, wallet_address: &str, token: &str) {
    if is_non_speculative(token) {
        return;
    }
    let timestamp = chrono::Utc::now();
    if let Err(e) = db.update_last_speculative_signal(wallet_address, timestamp).await {
        tracing::error!(
            wallet = %wallet_address,
            token = %token,
            error = %e,
            "Failed to update last speculative signal timestamp"
        );
    }
}

/// Main monitoring state
/// TTL cache of ACTIVE wallet addresses — avoids a DB query per webhook event.
pub type ActiveWalletCache =
    Arc<parking_lot::RwLock<Option<(std::time::Instant, std::collections::HashSet<String>)>>>;

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
    /// Webhook signature dedup cache: signature → first-seen Instant.
    /// Prevents processing the same transaction delivered by multiple
    /// orphaned webhooks.
    pub processed_signatures: Arc<parking_lot::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    /// TTL cache of ACTIVE wallet addresses (refreshed every 30s). The webhook
    /// handler receives 10K+ events/hour; without this, each event triggered a
    /// `get_wallets_by_status("ACTIVE")` DB query — the dominant DB load and a
    /// cause of connection churn. Now the DB is hit once per 30s, not per event.
    pub active_wallet_cache: ActiveWalletCache,
    /// Unified selection engine (B1): shared BUY/SELL decision pipeline used
    /// by both this monitoring path and the direct webhook handler.
    pub selection: Option<Arc<crate::engine::SelectionService>>,
    /// Shared secret expected in the `Authorization` header of Helius webhook
    /// deliveries. `None` = auth header not configured (accept all).
    pub helius_auth_header: Option<String>,
    /// Enforce mode: `false` (dry-run/fail-open) = log mismatches but accept;
    /// `true` = reject with HTTP 401.
    pub helius_auth_enforce: bool,
    /// Enforce mode for RPC signature verification (B2, staged).
    pub rpc_verify_enforce: bool,
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

        let metadata_cache = token_fetcher
            .as_ref()
            .map(|tf| tf.get_metadata_cache())
            .unwrap_or_else(|| Arc::new(RwLock::new(HashMap::new())));

        // Resolve Helius API key: the config crate does NOT interpolate ${VAR}
        // placeholders in YAML, so a literal like "${HELIUS_API_KEY}" would be
        // sent to Helius as the API key and rejected ("invalid api key provided").
        // Mirror the resolution done in main.rs for the token-metadata HeliusClient.
        let helius_api_key_resolved = {
            let from_config = config
                .monitoring
                .as_ref()
                .and_then(|m| m.helius_api_key.clone())
                .unwrap_or_default();
            if from_config.starts_with("${") {
                std::env::var("HELIUS_API_KEY").unwrap_or_default()
            } else {
                from_config
            }
        };

        let helius_client = Arc::new(HeliusClient::new(
            helius_api_key_resolved,
            metadata_cache,
        )?);

        let signal_aggregator = Arc::new(SignalAggregator::new(db.clone()));
        let mut pv = PreValidator::new(config.clone()).with_helius(helius_client.clone());
        if let Some(tf) = token_fetcher {
            pv = pv.with_token_fetcher(tf);
        }
        let pre_validator = Arc::new(pv);
        let exit_detector = Arc::new(ExitDetector::new().with_db(db.clone()));
        let wallet_performance = Arc::new(WalletPerformanceTracker::new_with_config(
            db.clone(),
            config.clone(),
        ));

        let helius_auth_header = config
            .monitoring
            .as_ref()
            .and_then(|m| m.resolved_helius_auth_header());
        let helius_auth_enforce = config
            .monitoring
            .as_ref()
            .map(|m| m.helius_auth_enforce)
            .unwrap_or(false);
        let rpc_verify_enforce = config
            .monitoring
            .as_ref()
            .map(|m| m.rpc_verify_enforce)
            .unwrap_or(false);

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
            processed_signatures: Arc::new(parking_lot::Mutex::new(
                std::collections::HashMap::new(),
            )),
            active_wallet_cache: Arc::new(parking_lot::RwLock::new(None)),
            selection: None,
            helius_auth_header,
            helius_auth_enforce,
            rpc_verify_enforce,
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

    /// Attach the unified selection engine (B1)
    pub fn with_selection(mut self, s: Arc<crate::engine::SelectionService>) -> Self {
        self.selection = Some(s);
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
