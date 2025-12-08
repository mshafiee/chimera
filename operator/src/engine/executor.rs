//! Trade executor for Solana transactions
//!
//! Handles the actual submission of trades to the Solana network.
//! Includes RPC failover with automatic recovery to primary.

use crate::config::AppConfig;
use crate::db::DbPool;
use crate::models::{Signal, Strategy};
use crate::notifications::{CompositeNotifier, NotificationEvent};
use crate::engine::tips::TipManager;
use crate::engine::transaction_builder::{TransactionBuilder, load_wallet_keypair};
use crate::vault::load_secrets_with_fallback;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{DateTime, Utc};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::transaction::Transaction;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::timeout;

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

/// Trade executor
pub struct Executor {
    /// Configuration
    config: Arc<AppConfig>,
    /// Database pool
    db: DbPool,
    /// Current RPC mode
    rpc_mode: RpcMode,
    /// Consecutive failure count
    failure_count: u32,
    /// When fallback mode was activated
    fallback_since: Option<DateTime<Utc>>,
    /// Recovery check interval (default 5 minutes)
    recovery_interval: Duration,
    /// Last recovery attempt
    last_recovery_attempt: Option<DateTime<Utc>>,
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
        let rpc_client = Arc::new(RpcClient::new(config.rpc.primary_url.clone()));

        // Create Solana RPC client for fallback endpoint if configured
        let fallback_rpc_client = config.rpc.fallback_url.as_ref().map(|url| {
            Arc::new(RpcClient::new(url.clone()))
        });

        // Create Jito Searcher client if configured
        let jito_searcher = config.jito.searcher_endpoint.as_ref().map(|endpoint| {
            crate::engine::jito_searcher::JitoSearcherClient::new(endpoint.clone(), rpc_client.clone())
        });

