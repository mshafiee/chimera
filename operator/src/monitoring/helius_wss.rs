//! Helius LaserStream WebSocket client for real-time transaction monitoring
//!
//! Provides a persistent WebSocket connection to Helius LaserStream for sub-second
//! transaction detection, eliminating HTTP cold starts and reducing latency.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tokio_util::sync::CancellationToken;

use super::{
    helius_wss_health::WebSocketHealth, helius_wss_subscription::SubscriptionManager,
    ExitDetector, RateLimiter,
};
use crate::circuit_breaker::CircuitBreaker;
use crate::db_abstraction::Database;
use crate::engine::EngineHandle;
use crate::models::{Action, Signal, SignalPayload, Strategy};
use crate::token::TokenParser;

/// Configuration for LaserStream WebSocket client
#[derive(Debug, Clone)]
pub struct LaserStreamConfig {
    /// WebSocket URL (wss://mainnet.helius-rpc.com/?api-key=...)
    pub websocket_url: String,
    /// Reconnection configuration
    pub reconnect: ReconnectConfig,
    /// Health check timeout (seconds)
    pub health_timeout_secs: u64,
    /// Commitment level for subscriptions (processed, confirmed, finalized)
    pub commitment: String,
}

/// Reconnection configuration with exponential backoff
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// Initial backoff in seconds
    pub initial_backoff_secs: u64,
    /// Maximum backoff in seconds
    pub max_backoff_secs: u64,
    /// Backoff multiplier (e.g., 2.0 for exponential doubling)
    pub backoff_multiplier: f64,
    /// Maximum retry attempts (0 = infinite retries)
    pub max_attempts: u32,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            initial_backoff_secs: 1,
            max_backoff_secs: 60,
            backoff_multiplier: 2.0,
            max_attempts: 0, // Infinite retries
        }
    }
}

/// WebSocket connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

/// LaserStream WebSocket client
pub struct LaserStreamClient {
    db: Arc<dyn Database>,
    engine: EngineHandle,
    config: LaserStreamConfig,
    circuit_breaker: Arc<CircuitBreaker>,
    token_parser: Arc<TokenParser>,
    rate_limiter: Arc<RateLimiter>,
    helius_client: Arc<super::HeliusClient>,
    health: Arc<WebSocketHealth>,
    subscription_manager: Arc<SubscriptionManager>,
    pending_exits: Arc<RwLock<Vec<super::ExitSignal>>>,
    exit_detector: Arc<ExitDetector>,
}

impl LaserStreamClient {
    pub fn new(
        db: Arc<dyn Database>,
        engine: EngineHandle,
        config: LaserStreamConfig,
        circuit_breaker: Arc<CircuitBreaker>,
        token_parser: Arc<TokenParser>,
        helius_client: Arc<super::HeliusClient>,
        exit_detector: Arc<ExitDetector>,
    ) -> Self {
        let rate_limiter = Arc::new(RateLimiter::new(40, 1));
        let health = Arc::new(WebSocketHealth::new(config.health_timeout_secs));
        let subscription_manager = Arc::new(SubscriptionManager::new(
            db.clone(),
            config.websocket_url.clone(),
            config.commitment.clone(),
        ));

        Self {
            db,
            engine,
            config,
            circuit_breaker,
            token_parser,
            rate_limiter,
            helius_client,
            health,
            subscription_manager,
            pending_exits: Arc::new(RwLock::new(Vec::new())),
            exit_detector,
        }
    }

