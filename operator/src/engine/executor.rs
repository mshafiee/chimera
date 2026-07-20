//! Trade executor for Solana transactions
//!
//! Handles the actual submission of trades to the Solana network.
//! Includes RPC failover with automatic recovery to primary.

use crate::config::AppConfig;
use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerState as CBState};
use crate::db_abstraction::Database;
use crate::engine::kelly_sizer::KellySizer;
use crate::engine::tips::TipManager;
use crate::engine::transaction_builder::{load_wallet_keypair, TransactionBuilder};
use crate::engine::{slippage, slippage::SlippageEstimate};
use crate::utils;
use crate::models::{Action, Signal, Strategy};
use crate::notifications::{CompositeNotifier, NotificationEvent};
use crate::price_cache::PriceCache;
use crate::vault::load_secrets_with_fallback;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{DateTime, Utc};
use rand::Rng;
use rust_decimal::prelude::*;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::Signer;
use solana_sdk::transaction::{Transaction, VersionedTransaction};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::timeout;

/// Maximum transaction size in bytes (raw, before base64 encoding)
/// Solana's limit is 1232 bytes for raw transaction size
const MAX_TX_SIZE_RAW: usize = 1232;
/// Maximum transaction size in bytes (base64 encoded)
/// Solana's limit is 1644 bytes for encoded transaction size
#[allow(dead_code)]
const MAX_TX_SIZE_ENCODED: usize = 1644;

/// Maximum acceptable Jupiter price impact (in percent) for a BUY entry.
///
/// Replaces the broken Liq/FDV Ghost-Chain heuristic in `slow_check`. Price
/// impact is the direct, accurate measure of "how much does this trade move the
/// market" — high impact means thin liquidity (the real ghost-chain / rug-exit
/// risk). A 5% cap permits normal copy-trade sizes into healthy pools while
/// rejecting entries that would dump the price on fill. Applied to BUY entries
/// only; EXIT/SELL signals are exempt so stop-losses can always close positions.
///
/// Defined as a fn because `rust_decimal::Decimal` is not `const`-constructible.
fn max_price_impact_pct() -> Decimal {
    Decimal::from_str("5").unwrap_or(Decimal::ZERO)
}

/// RPC mode for trade execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcMode {
    /// Primary RPC with Jito bundles
    Jito,
    /// Fallback to standard TPU
    Standard,
}

/// Jito-specific error classification for retry strategy
#[derive(Debug, Clone)]
pub enum JitoError {
    /// Retryable: insufficient tip, bundle timeout, network transient
    Retryable(String),
    /// Fatal: invalid transaction, insufficient balance, transaction too large
    Fatal(String),
    /// Network: Jito endpoint unavailable (may warrant fallback)
    Network(String),
}

/// RPC health status
#[derive(Debug, Clone)]
pub struct RpcHealth {
    /// Whether the RPC is healthy
    pub healthy: bool,
    /// Last check timestamp
    pub last_check: DateTime<Utc>,
    /// Latency in milliseconds (if healthy)
    pub latency_ms: Option<u64>,
}

/// Jito-specific health status
#[derive(Debug, Clone)]
pub struct JitoHealth {
    /// Whether Jito endpoint is healthy
    pub healthy: bool,
    /// Last check timestamp
    pub last_check: DateTime<Utc>,
    /// Endpoint latency in milliseconds (if healthy)
    pub latency_ms: Option<u64>,
    /// Bundle resolution success rate (0.0 to 1.0)
    pub resolution_success_rate: f64,
    /// Total bundle submissions tracked
    pub total_submissions: u64,
    /// Successful resolutions tracked
    pub successful_resolutions: u64,
}

/// Outcome from a single trade execution — returned by `execute()` instead of
/// mutating shared `parking_lot::Mutex` fields. Carries all per-signal results
/// so concurrent workers never clobber each other's state.
#[derive(Debug, Clone, Default)]
pub struct ExecutionOutcome {
    /// Transaction signature (simulated prefix for paper mode)
    pub signature: String,
    /// Whether the transaction was confirmed on-chain
    pub confirmed: bool,
    /// Fill price in SOL per whole token (pre-converted from lamports/base-unit)
    pub fill_price_sol_per_token: Option<Decimal>,
    /// Price impact percentage from Jupiter quote (e.g. 1.5 for 1.5%)
    pub price_impact_pct: Option<Decimal>,
    /// Virtual token amount from paper/devnet BUY (for DB storage)
    pub token_amount: Option<u64>,
    /// Estimated network fee in SOL (paper/devnet only; Live records real on-chain fees)
    pub estimated_fee_sol: Option<Decimal>,
    /// Real per-route DEX fee in SOL from the Jupiter quote
    /// (`routePlan[].swapInfo.feeAmount`). `None` for paper/devnet or when the
    /// quote lacked route info (P2-17).
    pub route_fee_sol: Option<Decimal>,
}

impl ExecutionOutcome {
    fn live(
        signature: String,
        confirmed: bool,
        fill_price_sol_per_token: Option<Decimal>,
        price_impact_pct: Option<Decimal>,
        route_fee_sol: Option<Decimal>,
    ) -> Self {
        Self {
            signature,
            confirmed,
            fill_price_sol_per_token,
            price_impact_pct,
            token_amount: None,
            estimated_fee_sol: None,
            route_fee_sol,
        }
    }
}

/// Convert lamports-per-base-unit to SOL-per-whole-token.
///
/// Formula: `lamports_per_base * 10^decimals / 1e9`
/// - 9-decimal token: factor = 1e9/1e9 = 1
/// - 6-decimal token: factor = 1e6/1e9 = 0.001
///
/// Returns `None` when `decimals` is `None` (unknown).
pub fn lamports_per_base_to_sol_per_token(
    lamports_per_base: Decimal,
    decimals: Option<u8>,
) -> Option<Decimal> {
    let d = decimals?;
    let token_units = Decimal::from(10u64.pow(d as u32));
    Some(lamports_per_base * token_units / Decimal::from(1_000_000_000u64))
}

/// F16/P1-13: convert a fill price to SOL/token without silently zeroing it.
///
/// Returns `None` (and logs a warning) when either the raw fill price or the
/// token decimals are unavailable. Previously callers did
/// `lamports_per_base_to_sol_per_token(lpb.unwrap_or(ZERO), decimals)`, which
/// recorded a `0` PnL/cost for unknown decimals or missing quotes — masking bad
/// data as a free fill.
pub fn convert_fill_price(
    lamports_per_base: Option<Decimal>,
    decimals: Option<u8>,
    trade_uuid: &str,
) -> Option<Decimal> {
    match (lamports_per_base, decimals) {
        (Some(lpb), Some(d)) => lamports_per_base_to_sol_per_token(lpb, Some(d)),
        (lpb, d) => {
            tracing::warn!(
                trade_uuid = %trade_uuid,
                has_fill_price = lpb.is_some(),
                has_decimals = d.is_some(),
                "Fill price unknown (missing quote amounts or token decimals) — marking as unknown, not zero"
            );
            None
        }
    }
}

/// Mutable execution state — wrapped in a Mutex so `execute` can take `&self`,
/// allowing the RwLock in Engine to be held as a read lock during the 60 s RPC call
/// instead of a write lock that would serialise all concurrent executions.
struct ExecutorMutableState {
    rpc_mode: RpcMode,
    failure_count: u32,
    fallback_since: Option<DateTime<Utc>>,
    last_recovery_attempt: Option<DateTime<Utc>>,
    /// Jito health tracking
    jito_health: Option<JitoHealth>,
    /// Jito bundle submission metrics
    jito_submissions: std::sync::atomic::AtomicU64,
    jito_resolutions_success: std::sync::atomic::AtomicU64,
    jito_resolutions_failed: std::sync::atomic::AtomicU64,
}

/// Trade executor
pub struct Executor {
    /// Configuration
    config: Arc<AppConfig>,
    /// Database
    db: Arc<dyn Database>,
    /// Interior-mutable RPC state (see ExecutorMutableState)
    mutable: parking_lot::Mutex<ExecutorMutableState>,
    /// Recovery check interval (default 5 minutes)
    recovery_interval: Duration,
    /// Notification service
    notifier: Option<Arc<CompositeNotifier>>,
    /// Latest RPC health status (cached)
    latest_rpc_health: Arc<RwLock<Option<RpcHealth>>>,
    /// HTTP client for RPC calls (fallback for health checks if needed)
    #[allow(dead_code)] // Reserved for future fallback health checks
    http_client: reqwest::Client,
    /// Solana RPC client for primary endpoint
    rpc_client: Arc<RpcClient>,
    /// Solana RPC client for fallback endpoint (if configured)
    #[allow(dead_code)] // Will be used when implementing fallback execution
    fallback_rpc_client: Option<Arc<RpcClient>>,
    /// Tip manager for dynamic tip calculation
    tip_manager: Option<Arc<TipManager>>,
    /// Jito Searcher client for direct bundle submission
    jito_searcher: Option<crate::engine::jito_searcher::JitoSearcherClient>,
    /// Price cache for volatility calculation
    price_cache: Option<Arc<PriceCache>>,
    /// Circuit breaker for checking trading permission
    circuit_breaker: Option<Arc<crate::circuit_breaker::CircuitBreaker>>,
    /// Shared market regime detector — reused across calls to avoid per-call construction.
    /// [R-L1] Previously a new MarketRegimeDetector was instantiated on every check_execution_costs
    /// call; now it is a field so the internal price_history is preserved across calls.
    market_regime_detector: Option<Arc<crate::engine::market_regime::MarketRegimeDetector>>,
    /// Kelly sizer for dynamic position sizing and friction gating
    kelly_sizer: Arc<KellySizer>,
    /// Metrics state for Prometheus metrics
    metrics: Option<Arc<crate::metrics::MetricsState>>,
}

impl Executor {
    /// Create a new executor
    pub fn new(config: Arc<AppConfig>, db: Arc<dyn Database>) -> Self {
        Self::with_circuit_breaker(config, db, None)
    }

