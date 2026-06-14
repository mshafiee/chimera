//! Trade executor for Solana transactions
//!
//! Handles the actual submission of trades to the Solana network.
//! Includes RPC failover with automatic recovery to primary.

use crate::config::AppConfig;
use crate::db::DbPool;
use crate::engine::tips::TipManager;
use crate::engine::transaction_builder::{load_wallet_keypair, TransactionBuilder};
use crate::models::{Signal, Strategy};
use crate::notifications::{CompositeNotifier, NotificationEvent};
use crate::price_cache::PriceCache;
use crate::vault::load_secrets_with_fallback;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{DateTime, Timelike, Utc};
use rust_decimal::prelude::*;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
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

/// RPC mode for trade execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcMode {
    /// Primary RPC with Jito bundles
    Jito,
    /// Fallback to standard TPU
    Standard,
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

/// Mutable execution state — wrapped in a Mutex so `execute` can take `&self`,
/// allowing the RwLock in Engine to be held as a read lock during the 60 s RPC call
/// instead of a write lock that would serialise all concurrent executions.
struct ExecutorMutableState {
    rpc_mode: RpcMode,
    failure_count: u32,
    fallback_since: Option<DateTime<Utc>>,
    last_recovery_attempt: Option<DateTime<Utc>>,
}

/// Trade executor
pub struct Executor {
    /// Configuration
    config: Arc<AppConfig>,
    /// Database pool
    db: DbPool,
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
    /// Price impact from the most recent Jupiter quote (set by execute_jito/execute_standard)
    last_price_impact: parking_lot::Mutex<Option<Decimal>>,
    /// Fill price (lamports per token base unit) from the most recent Jupiter quote
    last_fill_price_lamports_per_base: parking_lot::Mutex<Option<Decimal>>,
}