    /// Start the WebSocket client
    pub async fn start(&self, cancel_token: CancellationToken) -> Result<()> {
        tracing::info!(
            url = %self.config.websocket_url,
            commitment = %self.config.commitment,
            "Starting Helius LaserStream WebSocket client"
        );

        let mut retry_count = 0u32;

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("WebSocket client shutting down");
                    return Ok(());
                }
                result = self.connect_and_run() => {
                    match result {
                        Ok(_) => {
                            tracing::info!("WebSocket connection closed gracefully");
                            retry_count = 0; // Reset on successful close
                        }
                        Err(e) => {
                            retry_count += 1;
                            tracing::warn!(
                                error = %e,
                                retry_count = retry_count,
                                "WebSocket connection failed"
                            );

                            // Record failure for circuit breaker
                            self.health.record_failure();

                            // Check if we should stop retrying
                            if self.config.reconnect.max_attempts > 0
                                && retry_count >= self.config.reconnect.max_attempts
                            {
                                tracing::error!(
                                    "Max retry attempts reached ({})",
                                    self.config.reconnect.max_attempts
                                );
                                return Err(e.context("Max WebSocket reconnection attempts reached"));
                            }

                            // Calculate backoff with exponential increase
                            let backoff_secs = self.calculate_backoff(retry_count);
                            tracing::info!(
                                backoff_secs = backoff_secs,
                                "Reconnecting in {} seconds",
                                backoff_secs
                            );

                            // Wait before reconnecting
                            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                        }
                    }
                }
            }
        }
    }

    /// Calculate exponential backoff with jitter
    fn calculate_backoff(&self, retry_count: u32) -> u64 {
        let base_delay = self.config.reconnect.initial_backoff_secs as f64;
        let multiplier = self.config.reconnect.backoff_multiplier;
        let max_delay = self.config.reconnect.max_backoff_secs as f64;

        // Exponential backoff
        let exponential_delay = base_delay * multiplier.powi(retry_count as i32 - 1);

        // Cap at maximum
        let capped_delay = exponential_delay.min(max_delay);

        // Add jitter (±20%)
        let jitter = capped_delay * 0.2 * (rand::random::<f64>() - 0.5);
        let final_delay = (capped_delay + jitter).max(0.0);

        final_delay as u64
    }

    /// Connect to WebSocket and run the connection loop
    async fn connect_and_run(&self) -> Result<()> {
        self.health.set_state(ConnectionState::Connecting).await;

        tracing::info!("Connecting to Helius LaserStream WebSocket");

        // Connect to WebSocket
        let ws_stream = tokio_tungstenite::connect_async(&self.config.websocket_url)
            .await
            .context("Failed to connect to WebSocket")?
            .0;

        self.health.set_state(ConnectionState::Connected).await;
        self.health.reset_failures();
        tracing::info!("WebSocket connection established");

        // Sync subscriptions to ACTIVE wallets
        if let Err(e) = self.subscription_manager.sync_active_wallets().await {
            tracing::warn!(error = %e, "Failed to sync wallet subscriptions");
        }

        // Run connection loop
        let result = self.connection_loop(ws_stream).await;

        // Update state on disconnect
        self.health.set_state(ConnectionState::Disconnected).await;
        tracing::info!("WebSocket disconnected");

        result
    }

    /// Main connection loop for processing WebSocket messages
    async fn connection_loop(
        &self,
        mut ws_stream: WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Result<()> {
        // Spawn background task for periodic health checks
        let cancel_token = CancellationToken::new();
        let health_clone = self.health.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    _ = interval.tick() => {
                        if !health_clone.is_healthy().await {
                            tracing::warn!("WebSocket unhealthy, initiating reconnection");
                            break;
                        }
                    }
                }
            }
        });

        // Process messages
        while let Some(message_result) = ws_stream.next().await {
            let message = message_result.context("WebSocket error")?;

            match message {
                Message::Text(text) => {
                    self.health.record_message().await;
                    if let Err(e) = self.handle_text_message(text).await {
                        tracing::warn!(error = %e, "Failed to handle WebSocket message");
                    }
                }
                Message::Ping(data) => {
                    // Respond to ping with pong
                    if let Err(e) = ws_stream.send(Message::Pong(data)).await {
                        tracing::warn!(error = %e, "Failed to send pong");
                    }
                }
                Message::Pong(_) => {
                    // Server acknowledged our ping
                    self.health.record_pong().await;
                }
                Message::Close(_) => {
                    tracing::info!("WebSocket close received");
                    break;
                }
                Message::Binary(data) => {
                    tracing::warn!(len = data.len(), "Received unexpected binary message");
                }
                Message::Frame(_) => {
                    tracing::warn!("Received unexpected frame message");
                }
            }
        }

        Ok(())
    }

    /// Handle text message from WebSocket
    async fn handle_text_message(&self, text: String) -> Result<()> {
        // Parse JSON-RPC message
        let value: serde_json::Value = serde_json::from_str(&text)
            .context("Failed to parse WebSocket JSON")?;

        // Check if this is a subscription notification
        if let Some(method) = value.get("method").and_then(|m| m.as_str()) {
            match method {
                "subscriptionNotification" => {
                    if let Err(e) = self.handle_subscription_notification(&value).await {
                        tracing::warn!(error = %e, "Failed to handle subscription notification");
                    }
                }
                "pong" => {
                    self.health.record_pong().await;
                }
                _ => {
                    tracing::debug!(method = method, "Received unhandled WebSocket method");
                }
            }
        }

        Ok(())
    }

    /// Handle subscription notification (transaction event)
    async fn handle_subscription_notification(&self, value: &serde_json::Value) -> Result<()> {
        // Extract transaction data from subscription notification
        if let Some(result) = value.get("params").and_then(|p| p.get("result")) {
            if let Some(transaction) = result.get("transaction") {
                let tx: WebSocketTransaction = serde_json::from_value(transaction.clone())
                    .context("Failed to parse transaction")?;

                // Process the transaction
                if let Err(e) = self.process_websocket_transaction(tx).await {
                    tracing::warn!(error = %e, "Failed to process WebSocket transaction");
                }
            }
        }

        Ok(())
    }

    /// Process transaction received via WebSocket
    async fn process_websocket_transaction(&self, tx: WebSocketTransaction) -> Result<()> {
        // Extract wallet address from transaction
        let wallet_address = self.extract_wallet_address(&tx)?;

        // Check circuit breaker
        if !self.circuit_breaker.is_trading_allowed() {
            tracing::debug!("Circuit breaker triggered, skipping transaction");
            return Ok(());
        }

        // Verify wallet is ACTIVE
        let wallet = match self.db.get_wallet(&wallet_address).await {
            Ok(Some(w)) => w,
            Ok(None) => {
                tracing::debug!(wallet = %wallet_address, "Wallet not found in database");
                return Ok(());
            }
            Err(e) => {
                tracing::warn!(error = %e, wallet = %wallet_address, "Failed to query wallet");
                return Ok(());
            }
        };

        if wallet.status != "ACTIVE" {
            tracing::debug!(wallet = %wallet_address, status = %wallet.status, "Wallet not active");
            return Ok(());
        }

        // Parse transaction to extract swap details
        let transaction_json = serde_json::to_value(&tx.transaction)
            .context("Failed to serialize transaction data")?;

        let transaction_info = match super::transaction_parser::parse_transaction(&transaction_json, &wallet_address) {
            Ok(info) => info,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse transaction");
                return Ok(());
            }
        };

        let parsed_swap = match &transaction_info.parsed_swap {
            Some(parsed) => parsed,
            None => {
                tracing::debug!("Transaction is not a relevant swap");
                return Ok(());
            }
        };

        // Generate signal
        let signal = self.generate_signal(parsed_swap, &wallet_address)?;

        // Token safety fast-path check
        if let Some(token_address) = &signal.payload.token_address {
            if let Err(e) = self
                .token_parser
                .fast_check(token_address, signal.payload.strategy)
                .await
            {
                tracing::warn!(
                    error = %e,
                    token = %token_address,
                    "Token safety check failed"
                );
                return Ok(());
            }
        }

        // Queue signal with wallet WQS score
        let wallet_wqs = wallet
            .wqs_score
            .map(|score| score.to_f64().unwrap_or(0.0))
            .unwrap_or(0.0);

        if let Err(e) = self
            .engine
            .queue_signal(signal, Some(wallet_wqs))
            .await
        {
            tracing::warn!(error = %e, "Failed to queue signal");
            return Err(anyhow::anyhow!("Failed to queue signal: {}", e));
        }

        Ok(())
    }

    /// Extract wallet address from WebSocket transaction
    fn extract_wallet_address(&self, tx: &WebSocketTransaction) -> Result<String> {
        // Extract from transaction accounts
        if let Some(account_keys) = tx.transaction.message.get("accountKeys") {
            if let Some(keys) = account_keys.as_array() {
                if !keys.is_empty() {
                    // First account is typically the fee payer/signer
                    if let Some(address) = keys[0].as_str() {
                        return Ok(address.to_string());
                    }
                }
            }
        }

        Err(anyhow::anyhow!("Failed to extract wallet address from transaction"))
    }

    /// Generate signal from parsed swap
    fn generate_signal(
        &self,
        parsed_swap: &super::ParsedSwap,
        wallet_address: &str,
    ) -> Result<Signal> {

        let action = match parsed_swap.direction {
            super::SwapDirection::Buy => Action::Buy,
            super::SwapDirection::Sell => Action::Sell,
        };

        let strategy = match action {
            Action::Buy => Strategy::Shield, // Conservative for WebSocket signals
            Action::Sell => Strategy::Exit,
        };

        let payload = SignalPayload {
            strategy,
            token: parsed_swap.token_out.clone(),
            token_address: Some(parsed_swap.token_out.clone()),
            action,
            amount_sol: parsed_swap.amount_in,
            wallet_address: wallet_address.to_string(),
            trade_uuid: None,
            exit_fraction: None,
        };

        Ok(Signal {
            trade_uuid: payload.generate_trade_uuid(chrono::Utc::now().timestamp_millis()),
            payload,
            timestamp: chrono::Utc::now().timestamp_millis(),
            source_ip: None,
            liquidity_usd: None,
            force_slow_path: false,
            token_decimals: None,
        })
    }
}

/// Transaction received from Helius WebSocket
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketTransaction {
    pub signature: String,
    pub transaction: TransactionData,
}

/// Transaction data
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionData {
    pub message: serde_json::Value,
    pub meta: Option<serde_json::Value>,
}