    /// Create a new executor with optional circuit breaker
    pub fn with_circuit_breaker(
        config: Arc<AppConfig>,
        db: Arc<dyn Database>,
        circuit_breaker: Option<Arc<CircuitBreaker>>
    ) -> Self {
        let rpc_mode = if config.jito.enabled {
            RpcMode::Jito
        } else {
            RpcMode::Standard
        };

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.rpc.timeout_ms))
            .build()
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "Failed to create HTTP client with custom config — using default client");
                reqwest::Client::new()
            });

        // Create Solana RPC client for primary endpoint
        let rpc_client = Arc::new(RpcClient::new_with_timeout(
            config.rpc.primary_url.clone(),
            Duration::from_millis(config.rpc.timeout_ms),
        ));

        // Create Solana RPC client for fallback endpoint if configured
        let fallback_rpc_client = config.rpc.fallback_url.as_ref().map(|url| {
            Arc::new(RpcClient::new_with_timeout(
                url.clone(),
                Duration::from_millis(config.rpc.timeout_ms),
            ))
        });

        // Create Jito Searcher client if configured
        let jito_searcher = match config.jito.searcher_endpoint.as_ref() {
            Some(endpoint) => {
                match crate::engine::jito_searcher::JitoSearcherClient::new(
                    endpoint.clone(),
                    rpc_client.clone(),
                ) {
                    Ok(client) => Some(client),
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to create Jito Searcher client - Jito bundles will be unavailable");
                        None
                    }
                }
            }
            None => None,
        };

        // Create Kelly sizer for dynamic position sizing
        let kelly_sizer = Arc::new(KellySizer::new(db.clone()));

        Self {
            config,
            db,
            mutable: parking_lot::Mutex::new(ExecutorMutableState {
                rpc_mode,
                failure_count: 0,
                fallback_since: None,
                last_recovery_attempt: None,
                jito_health: None,
                jito_submissions: std::sync::atomic::AtomicU64::new(0),
                jito_resolutions_success: std::sync::atomic::AtomicU64::new(0),
                jito_resolutions_failed: std::sync::atomic::AtomicU64::new(0),
            }),
            recovery_interval: Duration::from_secs(300), // 5 minutes
            notifier: None,
            latest_rpc_health: Arc::new(RwLock::new(None)),
            http_client,
            rpc_client,
            fallback_rpc_client,
            tip_manager: None,
            jito_searcher,
            price_cache: None,
            circuit_breaker,
            market_regime_detector: None,
            kelly_sizer,
            metrics: None,
        }
    }

    /// Set the notification service
    pub fn with_notifier(mut self, notifier: Arc<CompositeNotifier>) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// Set the tip manager
    pub fn with_tip_manager(mut self, tip_manager: Arc<TipManager>) -> Self {
        self.tip_manager = Some(tip_manager);
        self
    }

    /// Set the metrics state
    pub fn with_metrics(mut self, metrics: Arc<crate::metrics::MetricsState>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Set the price cache for volatility calculation.
    /// Also initialises the shared MarketRegimeDetector from the same cache. [R-L1]
    pub fn with_price_cache(mut self, price_cache: Arc<PriceCache>) -> Self {
        let detector = Arc::new(crate::engine::market_regime::MarketRegimeDetector::new(
            price_cache.clone(),
        ));
        self.market_regime_detector = Some(detector);
        self.price_cache = Some(price_cache);
        self
    }

    /// Calculate retry backoff with jitter following Helius best practices.
    ///
    /// Returns a Duration with:
    /// - Base backoff: 2^attempt seconds (1s, 2s, 4s, 8s, 16s for attempts 0-4)
    /// - ±25% random jitter to prevent synchronized retries
    /// - Maximum capped at 30 seconds
    fn calculate_retry_backoff(attempt: u32) -> Duration {
        let base = 2u64.pow(attempt.min(4)); // Cap at 16s base (2^4)
        let jitter = rand::rng().random_range(-0.25..0.25); // ±25%
        let millis = ((base as f64) * (1.0 + jitter) * 1000.0) as u64;
        Duration::from_millis(millis.min(30000)) // Cap at 30s
    }

    /// Detect if an error message indicates RPC rate limiting
    fn is_rate_limit_error(error: &str) -> bool {
        let error_lower = error.to_lowercase();
        error_lower.contains("rate limit") ||
        error_lower.contains("429") ||
        error_lower.contains("too many requests") ||
        error_lower.contains("ratelimit") ||
        error_lower.contains("rate-limit")
    }

    /// Send notification if notifier is configured and rules allow it
    async fn notify(&self, event: NotificationEvent) {
        if let Some(ref notifier) = self.notifier {
            // Check notification rules before sending
            let rules = &self.config.notifications.rules;
            let should_send = match &event {
                NotificationEvent::CircuitBreakerTriggered { .. } => {
                    rules.circuit_breaker_triggered
                }
                NotificationEvent::CircuitBreakerRecovered => rules.circuit_breaker_triggered,
                NotificationEvent::WalletDrained { .. } => rules.wallet_drained,
                NotificationEvent::SystemCrash { .. } => rules.system_crash,
                NotificationEvent::PositionExited { .. } => rules.position_exited,
                NotificationEvent::RpcFallback { .. } => rules.rpc_fallback,
                NotificationEvent::WalletPromoted { .. } => rules.wallet_promoted,
                NotificationEvent::DailySummary { .. } => rules.daily_summary,
                // Jito-specific notifications: use same rules as RPC fallback
                NotificationEvent::JitoFallbackTriggered { .. } => rules.rpc_fallback,
                NotificationEvent::JitoRecovered { .. } => rules.rpc_fallback,
                NotificationEvent::JitoHealthChanged { .. } => rules.rpc_fallback,
            };

            if should_send {
                notifier.notify(event).await;
            }
        }
    }

    /// Send position exit notification
    pub async fn notify_position_exit(
        &self,
        token: &str,
        strategy: &str,
        pnl_percent: Decimal,
        pnl_sol: Decimal,
    ) {
        self.notify(NotificationEvent::PositionExited {
            token: token.to_string(),
            strategy: strategy.to_string(),
            pnl_percent,
            pnl_sol,
        })
        .await;
    }

    /// Execute a trade signal
    ///
    /// Returns an `ExecutionOutcome` carrying all per-signal results.
    #[tracing::instrument(skip(self, signal))]
    pub async fn execute(&self, signal: &Signal) -> Result<ExecutionOutcome, ExecutorError> {
        let mut attempts = 0;
        loop {
            attempts += 1;

            // Check if we should try to recover to primary
            if self.should_attempt_recovery() {
                self.try_recover_to_primary().await;
            }

            let rpc_mode = self.mutable.lock().rpc_mode;
            tracing::info!(
                trade_uuid = %signal.trade_uuid,
                strategy = %signal.payload.strategy,
                token = %signal.payload.token,
                action = %signal.payload.action,
                amount_sol = signal.payload.amount_sol.to_f64().unwrap_or(0.0),
                rpc_mode = ?rpc_mode,
                "Executing trade"
            );

            // Check if Spear is allowed in current mode
            if signal.payload.strategy == Strategy::Spear && rpc_mode == RpcMode::Standard {
                return Err(ExecutorError::SpearDisabled);
            }

            // Check circuit breaker for ENTRY trades (EXIT trades always allowed for stop-loss priority)
            if signal.payload.action == crate::models::Action::Buy {
                if let Some(circuit_breaker) = &self.circuit_breaker {
                    let current_state = circuit_breaker.current_state();
                    if current_state != CBState::Active {
                        let trip_reason = circuit_breaker.trip_reason();
                        let error_msg = format!(
                            "Circuit breaker is {:?}, reason: {:?}",
                            current_state, trip_reason
                        );
                        tracing::warn!(
                            trade_uuid = %signal.trade_uuid,
                            circuit_breaker_state = ?current_state,
                            trip_reason = ?trip_reason,
                            "Trade rejected due to circuit breaker"
                        );
                        return Err(ExecutorError::CircuitBreakerTripped(error_msg));
                    }
                }
            }

            // Check market conditions for BUY signals only — exits must always be allowed
            // through regardless of crash or volatility so stop-losses can close positions.
            if signal.payload.action == crate::models::Action::Buy {
                if let Err(e) = self
                    .check_market_conditions(&signal.payload.action, Some(signal.token_address()))
                    .await
                {
                    tracing::warn!(
                        trade_uuid = %signal.trade_uuid,
                        error = %e,
                        "Trade rejected due to market conditions"
                    );
                    return Err(ExecutorError::MarketConditionsUnfavorable(e.to_string()));
                }
            }

            // Validate amount bounds (config values are already Decimal)
            let min_position = self.config.strategy.min_position_sol;
            if signal.payload.amount_sol < min_position {
                return Err(ExecutorError::AmountTooSmall(
                    signal.payload.amount_sol,
                    min_position,
                ));
            }

            let max_position = self.config.strategy.max_position_sol;
            if signal.payload.amount_sol > max_position {
                return Err(ExecutorError::AmountTooLarge(
                    signal.payload.amount_sol,
                    max_position,
                ));
            }

            // Execute based on mode
            let result = match self.config.trade_mode {
                crate::config::TradeMode::Devnet => self.execute_devnet(signal).await,
                crate::config::TradeMode::Paper => self.execute_paper(signal).await,
                crate::config::TradeMode::Live => match rpc_mode {
                    RpcMode::Jito => self.execute_jito_with_retry(signal).await,
                    RpcMode::Standard => self.execute_standard(signal).await,
                },
            };

            // Price-impact gate (replaces the removed Liq/FDV Ghost-Chain ratio).
            // Reject BUY entries whose Jupiter-quoted price impact exceeds the cap —
            // high impact signals thin liquidity, the real rug-exit risk. EXIT/SELL
            // signals are exempt so stop-losses can always close positions. Verified
            // majors also pass through here; their deep liquidity keeps impact low.
            if signal.payload.action == Action::Buy {
                let max_impact = max_price_impact_pct();
                if let Ok(ref outcome) = result {
                    if let Some(impact) = outcome.price_impact_pct {
                        if impact > max_impact {
                            tracing::warn!(
                                trade_uuid = %signal.trade_uuid,
                                token = %signal.payload.token,
                                price_impact_pct = %impact,
                                max_pct = %max_impact,
                                "Trade rejected: price impact exceeds cap (thin liquidity)"
                            );
                            return Err(ExecutorError::TransactionFailed(format!(
                                "Price impact {:.2}% exceeds max {:.0}% — thin liquidity",
                                impact, max_impact
                            )));
                        }
                    }
                }
            }

            // Handle retry for expired blockhash
            match &result {
                Err(ExecutorError::BlockhashExpired) => {
                    if attempts < 5 {
                        // Helius-compliant retry: 1s, 2s, 4s, 8s, 16s with ±25% jitter
                        let backoff = Self::calculate_retry_backoff(attempts);
                        tracing::warn!(
                            trade_uuid = %signal.trade_uuid,
                            attempt = attempts,
                            backoff_ms = backoff.as_millis(),
                            "Blockhash expired/invalid. Re-requesting fresh quote and retrying with Helius-compliant backoff..."
                        );
                        // The loop will restart, causing TransactionBuilder to fetch a NEW quote
                        // from Jupiter with a FRESH blockhash.
                        tokio::time::sleep(backoff).await;
                        continue;
                    } else {
                        tracing::error!(
                            trade_uuid = %signal.trade_uuid,
                            attempts = attempts,
                            "Blockhash expired after maximum retries. Transaction failed."
                        );
                    }
                }
                Err(ExecutorError::V0ReconstructionFailed(e)) => {
                    // V0 reconstruction failed - try re-requesting from Jupiter
                    if attempts < 5 {
                        let backoff = Self::calculate_retry_backoff(attempts);
                        tracing::warn!(
                            trade_uuid = %signal.trade_uuid,
                            attempt = attempts,
                            error = %e,
                            backoff_ms = backoff.as_millis(),
                            "V0 reconstruction failed. Re-requesting fresh quote from Jupiter with Helius-compliant backoff..."
                        );
                        tokio::time::sleep(backoff).await;
                        continue;
                    } else {
                        tracing::error!(
                            trade_uuid = %signal.trade_uuid,
                            attempts = attempts,
                            error = %e,
                            "V0 reconstruction failed after maximum retries. Transaction failed."
                        );
                    }
                }
                Err(ExecutorError::Rpc(ref rpc_err)) => {
                    // Check if this is a rate limit error and handle with degradation system
                    if Self::is_rate_limit_error(rpc_err) && self.config.degradation.rpc_rate_limit_enabled {
                        if attempts < 10 {
                            let backoff = crate::engine::handle_rpc_rate_limit().await;
                            tracing::warn!(
                                trade_uuid = %signal.trade_uuid,
                                attempt = attempts,
                                backoff_ms = backoff.as_millis(),
                                error = %rpc_err,
                                "RPC rate limit detected. Applying degradation backoff..."
                            );
                            tokio::time::sleep(backoff).await;
                            continue;
                        } else {
                            tracing::error!(
                                trade_uuid = %signal.trade_uuid,
                                attempts = attempts,
                                error = %rpc_err,
                                "RPC rate limit: maximum retries exceeded"
                            );
                        }
                    }
                }
                _ => {}
            }

            // Handle result and track failures
            match &result {
                Ok(outcome) => {
                    self.mutable.lock().failure_count = 0;
                    tracing::info!(
                        trade_uuid = %signal.trade_uuid,
                        signature = %outcome.signature,
                        "Trade executed successfully"
                    );

                    // Record tip if using Jito and tip manager is available.
                    // The tip amount is always calculated (for cost tracking) so paper
                    // mode projects realistic live costs, but it is only RECORDED into
                    // the TipManager's success history for Live trades — paper/devnet
                    // never submit a real bundle, so recording them as successes would
                    // pollute the percentile data and skew future live tip calculations.
                    let jito_tip = if rpc_mode == RpcMode::Jito {
                        let tip = self.calculate_jito_tip(signal).await;
                        if self.config.trade_mode == crate::config::TradeMode::Live {
                            if let Some(ref tip_manager) = self.tip_manager {
                                if let Err(e) = tip_manager
                                    .record_tip(
                                        tip,
                                        Some(&outcome.signature),
                                        signal.payload.strategy,
                                        true, // success
                                    )
                                    .await
                                {
                                    tracing::warn!(
                                        error = %e,
                                        "Failed to record tip in TipManager"
                                    );
                                }
                            }
                        }
                        tip
                    } else {
                        Decimal::ZERO
                    };

                    // Track costs: Jito tip, DEX fee, slippage.
                    // F5/F6: slippage uses the unified estimate (engine::slippage),
                    // preferring Jupiter's real priceImpactPct (outcome.price_impact_pct),
                    // then the liquidity-aware sqrt model, then the config tier.
                    // P2-17/F22: DEX fee is the real per-route fee from the quote
                    // (routePlan[].swapInfo.feeAmount) when the outcome carries it,
                    // else the flat config rate.
                    let dex_fee_sol = outcome.route_fee_sol.unwrap_or_else(|| {
                        signal.payload.amount_sol * self.config.strategy.dex_fee_rate
                    });
                    let slippage = self.slippage_estimate(signal, outcome.price_impact_pct);
                    if outcome.price_impact_pct.is_none() {
                        tracing::debug!(
                            trade_uuid = %signal.trade_uuid,
                            expected_slippage_pct = %(slippage.expected_fraction * Decimal::from(100)),
                            "No Jupiter price impact — using estimated slippage for cost tracking"
                        );
                    }
                    let slippage_cost_sol = slippage.expected_cost_sol(signal.payload.amount_sol);

                    // Update trade costs in database
                    if let Err(e) = self
                        .db
                        .update_trade_costs(
                            &signal.trade_uuid,
                            jito_tip,
                            dex_fee_sol,
                            slippage_cost_sol,
                        )
                        .await
                    {
                        tracing::warn!(
                            trade_uuid = %signal.trade_uuid,
                            error = %e,
                            "Failed to update trade costs"
                        );
                    } else {
                        let total_cost = jito_tip + dex_fee_sol + slippage_cost_sol;
                        tracing::debug!(
                            trade_uuid = %signal.trade_uuid,
                            jito_tip = jito_tip.to_f64().unwrap_or(0.0),
                            dex_fee = dex_fee_sol.to_f64().unwrap_or(0.0),
                            slippage = slippage_cost_sol.to_f64().unwrap_or(0.0),
                            total_cost = total_cost.to_f64().unwrap_or(0.0),
                            "Trade costs recorded"
                        );
                    }
                }
                Err(e) => {
                    let failure_count = {
                        let mut state = self.mutable.lock();
                        state.failure_count += 1;
                        state.failure_count
                    };
                    tracing::error!(
                        trade_uuid = %signal.trade_uuid,
                        error = %e,
                        failure_count = failure_count,
                        "Trade execution failed"
                    );

                    // Record costs even for failed trades — Jito tip was still paid
                    if rpc_mode == RpcMode::Jito {
                        let jito_tip = self.calculate_jito_tip(signal).await;
                        if let Err(cost_err) = self
                            .db
                            .update_trade_costs(
                                &signal.trade_uuid,
                                jito_tip,
                                Decimal::ZERO,
                                Decimal::ZERO,
                            )
                            .await
                        {
                            tracing::warn!(
                                trade_uuid = %signal.trade_uuid,
                                error = %cost_err,
                                "Failed to record Jito tip cost for failed trade"
                            );
                        }
                        if self.config.trade_mode == crate::config::TradeMode::Live {
                            if let Some(ref tip_manager) = self.tip_manager {
                                if let Err(tip_err) = tip_manager
                                    .record_tip(
                                        jito_tip,
                                        None, // No signature for failed trades
                                        signal.payload.strategy,
                                        false, // failure
                                    )
                                    .await
                                {
                                    tracing::warn!(
                                        error = %tip_err,
                                        "Failed to record failed tip in TipManager"
                                    );
                                }
                            }
                        }
                    }

                    // [R-H3] Check if we need to switch to fallback.
                    // Use Jito-specific threshold when in Jito mode, otherwise use RPC threshold.
                    // The switch_to_fallback method itself will check disable_fallback and min_failures_before_fallback.
                    let threshold = if rpc_mode == RpcMode::Jito {
                        self.config.jito.min_failures_before_fallback
                    } else {
                        self.config.rpc.max_consecutive_failures
                    };

                    if failure_count >= threshold {
                        self.switch_to_fallback().await;
                    }
                }
            }

            return result;
        }
    }

    /// Check market conditions before executing trades
    /// Returns Ok(()) if conditions are favorable, Err with reason otherwise
    async fn check_market_conditions(
        &self,
        _action: &crate::models::Action,
        token_address: Option<&str>,
    ) -> Result<(), String> {
        // Check 1: SOL price crash (>10% drop in last hour)
        // This requires price history - check if we have sufficient data
        if let Some(ref price_cache) = self.price_cache {
            // Get SOL price history to check for crash
            let sol_mint = crate::constants::mints::SOL;
            let history = price_cache.price_history_read();
            if let Some(sol_history) = history.get(sol_mint) {
                if sol_history.len() >= 2 {
                    let one_hour_ago = Utc::now() - chrono::Duration::hours(1);
                    // Find price from 1 hour ago (or closest)
                    let mut price_1h_ago = None;
                    let mut current_price = None;

                    for (timestamp, price) in sol_history.iter().rev() {
                        if current_price.is_none() {
                            current_price = Some(*price);
                        }
                        if *timestamp <= one_hour_ago && price_1h_ago.is_none() {
                            price_1h_ago = Some(*price);
                            break;
                        }
                    }

                    if let (Some(old_price), Some(new_price)) = (price_1h_ago, current_price) {
                        if old_price > Decimal::ZERO {
                            let drop_percent =
                                ((old_price - new_price) / old_price) * Decimal::from(100);
                            if drop_percent > Decimal::from(10) {
                                return Err(format!(
                                    "SOL price crash detected: {:.2}% drop in last hour ({} -> {})",
                                    drop_percent, old_price, new_price
                                ));
                            }
                        }
                    }
                }
            }
        }

        // Check 2: High volatility (>30% daily volatility)
        if let Some(ref price_cache) = self.price_cache {
            if let Some(volatility) = price_cache.get_sol_volatility() {
                if volatility > 30.0 {
                    return Err(format!(
                        "High market volatility detected: {:.2}% (threshold: 30%)",
                        volatility
                    ));
                }
            }
        }

        // Check 3: Individual token crash (>30% drop in last 15 minutes)
        // A token can crash even when SOL is stable — we must check the
        // specific token's price action before opening a new position.
        if let (Some(token_addr), Some(ref price_cache)) = (token_address, &self.price_cache) {
            let history = price_cache.price_history_read();
            if let Some(token_history) = history.get(token_addr) {
                if token_history.len() >= 2 {
                    let fifteen_min_ago = Utc::now() - chrono::Duration::minutes(15);
                    let mut price_15m_ago = None;
                    let mut current_token_price = None;

                    for (timestamp, price) in token_history.iter().rev() {
                        if current_token_price.is_none() {
                            current_token_price = Some(*price);
                        }
                        if *timestamp <= fifteen_min_ago && price_15m_ago.is_none() {
                            price_15m_ago = Some(*price);
                            break;
                        }
                    }

                    if let (Some(old_price), Some(new_price)) = (price_15m_ago, current_token_price)
                    {
                        if old_price > Decimal::ZERO {
                            let drop_percent =
                                ((old_price - new_price) / old_price) * Decimal::from(100);
                            if drop_percent > Decimal::from(30) {
                                return Err(format!(
                                    "Token price crash detected: {:.2}% drop in last 15 min ({} -> {})",
                                    drop_percent, old_price, new_price
                                ));
                            }
                        }
                    }
                }
            }
        }

        // Note: off-hours position size reduction is applied in engine/mod.rs at execution
        // time (not at webhook receipt), so the reduced amount is already in signal.payload.amount_sol.

        // All checks passed
        Ok(())
    }

    /// Check if we should attempt recovery to primary RPC
    fn should_attempt_recovery(&self) -> bool {
        let state = self.mutable.lock();

        // Only attempt recovery if we're in fallback mode
        if state.rpc_mode != RpcMode::Standard || state.fallback_since.is_none() {
            return false;
        }

        // Check if Jito is configured
        if !self.config.jito.enabled {
            return false;
        }

        let now = Utc::now();

        // Check if enough time has passed since fallback
        if let Some(fallback_time) = state.fallback_since {
            let elapsed = now.signed_duration_since(fallback_time);
            if elapsed < chrono::Duration::from_std(self.recovery_interval).unwrap_or_default() {
                return false;
            }
        }

        // Check if enough time has passed since last recovery attempt
        if let Some(last_attempt) = state.last_recovery_attempt {
            let elapsed = now.signed_duration_since(last_attempt);
            if elapsed < chrono::Duration::from_std(self.recovery_interval).unwrap_or_default() {
                return false;
            }
        }

        true
    }

    /// Attempt to recover to primary RPC
    async fn try_recover_to_primary(&self) {
        self.mutable.lock().last_recovery_attempt = Some(Utc::now());

        tracing::info!("Attempting to recover to primary RPC (Jito)");

        // Perform health check on primary RPC
        match self.check_primary_health().await {
            Ok(health) if health.healthy => {
                tracing::info!(
                    latency_ms = health.latency_ms,
                    "Primary RPC is healthy, switching back to Jito mode"
                );

                {
                    let mut state = self.mutable.lock();
                    state.rpc_mode = RpcMode::Jito;
                    state.fallback_since = None;
                    state.failure_count = 0;
                }

                // Send Jito recovery notification
                if let Some(ref notifier) = self.notifier {
                    notifier
                        .notify(NotificationEvent::JitoRecovered {
                            latency_ms: health.latency_ms.unwrap_or(0),
                        })
                        .await;
                }

                // Log recovery to config audit
                if let Err(e) = self
                    .db
                    .log_config_change(
                        "rpc_mode",
                        Some("JITO"),
                        "STANDARD",
                        "SYSTEM_RECOVERY",
                        Some("Primary RPC recovered, switching back from fallback"),
                    )
                    .await
                {
                    tracing::error!(error = %e, "Failed to log RPC mode recovery");
                }
            }
            Ok(_) => {
                tracing::warn!("Primary RPC health check failed, staying in fallback mode");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Primary RPC health check error, staying in fallback mode");
            }
        }
    }

    /// Check health of primary RPC
    async fn check_primary_health(&self) -> Result<RpcHealth, ExecutorError> {
        self.check_health_impl(&self.config.rpc.primary_url, false)
            .await
    }

    /// Check health of active RPC
    async fn check_active_health(&self) -> Result<RpcHealth, ExecutorError> {
        // active_rpc_url() returns a &str from a lock, we must drop it before await
        let active_url = self.active_rpc_url().to_string();
        self.check_health_impl(&active_url, true).await
    }

    /// Internal health check implementation
    async fn check_health_impl(
        &self,
        url: &str,
        update_cache: bool,
    ) -> Result<RpcHealth, ExecutorError> {
        let start = std::time::Instant::now();

        let health_check = async {
            let response = self
                .http_client
                .post(url)
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "getHealth"
                }))
                .send()
                .await
                .map_err(|e| ExecutorError::Rpc(format!("RPC health check failed: {}", e)))?;

            if !response.status().is_success() {
                return Err(ExecutorError::Rpc(format!(
                    "RPC returned status: {}",
                    response.status()
                )));
            }

            let body: serde_json::Value = response
                .json()
                .await
                .map_err(|e| ExecutorError::Rpc(format!("Failed to parse RPC response: {}", e)))?;

            if body.get("error").is_some() {
                return Err(ExecutorError::Rpc(format!(
                    "RPC returned error: {:?}",
                    body["error"]
                )));
            }

            Ok(())
        };

        let timeout_duration = Duration::from_millis(self.config.rpc.timeout_ms);
        match timeout(timeout_duration, health_check).await {
            Ok(Ok(())) => {
                if self.config.rpc.functional_health_check {
                    let probe_result = self
                        .http_client
                        .post(url)
                        .json(&serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 2,
                            "method": "getLatestBlockhash",
                            "params": [{"commitment": "confirmed"}]
                        }))
                        .timeout(Duration::from_millis(self.config.rpc.timeout_ms))
                        .send()
                        .await;

                    match probe_result {
                        Ok(resp) if resp.status().is_success() => {
                            let body: serde_json::Value = resp.json().await.unwrap_or_default();
                            if body.get("result").is_none() {
                                let health = RpcHealth {
                                    healthy: false,
                                    last_check: Utc::now(),
                                    latency_ms: None,
                                };
                                if update_cache {
                                    *self.latest_rpc_health.write().await = Some(health);
                                }
                                let err = ExecutorError::Rpc(
                                    "RPC functional probe returned error body despite getHealth ok"
                                        .to_string(),
                                );
                                tracing::warn!(error = %err, "RPC functional health probe failed");
                                return Err(err);
                            }
                        }
                        Ok(resp) => {
                            let health = RpcHealth {
                                healthy: false,
                                last_check: Utc::now(),
                                latency_ms: None,
                            };
                            if update_cache {
                                *self.latest_rpc_health.write().await = Some(health);
                            }
                            let err = ExecutorError::Rpc(format!(
                                "RPC functional probe returned HTTP {}",
                                resp.status()
                            ));
                            tracing::warn!(error = %err, "RPC functional health probe failed");
                            return Err(err);
                        }
                        Err(e) => {
                            let health = RpcHealth {
                                healthy: false,
                                last_check: Utc::now(),
                                latency_ms: None,
                            };
                            if update_cache {
                                *self.latest_rpc_health.write().await = Some(health);
                            }
                            let err = ExecutorError::Rpc(format!(
                                "RPC functional probe network error: {}",
                                e
                            ));
                            tracing::warn!(error = %err, "RPC functional health probe failed");
                            return Err(err);
                        }
                    }
                }

                let latency = start.elapsed().as_millis() as u64;
                let health = RpcHealth {
                    healthy: true,
                    last_check: Utc::now(),
                    latency_ms: Some(latency),
                };

                if update_cache {
                    *self.latest_rpc_health.write().await = Some(health.clone());
                }
                tracing::debug!(latency_ms = latency, "RPC health check passed");

                Ok(health)
            }
            Ok(Err(e)) => {
                let health = RpcHealth {
                    healthy: false,
                    last_check: Utc::now(),
                    latency_ms: None,
                };
                if update_cache {
                    *self.latest_rpc_health.write().await = Some(health);
                }
                tracing::warn!(error = %e, "RPC health check failed");
                Err(e)
            }
            Err(_) => {
                let health = RpcHealth {
                    healthy: false,
                    last_check: Utc::now(),
                    latency_ms: None,
                };
                if update_cache {
                    *self.latest_rpc_health.write().await = Some(health);
                }
                tracing::warn!("RPC health check timed out");
                Err(ExecutorError::Timeout)
            }
        }
    }

    /// Get latest RPC health status (non-blocking read)
    pub async fn get_rpc_health(&self) -> Option<RpcHealth> {
        self.latest_rpc_health.read().await.clone()
    }

    /// Check RPC health and update cache (for periodic health checks)
    pub async fn refresh_rpc_health(&self) {
        let _ = self.check_active_health().await;
    }

    /// Check Jito endpoint health via lightweight connectivity test
    ///
    /// Returns JitoHealth with submission metrics and endpoint status.
    /// This is a non-invasive check that doesn't submit real bundles.
    pub async fn check_jito_health(&self) -> Result<JitoHealth, ExecutorError> {
        let start = std::time::Instant::now();

        // Check if Jito client is configured
        let jito_client = self.jito_searcher.as_ref().ok_or_else(|| {
            ExecutorError::TransactionFailed("Jito Searcher client not configured".to_string())
        })?;

        // Attempt a lightweight GET request to the Jito endpoint
        // This checks connectivity without submitting a bundle
        let health_url = format!("{}/health", jito_client.endpoint());

        let response = self
            .http_client
            .get(&health_url)
            .timeout(Duration::from_secs(5))
            .send()
            .await;

        let latency = start.elapsed().as_millis() as u64;

        // Calculate resolution success rate from atomic counters
        let state = self.mutable.lock();
        let total_submissions = state.jito_submissions.load(std::sync::atomic::Ordering::Relaxed);
        let successful_resolutions = state.jito_resolutions_success.load(std::sync::atomic::Ordering::Relaxed);
        let failed_resolutions = state.jito_resolutions_failed.load(std::sync::atomic::Ordering::Relaxed);
        drop(state);

        let resolution_success_rate = if total_submissions > 0 {
            successful_resolutions as f64 / total_submissions as f64
        } else {
            1.0 // No submissions yet, assume healthy
        };

        let health = match response {
            Ok(resp) if resp.status().is_success() => {
                JitoHealth {
                    healthy: true,
                    last_check: Utc::now(),
                    latency_ms: Some(latency),
                    resolution_success_rate,
                    total_submissions,
                    successful_resolutions,
                }
            }
            Ok(resp) => {
                tracing::warn!(
                    status = %resp.status(),
                    "Jito health check returned non-success status"
                );
                JitoHealth {
                    healthy: false,
                    last_check: Utc::now(),
                    latency_ms: Some(latency),
                    resolution_success_rate,
                    total_submissions,
                    successful_resolutions,
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Jito health check failed");
                JitoHealth {
                    healthy: false,
                    last_check: Utc::now(),
                    latency_ms: None,
                    resolution_success_rate,
                    total_submissions,
                    successful_resolutions,
                }
            }
        };

        // Update cached health
        let previous_health = self.mutable.lock().jito_health.clone();
        self.mutable.lock().jito_health = Some(health.clone());

        // Update Prometheus metrics
        self.update_jito_health_metrics(&health);

        // Send notification if health status changed
        if let Some(prev) = previous_health {
            if prev.healthy != health.healthy {
                if let Some(ref notifier) = self.notifier {
                    notifier
                        .notify(NotificationEvent::JitoHealthChanged {
                            healthy: health.healthy,
                            latency_ms: health.latency_ms,
                            success_rate: health.resolution_success_rate,
                        })
                        .await;
                }
            }
        }

        Ok(health)
    }

    /// Get latest Jito health status (non-blocking read)
    pub fn get_jito_health(&self) -> Option<JitoHealth> {
        self.mutable.lock().jito_health.clone()
    }

    /// Record a Jito bundle submission (for health tracking)
    fn record_jito_submission(&self) {
        self.mutable.lock().jito_submissions.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Record a successful Jito bundle resolution
    fn record_jito_resolution_success(&self) {
        self.mutable.lock().jito_resolutions_success.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.record_jito_resolution_metrics("success");
    }

    /// Record a failed Jito bundle resolution
    fn record_jito_resolution_failure(&self) {
        self.mutable.lock().jito_resolutions_failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.record_jito_resolution_metrics("failed");
    }

    /// Get Jito submission statistics
    pub fn get_jito_stats(&self) -> (u64, u64, u64) {
        let state = self.mutable.lock();
        (
            state.jito_submissions.load(std::sync::atomic::Ordering::Relaxed),
            state.jito_resolutions_success.load(std::sync::atomic::Ordering::Relaxed),
            state.jito_resolutions_failed.load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// Record Jito bundle submission to Prometheus metrics
    fn record_jito_submission_metrics(&self, mode: &str) {
        if let Some(ref metrics) = self.metrics {
            metrics
                .jito_submissions
                .with_label_values(&[mode])
                .inc();
        }
    }

    /// Record Jito bundle resolution to Prometheus metrics
    fn record_jito_resolution_metrics(&self, status: &str) {
        if let Some(ref metrics) = self.metrics {
            metrics
                .jito_resolutions
                .with_label_values(&[status])
                .inc();
        }
    }

    /// Record Jito retry to Prometheus metrics
    fn record_jito_retry_metrics(&self, attempt: u32) {
        if let Some(ref metrics) = self.metrics {
            metrics
                .jito_retry_total
                .with_label_values(&[&attempt.to_string()])
                .inc();
        }
    }

    /// Update Jito health gauge from health check
    fn update_jito_health_metrics(&self, health: &JitoHealth) {
        if let Some(ref metrics) = self.metrics {
            metrics
                .jito_health
                .set(if health.healthy { 1 } else { 0 });
        }
    }

    /// Execute via Jito bundle
    async fn execute_jito(&self, signal: &Signal) -> Result<ExecutionOutcome, ExecutorError> {
        // Check if this is an exit trade and Helius Staked Connections are enabled
        let is_exit = signal.payload.action == crate::models::Action::Sell;
        let use_helius_for_exit = is_exit && self.config.jito.helius_staked_exits;

        if use_helius_for_exit {
            tracing::info!(
                trade_uuid = %signal.trade_uuid,
                "Exit trade: using Helius Staked Connections for high landing rate"
            );
            return self.execute_via_helius_staked(signal).await;
        }

        tracing::info!(
            trade_uuid = %signal.trade_uuid,
            "Executing trade via Jito bundle"
        );

        // Skip RPC health check - proceed with transaction
        let active_url = self.active_rpc_url();
        tracing::debug!(rpc_url = %active_url, "Proceeding with Jito trade execution");

        // Load wallet keypair from vault
        let secrets = load_secrets_with_fallback().map_err(|e| {
            ExecutorError::TransactionFailed(format!("Failed to load vault: {}", e))
        })?;
        let wallet_keypair = load_wallet_keypair(&secrets).map_err(|e| {
            ExecutorError::TransactionFailed(format!("Failed to load keypair: {}", e))
        })?;

        // Build transaction (use active RPC client)
        let active_client = self.active_rpc_client();
        let transaction_builder =
            TransactionBuilder::new(active_client.clone(), self.config.clone()).map_err(|e| {
                ExecutorError::TransactionFailed(format!(
                    "Failed to create transaction builder: {}",
                    e
                ))
            })?;
        // F5/F6: unified slippage tolerance drives the on-chain slippageBps.
        let pre_slippage = self.slippage_estimate(signal, None);
        let built_tx = transaction_builder
            .build_swap_transaction(signal, &wallet_keypair, pre_slippage.tolerance_bps)
            .await
            .map_err(|e| {
                ExecutorError::TransactionFailed(format!("Failed to build transaction: {}", e))
            })?;

        // Capture actual price impact and fill price from Jupiter quote.
        // [B-M1] Token decimals are now populated at webhook time from on-chain metadata,
        // enabling correct SOL-per-token conversion without hardcoding 9 decimals.
        // F16/P1-13: unknown decimals now surface as a sentinel rather than ZERO.
        let price_impact = built_tx.price_impact_pct();
        let fill_price_sol = built_tx
            .fill_price_lamports_per_base()
            .and_then(|lpb| lamports_per_base_to_sol_per_token(lpb, signal.token_decimals));

        // Calculate dynamic tip
        let tip = self.calculate_jito_tip(signal).await;

        // Check total execution cost cap (uses the unified estimate now)
        self.check_execution_costs(
            signal,
            built_tx.price_impact_pct(),
            tip,
            built_tx.route_fee_sol(),
        ).await?;

        tracing::debug!(
            tip_sol = tip.to_f64().unwrap_or(0.0),
            strategy = %signal.payload.strategy,
            "Calculated Jito tip"
        );

        // Submit to Jito via direct Jito Searcher (preferred) or Helius Sender API (fallback)

        // Try direct Jito Searcher first if configured.
        if let Some(ref jito_searcher) = self.jito_searcher {
            // Cap tip at 1 SOL before lamport conversion to avoid u64 overflow.
            let capped_tip = tip.min(Decimal::ONE);
            if capped_tip < tip {
                tracing::warn!(
                    original_tip = %tip,
                    capped_tip = %capped_tip,
                    "Jito tip capped at 1 SOL to prevent u64 overflow"
                );
            }
                        let tip_lamports = utils::sol_to_lamports(capped_tip).map_err(|e| {
                            ExecutorError::TransactionFailed(format!("Failed to convert tip to lamports: {}", e))
                        })?;

            // D3: legacy transactions inline the tip as the last instruction and
            // ship as a single-tx bundle (one signature, atomic at tx level). V0
            // cannot be safely inlined without ALT reconstruction, so it keeps the
            // separate-tip two-tx bundle (still atomic at the bundle level).
            let submit_result = match &built_tx {
                crate::engine::transaction_builder::BuiltTransaction::Legacy {
                    transaction,
                    blockhash,
                    ..
                } => match self.inline_and_serialize_tip(
                    transaction,
                    &wallet_keypair,
                    tip_lamports,
                    *blockhash,
                ) {
                    Ok(bytes) => {
                        tracing::info!(
                            trade_uuid = %signal.trade_uuid,
                            "Jito tip inlined into legacy swap tx (single-tx bundle)"
                        );
                        jito_searcher.submit_single_bundle(&bytes).await
                    }
                    Err(e) => {
                        tracing::warn!(
                            trade_uuid = %signal.trade_uuid,
                            error = %e,
                            "Could not inline Jito tip into legacy tx; falling back to separate-tip bundle"
                        );
                        let bytes = bincode::serde::encode_to_vec(transaction, bincode::config::legacy())
                            .map_err(|e| {
                                ExecutorError::TransactionFailed(format!(
                                    "Serialization error: {}",
                                    e
                                ))
                            })?;
                        jito_searcher.submit_bundle(&bytes, tip_lamports, &wallet_keypair).await
                    }
                }
                crate::engine::transaction_builder::BuiltTransaction::Versioned {
                    transaction_bytes,
                    ..
                } => {
                    // V0: already serialized+signed by the builder; separate tip.
                    jito_searcher
                        .submit_bundle(transaction_bytes, tip_lamports, &wallet_keypair)
                        .await
                }
            };

            match submit_result {
                Ok(bundle_ref) => {
                    // Track bundle submission for health monitoring
                    self.record_jito_submission();
                    self.record_jito_submission_metrics("jito");

                    tracing::info!(
                        trade_uuid = %signal.trade_uuid,
                        bundle_ref = %bundle_ref,
                        "Bundle submitted via direct Jito Searcher"
                    );
                    // F12: sendBundle returns a UUID, not a signature. Resolve
                    // it to the real landed tx signature before polling; if it
                    // can't be resolved, mark unconfirmed and let recovery
                    // reconcile — never poll the UUID as a signature.
                    let (signature, confirmed) = if let Some(bundle_id) =
                        bundle_ref.strip_prefix("bundle:")
                    {
                        match jito_searcher.resolve_bundle_signature(bundle_id).await {
                            Some(sig) => {
                                // Track successful resolution
                                self.record_jito_resolution_success();

                                let confirmed = self
                                    .poll_signature_confirmation(&sig, &signal.trade_uuid)
                                    .await?;
                                (sig, confirmed)
                            }
                            None => {
                                // Track failed resolution
                                self.record_jito_resolution_failure();

                                tracing::warn!(
                                    trade_uuid = %signal.trade_uuid,
                                    bundle_id,
                                    "Could not resolve Jito bundle to a signature; marking unconfirmed for recovery"
                                );
                                (bundle_ref.clone(), false)
                            }
                        }
                    } else {
                        // Defensive: not a bundle ref (unexpected); poll as-is.
                        let confirmed = self
                            .poll_signature_confirmation(&bundle_ref, &signal.trade_uuid)
                            .await?;
                        (bundle_ref, confirmed)
                    };
                    return Ok(ExecutionOutcome::live(
                        signature,
                        confirmed,
                        fill_price_sol,
                        price_impact,
                        built_tx.route_fee_sol(),
                    ));
                }
                Err(e) => {
                    tracing::warn!(
                        trade_uuid = %signal.trade_uuid,
                        error = %e,
                        "Direct Jito Searcher failed, trying Helius fallback"
                    );
                    // Fall through to Helius fallback if configured
                }
            }
        }

        // Fallback to Helius Sender API if configured and enabled.
        // F11: Helius Sender only supports LEGACY transactions — short-circuit
        // V0 here (skip straight to TPU) instead of passing V0 bytes that fail
        // silently in the bundle.
        if self.config.jito.helius_fallback {
            if let Some(helius_api_key) = secrets.rpc_api_key.as_ref() {
                let helius_attempt = match &built_tx {
                    crate::engine::transaction_builder::BuiltTransaction::Legacy {
                        transaction,
                        blockhash,
                        ..
                    } => {
                        // D3: inline the tip into the legacy swap tx and ship a
                        // single-tx bundle via Helius (one signature, atomic).
                        let capped_tip = tip.min(Decimal::ONE);
            let tip_lamports = utils::sol_to_lamports(capped_tip).map_err(|e| {
                ExecutorError::TransactionFailed(format!("Failed to convert tip to lamports: {}", e))
            })?;
                        match self.inline_and_serialize_tip(
                            transaction,
                            &wallet_keypair,
                            tip_lamports,
                            *blockhash,
                        ) {
                            Ok(bytes) => Some(
                                self.submit_via_helius_single_bundle(&bytes, helius_api_key)
                                    .await,
                            ),
                            Err(e) => {
                                tracing::warn!(
                                    trade_uuid = %signal.trade_uuid,
                                    error = %e,
                                    "Could not inline tip for Helius; falling through to TPU"
                                );
                                None
                            }
                        }
                    }
                    crate::engine::transaction_builder::BuiltTransaction::Versioned { .. } => {
                        tracing::info!(
                            trade_uuid = %signal.trade_uuid,
                            "Skipping Helius Sender (V0 tx) — Helius is legacy-only; going to TPU"
                        );
                        None
                    }
                };

                if let Some(Ok(bundle_ref)) = helius_attempt {
                    tracing::info!(
                        trade_uuid = %signal.trade_uuid,
                        bundle_ref = %bundle_ref,
                        "Bundle submitted via Helius Sender API"
                    );
                    // Track bundle submission (Helius still uses Jito bundles)
                    self.record_jito_submission();
                    self.record_jito_submission_metrics("helius");

                    // F12: resolve the Helius bundle UUID to the real tx
                    // signature before polling; never poll the UUID itself.
                    let (signature, confirmed) = if let Some(bundle_id) =
                        bundle_ref.strip_prefix("bundle:")
                    {
                        match self
                            .resolve_helius_bundle_signature(bundle_id, helius_api_key)
                            .await
                        {
                            Some(sig) => {
                                // Track successful resolution
                                self.record_jito_resolution_success();

                                let confirmed = self
                                    .poll_signature_confirmation(&sig, &signal.trade_uuid)
                                    .await?;
                                (sig, confirmed)
                            }
                            None => {
                                // Track failed resolution
                                self.record_jito_resolution_failure();

                                tracing::warn!(
                                    trade_uuid = %signal.trade_uuid,
                                    bundle_id,
                                    "Could not resolve Helius bundle to a signature; marking unconfirmed for recovery"
                                );
                                (bundle_ref.clone(), false)
                            }
                        }
                    } else {
                        let confirmed = self
                            .poll_signature_confirmation(&bundle_ref, &signal.trade_uuid)
                            .await?;
                        (bundle_ref, confirmed)
                    };
                    return Ok(ExecutionOutcome::live(
                        signature,
                        confirmed,
                        fill_price_sol,
                        price_impact,
                        built_tx.route_fee_sol(),
                    ));
                }
            }
        }

        // Final fallback: Submit via standard TPU
        tracing::warn!(
            trade_uuid = %signal.trade_uuid,
            "Jito bundle submission failed, falling back to standard TPU"
        );

        // Sign and send transaction via standard RPC (handles both legacy and versioned)
        // F13/P1-10: poll for confirmation on the legacy path instead of
        // assuming `confirmed = true` (which broke tracking when the tx hadn't
        // actually landed).
        let (signature, confirmed) = match &built_tx {
            crate::engine::transaction_builder::BuiltTransaction::Legacy {
                transaction, ..
            } => {
                let sig = self
                    .submit_transaction(transaction, &wallet_keypair)
                    .await?;
                let confirmed = self
                    .poll_signature_confirmation(&sig, &signal.trade_uuid)
                    .await?;
                (sig, confirmed)
            }
            crate::engine::transaction_builder::BuiltTransaction::Versioned {
                transaction_bytes,
                ..
            } => {
                let sig = self
                    .submit_versioned_transaction(
                        transaction_bytes,
                        &wallet_keypair,
                        built_tx.blockhash(),
                    )
                    .await?;
                let confirmed = self
                    .poll_signature_confirmation(&sig, &signal.trade_uuid)
                    .await?;
                (sig, confirmed)
            }
        };

        Ok(ExecutionOutcome::live(
            signature,
            confirmed,
            fill_price_sol,
            price_impact,
            built_tx.route_fee_sol(),
        ))
    }

    /// Inline a Jito tip into a legacy swap tx, re-sign it, and serialize.
    /// Shared by the direct-Jito and Helius legacy submission paths so they
    /// cannot diverge on the inline→sign→serialize sequence. Returns signed
    /// bytes ready for a single-tx bundle.
    fn inline_and_serialize_tip(
        &self,
        transaction: &Transaction,
        wallet_keypair: &solana_sdk::signature::Keypair,
        tip_lamports: u64,
        blockhash: solana_sdk::hash::Hash,
    ) -> Result<Vec<u8>, ExecutorError> {
        use crate::engine::tip_inlining;
        use solana_sdk::signature::Signer;

        let tipped = tip_inlining::inline_jito_tip(
            transaction,
            &wallet_keypair.pubkey(),
            &crate::engine::jito_searcher::next_tip_account(),
            tip_lamports,
            blockhash,
        )
        .map_err(|e| {
            ExecutorError::TransactionFailed(format!("Failed to inline Jito tip: {}", e))
        })?;
        let mut tipped = tipped;
        tipped.sign(&[wallet_keypair], blockhash);
        bincode::serde::encode_to_vec(&tipped, bincode::config::legacy()).map_err(|e| {
            ExecutorError::TransactionFailed(format!("Failed to serialize tipped tx: {}", e))
        })
    }

    /// Submit a **single-transaction** bundle via Helius Sender (D3).
    ///
    /// `tipped_tx_bytes` is the swap transaction with the Jito tip already
    /// inlined as its last instruction (see `engine::tip_inlining`). Ships as a
    /// one-element bundle — one signature, atomic at the transaction level.
    /// Returns a `bundle:<uuid>` ref the caller resolves via
    /// [`Self::resolve_helius_bundle_signature`] before polling (F12).
    async fn submit_via_helius_single_bundle(
        &self,
        tipped_tx_bytes: &[u8],
        api_key: &str,
    ) -> Result<String, ExecutorError> {
        // JSON-RPC `sendBundle` at the Solana RPC host (per Helius docs) so that
        // submit and getBundleStatuses share one namespace/endpoint. The body is
        // the same JSON-RPC shape as the direct-Jito sendBundle.
        let url = crate::utils::helius_rpc_url(api_key);
        let tx_base64 = BASE64.encode(tipped_tx_bytes);
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [[tx_base64]]
        });

        let response = self
            .http_client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ExecutorError::Rpc(format!("Helius Sender request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            // Attempt to extract error text, but log if extraction fails
            let error_text = match response.text().await {
                Ok(text) => text,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to extract error text from Helius response"
                    );
                    format!("Failed to extract error text: {}", e)
                }
            };
            return Err(ExecutorError::Rpc(format!(
                "Helius Sender API error: {} - {}",
                status, error_text
            )));
        }

        let result: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ExecutorError::Rpc(format!("Failed to parse Helius response: {}", e)))?;

        // F12: sendBundle returns a bundle UUID (`result` string), never a
        // signature. Tag it so the caller resolves it via getBundleStatuses.
        let bundle_id = result
            .get("result")
            .and_then(|v| v.as_str())
            .or_else(|| result.get("bundleId").and_then(|v| v.as_str()))
            .ok_or_else(|| ExecutorError::Rpc("No result/bundleId in Helius response".to_string()))?;

        Ok(format!("bundle:{}", bundle_id))
    }

    /// Resolve a Helius bundle UUID to its real landed SWAP transaction signature
    /// via the shared `getBundleStatuses` resolver. Returns `None` if unresolved
    /// (caller marks unconfirmed for recovery — never polls the UUID as a signature).
    async fn resolve_helius_bundle_signature(
        &self,
        bundle_id: &str,
        api_key: &str,
    ) -> Option<String> {
        // getBundleStatuses is served at the Solana RPC host (per Helius docs),
        // NOT at api.helius.xyz/v0/bundles.
        let url = crate::utils::helius_rpc_url(api_key);
        crate::engine::jito_searcher::resolve_bundle_status(&self.http_client, &url, bundle_id).await
    }

    /// Execute via Helius RPC with Staked Connections (high landing rate for exits)
    async fn execute_via_helius_staked(
        &self,
        signal: &Signal,
    ) -> Result<ExecutionOutcome, ExecutorError> {
        tracing::info!(
            trade_uuid = %signal.trade_uuid,
            "Executing exit trade via Helius Staked Connections"
        );

        // Load wallet keypair from vault
        let secrets = load_secrets_with_fallback().map_err(|e| {
            ExecutorError::TransactionFailed(format!("Failed to load vault: {}", e))
        })?;
        let wallet_keypair = load_wallet_keypair(&secrets).map_err(|e| {
            ExecutorError::TransactionFailed(format!("Failed to load keypair: {}", e))
        })?;

        // Build transaction (use active RPC client)
        let active_client = self.active_rpc_client();
        let transaction_builder =
            TransactionBuilder::new(active_client.clone(), self.config.clone()).map_err(|e| {
                ExecutorError::TransactionFailed(format!(
                    "Failed to create transaction builder: {}",
                    e
                ))
            })?;

        let pre_slippage = self.slippage_estimate(signal, None);
        let built_tx = transaction_builder
            .build_swap_transaction(signal, &wallet_keypair, pre_slippage.tolerance_bps)
            .await
            .map_err(|e| {
                ExecutorError::TransactionFailed(format!("Failed to build transaction: {}", e))
            })?;

        // Capture actual price impact and fill price from Jupiter quote
        let price_impact = built_tx.price_impact_pct();
        let fill_price_sol = built_tx
            .fill_price_lamports_per_base()
            .and_then(|lpb| lamports_per_base_to_sol_per_token(lpb, signal.token_decimals));

        // Calculate dynamic tip (for cost tracking, though Helius uses priority fees)
        let tip = self.calculate_jito_tip(signal).await;

        // Check total execution cost cap
        self.check_execution_costs(
            signal,
            built_tx.price_impact_pct(),
            tip,
            built_tx.route_fee_sol(),
        ).await?;

        tracing::debug!(
            tip_sol = tip.to_f64().unwrap_or(0.0),
            strategy = %signal.payload.strategy,
            "Calculated tip for cost tracking (Helius uses priority fees)"
        );

        // Submit via Helius RPC with staked connection prioritization
        let (signature, confirmed) = match &built_tx {
            crate::engine::transaction_builder::BuiltTransaction::Legacy {
                transaction,
                ..
            } => {
                let sig = self
                    .submit_transaction_helius_staked(transaction, &wallet_keypair)
                    .await?;
                let confirmed = self
                    .poll_signature_confirmation(&sig, &signal.trade_uuid)
                    .await?;
                (sig, confirmed)
            }
            crate::engine::transaction_builder::BuiltTransaction::Versioned {
                transaction_bytes,
                ..
            } => {
                let sig = self
                    .submit_versioned_transaction_helius_staked(
                        transaction_bytes,
                        &wallet_keypair,
                        built_tx.blockhash(),
                    )
                    .await?;
                let confirmed = self
                    .poll_signature_confirmation(&sig, &signal.trade_uuid)
                    .await?;
                (sig, confirmed)
            }
        };

        // Record costs (tip is tracked but Helius uses priority fees)
        let dex_fee_sol = built_tx.route_fee_sol().unwrap_or_else(|| {
            signal.payload.amount_sol * self.config.strategy.dex_fee_rate
        });
        let slippage = self.slippage_estimate(signal, built_tx.price_impact_pct());
        let slippage_cost_sol = slippage.expected_cost_sol(signal.payload.amount_sol);

        if let Err(e) = self
            .db
            .update_trade_costs(
                &signal.trade_uuid,
                tip, // Track tip for consistency, though Helius uses priority fees
                dex_fee_sol,
                slippage_cost_sol,
            )
            .await
        {
            tracing::warn!(
                trade_uuid = %signal.trade_uuid,
                error = %e,
                "Failed to update trade costs for Helius exit"
            );
        }

        Ok(ExecutionOutcome::live(
            signature,
            confirmed,
            fill_price_sol,
            price_impact,
            built_tx.route_fee_sol(),
        ))
    }

    /// Submit transaction via Helius RPC with staked connection prioritization
    async fn submit_transaction_helius_staked(
        &self,
        transaction: &Transaction,
        keypair: &solana_sdk::signature::Keypair,
    ) -> Result<String, ExecutorError> {
        // Get Helius API key from vault for staked connection
        let secrets = load_secrets_with_fallback().map_err(|e| {
            ExecutorError::TransactionFailed(format!("Failed to load vault: {}", e))
        })?;

        let helius_api_key = secrets.rpc_api_key.as_ref().ok_or_else(|| {
            ExecutorError::TransactionFailed(
                "Helius API key not found in vault for staked connection".to_string(),
            )
        })?;

        let helius_url = crate::utils::helius_rpc_url(helius_api_key);

        let tx_bytes = bincode::serde::encode_to_vec(transaction, bincode::config::legacy())
            .map_err(|e| ExecutorError::TransactionFailed(format!("Serialization error: {}", e)))?;

        self.validate_transaction_size(&tx_bytes)?;

        let tx_base64 = BASE64.encode(&tx_bytes);

        // Submit with prioritization fee for staked connection priority
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [
                tx_base64,
                {
                    "encoding": "base64",
                    "skipPreflight": false,
                    "maxRetries": 3,
                    "prioritizationFee": 10000  // 10000 lamports priority fee for staked connection
                }
            ]
        });

        let rpc_timeout = Duration::from_secs(30);

        let response = timeout(
            rpc_timeout,
            self.http_client.post(&helius_url).json(&payload).send(),
        )
        .await
        .map_err(|_| {
            ExecutorError::TransactionFailed(
                "Helius staked connection submission timed out after 30s".to_string(),
            )
        })?
        .map_err(|e| {
            ExecutorError::Rpc(format!("Helius staked connection request failed: {}", e))
        })?;

        let result: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ExecutorError::Rpc(format!("Failed to parse Helius response: {}", e)))?;

        let signature = result
            .get("result")
            .and_then(|r| r.as_str())
            .ok_or_else(|| ExecutorError::Rpc("No signature in Helius response".to_string()))?;

        tracing::info!(
            signature = %signature,
            "Transaction submitted via Helius Staked Connection"
        );

        Ok(signature.to_string())
    }

    /// Submit versioned transaction via Helius RPC with staked connection prioritization
    async fn submit_versioned_transaction_helius_staked(
        &self,
        transaction_bytes: &[u8],
        wallet_keypair: &solana_sdk::signature::Keypair,
        recent_blockhash: solana_sdk::hash::Hash,
    ) -> Result<String, ExecutorError> {
        tracing::debug!("Starting VersionedTransaction submission via Helius Staked Connection");

        // Validate transaction size
        self.validate_transaction_size(transaction_bytes)?;

        // Parse the versioned transaction
        let versioned_tx: VersionedTransaction =
            bincode::serde::decode_from_slice(transaction_bytes, bincode::config::legacy())
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to deserialize versioned transaction");
                    ExecutorError::TransactionFailed(format!(
                        "Failed to deserialize versioned transaction: {}",
                        e
                    ))
                })?
                .0;

        // Get Helius API key from vault
        let secrets = load_secrets_with_fallback().map_err(|e| {
            ExecutorError::TransactionFailed(format!("Failed to load vault: {}", e))
        })?;

        let helius_api_key = secrets.rpc_api_key.as_ref().ok_or_else(|| {
            ExecutorError::TransactionFailed(
                "Helius API key not found in vault for staked connection".to_string(),
            )
        })?;

        let helius_url = crate::utils::helius_rpc_url(helius_api_key);

        // Update the message's recent_blockhash if needed (reuse V0 reconstruction logic)
        use solana_sdk::message::VersionedMessage;
        use solana_sdk::signature::Signer;

        let updated_message = match &versioned_tx.message {
            VersionedMessage::Legacy(legacy_msg) => {
                let mut new_msg = legacy_msg.clone();
                new_msg.recent_blockhash = recent_blockhash;
                VersionedMessage::Legacy(new_msg)
            }
            VersionedMessage::V0(_) => {
                // V0 blockhash already refreshed in builder, use as-is
                versioned_tx.message.clone()
            }
        };

        // Get the message hash and sign
        let message_hash = updated_message.hash();
        let signature = wallet_keypair
            .try_sign_message(&message_hash.to_bytes())
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to sign message");
                ExecutorError::TransactionFailed(format!("Failed to sign message: {}", e))
            })?;

        // Create new transaction with updated signature
        let mut new_signatures = versioned_tx.signatures.clone();
        if new_signatures.is_empty() {
            new_signatures.push(signature);
        } else {
            new_signatures[0] = signature;
        }

        let signed_tx = VersionedTransaction {
            signatures: new_signatures,
            message: updated_message,
        };

        // Serialize the signed transaction
        let signed_bytes =
            bincode::serde::encode_to_vec(&signed_tx, bincode::config::legacy()).map_err(|e| {
                ExecutorError::TransactionFailed(format!(
                    "Failed to serialize versioned transaction: {}",
                    e
                ))
            })?;

        self.validate_transaction_size(&signed_bytes)?;

        let tx_base64 = BASE64.encode(&signed_bytes);

        // Submit via Helius with prioritization fee
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [
                tx_base64,
                {
                    "encoding": "base64",
                    "skipPreflight": false,
                    "maxRetries": 3,
                    "prioritizationFee": 10000
                }
            ]
        });

        let rpc_timeout = Duration::from_secs(30);

        let response = timeout(
            rpc_timeout,
            self.http_client.post(&helius_url).json(&payload).send(),
        )
        .await
        .map_err(|_| {
            ExecutorError::TransactionFailed(
                "Helius staked connection submission timed out after 30s".to_string(),
            )
        })?
        .map_err(|e| {
            ExecutorError::Rpc(format!("Helius staked connection request failed: {}", e))
        })?;

        let result: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ExecutorError::Rpc(format!("Failed to parse Helius response: {}", e)))?;

        let sig = result
            .get("result")
            .and_then(|r| r.as_str())
            .ok_or_else(|| ExecutorError::Rpc("No signature in Helius response".to_string()))?;

        tracing::info!(
            signature = %sig,
            "Versioned transaction submitted via Helius Staked Connection"
        );

        Ok(sig.to_string())
    }

    /// Execute via standard TPU
    async fn execute_standard(&self, signal: &Signal) -> Result<ExecutionOutcome, ExecutorError> {
        tracing::info!(
            trade_uuid = %signal.trade_uuid,
            "Executing trade via standard TPU"
        );

        // Skip RPC health check for devnet - just proceed with transaction
        // The actual transaction submission will fail if RPC is unavailable
        let active_url = self.active_rpc_url();
        tracing::debug!(rpc_url = %active_url, "Proceeding with trade execution");

        // Load wallet keypair from vault
        let secrets = load_secrets_with_fallback().map_err(|e| {
            ExecutorError::TransactionFailed(format!("Failed to load vault: {}", e))
        })?;
        let wallet_keypair = load_wallet_keypair(&secrets).map_err(|e| {
            ExecutorError::TransactionFailed(format!("Failed to load keypair: {}", e))
        })?;

        // Build transaction (use active RPC client)
        let active_client = self.active_rpc_client();
        let transaction_builder =
            TransactionBuilder::new(active_client.clone(), self.config.clone()).map_err(|e| {
                ExecutorError::TransactionFailed(format!(
                    "Failed to create transaction builder: {}",
                    e
                ))
            })?;
        let pre_slippage = self.slippage_estimate(signal, None);
        let built_tx = transaction_builder
            .build_swap_transaction(signal, &wallet_keypair, pre_slippage.tolerance_bps)
            .await
            .map_err(|e| {
                ExecutorError::TransactionFailed(format!("Failed to build transaction: {}", e))
            })?;

        // Capture actual price impact and fill price from Jupiter quote.
        // [B-M1] Also capture token decimals for correct SOL-per-token conversion.
        // F16/P1-13: unknown decimals surface as a sentinel rather than ZERO.
        let price_impact = built_tx.price_impact_pct();
        let fill_price_sol = built_tx
            .fill_price_lamports_per_base()
            .and_then(|lpb| lamports_per_base_to_sol_per_token(lpb, signal.token_decimals));

        // Check total execution cost cap
        self.check_execution_costs(
            signal,
            built_tx.price_impact_pct(),
            Decimal::ZERO,
            built_tx.route_fee_sol(),
        ).await?;

        // Submit transaction via RPC
        let (signature, confirmed) = match &built_tx {
            crate::engine::transaction_builder::BuiltTransaction::Legacy {
                transaction, ..
            } => {
                let sig = self
                    .submit_transaction(transaction, &wallet_keypair)
                    .await?;
                (sig, true)
            }
            crate::engine::transaction_builder::BuiltTransaction::Versioned {
                transaction_bytes,
                ..
            } => {
                let sig = self
                    .submit_versioned_transaction(
                        transaction_bytes,
                        &wallet_keypair,
                        built_tx.blockhash(),
                    )
                    .await?;
                let confirmed = self
                    .poll_signature_confirmation(&sig, &signal.trade_uuid)
                    .await?;
                (sig, confirmed)
            }
        };

        Ok(ExecutionOutcome::live(
            signature,
            confirmed,
            fill_price_sol,
            price_impact,
            built_tx.route_fee_sol(),
        ))
    }

    /// Validate transaction size before submission
    fn validate_transaction_size(&self, tx_bytes: &[u8]) -> Result<(), ExecutorError> {
        if tx_bytes.len() > MAX_TX_SIZE_RAW {
            tracing::error!(
                actual_size = tx_bytes.len(),
                max_size = MAX_TX_SIZE_RAW,
                "Transaction size exceeds Solana limit"
            );
            return Err(ExecutorError::TransactionTooLarge {
                actual: tx_bytes.len(),
                max: MAX_TX_SIZE_RAW,
            });
        }
        Ok(())
    }

    /// Submit a signed transaction to the RPC
    async fn submit_transaction(
        &self,
        transaction: &Transaction,
        _keypair: &solana_sdk::signature::Keypair,
    ) -> Result<String, ExecutorError> {
        // Ensure transaction is signed
        // Note: TransactionBuilder should have already signed it, but we verify here

        // Validate transaction size before submission
        let tx_bytes = bincode::serde::encode_to_vec(transaction, bincode::config::legacy())
            .map_err(|e| {
                ExecutorError::TransactionFailed(format!("Failed to serialize transaction: {}", e))
            })?;
        self.validate_transaction_size(&tx_bytes)?;

        // Send transaction via RPC (use active RPC client)
        // 60-second timeout mirrors the versioned path; without it a stuck RPC node
        // can block this Tokio task indefinitely and freeze the position in EXECUTING.
        let active_client = self.active_rpc_client();
        let signature = timeout(
            Duration::from_secs(60),
            active_client.send_and_confirm_transaction(transaction),
        )
        .await
        .map_err(|_| {
            ExecutorError::TransactionFailed(
                "Legacy transaction confirmation timed out after 60s".to_string(),
            )
        })?
        .map_err(|e| {
            ExecutorError::TransactionFailed(format!("Transaction submission failed: {}", e))
        })?;

        tracing::info!(
            signature = %signature,
            "Transaction submitted successfully"
        );

        Ok(signature.to_string())
    }

    /// Submit a versioned transaction to the RPC
    /// Properly signs the VersionedTransaction with an updated blockhash.
    ///
    /// `recent_blockhash` is threaded in from the build step (P1-11): the
    /// builder already refreshed the V0 message's blockhash, so we reuse it
    /// here instead of issuing extra `getLatestBlockhash` / `is_blockhash_valid`
    /// RPCs per swap. A hard blockhash expiry is still caught after submission
    /// (RPC error -32004 → `BlockhashExpired`).
    async fn submit_versioned_transaction(
        &self,
        transaction_bytes: &[u8],
        wallet_keypair: &solana_sdk::signature::Keypair,
        recent_blockhash: solana_sdk::hash::Hash,
    ) -> Result<String, ExecutorError> {
        tracing::debug!("Starting VersionedTransaction signing and submission");

        // Validate transaction size before processing
        self.validate_transaction_size(transaction_bytes)?;

        // Parse the versioned transaction using legacy bincode format for Solana compatibility
        let versioned_tx: VersionedTransaction =
            bincode::serde::decode_from_slice(transaction_bytes, bincode::config::legacy())
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to deserialize versioned transaction");
                    ExecutorError::TransactionFailed(format!(
                        "Failed to deserialize versioned transaction: {}",
                        e
                    ))
                })?
                .0;

        tracing::debug!("Parsed VersionedTransaction successfully");

        // P1-11: skip the per-submit `is_blockhash_valid` + `getLatestBlockhash`
        // RPCs. The blockhash threaded in from the build step is already fresh
        // (the builder refreshed the V0 message). A hard expiry is still caught
        // post-submission via RPC error -32004 → `BlockhashExpired` below.
        //
        // The transaction from Jupiter is unsigned, so we need to:
        // 1. Update the message's recent_blockhash (if needed)
        // 2. Sign the message hash with our keypair
        // 3. Add the signature to the transaction

        use crate::engine::v0_reconstruction;
        use solana_sdk::message::VersionedMessage;
        use solana_sdk::signature::Signer;

        // Update the message's recent_blockhash
        // For VersionedMessage, we need to handle V0 and Legacy differently
        let updated_message = match &versioned_tx.message {
            VersionedMessage::V0(_v0_msg) => {
                // Check if V0 transactions should be rejected
                if self.config.jupiter.reject_v0_transactions {
                    tracing::error!("V0 transaction rejected due to configuration (reject_v0_transactions=true)");
                    return Err(ExecutorError::TransactionFailed(
                        "V0 transactions are disabled by configuration".to_string(),
                    ));
                }

                // V0 messages use Address Lookup Tables (ALTs). Refreshing the
                // blockhash is a direct public-field swap on a clone (F10) — no
                // per-ALT RPC fetch or message recompilation.
                if self.config.jupiter.reconstruct_v0_on_blockhash_expiry {
                    tracing::debug!(
                        "V0 transaction detected: refreshing message blockhash field"
                    );

                    match v0_reconstruction::refresh_v0_blockhash(
                        &versioned_tx,
                        recent_blockhash,
                    ) {
                        Ok(refreshed) => {
                            tracing::debug!("Refreshed V0 message blockhash");
                            refreshed
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "Failed to refresh V0 blockhash, using original message. \
                                 Transaction may fail if blockhash is stale."
                            );
                            versioned_tx.message.clone()
                        }
                    }
                } else {
                    tracing::debug!(
                        "V0 transaction detected: refresh disabled, using original message"
                    );
                    // Reconstruction disabled - use original message
                    versioned_tx.message.clone()
                }
            }
            VersionedMessage::Legacy(legacy_msg) => {
                // For legacy message, update blockhash
                let mut new_msg = legacy_msg.clone();
                new_msg.recent_blockhash = recent_blockhash;
                VersionedMessage::Legacy(new_msg)
            }
        };

        // Get the message hash that needs to be signed (with updated blockhash)
        let message_hash = updated_message.hash();

        // Sign the message hash with our keypair
        // Use try_sign_message which is available on Signer trait
        tracing::debug!("Signing message hash");
        let signature = wallet_keypair
            .try_sign_message(&message_hash.to_bytes())
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to sign message");
                ExecutorError::TransactionFailed(format!("Failed to sign message: {}", e))
            })?;

        tracing::debug!(signature = %signature, "Message signed successfully");

        // Create a new transaction with our signature
        // The transaction from Jupiter may have placeholder signatures
        // We need to replace the first signature (or add it if empty)
        let mut new_signatures = versioned_tx.signatures.clone();
        if new_signatures.is_empty() {
            new_signatures.push(signature);
        } else {
            // Replace the first signature (assuming it's a placeholder for our keypair)
            new_signatures[0] = signature;
        }

        // Create signed transaction with updated message and signature
        let signed_tx = VersionedTransaction {
            signatures: new_signatures,
            message: updated_message,
        };

        tracing::debug!("Created signed VersionedTransaction");

        // Serialize the signed transaction with the unified bincode 2.x serde
        // API using the legacy config (identical wire format to bincode 1.3).
        let signed_bytes =
            bincode::serde::encode_to_vec(&signed_tx, bincode::config::legacy()).map_err(|e| {
                ExecutorError::TransactionFailed(format!(
                    "Failed to serialize versioned transaction: {}",
                    e
                ))
            })?;

        // Validate signed transaction size before submission
        self.validate_transaction_size(&signed_bytes)?;

        let tx_base64 = BASE64.encode(&signed_bytes);

        tracing::debug!(
            tx_base64_len = tx_base64.len(),
            "Serialized transaction, submitting to RPC"
        );

        // Use direct HTTP POST with reqwest for proper timeout control
        // The RPC client's send() method doesn't respect timeouts properly
        let rpc_timeout = Duration::from_secs(30); // 30 second timeout for transaction submission

        // Construct RPC request payload
        let rpc_payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [
                tx_base64,
                {
                    "encoding": "base64",
                    "maxRetries": 3,
                }
            ]
        });

        let active_url = self.active_rpc_url();
        tracing::debug!(rpc_url = %active_url, "Submitting transaction via direct HTTP");

        // Submit via direct HTTP POST with proper timeout (use active RPC URL)
        let response_result = timeout(
            rpc_timeout,
            self.http_client.post(active_url).json(&rpc_payload).send(),
        )
        .await;

        let response = match response_result {
            Ok(Ok(resp)) => resp.json::<serde_json::Value>().await.map_err(|e| {
                tracing::error!(error = %e, "Failed to parse RPC response");
                ExecutorError::TransactionFailed(format!("Failed to parse RPC response: {}", e))
            })?,
            Ok(Err(e)) => {
                tracing::error!(error = %e, "RPC HTTP request failed");
                return Err(ExecutorError::TransactionFailed(format!(
                    "RPC HTTP request failed: {}",
                    e
                )));
            }
            Err(_) => {
                tracing::error!("RPC sendTransaction timed out after 30 seconds");
                return Err(ExecutorError::TransactionFailed(
                    "Transaction submission timed out".to_string(),
                ));
            }
        };

        tracing::debug!(response = ?response, "Received RPC response");

        // Handle both success and error responses
        let signature = if let Some(err) = response.get("error") {
            let error_code = err.get("code").and_then(|c| c.as_i64());
            // Extract error message with better error handling
            // Store as String to avoid lifetime issues
            let error_msg = err
                .get("message")
                .and_then(|m| m.as_str().map(|s| s.to_string()))
                .or_else(|| {
                    // Try to extract error as JSON string if message is not a string
                    err.get("message")
                        .and_then(|m| serde_json::to_string(m).ok())
                })
                .unwrap_or_else(|| {
                    // If all else fails, format the entire error object
                    tracing::warn!(
                        error_obj = ?err,
                        "RPC error object missing 'message' field, using full error object"
                    );
                    "Unknown RPC error (see logs for details)".to_string()
                });

            tracing::error!(
                error_code = ?error_code,
                error = %error_msg,
                error_obj = ?err,
                "RPC returned error"
            );

            // Check for blockhash not found error (common with V0 transactions)
            // This can happen if Jupiter's blockhash expires between quote and submission
            // Error code -32004 is "Blockhash not found" in Solana RPC
            if error_code == Some(-32004)
                || error_msg.to_lowercase().contains("blockhash not found")
                || error_msg.to_lowercase().contains("blockhash expired")
            {
                tracing::warn!(
                    error_code = ?error_code,
                    error = %error_msg,
                    "Blockhash not found/expired error detected. This is common with V0 transactions. \
                    Triggering re-quote from Jupiter with fresh blockhash."
                );
                return Err(ExecutorError::BlockhashExpired);
            }

            // Check for address lookup table error specifically
            // V0 transactions use Address Lookup Tables (ALTs) which are mainnet-specific
            // Devnet doesn't have the same ALTs, causing this error
            if error_msg.contains("address table account")
                || error_msg.contains("address lookup table")
                || error_msg.contains("address table")
            {
                let detailed_error = format!(
                    "Address Lookup Table (ALT) error: {}. \
                    Jupiter returns V0 transactions that use ALTs not available on devnet. \
                    Solutions: (1) Use mainnet RPC for testing, (2) Request legacy transactions from Jupiter (if supported), \
                    or (3) Use a different token/dex that supports legacy transactions.",
                    error_msg
                );
                tracing::error!(error = %detailed_error, "ALT error detected");
                return Err(ExecutorError::TransactionFailed(detailed_error));
            }

            return Err(ExecutorError::TransactionFailed(format!(
                "RPC error: {}",
                error_msg
            )));
        } else if let Some(sig) = response.get("result").and_then(|r| r.as_str()) {
            sig
        } else {
            tracing::error!(response = ?response, "Invalid RPC response format");
            return Err(ExecutorError::TransactionFailed(
                "Invalid RPC response format".to_string(),
            ));
        };

        tracing::info!(
            signature = %signature,
            "Versioned transaction submitted successfully"
        );

        Ok(signature.to_string())
    }

    /// Unified slippage estimate (F5/F6). Prefers Jupiter's real `priceImpactPct`
    /// when available, falls back to the liquidity-aware sqrt model, then the
    /// config size-tier. Returns both the expected-impact fraction (for cost
    /// bookkeeping) and the strategy-clamped Jupiter tolerance (`slippageBps`).
    fn slippage_estimate(
        &self,
        signal: &Signal,
        jupiter_impact_pct: Option<Decimal>,
    ) -> SlippageEstimate {
        let fallback = slippage::FallbackTiers {
            small_fraction: self.config.strategy.slippage_fallback_small_percent,
            large_fraction: self.config.strategy.slippage_fallback_large_percent,
            threshold_sol: self.config.strategy.slippage_fallback_threshold_sol,
        };
        let sol_price = self
            .price_cache
            .as_ref()
            .and_then(|c| c.get_price_usd(crate::constants::mints::SOL));
        slippage::estimate(
            signal.payload.strategy,
            jupiter_impact_pct,
            signal.payload.amount_sol,
            signal.liquidity_usd,
            sol_price,
            fallback,
        )
    }

    /// Calculate dynamic Jito tip based on strategy and history with failure rate scaling
    pub async fn calculate_jito_tip(&self, signal: &Signal) -> Decimal {
        // Use TipManager if available, otherwise fall back to simple strategy-based calculation
        if let Some(ref tip_manager) = self.tip_manager {
            // Get recent failure rate for dynamic tip scaling
            let failure_rate = tip_manager.get_recent_failure_rate().await.unwrap_or(0.0);

            // Use dynamic tip scaling with failure rate data
            tip_manager.calculate_dynamic_tip_with_load(
                signal.payload.strategy,
                signal.payload.amount_sol,
                failure_rate,
            ).await
        } else {
            // Fallback to simple strategy-based tip calculation
            // Scale tip by trade size (tip_percent_max default 10%), with strategy-specific floors
            let strategy_floor = match signal.payload.strategy {
                Strategy::Shield => self.config.jito.tip_floor_sol,
                Strategy::Spear => {
                    // Use slightly higher floor for Spear to ensure bundle inclusion
                    (self.config.jito.tip_floor_sol + self.config.jito.tip_ceiling_sol)
                        / Decimal::from(2)
                }
                Strategy::Exit => self.config.jito.tip_floor_sol, // Use floor for exits, not ceiling
            };

            // Calculate tip as percentage of trade size (realistic MEV cost)
            let percentage_based_tip = signal.payload.amount_sol * self.config.jito.tip_percent_max;

            // Apply floor, percentage cap, and ceiling
            let tip = percentage_based_tip
                .max(strategy_floor)
                .min(self.config.jito.tip_ceiling_sol);

            tip
        }
    }

    /// Classify an ExecutorError into Jito-specific categories for retry strategy
    fn classify_jito_error(&self, error: &ExecutorError) -> JitoError {
        match error {
            // Fatal errors - should NOT retry
            ExecutorError::AmountTooSmall(_, _)
            | ExecutorError::AmountTooLarge(_, _)
            | ExecutorError::InsufficientBalance { .. }
            | ExecutorError::TransactionTooLarge { .. }
            | ExecutorError::SpearDisabled
            | ExecutorError::CircuitBreakerTripped(_)
            | ExecutorError::MarketConditionsUnfavorable(_)
            | ExecutorError::ExecutionCostTooHigh { .. } => {
                JitoError::Fatal(error.to_string())
            }

            // Network errors - may warrant fallback consideration
            ExecutorError::Timeout => {
                JitoError::Network(error.to_string())
            }

            // RPC and transaction errors - check if retryable
            ExecutorError::Rpc(msg) | ExecutorError::TransactionFailed(msg) => {
                let msg_lower = msg.to_lowercase();

                // Retryable Jito-specific errors
                if msg_lower.contains("insufficient tip")
                    || msg_lower.contains("bundle timeout")
                    || msg_lower.contains("deadline exceeded")
                    || msg_lower.contains("timed out")
                    || msg_lower.contains("network")
                    || msg_lower.contains("connection")
                    || msg_lower.contains("rate limit")
                {
                    JitoError::Retryable(error.to_string())
                }
                // Fatal transaction errors
                else if msg_lower.contains("insufficient balance")
                    || msg_lower.contains("invalid transaction")
                    || msg_lower.contains("transaction too large")
                {
                    JitoError::Fatal(error.to_string())
                }
                // Network connectivity issues
                else if msg_lower.contains("endpoint")
                    || msg_lower.contains("unavailable")
                    || msg_lower.contains("dns")
                    || msg_lower.contains("resolve")
                {
                    JitoError::Network(error.to_string())
                }
                // Default to retryable for RPC errors
                else {
                    JitoError::Retryable(error.to_string())
                }
            }

            // Blockhash expired - always retryable
            ExecutorError::BlockhashExpired => {
                JitoError::Retryable(error.to_string())
            }

            // V0 reconstruction errors - retryable (might be transient)
            ExecutorError::V0ReconstructionFailed(_) => {
                JitoError::Retryable(error.to_string())
            }

            // ALT errors - fatal (not recoverable without different parameters)
            ExecutorError::AddressLookupTableUnavailable(_) => {
                JitoError::Fatal(error.to_string())
            }
        }
    }

    /// Calculate adaptive Jito tip with increase on retry attempts
    async fn calculate_adaptive_jito_tip(&self, signal: &Signal, attempt: u32) -> u64 {
        let base_tip_sol = self.calculate_jito_tip(signal).await;

        if attempt > 1 {
            // Increase tip by 20% per retry attempt (compensating)
            let multiplier = 1.0 + (0.2 * (attempt - 1) as f64);
            let increased_tip = base_tip_sol * rust_decimal::Decimal::from_str(
                &format!("{}", multiplier)
            ).unwrap_or(rust_decimal::Decimal::from(2));

            // Convert to lamports (1 SOL = 1,000,000,000 lamports)
            (increased_tip * rust_decimal::Decimal::from(1_000_000_000u64))
                .to_u64()
                .unwrap_or_else(|| {
                    (base_tip_sol * rust_decimal::Decimal::from(1_000_000_000u64))
                        .to_u64()
                        .unwrap_or(1000) // Minimum 1000 lamports
                })
        } else {
            (base_tip_sol * rust_decimal::Decimal::from(1_000_000_000u64))
                .to_u64()
                .unwrap_or(1000)
        }
    }

    /// Calculate retry backoff duration with exponential backoff + jitter
    fn calculate_jito_backoff(&self, attempt: u32) -> Duration {
        // Exponential backoff: 200ms * 2^(attempt-1), with ±25% jitter
        let base_ms = 200u64;
        let exponential = base_ms.saturating_mul(1u64.saturating_pow(attempt.saturating_sub(1)));

        // Add ±25% jitter
        let jitter_factor = 0.75 + (rand::random::<f64>() * 0.5); // 0.75 to 1.25
        let with_jitter = (exponential as f64 * jitter_factor) as u64;

        // Cap at 5 seconds max backoff
        Duration::from_millis(with_jitter.min(5000))
    }

    /// Execute via Jito with intelligent retry logic for Jito-specific errors
    ///
    /// This wraps `execute_jito` with classification-based retry strategy:
    /// - Retryable errors (insufficient tip, timeout): retry with increased tip
    /// - Fatal errors (insufficient balance, invalid tx): fail immediately
    /// - Network errors: may trigger fallback consideration
    async fn execute_jito_with_retry(&self, signal: &Signal) -> Result<ExecutionOutcome, ExecutorError> {
        let mut attempts = 0;
        let max_attempts = self.config.jito.max_retries;

        loop {
            attempts += 1;

            match self.execute_jito(signal).await {
                Ok(result) => {
                    // Success - log retry if it took multiple attempts
                    if attempts > 1 {
                        tracing::info!(
                            trade_uuid = %signal.trade_uuid,
                            attempts = attempts,
                            "Jito execution succeeded after retry"
                        );
                    }
                    return Ok(result);
                },
                Err(e) => {
                    let jito_error = self.classify_jito_error(&e);

                    match jito_error {
                        JitoError::Retryable(reason) if attempts < max_attempts => {
                            // Record retry metric
                            self.record_jito_retry_metrics(attempts);

                            let backoff = self.calculate_jito_backoff(attempts);
                            tracing::warn!(
                                trade_uuid = %signal.trade_uuid,
                                attempt = attempts,
                                max_attempts = max_attempts,
                                backoff_ms = backoff.as_millis(),
                                reason = %reason,
                                "Jito execution failed with retryable error - retrying with increased tip"
                            );

                            // Sleep before retry
                            tokio::time::sleep(backoff).await;
                            continue;
                        },
                        JitoError::Retryable(reason) => {
                            tracing::error!(
                                trade_uuid = %signal.trade_uuid,
                                attempts = attempts,
                                reason = %reason,
                                "Jito execution failed after maximum retry attempts"
                            );
                            return Err(e);
                        },
                        JitoError::Fatal(reason) => {
                            tracing::error!(
                                trade_uuid = %signal.trade_uuid,
                                reason = %reason,
                                "Jito execution failed with fatal error - not retryable"
                            );
                            return Err(e);
                        },
                        JitoError::Network(reason) => {
                            tracing::warn!(
                                trade_uuid = %signal.trade_uuid,
                                reason = %reason,
                                "Jito execution failed with network error - may warrant fallback"
                            );
                            // Network errors are returned to caller for fallback consideration
                            return Err(e);
                        },
                    }
                }
            }
        }
    }

    /// Check if the total execution costs (tip + fee + slippage) exceed the configured limit
    async fn check_execution_costs(
        &self,
        signal: &Signal,
        price_impact_pct: Option<Decimal>,
        tip: Decimal,
        route_fee_sol: Option<Decimal>,
    ) -> Result<(), ExecutorError> {
        // Capital-aware gate: reject trades below minimum live position size
        // Fixed Jito costs (0.001 SOL tip floor) are uneconomical for tiny positions
        let min_live_position = self.config.position_sizing.min_live_position_sol;
        if signal.payload.amount_sol < min_live_position {
            tracing::warn!(
                trade_uuid = %signal.trade_uuid,
                amount_sol = %signal.payload.amount_sol,
                min_live_position_sol = %min_live_position,
                "Trade rejected: position size below minimum live threshold"
            );
            return Err(ExecutorError::ExecutionCostTooHigh {
                cost: tip,
                cost_pct: (tip / signal.payload.amount_sol).to_f64().unwrap_or(0.0) * 100.0,
                limit_pct: (min_live_position / signal.payload.amount_sol).to_f64().unwrap_or(100.0) * 100.0,
                strategy: signal.payload.strategy,
            });
        }

        // P2-17/F22: real per-route fee when available, else flat config rate.
        let dex_fee = route_fee_sol.unwrap_or_else(|| {
            signal.payload.amount_sol * self.config.strategy.dex_fee_rate
        });
        // F5/F6: use the SAME unified slippage estimate as the post-trade
        // recorded cost, so the gate and the recorded cost never disagree.
        // When Jupiter's priceImpactPct is absent this falls back to the
        // liquidity/config estimate (NOT zero — the gate must not under-count).
        let slippage = self
            .slippage_estimate(signal, price_impact_pct)
            .expected_cost_sol(signal.payload.amount_sol);
        let total_cost = tip + dex_fee + slippage;
        let cost_pct = if !signal.payload.amount_sol.is_zero() {
            total_cost / signal.payload.amount_sol
        } else {
            Decimal::ZERO
        };

        // [FRICTION-GATING] Apply dynamic friction gating: reject trades where
        // expected edge (from Kelly sizing) is less than or equal to total friction.
        // This prevents unprofitable micro-trades where transaction costs eat the
        // entire mathematical edge.
        if self.config.strategy.friction_gating_enabled {
            // Calculate Kelly metrics for this wallet
            let kelly_result = self.kelly_sizer.calculate_kelly(
                &signal.payload.wallet_address,
                signal.payload.strategy,
                14, // 14-day lookback for recent performance
            ).await;

            if let Ok(kelly) = kelly_result {
                // Expected profit = position_size * expected_return_pct
                // This is the actual expected profit in SOL, NOT the position size
                let expected_profit = kelly.expected_profit_sol(signal.payload.amount_sol);

                // Only reject if expected profit is less than or equal to total cost
                if expected_profit <= total_cost {
                    tracing::warn!(
                        trade_uuid = %signal.trade_uuid,
                        expected_profit_sol = %expected_profit,
                        total_cost_sol = %total_cost,
                        expected_return_pct = %kelly.expected_return_pct(),
                        win_rate = %kelly.win_rate,
                        avg_win_pct = %kelly.avg_win,
                        avg_loss_pct = %kelly.avg_loss,
                        position_size_sol = %signal.payload.amount_sol,
                        "Trade rejected: expected profit is less than transaction friction"
                    );
                    return Err(ExecutorError::ExecutionCostTooHigh {
                        cost: total_cost,
                        cost_pct: cost_pct.to_f64().unwrap_or(0.0) * 100.0,
                        limit_pct: kelly.expected_return_pct().to_f64().unwrap_or(0.0) * 100.0,
                        strategy: signal.payload.strategy,
                    });
                }

                tracing::debug!(
                    trade_uuid = %signal.trade_uuid,
                    expected_profit_sol = %expected_profit,
                    total_cost_sol = %total_cost,
                    net_expected_profit_sol = %(expected_profit - total_cost),
                    expected_return_pct = %kelly.expected_return_pct(),
                    "Friction gating passed: expected profit exceeds transaction costs"
                );
            } else {
                // If Kelly calculation fails (insufficient data, error), log but don't block
                tracing::debug!(
                    trade_uuid = %signal.trade_uuid,
                    error = %kelly_result.err().unwrap_or_default(),
                    "Kelly calculation failed for friction gating — proceeding with cost-only validation"
                );
            }
        }

        let mut limit = match signal.payload.strategy {
            Strategy::Shield => self.config.strategy.shield_max_total_cost_percent,
            Strategy::Spear => self.config.strategy.spear_max_total_cost_percent,
            Strategy::Exit => {
                // Never block an exit on cost — but warn when slippage is unusually high
                // so operators can spot illiquid tokens or rug conditions in the logs.
                let high_cost_threshold = Decimal::from_str("0.10").unwrap_or(Decimal::ZERO);
                if cost_pct > high_cost_threshold {
                    tracing::warn!(
                        trade_uuid = %signal.trade_uuid,
                        cost_pct = %cost_pct,
                        cost_sol = %total_cost,
                        "High execution cost on EXIT signal — proceeding anyway"
                    );
                }
                return Ok(());
            }
        };

        // Apply dynamic limit expansion in high volatility regimes.
        // [R-L1] Use the shared MarketRegimeDetector (initialised in with_price_cache) instead
        // of constructing a new one per-call. A fresh detector has an empty price_history so
        // detect_effective_regime always returned Sideways — the Bull/Bear expansion never fired.
        // The shared detector uses the same Arc<PriceCache> so its price_history has real data.
        if limit > Decimal::ZERO {
            if let Some(ref detector) = self.market_regime_detector {
                use crate::engine::market_regime::MarketRegime;
                use std::str::FromStr;

                let regime = detector.detect_token_regime(signal.token_address());

                let multiplier = match regime {
                    MarketRegime::Bull | MarketRegime::Bear => {
                        Decimal::from_str("1.5").unwrap_or(Decimal::from(3) / Decimal::from(2))
                    } // Allow 50% more slippage in fast markets
                    MarketRegime::Sideways => Decimal::ONE,
                };

                limit *= multiplier;

                if multiplier > Decimal::ONE {
                    tracing::debug!(
                        trade_uuid = %signal.trade_uuid,
                        regime = %regime,
                        multiplier = %multiplier,
                        expanded_limit = %limit,
                        "Expanded execution cost limit for high volatility regime"
                    );
                }
            }
        }

        if limit > Decimal::ZERO && cost_pct > limit {
            return Err(ExecutorError::ExecutionCostTooHigh {
                cost: total_cost,
                cost_pct: cost_pct.to_f64().unwrap_or(0.0) * 100.0,
                limit_pct: limit.to_f64().unwrap_or(0.0) * 100.0,
                strategy: signal.payload.strategy,
            });
        }
        Ok(())
    }

    /// Switch to fallback RPC mode
    async fn switch_to_fallback(&self) {
        // Check if Jito fallback is disabled by configuration
        if self.config.jito.disable_fallback {
            tracing::warn!(
                "Jito fallback is disabled by configuration (jito.disable_fallback = true) - staying in Jito mode despite failures"
            );
            // Reset failure count to prevent re-triggering
            self.mutable.lock().failure_count = 0;
            return;
        }

        // Check failure count against Jito-specific threshold
        let (failure_count, should_fallback) = {
            let state = self.mutable.lock();
            let count = state.failure_count;
            let threshold = self.config.jito.min_failures_before_fallback;
            (count, count >= threshold)
        };

        if !should_fallback {
            tracing::debug!(
                failure_count = failure_count,
                threshold = self.config.jito.min_failures_before_fallback,
                "Failure count below Jito threshold - not switching to fallback"
            );
            return;
        }

        // Proceed with fallback if URL is configured
        if self.config.rpc.fallback_url.is_some() {
            let (reason, previous_mode) = {
                let state = self.mutable.lock();
                let reason = format!(
                    "Consecutive Jito failures ({}) exceeded threshold ({})",
                    state.failure_count,
                    self.config.jito.min_failures_before_fallback
                );
                let previous_mode = state.rpc_mode;
                (reason, previous_mode)
            };

            tracing::warn!(
                previous_mode = ?previous_mode,
                failure_count = failure_count,
                threshold = self.config.jito.min_failures_before_fallback,
                "Switching to fallback RPC mode (Standard TPU)"
            );

            {
                let mut state = self.mutable.lock();
                state.rpc_mode = RpcMode::Standard;
                state.fallback_since = Some(Utc::now());
                state.failure_count = 0;
            }

            // Record Jito fallback metrics
            if let Some(ref metrics) = self.metrics {
                metrics
                    .jito_fallback_total
                    .with_label_values(&["threshold_exceeded"])
                    .inc();
            }

            // Send notification for Jito fallback
            self.notify(NotificationEvent::JitoFallbackTriggered {
                reason: reason.clone(),
                failure_count,
                threshold: self.config.jito.min_failures_before_fallback,
            })
            .await;

            // Log to config audit
            if let Err(e) = self
                .db
                .log_config_change(
                    "rpc_mode",
                    Some("JITO"),
                    "STANDARD",
                    "SYSTEM_FAILOVER",
                    Some(&reason),
                )
                .await
            {
                tracing::error!(error = %e, "Failed to log RPC mode change");
            }
        } else {
            // [R-M3] No fallback URL configured: warn and reset failure count
            tracing::warn!(
                "No fallback RPC URL configured; trading on degraded primary. Reset failure count."
            );
            self.mutable.lock().failure_count = 0;
        }
    }

    /// Get current RPC mode
    pub fn rpc_mode(&self) -> RpcMode {
        self.mutable.lock().rpc_mode
    }

    /// Check if currently in fallback mode
    pub fn is_in_fallback(&self) -> bool {
        self.mutable.lock().fallback_since.is_some()
    }

    /// Estimate network fee in SOL for paper/devnet PnL realism.
    ///
    /// Approximation: base fee (5000 lamports) + median priority fee × estimated CU / 1e6,
    /// then convert lamports to SOL (÷ 1e9).
    async fn estimate_network_fee(&self) -> Decimal {
        let client = self.active_rpc_client();
        let base_fee_lamports: u64 = 5000;

        let priority_fee_lamports: u64 = client
            .get_recent_prioritization_fees(&[])
            .await
            .ok()
            .and_then(|fees| {
                let mut sorted = fees
                    .iter()
                    .map(|f| f.prioritization_fee)
                    .collect::<Vec<_>>();
                sorted.sort();
                sorted.get(sorted.len() / 2).copied()
            })
            .unwrap_or(100_000);

        let estimated_cu: u64 = 200_000;
        let total_lamports = base_fee_lamports
            + (priority_fee_lamports as u128 * estimated_cu as u128 / 1_000_000) as u64;
        Decimal::from(total_lamports) / Decimal::from(1_000_000_000u64)
    }

    /// Fetch real Jupiter quotes for paper/devnet modes. Returns the data needed
    /// to build an `ExecutionOutcome` without mutating any shared state.
    ///
    /// Returns `(price_impact_pct, fill_price_sol_per_token, token_amount,
    /// route_fee_sol)`. The route fee lets paper mode charge the same real
    /// per-route DEX fee that live mode would pay (P2-17/F22 parity) instead
    /// of the flat `amount × dex_fee_rate` estimate.
    async fn get_paper_prices(
        &self,
        signal: &Signal,
    ) -> Result<(Option<Decimal>, Option<Decimal>, Option<u64>, Option<Decimal>), ExecutorError> {
        let active_client = self.active_rpc_client();
        let tx_builder = TransactionBuilder::new(active_client.clone(), self.config.clone())
            .map_err(|e| ExecutorError::TransactionFailed(format!("TransactionBuilder: {}", e)))?;

        let sol_mint = crate::constants::mints::SOL;

        match signal.payload.action {
            Action::Buy => {
                let amount_lamports = crate::utils::sol_to_lamports(signal.payload.amount_sol).map_err(|e| {
                    ExecutorError::TransactionFailed(format!("Failed to convert SOL amount to lamports: {}", e))
                })?;
                let result = tx_builder
                    .get_quote_prices(sol_mint, signal.token_address(), amount_lamports)
                    .await
                    .map_err(|e| {
                        ExecutorError::TransactionFailed(format!("Jupiter quote: {}", e))
                    })?;

                let fill_price_sol = convert_fill_price(
                    result.fill_price_lamports_per_base,
                    signal.token_decimals,
                    &signal.trade_uuid,
                );
                Ok((
                    result.price_impact_pct,
                    fill_price_sol,
                    Some(result.out_amount),
                    result.route_fee_sol,
                ))
            }
            Action::Sell => {
                // BUY and SELL produce different trade UUIDs, so a SELL cannot be
                // matched to its opening position by the SELL signal's UUID. Look up
                // the active position by (wallet, token) — positions are held
                // one-per-token-per-wallet.
                let position = self
                    .db
                    .get_active_position_by_wallet_token(
                        &signal.payload.wallet_address,
                        signal.token_address(),
                    )
                    .await
                    .map_err(|e| ExecutorError::TransactionFailed(format!("DB lookup: {}", e)))?
                    .ok_or_else(|| {
                        ExecutorError::TransactionFailed(format!(
                            "No active position for SELL: wallet={}, token={}",
                            signal.payload.wallet_address,
                            signal.token_address()
                        ))
                    })?;

                let token_amount =
                    position
                        .token_amount
                        .and_then(|d| d.to_u64())
                        .ok_or_else(|| {
                            ExecutorError::TransactionFailed(
                                "No token_amount in position for paper SELL".to_string(),
                            )
                        })?;

                let exit_fraction = signal.payload.exit_fraction.unwrap_or(Decimal::ONE);
                let sell_amount = (Decimal::from(token_amount) * exit_fraction)
                    .to_u64()
                    .unwrap_or(0);

                let result = tx_builder
                    .get_quote_prices(signal.token_address(), sol_mint, sell_amount)
                    .await
                    .map_err(|e| {
                        ExecutorError::TransactionFailed(format!("Jupiter quote: {}", e))
                    })?;

                let fill_price_sol = convert_fill_price(
                    result.fill_price_lamports_per_base,
                    signal.token_decimals,
                    &signal.trade_uuid,
                );
                Ok((result.price_impact_pct, fill_price_sol, None, result.route_fee_sol))
            }
        }
    }

    async fn execute_paper(&self, signal: &Signal) -> Result<ExecutionOutcome, ExecutorError> {
        tracing::info!(
            trade_uuid = %signal.trade_uuid,
            action = %signal.payload.action,
            "Paper mode: fetching real Jupiter quote, no on-chain submission"
        );

        let (price_impact, fill_price_sol, token_amount, route_fee_sol) =
            self.get_paper_prices(signal).await?;
        let estimated_fee_sol = self.estimate_network_fee().await;

        // Apply the same cost efficiency gate as live Jito mode.
        // Paper trading must be an exact simulation: if live mode would reject
        // the trade because the Jito tip + DEX fee + slippage exceeds the
        // strategy cost limit, paper mode must also reject it. Without this,
        // paper trades execute at a loss that live mode would never allow.
        let tip = self.calculate_jito_tip(signal).await;
        self.check_execution_costs(signal, price_impact, tip, route_fee_sol)
            .await?;

        Ok(ExecutionOutcome {
            signature: format!("simulated_{}", signal.trade_uuid),
            confirmed: true,
            fill_price_sol_per_token: fill_price_sol,
            price_impact_pct: price_impact,
            token_amount,
            estimated_fee_sol: Some(estimated_fee_sol),
            route_fee_sol,
        })
    }

    async fn execute_devnet(&self, signal: &Signal) -> Result<ExecutionOutcome, ExecutorError> {
        tracing::info!(
            trade_uuid = %signal.trade_uuid,
            "Devnet mode: real Jupiter quote + minimal tx on devnet"
        );

        let (price_impact, fill_price_sol, token_amount, route_fee_sol) =
            self.get_paper_prices(signal).await?;
        let estimated_fee_sol = self.estimate_network_fee().await;

        let secrets = load_secrets_with_fallback().map_err(|e| {
            ExecutorError::TransactionFailed(format!("Failed to load vault: {}", e))
        })?;
        let wallet_keypair = load_wallet_keypair(&secrets).map_err(|e| {
            ExecutorError::TransactionFailed(format!("Failed to load keypair: {}", e))
        })?;

        let active = self.active_rpc_client();
        let blockhash = crate::metrics::timed_rpc(
            "primary",
            "getLatestBlockhash",
            active.get_latest_blockhash(),
        )
        .await
        .map_err(|e| ExecutorError::TransactionFailed(format!("Blockhash: {}", e)))?;

        let noop_ix = solana_system_interface::instruction::transfer(
            &wallet_keypair.pubkey(),
            &wallet_keypair.pubkey(),
            0,
        );
        let mut tx = Transaction::new_with_payer(&[noop_ix], Some(&wallet_keypair.pubkey()));
        tx.sign(&[&wallet_keypair], blockhash);

        let signature_str = crate::metrics::timed_rpc(
            "primary",
            "sendTransaction",
            active.send_transaction(&tx),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Devnet submission failed");
            ExecutorError::TransactionFailed(format!("Devnet submission: {}", e))
        })?
        .to_string();

        let confirmed = self
            .poll_signature_confirmation(&signature_str, &signal.trade_uuid)
            .await?;

        tracing::info!(
            trade_uuid = %signal.trade_uuid,
            signature = %signature_str,
            confirmed = confirmed,
            "Devnet transaction submitted"
        );

        Ok(ExecutionOutcome {
            signature: signature_str,
            confirmed,
            fill_price_sol_per_token: fill_price_sol,
            price_impact_pct: price_impact,
            token_amount,
            estimated_fee_sol: Some(estimated_fee_sol),
            route_fee_sol,
        })
    }

    /// Get time spent in fallback mode
    pub fn fallback_duration(&self) -> Option<chrono::Duration> {
        self.mutable
            .lock()
            .fallback_since
            .map(|t| Utc::now().signed_duration_since(t))
    }

    /// Get the active RPC client based on current mode
    /// In STANDARD mode with fallback configured, returns fallback client
    /// Otherwise returns primary client
    fn active_rpc_client(&self) -> Arc<RpcClient> {
        if self.mutable.lock().rpc_mode == RpcMode::Standard {
            if let Some(ref fallback_client) = self.fallback_rpc_client {
                return fallback_client.clone();
            }
        }
        self.rpc_client.clone()
    }

    /// Expose active RPC client publicly
    pub fn active_rpc_client_pub(&self) -> Arc<RpcClient> {
        self.active_rpc_client()
    }

    /// Get the active RPC URL based on current mode
    /// In STANDARD mode with fallback configured, returns fallback URL
    /// Otherwise returns primary URL
    fn active_rpc_url(&self) -> &str {
        if self.mutable.lock().rpc_mode == RpcMode::Standard {
            if let Some(ref fallback_url) = self.config.rpc.fallback_url {
                return fallback_url;
            }
        }
        &self.config.rpc.primary_url
    }

    /// Poll `getSignatureStatuses` up to `max_polls` times with `interval` between polls.
    ///
    /// Returns `true` if confirmed, `false` if still pending after all polls.
    /// Returns `Err` only if the transaction is definitively failed on-chain.
    ///
    /// Callers that get `false` should record the signature and let the recovery manager
    /// handle the stuck position — do NOT re-submit without first verifying the tx is gone.
    async fn poll_signature_confirmation(
        &self,
        signature: &str,
        trade_uuid: &str,
    ) -> Result<bool, ExecutorError> {
        use solana_sdk::signature::Signature;
        use std::str::FromStr;

        let sig = Signature::from_str(signature).map_err(|e| {
            ExecutorError::TransactionFailed(format!("Invalid signature {}: {}", signature, e))
        })?;

        let max_polls = 3u32;
        let interval = Duration::from_secs(2);

        for attempt in 1..=max_polls {
            tokio::time::sleep(interval).await;

            match self.rpc_client.get_signature_status(&sig).await {
                Ok(Some(Ok(()))) => {
                    tracing::info!(
                        trade_uuid = %trade_uuid,
                        signature = %signature,
                        attempt = attempt,
                        "Transaction confirmed on-chain"
                    );
                    return Ok(true);
                }
                Ok(Some(Err(tx_err))) => {
                    tracing::error!(
                        trade_uuid = %trade_uuid,
                        signature = %signature,
                        error = %tx_err,
                        "Transaction failed on-chain"
                    );
                    return Err(ExecutorError::TransactionFailed(format!(
                        "Transaction failed on-chain: {}",
                        tx_err
                    )));
                }
                Ok(None) => {
                    tracing::debug!(
                        trade_uuid = %trade_uuid,
                        signature = %signature,
                        attempt = attempt,
                        max_polls = max_polls,
                        "Transaction not yet confirmed"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        error = %e,
                        "RPC error checking signature status"
                    );
                }
            }
        }

        tracing::warn!(
            trade_uuid = %trade_uuid,
            signature = %signature,
            "Transaction unconfirmed after {} polls — recovery manager will handle",
            max_polls
        );
        Ok(false)
    }
}