impl Executor {
    /// Create a new executor
    pub fn new(config: Arc<AppConfig>, db: DbPool) -> Self {
        let rpc_mode = if config.jito.enabled {
            RpcMode::Jito
        } else {
            RpcMode::Standard
        };

        // Create HTTP client with timeout (reserved for fallback scenarios)
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.rpc.timeout_ms))
            .build()
            .expect("Failed to create HTTP client");

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
        let jito_searcher = config.jito.searcher_endpoint.as_ref().map(|endpoint| {
            crate::engine::jito_searcher::JitoSearcherClient::new(
                endpoint.clone(),
                rpc_client.clone(),
            )
        });

        Self {
            config,
            db,
            mutable: parking_lot::Mutex::new(ExecutorMutableState {
                rpc_mode,
                failure_count: 0,
                fallback_since: None,
                last_recovery_attempt: None,
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
            last_price_impact: parking_lot::Mutex::new(None),
            last_fill_price_lamports_per_base: parking_lot::Mutex::new(None),
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

    /// Set the price cache for volatility calculation
    pub fn with_price_cache(mut self, price_cache: Arc<PriceCache>) -> Self {
        self.price_cache = Some(price_cache);
        self
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
                NotificationEvent::WalletDrained { .. } => rules.wallet_drained,
                NotificationEvent::SystemCrash { .. } => rules.system_crash,
                NotificationEvent::PositionExited { .. } => rules.position_exited,
                NotificationEvent::RpcFallback { .. } => rules.rpc_fallback,
                NotificationEvent::WalletPromoted { .. } => rules.wallet_promoted,
                NotificationEvent::DailySummary { .. } => rules.daily_summary,
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
    /// Returns the transaction signature on success
    pub async fn execute(&self, signal: &Signal) -> Result<String, ExecutorError> {
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

            // Check market conditions before executing (skip off-hours gate for exits)
            if let Err(e) = self.check_market_conditions(&signal.payload.action).await {
                tracing::warn!(
                    trade_uuid = %signal.trade_uuid,
                    error = %e,
                    "Trade rejected due to market conditions"
                );
                return Err(ExecutorError::MarketConditionsUnfavorable(e.to_string()));
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
            let result = match rpc_mode {
                RpcMode::Jito => self.execute_jito(signal).await,
                RpcMode::Standard => self.execute_standard(signal).await,
            };

            // Handle retry for expired blockhash
            match &result {
                Err(ExecutorError::BlockhashExpired) => {
                    if attempts < 3 {
                        // Exponential backoff: 200ms, 400ms, 800ms
                        let backoff_ms = 200 * (1 << (attempts - 1));
                        tracing::warn!(
                            trade_uuid = %signal.trade_uuid,
                            attempt = attempts,
                            backoff_ms = backoff_ms,
                            "Blockhash expired/invalid. Re-requesting fresh quote and retrying with exponential backoff..."
                        );
                        // The loop will restart, causing TransactionBuilder to fetch a NEW quote
                        // from Jupiter with a FRESH blockhash.
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
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
                    if attempts < 3 {
                        let backoff_ms = 200 * (1 << (attempts - 1));
                        tracing::warn!(
                            trade_uuid = %signal.trade_uuid,
                            attempt = attempts,
                            error = %e,
                            backoff_ms = backoff_ms,
                            "V0 reconstruction failed. Re-requesting fresh quote from Jupiter with exponential backoff..."
                        );
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
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
                _ => {}
            }

            // Handle result and track failures
            match &result {
                Ok(sig) => {
                    self.mutable.lock().failure_count = 0;
                    tracing::info!(
                        trade_uuid = %signal.trade_uuid,
                        signature = %sig,
                        "Trade executed successfully"
                    );

                    // Record tip if using Jito and tip manager is available
                    let jito_tip = if rpc_mode == RpcMode::Jito {
                        let tip = self.calculate_jito_tip(signal);
                        if let Some(ref tip_manager) = self.tip_manager {
                            if let Err(e) = tip_manager
                                .record_tip(
                                    tip,
                                    Some(sig),
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
                        tip
                    } else {
                        Decimal::ZERO
                    };

                    // Track costs: Jito tip, DEX fee, slippage
                    let dex_fee_rate = self.config.strategy.dex_fee_rate;
                    let dex_fee_sol = signal.payload.amount_sol * dex_fee_rate;
                    // Use actual price impact from Jupiter quote when available;
                    // fall back to a size-based conservative estimate otherwise.
                    let slippage_percent = self.last_price_impact.lock().take()
                        .map(|pct| pct / Decimal::from(100)) // convert 1.5% → 0.015 fraction
                        .unwrap_or_else(|| {
                            let half_sol = Decimal::from_str("0.5").unwrap();
                            if signal.payload.amount_sol < half_sol {
                                Decimal::from_str("0.005").unwrap()
                            } else {
                                Decimal::from_str("0.01").unwrap()
                            }
                        });
                    let slippage_cost_sol = signal.payload.amount_sol * slippage_percent;

                    // Update trade costs in database
                    if let Err(e) = crate::db::update_trade_costs(
                        &self.db,
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

                    // Record failed tip if using Jito and tip manager is available
                    if rpc_mode == RpcMode::Jito {
                        if let Some(ref tip_manager) = self.tip_manager {
                            let tip = self.calculate_jito_tip(signal);
                            if let Err(e) = tip_manager
                                .record_tip(
                                    tip,
                                    None, // No signature for failed trades
                                    signal.payload.strategy,
                                    false, // failure
                                )
                                .await
                            {
                                tracing::warn!(
                                    error = %e,
                                    "Failed to record failed tip in TipManager"
                                );
                            }
                        }
                    }

                    // Check if we need to switch to fallback
                    if failure_count >= self.config.rpc.max_consecutive_failures
                        && rpc_mode == RpcMode::Jito
                    {
                        self.switch_to_fallback().await;
                    }
                }
            }

            return result;
        }
    }

    /// Check market conditions before executing trades
    /// Returns Ok(()) if conditions are favorable, Err with reason otherwise
    async fn check_market_conditions(&self, action: &crate::models::Action) -> Result<(), String> {
        // Check 1: SOL price crash (>10% drop in last hour)
        // This requires price history - check if we have sufficient data
        if let Some(ref price_cache) = self.price_cache {
            // Get SOL price history to check for crash
            let sol_mint = crate::constants::mints::SOL;
            let history = price_cache.price_history.read();
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

        // Check 3: Low liquidity period (off-hours) — only for new positions, not exits.
        // Position size is already reduced by off_hours_size_multiplier at signal time
        // (webhook.rs). Log a reminder here so the execution trace is complete.
        if matches!(action, crate::models::Action::Buy) {
            let hour_utc = Utc::now().time().hour();
            if (2..6).contains(&hour_utc) {
                tracing::warn!(
                    hour_utc = hour_utc,
                    "Executing during off-hours window (2–6 AM UTC): position size was reduced at signal time"
                );
            }
        }

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

                // Log recovery to config audit
                if let Err(e) = crate::db::log_config_change(
                    &self.db,
                    "rpc_mode",
                    Some("STANDARD"),
                    "JITO",
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
        let start = std::time::Instant::now();

        // Always check the PRIMARY URL, not the currently active one.
        // When in Standard (fallback) mode we are trying to determine if we can
        // recover to primary — checking the fallback's health is meaningless here.
        let active_url = &self.config.rpc.primary_url;
        let health_check = async {
            let response = self
                .http_client
                .post(active_url)
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

            // Parse response to check if healthy
            let body: serde_json::Value = response
                .json()
                .await
                .map_err(|e| ExecutorError::Rpc(format!("Failed to parse RPC response: {}", e)))?;

            // Check for error in response
            if body.get("error").is_some() {
                return Err(ExecutorError::Rpc(format!(
                    "RPC returned error: {:?}",
                    body["error"]
                )));
            }

            Ok(())
        };

        // Apply timeout
        let timeout_duration = Duration::from_millis(self.config.rpc.timeout_ms);
        match timeout(timeout_duration, health_check).await {
            Ok(Ok(())) => {
                let latency = start.elapsed().as_millis() as u64;
                let health = RpcHealth {
                    healthy: true,
                    last_check: Utc::now(),
                    latency_ms: Some(latency),
                };

                // Cache the health status
                *self.latest_rpc_health.write().await = Some(health.clone());
                tracing::debug!(latency_ms = latency, "RPC health check passed");

                Ok(health)
            }
            Ok(Err(e)) => {
                // Cache unhealthy status
                let health = RpcHealth {
                    healthy: false,
                    last_check: Utc::now(),
                    latency_ms: None,
                };
                *self.latest_rpc_health.write().await = Some(health);
                tracing::warn!(error = %e, "RPC health check failed");
                Err(e)
            }
            Err(_) => {
                // Timeout - cache unhealthy status
                let health = RpcHealth {
                    healthy: false,
                    last_check: Utc::now(),
                    latency_ms: None,
                };
                *self.latest_rpc_health.write().await = Some(health);
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
        let _ = self.check_primary_health().await;
    }

    /// Execute via Jito bundle
    async fn execute_jito(&self, signal: &Signal) -> Result<String, ExecutorError> {
        tracing::info!(
            trade_uuid = %signal.trade_uuid,
            "Executing trade via Jito bundle"
        );

        // Check if devnet simulation mode is enabled
        if self.config.jupiter.devnet_simulation_mode {
            tracing::info!(
                trade_uuid = %signal.trade_uuid,
                "Devnet simulation mode: skipping RPC submission, returning simulated signature"
            );
            // Return a simulated signature (format: "simulated_<uuid>")
            return Ok(format!("simulated_{}", signal.trade_uuid));
        }

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
            TransactionBuilder::new(active_client.clone(), self.config.clone());
        let built_tx = transaction_builder
            .build_swap_transaction(signal, &wallet_keypair)
            .await
            .map_err(|e| {
                ExecutorError::TransactionFailed(format!("Failed to build transaction: {}", e))
            })?;

        // Capture actual price impact and fill price from Jupiter quote
        *self.last_price_impact.lock() = built_tx.price_impact_pct();
        *self.last_fill_price_lamports_per_base.lock() = built_tx.fill_price_lamports_per_base();

        // Calculate dynamic tip
        let tip = self.calculate_jito_tip(signal);
        
        // Check total execution cost cap
        self.check_execution_costs(signal, built_tx.price_impact_pct(), tip)?;

        tracing::debug!(
            tip_sol = tip.to_f64().unwrap_or(0.0),
            strategy = %signal.payload.strategy,
            "Calculated Jito tip"
        );

        // Submit to Jito via direct Jito Searcher (preferred) or Helius Sender API (fallback)

        // Try direct Jito Searcher first if configured
        // Note: Jito bundles currently only support legacy transactions
        if let Some(ref jito_searcher) = self.jito_searcher {
            let tip_lamports = (tip * Decimal::from(1_000_000_000u64)).to_u64().unwrap_or_else(|| {
                tracing::warn!(tip = %tip, "Jito tip conversion overflow — clamping to 0.01 SOL (10_000_000 lamports)");
                10_000_000u64
            }); // Convert SOL to lamports

            // Serialize based on transaction type using Legacy config for Solana wire compatibility
            let tx_bytes = match &built_tx {
                crate::engine::transaction_builder::BuiltTransaction::Legacy {
                    transaction,
                    ..
                } => bincode::serde::encode_to_vec(transaction, bincode::config::legacy())
                    .map_err(|e| {
                        ExecutorError::TransactionFailed(format!("Serialization error: {}", e))
                    })?,
                crate::engine::transaction_builder::BuiltTransaction::Versioned {
                    transaction_bytes,
                    ..
                } => {
                    // Already serialized and signed by transaction_builder
                    transaction_bytes.clone()
                }
            };

            match jito_searcher
                .submit_bundle(&tx_bytes, tip_lamports, &wallet_keypair)
                .await
            {
                Ok(signature) => {
                    tracing::info!(
                        trade_uuid = %signal.trade_uuid,
                        signature = %signature,
                        "Bundle submitted via direct Jito Searcher"
                    );
                    // Poll for confirmation; on definitive on-chain failure propagate error.
                    // On timeout (still pending) return the signature so recovery handles it.
                    self.poll_signature_confirmation(&signature, &signal.trade_uuid).await?;
                    return Ok(signature);
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

        // Fallback to Helius Sender API if configured and enabled
        // Note: Helius Sender currently only supports legacy transactions
        if self.config.jito.helius_fallback {
            if let Some(helius_api_key) = secrets.rpc_api_key.as_ref() {
                // Serialize transaction to bytes for Helius Sender
                let tx_bytes = match &built_tx {
                    crate::engine::transaction_builder::BuiltTransaction::Legacy {
                        transaction,
                        ..
                    } => bincode1::serialize(transaction).map_err(|e| {
                        ExecutorError::TransactionFailed(format!(
                            "Failed to serialize transaction: {}",
                            e
                        ))
                    })?,
                    crate::engine::transaction_builder::BuiltTransaction::Versioned {
                        transaction_bytes,
                        ..
                    } => transaction_bytes.clone(),
                };

                match self
                    .submit_via_helius_sender(&tx_bytes, tip, helius_api_key, &wallet_keypair)
                    .await
                {
                    Ok(signature) => {
                        tracing::info!(
                            trade_uuid = %signal.trade_uuid,
                            signature = %signature,
                            "Bundle submitted via Helius Sender API"
                        );
                        self.poll_signature_confirmation(&signature, &signal.trade_uuid).await?;
                        return Ok(signature);
                    }
                    Err(e) => {
                        tracing::warn!(
                            trade_uuid = %signal.trade_uuid,
                            error = %e,
                            "Helius Sender API also failed"
                        );
                    }
                }
            }
        }

        // Final fallback: Submit via standard TPU
        tracing::warn!(
            trade_uuid = %signal.trade_uuid,
            "Jito bundle submission failed, falling back to standard TPU"
        );

        // Sign and send transaction via standard RPC (handles both legacy and versioned)
        let signature = match &built_tx {
            crate::engine::transaction_builder::BuiltTransaction::Legacy {
                transaction, ..
            } => {
                self.submit_transaction(transaction, &wallet_keypair)
                    .await?
            }
            crate::engine::transaction_builder::BuiltTransaction::Versioned {
                transaction_bytes,
                ..
            } => {
                self.submit_versioned_transaction(transaction_bytes, &wallet_keypair)
                    .await?
            }
        };

        Ok(signature)
    }

    /// Submit transaction via Helius Sender API (Jito bundles)
    async fn submit_via_helius_sender(
        &self,
        tx_bytes: &[u8],
        tip_sol: Decimal,
        api_key: &str,
        tip_keypair: &solana_sdk::signature::Keypair,
    ) -> Result<String, ExecutorError> {
        use solana_sdk::pubkey::Pubkey;
        use solana_sdk::signature::Signer;
        use solana_system_interface::instruction as system_instruction;
        use std::str::FromStr;

        let url = format!("https://api.helius.xyz/v0/send-bundle?api-key={}", api_key);

        // Convert SOL to lamports
        let tip_lamports = (tip_sol * Decimal::from(1_000_000_000u64)).to_u64().unwrap_or_else(|| {
            tracing::warn!(tip_sol = %tip_sol, "Jito tip conversion overflow — clamping to 0.01 SOL (10_000_000 lamports)");
            10_000_000u64
        });

        // Build proper tip transaction (SOL transfer to Jito tip account)
        let jito_tip_account =
            Pubkey::from_str("96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU4").map_err(|e| {
                ExecutorError::TransactionFailed(format!("Invalid Jito tip account: {}", e))
            })?;
        let recent_blockhash = self.rpc_client.get_latest_blockhash().await.map_err(|e| {
            ExecutorError::Rpc(format!("Failed to get blockhash for tip tx: {}", e))
        })?;
        let tip_instruction = system_instruction::transfer(
            &tip_keypair.pubkey(),
            &jito_tip_account,
            tip_lamports,
        );
        let mut tip_tx = Transaction::new_with_payer(&[tip_instruction], Some(&tip_keypair.pubkey()));
        tip_tx.sign(&[tip_keypair], recent_blockhash);
        let tip_tx_bytes =
            bincode::serde::encode_to_vec(&tip_tx, bincode::config::legacy()).map_err(|e| {
                ExecutorError::TransactionFailed(format!("Failed to serialize tip tx: {}", e))
            })?;
        let tip_tx_base64 = BASE64.encode(&tip_tx_bytes);

        // Proper two-transaction bundle: [tip_tx, swap_tx] (tip first per Jito spec)
        let swap_tx_base64 = BASE64.encode(tx_bytes);
        let payload = serde_json::json!({
            "transactions": [tip_tx_base64, swap_tx_base64],
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

        // Parse response to get bundle signature
        let result: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ExecutorError::Rpc(format!("Failed to parse Helius response: {}", e)))?;

        // Extract signature from response
        let signature = result
            .get("signature")
            .or_else(|| result.get("bundleId"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ExecutorError::Rpc("No signature in Helius response".to_string()))?;

        Ok(signature.to_string())
    }

    /// Execute via standard TPU
    async fn execute_standard(&self, signal: &Signal) -> Result<String, ExecutorError> {
        tracing::info!(
            trade_uuid = %signal.trade_uuid,
            "Executing trade via standard TPU"
        );

        // Check if devnet simulation mode is enabled
        if self.config.jupiter.devnet_simulation_mode {
            tracing::info!(
                trade_uuid = %signal.trade_uuid,
                "Devnet simulation mode: skipping RPC submission, returning simulated signature"
            );
            // Return a simulated signature (format: "simulated_<uuid>")
            return Ok(format!("simulated_{}", signal.trade_uuid));
        }

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
            TransactionBuilder::new(active_client.clone(), self.config.clone());
        let built_tx = transaction_builder
            .build_swap_transaction(signal, &wallet_keypair)
            .await
            .map_err(|e| {
                ExecutorError::TransactionFailed(format!("Failed to build transaction: {}", e))
            })?;

        // Capture actual price impact and fill price from Jupiter quote
        *self.last_price_impact.lock() = built_tx.price_impact_pct();
        *self.last_fill_price_lamports_per_base.lock() = built_tx.fill_price_lamports_per_base();

        // Check total execution cost cap
        self.check_execution_costs(signal, built_tx.price_impact_pct(), Decimal::ZERO)?;

        // Submit transaction via RPC
        let signature = match &built_tx {
            crate::engine::transaction_builder::BuiltTransaction::Legacy {
                transaction, ..
            } => {
                self.submit_transaction(transaction, &wallet_keypair)
                    .await?
            }
            crate::engine::transaction_builder::BuiltTransaction::Versioned {
                transaction_bytes,
                ..
            } => {
                self.submit_versioned_transaction(transaction_bytes, &wallet_keypair)
                    .await?
            }
        };

        Ok(signature)
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
    /// Properly signs the VersionedTransaction with updated blockhash
    async fn submit_versioned_transaction(
        &self,
        transaction_bytes: &[u8],
        wallet_keypair: &solana_sdk::signature::Keypair,
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

        // Validate Jupiter's blockhash
        let jupiter_blockhash = versioned_tx.message.recent_blockhash();
        let active_client = self.active_rpc_client();
        let is_valid = match active_client
            .is_blockhash_valid(jupiter_blockhash, CommitmentConfig::processed())
            .await
        {
            Ok(valid) => valid,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    blockhash = %jupiter_blockhash,
                    "Failed to validate blockhash, assuming invalid for safety"
                );
                false
            }
        };

        // Get recent blockhash (use active RPC client)
        // We need this for both validation and reconstruction
        let recent_blockhash = active_client.get_latest_blockhash().await.map_err(|e| {
            tracing::error!(error = %e, "Failed to get blockhash");
            ExecutorError::Rpc(format!("Failed to get blockhash: {}", e))
        })?;

        tracing::debug!(blockhash = %recent_blockhash, "Got recent blockhash");

        if !is_valid {
            // Check if this is a V0 transaction (uses ALTs)
            let is_v0 = matches!(
                versioned_tx.message,
                solana_sdk::message::VersionedMessage::V0(_)
            );
            if is_v0 {
                tracing::warn!(
                    blockhash = %jupiter_blockhash,
                    "V0 transaction blockhash expired/invalid. Will attempt reconstruction with fresh blockhash."
                );
                // Continue processing - reconstruction will happen below
            } else {
                tracing::warn!(
                    blockhash = %jupiter_blockhash,
                    "Legacy transaction blockhash expired/invalid. Re-requesting fresh quote from Jupiter."
                );
                return Err(ExecutorError::BlockhashExpired);
            }
        }

        // For VersionedTransaction, we need to manually sign the message hash
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

                // V0 messages use Address Lookup Tables (ALTs) and require reconstruction
                // to update the blockhash. Attempt to reconstruct with fresh blockhash if enabled.
                if self.config.jupiter.reconstruct_v0_on_blockhash_expiry {
                    tracing::debug!(
                        "V0 transaction detected: Attempting to reconstruct message with fresh blockhash"
                    );

                    match v0_reconstruction::reconstruct_v0_message_with_blockhash(
                        &versioned_tx,
                        recent_blockhash,
                        &active_client,
                    )
                    .await
                    {
                        Ok(reconstructed) => {
                            tracing::info!(
                                "Successfully reconstructed V0 message with fresh blockhash"
                            );
                            reconstructed
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "Failed to reconstruct V0 message, falling back to original message. \
                                Transaction may fail if blockhash is stale."
                            );
                            // Fallback to original message if reconstruction fails
                            // This maintains backward compatibility
                            versioned_tx.message.clone()
                        }
                    }
                } else {
                    tracing::debug!(
                        "V0 transaction detected: Reconstruction disabled, using original message"
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

        // Serialize the signed transaction using bincode 1.3 for Solana compatibility
        // bincode 2.0 (standard) uses u64 for lengths, but Solana requires u16/u32 logic
        // consistent with bincode 1.3
        let signed_bytes = bincode1::serialize(&signed_tx).map_err(|e| {
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
                    "skipPreflight": true, // Skip preflight to avoid address lookup table validation issues
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

    /// Calculate dynamic Jito tip based on strategy and history
    pub fn calculate_jito_tip(&self, signal: &Signal) -> Decimal {
        // Use TipManager if available, otherwise fall back to simple strategy-based calculation
        if let Some(ref tip_manager) = self.tip_manager {
            // Pass Decimal directly - no conversion needed
            tip_manager.calculate_tip(signal.payload.strategy, signal.payload.amount_sol)
        } else {
            // Fallback to simple strategy-based tip calculation
            let base_tip = match signal.payload.strategy {
                Strategy::Shield => self.config.jito.tip_floor_sol,
                Strategy::Spear => {
                    // Use higher tip for Spear to ensure bundle inclusion
                    (self.config.jito.tip_floor_sol + self.config.jito.tip_ceiling_sol)
                        / Decimal::from(2)
                }
                Strategy::Exit => self.config.jito.tip_ceiling_sol, // Max tip for exits
            };

            // Apply percentage cap
            let max_by_percent = signal.payload.amount_sol * self.config.jito.tip_percent_max;
            let tip = base_tip
                .min(max_by_percent)
                .min(self.config.jito.tip_ceiling_sol);

            tip.max(self.config.jito.tip_floor_sol)
        }
    }

    /// Check if the total execution costs (tip + fee + slippage) exceed the configured limit
    fn check_execution_costs(&self, signal: &Signal, price_impact_pct: Option<Decimal>, tip: Decimal) -> Result<(), ExecutorError> {
        let dex_fee = signal.payload.amount_sol * self.config.strategy.dex_fee_rate;
        let price_impact = price_impact_pct
            .map(|pct| pct / Decimal::from(100))
            .unwrap_or(Decimal::ZERO);
        let slippage = signal.payload.amount_sol * price_impact;
        let total_cost = tip + dex_fee + slippage;
        let cost_pct = if !signal.payload.amount_sol.is_zero() {
            total_cost / signal.payload.amount_sol
        } else {
            Decimal::ZERO
        };
        
        let mut limit = match signal.payload.strategy {
            Strategy::Shield => self.config.strategy.shield_max_total_cost_percent,
            Strategy::Spear => self.config.strategy.spear_max_total_cost_percent,
            _ => Decimal::ZERO,
        };

        // Apply dynamic limit expansion in high volatility regimes
        if limit > Decimal::ZERO {
            if let Some(ref cache) = self.price_cache {
                // Inline import to avoid cluttering file top
                use crate::engine::market_regime::{MarketRegime, MarketRegimeDetector};
                use std::str::FromStr;
                
                let detector = MarketRegimeDetector::new(cache.clone());
                let regime = detector.detect_effective_regime(signal.token_address());
                
                let multiplier = match regime {
                    MarketRegime::Bull | MarketRegime::Bear => Decimal::from_str("1.5").unwrap(), // Allow 50% more slippage in fast markets
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
        if self.config.rpc.fallback_url.is_some() {
            let (reason, previous_mode) = {
                let state = self.mutable.lock();
                let reason = format!(
                    "Consecutive RPC failures ({}) exceeded threshold",
                    state.failure_count
                );
                let previous_mode = state.rpc_mode;
                (reason, previous_mode)
            };

            tracing::warn!(
                previous_mode = ?previous_mode,
                "Switching to fallback RPC mode"
            );

            {
                let mut state = self.mutable.lock();
                state.rpc_mode = RpcMode::Standard;
                state.fallback_since = Some(Utc::now());
                state.failure_count = 0;
            }

            // Send notification
            self.notify(NotificationEvent::RpcFallback {
                reason: reason.clone(),
            })
            .await;

            // Log to config audit
            if let Err(e) = crate::db::log_config_change(
                &self.db,
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

    /// Get fill price in SOL per token from the most recent Jupiter quote.
    /// Assumes 9-decimal tokens (standard SPL). Returns None if unavailable.
    pub fn get_last_fill_price_sol_per_token(&self) -> Option<Decimal> {
        let lamports_per_base = (*self.last_fill_price_lamports_per_base.lock())?;
        // Convert lamports/base_unit → SOL/token: divide by 1e9 (lamports→SOL) / 1e9 (base→token)
        // For 9-decimal token: price_sol_per_token = lamports_per_base_unit / 1e9
        let lamports_per_sol = Decimal::from(1_000_000_000u64);
        Some(lamports_per_base / lamports_per_sol)
    }

    /// Get time spent in fallback mode
    pub fn fallback_duration(&self) -> Option<chrono::Duration> {
        self.mutable.lock().fallback_since
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