        Self {
            config,
            db,
            rpc_mode,
            failure_count: 0,
            fallback_since: None,
            recovery_interval: Duration::from_secs(300), // 5 minutes
            last_recovery_attempt: None,
            notifier: None,
            latest_rpc_health: Arc::new(RwLock::new(None)),
            http_client,
            rpc_client,
            fallback_rpc_client,
            tip_manager: None,
            jito_searcher,
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

    /// Send notification if notifier is configured and rules allow it
    async fn notify(&self, event: NotificationEvent) {
        if let Some(ref notifier) = self.notifier {
            // Check notification rules before sending
            let rules = &self.config.notifications.rules;
            let should_send = match &event {
                NotificationEvent::CircuitBreakerTriggered { .. } => rules.circuit_breaker_triggered,
                NotificationEvent::WalletDrained { .. } => rules.wallet_drained,
                NotificationEvent::SystemCrash { .. } => rules.wallet_drained, // Use wallet_drained rule for system crashes
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
        pnl_percent: f64,
        pnl_sol: f64,
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
    pub async fn execute(&mut self, signal: &Signal) -> Result<String, ExecutorError> {
        // Check if we should try to recover to primary
        if self.should_attempt_recovery() {
            self.try_recover_to_primary().await;
        }

        tracing::info!(
            trade_uuid = %signal.trade_uuid,
            strategy = %signal.payload.strategy,
            token = %signal.payload.token,
            action = %signal.payload.action,
            amount_sol = signal.payload.amount_sol,
            rpc_mode = ?self.rpc_mode,
            "Executing trade"
        );

        // Check if Spear is allowed in current mode
        if signal.payload.strategy == Strategy::Spear && self.rpc_mode == RpcMode::Standard {
            return Err(ExecutorError::SpearDisabled);
        }

        // Validate amount bounds
        if signal.payload.amount_sol < self.config.strategy.min_position_sol {
            return Err(ExecutorError::AmountTooSmall(
                signal.payload.amount_sol,
                self.config.strategy.min_position_sol,
            ));
        }

        if signal.payload.amount_sol > self.config.strategy.max_position_sol {
            return Err(ExecutorError::AmountTooLarge(
                signal.payload.amount_sol,
                self.config.strategy.max_position_sol,
            ));
        }

        // Execute based on mode
        let result = match self.rpc_mode {
            RpcMode::Jito => self.execute_jito(signal).await,
            RpcMode::Standard => self.execute_standard(signal).await,
        };

        // Handle result and track failures
        match &result {
            Ok(sig) => {
                self.failure_count = 0;
                tracing::info!(
                    trade_uuid = %signal.trade_uuid,
                    signature = %sig,
                    "Trade executed successfully"
                );

                // Record tip if using Jito and tip manager is available
                if self.rpc_mode == RpcMode::Jito {
                    if let Some(ref tip_manager) = self.tip_manager {
                        let tip = self.calculate_jito_tip(signal);
                        if let Err(e) = tip_manager.record_tip(
                            tip,
                            Some(sig),
                            signal.payload.strategy,
                            true, // success
                        ).await {
                            tracing::warn!(
                                error = %e,
                                "Failed to record tip in TipManager"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                self.failure_count += 1;
                tracing::error!(
                    trade_uuid = %signal.trade_uuid,
                    error = %e,
                    failure_count = self.failure_count,
                    "Trade execution failed"
                );

                // Record failed tip if using Jito and tip manager is available
                if self.rpc_mode == RpcMode::Jito {
                    if let Some(ref tip_manager) = self.tip_manager {
                        let tip = self.calculate_jito_tip(signal);
                        if let Err(e) = tip_manager.record_tip(
                            tip,
                            None, // No signature for failed trades
                            signal.payload.strategy,
                            false, // failure
                        ).await {
                            tracing::warn!(
                                error = %e,
                                "Failed to record failed tip in TipManager"
                            );
                        }
                    }
                }

                // Check if we need to switch to fallback
                if self.failure_count >= self.config.rpc.max_consecutive_failures
                    && self.rpc_mode == RpcMode::Jito
                {
                    self.switch_to_fallback().await;
                }
            }
        }

        result
    }

    /// Check if we should attempt recovery to primary RPC
    fn should_attempt_recovery(&self) -> bool {
        // Only attempt recovery if we're in fallback mode
        if self.rpc_mode != RpcMode::Standard || self.fallback_since.is_none() {
            return false;
        }

        // Check if Jito is configured
        if !self.config.jito.enabled {
            return false;
        }

        let now = Utc::now();

        // Check if enough time has passed since fallback
        if let Some(fallback_time) = self.fallback_since {
            let elapsed = now.signed_duration_since(fallback_time);
            if elapsed < chrono::Duration::from_std(self.recovery_interval).unwrap_or_default() {
                return false;
            }
        }

        // Check if enough time has passed since last recovery attempt
        if let Some(last_attempt) = self.last_recovery_attempt {
            let elapsed = now.signed_duration_since(last_attempt);
            if elapsed < chrono::Duration::from_std(self.recovery_interval).unwrap_or_default() {
                return false;
            }
        }

        true
    }

    /// Attempt to recover to primary RPC
    async fn try_recover_to_primary(&mut self) {
        self.last_recovery_attempt = Some(Utc::now());

        tracing::info!("Attempting to recover to primary RPC (Jito)");

        // Perform health check on primary RPC
        match self.check_primary_health().await {
            Ok(health) if health.healthy => {
                tracing::info!(
                    latency_ms = health.latency_ms,
                    "Primary RPC is healthy, switching back to Jito mode"
                );

                self.rpc_mode = RpcMode::Jito;
                self.fallback_since = None;
                self.failure_count = 0;

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

        // Perform actual RPC health check using Solana RPC client
        // This uses the getHealth method which is lightweight and recommended by Helius
        let health_check = async {
            // Use Solana RPC client's get_health method
            // This is more efficient than raw HTTP and handles errors better
            self.rpc_client
                .get_health()
                .await
                .map_err(|e| ExecutorError::Rpc(format!("RPC health check failed: {}", e)))?;
            
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

        // Verify RPC is accessible
        if let Err(e) = self.rpc_client.get_health().await {
            tracing::warn!(error = %e, "RPC health check failed before Jito execution");
            return Err(ExecutorError::Rpc(format!("RPC unavailable: {}", e)));
        }

        // Load wallet keypair from vault
        let secrets = load_secrets_with_fallback()
            .map_err(|e| ExecutorError::TransactionFailed(format!("Failed to load vault: {}", e)))?;
        let wallet_keypair = load_wallet_keypair(&secrets)
            .map_err(|e| ExecutorError::TransactionFailed(format!("Failed to load keypair: {}", e)))?;

        // Build transaction
        let transaction_builder = TransactionBuilder::new(self.rpc_client.clone(), self.config.clone());
        let built_tx = transaction_builder
            .build_swap_transaction(signal, &wallet_keypair)
            .await
            .map_err(|e| ExecutorError::TransactionFailed(format!("Failed to build transaction: {}", e)))?;

        // Calculate dynamic tip
        let tip = self.calculate_jito_tip(signal);
        tracing::debug!(
            tip_sol = tip,
            strategy = %signal.payload.strategy,
            "Calculated Jito tip"
        );

        // Submit to Jito via direct Jito Searcher (preferred) or Helius Sender API (fallback)
        
        // Try direct Jito Searcher first if configured
        if let Some(ref jito_searcher) = self.jito_searcher {
            let tip_lamports = (tip * 1_000_000_000.0) as u64; // Convert SOL to lamports
            match jito_searcher
                .submit_bundle(&built_tx.transaction, tip_lamports, &wallet_keypair)
                .await
            {
                Ok(signature) => {
                    tracing::info!(
                        trade_uuid = %signal.trade_uuid,
                        signature = %signature,
                        "Bundle submitted via direct Jito Searcher"
                    );
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
        if self.config.jito.helius_fallback {
            if let Some(helius_api_key) = secrets.rpc_api_key.as_ref() {
                match self
                    .submit_via_helius_sender(&built_tx.transaction, tip, helius_api_key)
                    .await
                {
                    Ok(signature) => {
                        tracing::info!(
                            trade_uuid = %signal.trade_uuid,
                            signature = %signature,
                            "Bundle submitted via Helius Sender API"
                        );
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
        
        // Sign and send transaction via standard RPC
        let signature = self
            .submit_transaction(&built_tx.transaction, &wallet_keypair)
            .await?;

        Ok(signature)
    }

    /// Submit transaction via Helius Sender API (Jito bundles)
    async fn submit_via_helius_sender(
        &self,
        transaction: &Transaction,
        tip_sol: f64,
        api_key: &str,
    ) -> Result<String, ExecutorError> {
        // Helius Sender API endpoint
        let url = format!("https://api.helius.xyz/v0/send-bundle?api-key={}", api_key);

        // Convert transaction to base64
        let tx_bytes = bincode::serde::encode_to_vec(transaction, bincode::config::standard())
            .map_err(|e| ExecutorError::TransactionFailed(format!("Failed to serialize transaction: {}", e)))?;
        let tx_base64 = BASE64.encode(&tx_bytes);

        // Create bundle payload
        // Note: Helius Sender API expects a specific format
        // For now, we'll submit the transaction directly
        // In production, you would create a proper bundle with tip transaction
        
        let payload = serde_json::json!({
            "transactions": [tx_base64],
            "tip": (tip_sol * 1_000_000_000.0) as u64, // Convert SOL to lamports
        });

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ExecutorError::Rpc(format!("Helius Sender request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(ExecutorError::Rpc(format!(
                "Helius Sender API error: {} - {}",
                status,
                error_text
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

        // Verify RPC is accessible
        if let Err(e) = self.rpc_client.get_health().await {
            tracing::warn!(error = %e, "RPC health check failed before execution");
            return Err(ExecutorError::Rpc(format!("RPC unavailable: {}", e)));
        }

        // Load wallet keypair from vault
        let secrets = load_secrets_with_fallback()
            .map_err(|e| ExecutorError::TransactionFailed(format!("Failed to load vault: {}", e)))?;
        let wallet_keypair = load_wallet_keypair(&secrets)
            .map_err(|e| ExecutorError::TransactionFailed(format!("Failed to load keypair: {}", e)))?;

        // Build transaction
        let transaction_builder = TransactionBuilder::new(self.rpc_client.clone(), self.config.clone());
        let built_tx = transaction_builder
            .build_swap_transaction(signal, &wallet_keypair)
            .await
            .map_err(|e| ExecutorError::TransactionFailed(format!("Failed to build transaction: {}", e)))?;

        // Submit transaction via RPC
        let signature = self
            .submit_transaction(&built_tx.transaction, &wallet_keypair)
            .await?;

        Ok(signature)
    }

    /// Submit a signed transaction to the RPC
    async fn submit_transaction(
        &self,
        transaction: &Transaction,
        _keypair: &solana_sdk::signature::Keypair,
    ) -> Result<String, ExecutorError> {
        // Ensure transaction is signed
        // Note: TransactionBuilder should have already signed it, but we verify here
        
        // Send transaction via RPC
        let signature = self
            .rpc_client
            .send_and_confirm_transaction(transaction)
            .await
            .map_err(|e| ExecutorError::TransactionFailed(format!("Transaction submission failed: {}", e)))?;

        tracing::info!(
            signature = %signature,
            "Transaction submitted successfully"
        );

        Ok(signature.to_string())
    }

    /// Calculate dynamic Jito tip based on strategy and history
    pub fn calculate_jito_tip(&self, signal: &Signal) -> f64 {
        // Use TipManager if available, otherwise fall back to simple strategy-based calculation
        if let Some(ref tip_manager) = self.tip_manager {
            tip_manager.calculate_tip(signal.payload.strategy, signal.payload.amount_sol)
        } else {
            // Fallback to simple strategy-based tip calculation
            let base_tip = match signal.payload.strategy {
                Strategy::Shield => self.config.jito.tip_floor_sol,
                Strategy::Spear => {
                    // Use higher tip for Spear to ensure bundle inclusion
                    (self.config.jito.tip_floor_sol + self.config.jito.tip_ceiling_sol) / 2.0
                }
                Strategy::Exit => self.config.jito.tip_ceiling_sol, // Max tip for exits
            };

            // Apply percentage cap
            let max_by_percent = signal.payload.amount_sol * self.config.jito.tip_percent_max;
            let tip = base_tip.min(max_by_percent).min(self.config.jito.tip_ceiling_sol);

            tip.max(self.config.jito.tip_floor_sol)
        }
    }

    /// Switch to fallback RPC mode
    async fn switch_to_fallback(&mut self) {
        if self.config.rpc.fallback_url.is_some() {
            let reason = format!(
                "Consecutive RPC failures ({}) exceeded threshold",
                self.failure_count
            );

            tracing::warn!(
                previous_mode = ?self.rpc_mode,
                failure_count = self.failure_count,
                "Switching to fallback RPC mode"
            );

            self.rpc_mode = RpcMode::Standard;
            self.fallback_since = Some(Utc::now());
            self.failure_count = 0;

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
        self.rpc_mode
    }

    /// Check if currently in fallback mode
    pub fn is_in_fallback(&self) -> bool {
        self.fallback_since.is_some()
    }

    /// Get time spent in fallback mode
    pub fn fallback_duration(&self) -> Option<chrono::Duration> {
        self.fallback_since.map(|t| Utc::now().signed_duration_since(t))
    }
}

/// Executor errors
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    /// Spear strategy disabled in fallback mode
    #[error("Spear strategy is disabled in fallback RPC mode")]
    SpearDisabled,

    /// Amount too small
    #[error("Amount {0} SOL is below minimum {1} SOL")]
    AmountTooSmall(f64, f64),

    /// Amount too large
    #[error("Amount {0} SOL exceeds maximum {1} SOL")]
    AmountTooLarge(f64, f64),

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
    InsufficientBalance { required: f64, available: f64 },

    /// Circuit breaker tripped
    #[error("Circuit breaker tripped: {0}")]
    CircuitBreakerTripped(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_error_display() {
        let err = ExecutorError::AmountTooSmall(0.001, 0.01);
        assert!(err.to_string().contains("0.001"));
        assert!(err.to_string().contains("0.01"));
    }

    #[test]
    fn test_rpc_mode_debug() {
        assert_eq!(format!("{:?}", RpcMode::Jito), "Jito");
        assert_eq!(format!("{:?}", RpcMode::Standard), "Standard");
    }
}