/// Executor errors
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    /// Spear strategy disabled in fallback mode
    #[error("Spear strategy is disabled in fallback RPC mode")]
    SpearDisabled,

    #[error("Blockhash expired, retry required")]
    BlockhashExpired,

    /// Amount too small
    #[error("Amount {0} SOL is below minimum {1} SOL")]
    AmountTooSmall(Decimal, Decimal),

    /// Amount too large
    #[error("Amount {0} SOL exceeds maximum {1} SOL")]
    AmountTooLarge(Decimal, Decimal),

    /// RPC error
    #[error("RPC error: {0}")]
    Rpc(String),

    /// Transaction failed
    #[error("Transaction failed: {0}")]
    TransactionFailed(String),

    /// Timeout
    #[error("Execution timed out")]
    Timeout,

    /// Insufficient balance
    #[error("Insufficient balance: required {required} SOL, available {available} SOL")]
    InsufficientBalance {
        required: Decimal,
        available: Decimal,
    },

    /// Circuit breaker tripped
    #[error("Circuit breaker tripped: {0}")]
    CircuitBreakerTripped(String),

    /// Transaction size exceeds Solana limits
    #[error("Transaction too large: {actual} bytes (max: {max} bytes)")]
    TransactionTooLarge { actual: usize, max: usize },

    /// Market conditions unfavorable for trading
    #[error("Market conditions unfavorable: {0}")]
    MarketConditionsUnfavorable(String),

    /// V0 message reconstruction failed
    #[error("V0 message reconstruction failed: {0}")]
    V0ReconstructionFailed(String),

    /// Address Lookup Table unavailable
    #[error("Address Lookup Table unavailable: {0}")]
    AddressLookupTableUnavailable(String),

    /// Execution cost too high (exceeds total cost percent)
    #[error("Total execution cost {cost} SOL ({cost_pct:.1}%) exceeds limit {limit_pct:.1}% for {strategy:?}")]
    ExecutionCostTooHigh {
        cost: Decimal,
        cost_pct: f64,
        limit_pct: f64,
        strategy: Strategy,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_error_display() {
        use rust_decimal::Decimal;
        use std::str::FromStr;
        let err = ExecutorError::AmountTooSmall(
            Decimal::from_str("0.001").unwrap(),
            Decimal::from_str("0.01").unwrap(),
        );
        assert!(err.to_string().contains("0.001"));
        assert!(err.to_string().contains("0.01"));
    }

    #[test]
    fn test_rpc_mode_debug() {
        assert_eq!(format!("{:?}", RpcMode::Jito), "Jito");
        assert_eq!(format!("{:?}", RpcMode::Standard), "Standard");
    }
}
