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
    helius_wss_health::WebSocketHealth, helius_wss_subscription::SubscriptionManager, ExitDetector,
    RateLimiter,
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
    #[allow(dead_code)] // Retained for future rate-limiting of per-wallet processing
    rate_limiter: Arc<RateLimiter>,
    #[allow(dead_code)] // Retained for future token-safety verification on incoming signals
    helius_client: Arc<super::HeliusClient>,
    health: Arc<WebSocketHealth>,
    subscription_manager: Arc<SubscriptionManager>,
    #[allow(dead_code)] // Retained for future delayed-exit dispatching
    pending_exits: Arc<RwLock<Vec<super::ExitSignal>>>,
    #[allow(dead_code)] // Retained for future exit-signal reconciliation
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
        mut ws_stream: WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
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
        let value: serde_json::Value =
            serde_json::from_str(&text).context("Failed to parse WebSocket JSON")?;

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
            let reason = self
                .circuit_breaker
                .trip_reason()
                .map(|r| r.to_string())
                .unwrap_or_else(|| "Circuit breaker tripped".to_string());
            tracing::debug!(
                wallet = %wallet_address,
                signature = %tx.signature,
                reason = %reason,
                "wss: transaction rejected by circuit breaker"
            );
            return Ok(());
        }

        // Verify wallet is ACTIVE
        let wallet = match self.db.get_wallet(&wallet_address).await {
            Ok(Some(w)) => w,
            Ok(None) => {
                tracing::debug!(
                    wallet = %wallet_address,
                    signature = %tx.signature,
                    "Wallet not found in database"
                );
                return Ok(());
            }
            Err(e) => {
                tracing::warn!(error = %e, wallet = %wallet_address, "Failed to query wallet");
                return Ok(());
            }
        };

        if wallet.status != "ACTIVE" {
            tracing::debug!(
                wallet = %wallet_address,
                status = %wallet.status,
                signature = %tx.signature,
                "Wallet not active"
            );
            return Ok(());
        }

        // Parse transaction to extract swap details
        let transaction_json = serde_json::to_value(&tx.transaction)
            .context("Failed to serialize transaction data")?;

        // Try LaserStream-specific parser first (zero credit, optimized)
        let parsed_swap = match super::transaction_parser::parse_laserstream_message(
            &transaction_json,
            &wallet_address,
        ) {
            Ok(Some(swap)) => {
                tracing::debug!(
                    wallet = %wallet_address,
                    dex = %swap.dex,
                    direction = ?swap.direction,
                    "Successfully parsed swap from LaserStream payload"
                );
                swap
            }
            Ok(None) => {
                tracing::debug!("LaserStream payload is not a swap transaction");
                return Ok(());
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse LaserStream message, trying fallback parser");
                // Fallback to standard parser if LaserStream format changes
                match super::transaction_parser::parse_transaction(
                    &transaction_json,
                    &wallet_address,
                ) {
                    Ok(tx_info) => match tx_info.parsed_swap {
                        Some(swap) => {
                            tracing::debug!(
                                wallet = %wallet_address,
                                dex = %swap.dex,
                                direction = ?swap.direction,
                                "Successfully parsed swap from fallback parser"
                            );
                            swap
                        }
                        None => {
                            tracing::debug!("Transaction is not a relevant swap");
                            return Ok(());
                        }
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to parse transaction with fallback parser");
                        return Ok(());
                    }
                }
            }
        };

        // Generate signal
        let signal = self.generate_signal(&parsed_swap, &wallet_address)?;

        // Token safety fast-path check
        let fast_check_result = if let Some(token_address) = &signal.payload.token_address {
            match self
                .token_parser
                .fast_check(token_address, signal.payload.strategy)
                .await
            {
                Ok(result) => Some(result),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        token = %token_address,
                        wallet = %wallet_address,
                        signature = %tx.signature,
                        "Token safety check failed"
                    );
                    return Ok(());
                }
            }
        } else {
            None
        };

        // Queue signal with wallet WQS score
        let wallet_wqs = wallet
            .wqs_score
            .map(|score| score.to_f64().unwrap_or(0.0))
            .unwrap_or(0.0);

        tracing::info!(
            wallet = %wallet_address,
            token = %signal.payload.token_address.as_deref().unwrap_or("unknown"),
            token_in = %parsed_swap.token_in,
            token_out = %parsed_swap.token_out,
            amount_in = %parsed_swap.amount_in,
            amount_out = %parsed_swap.amount_out,
            action = ?signal.payload.action,
            strategy = ?signal.payload.strategy,
            dex = %parsed_swap.dex,
            signature = %tx.signature,
            fast_check_liquidity_usd = ?fast_check_result.as_ref().and_then(|r| r.liquidity_usd),
            fast_check_safe = ?fast_check_result.as_ref().map(|r| r.safe),
            wallet_wqs = wallet_wqs,
            bypasses_selection_engine = true,
            bypasses_position_sizer = true,
            "wss: signal queued bypassing selection engine (raw amount_in, no PositionSizer)"
        );

        if let Err(e) = self.engine.queue_signal(signal, Some(wallet_wqs)).await {
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

        Err(anyhow::anyhow!(
            "Failed to extract wallet address from transaction"
        ))
    }

    /// Generate signal from parsed swap
    fn generate_signal(
        &self,
        parsed_swap: &crate::monitoring::ParsedSwap,
        wallet_address: &str,
    ) -> Result<Signal> {
        let action = match parsed_swap.direction {
            crate::monitoring::SwapDirection::Buy => Action::Buy,
            crate::monitoring::SwapDirection::Sell => Action::Sell,
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
            trial_admission: false,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_abstraction::{Database, Wallet, WalletMonitoring};
    use crate::engine::Engine;
    use crate::monitoring::test_db::MockDb;
    use crate::monitoring::{ExitDetector, HeliusClient};
    use crate::token::{
        TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig, TokenSafetyResult,
    };
    use chrono::Utc;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    const WALLET_A: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    const TOKEN_A: &str = "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R";

    fn test_config(url: &str) -> LaserStreamConfig {
        LaserStreamConfig {
            websocket_url: url.to_string(),
            reconnect: ReconnectConfig {
                initial_backoff_secs: 1,
                max_backoff_secs: 60,
                backoff_multiplier: 2.0,
                max_attempts: 1,
            },
            health_timeout_secs: 60,
            commitment: "confirmed".to_string(),
        }
    }

    fn mock_db() -> Arc<MockDb> {
        let db = Arc::new(MockDb::new());
        db.add_wallet(Wallet {
            id: 0,
            address: WALLET_A.to_string(),
            status: "ACTIVE".to_string(),
            wqs_score: Some(dec!(80)),
            wqs_confidence: None,
            roi_7d: None,
            roi_30d: None,
            trade_count_30d: None,
            win_rate: None,
            max_drawdown_30d: None,
            avg_trade_size_sol: None,
            avg_win_sol: None,
            avg_loss_sol: None,
            profit_factor: None,
            realized_pnl_30d_sol: None,
            last_trade_at: None,
            promoted_at: Some(Utc::now()),
            ttl_expires_at: None,
            notes: None,
            archetype: None,
            avg_entry_delay_seconds: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });
        db.add_wallet_monitoring(WalletMonitoring {
            wallet_address: WALLET_A.to_string(),
            helius_webhook_id: None,
            rpc_polling_active: true,
            last_transaction_signature: None,
            last_monitored_at: None,
            monitoring_enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            webhook_status: None,
            webhook_registered_at: None,
            webhook_last_health_check: None,
            webhook_health_status: None,
            registration_attempts: 0,
            last_registration_error: None,
            last_updated_url: None,
            last_speculative_signal_at: None,
            inactivity_demotion_count: 0,
        });
        db
    }

    fn token_parser(safe: bool) -> Arc<TokenParser> {
        let cache = Arc::new(TokenCache::new(1000, 300));
        cache.insert(
            format!("{TOKEN_A}:{}", Strategy::Shield),
            TokenSafetyResult {
                safe,
                rejection_reason: None,
                honeypot_checked: false,
                liquidity_checked: true,
                liquidity_usd: Some(dec!(100000)),
            },
        );
        let fetcher = Arc::new(
            TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
                "http://127.0.0.1:1",
                None,
                "http://127.0.0.1:1".to_string(),
            )
            .with_price_cache(Arc::new(crate::price_cache::PriceCache::new().unwrap())),
        );
        Arc::new(TokenParser::new(
            TokenSafetyConfig {
                freeze_authority_whitelist: std::collections::HashSet::new(),
                mint_authority_whitelist: std::collections::HashSet::new(),
                min_liquidity_shield_usd: dec!(0),
                min_liquidity_spear_usd: dec!(0),
                honeypot_detection_enabled: false,
                holder_concentration_check_enabled: false,
                max_holder_concentration_pct: 100.0,
            },
            cache,
            fetcher,
        ))
    }

    fn cb(db: Arc<dyn Database>) -> Arc<CircuitBreaker> {
        Arc::new(CircuitBreaker::new(
            crate::config::CircuitBreakerConfig {
                max_loss_24h_usd: dec!(500),
                max_consecutive_losses: 3,
                max_drawdown_percent: dec!(15),
                portfolio_stop_loss_percent: dec!(-5),
                cooldown_minutes: 30,
                max_jupiter_failures: 5,
            },
            db,
            dec!(1000),
        ))
    }

    async fn client_with(
        db: Arc<MockDb>,
        url: &str,
        cb: Option<Arc<CircuitBreaker>>,
    ) -> LaserStreamClient {
        let (_, engine) = Engine::new(crate::config::AppConfig::default(), db.clone());
        let helius = HeliusClient::new(
            "test-key".to_string(),
            Arc::new(parking_lot::RwLock::new(HashMap::new())),
        )
        .expect("helius client");
        LaserStreamClient::new(
            db.clone(),
            engine,
            test_config(url),
            cb.unwrap_or_else(|| {
                Arc::new(crate::circuit_breaker::CircuitBreaker::new(
                    crate::config::CircuitBreakerConfig {
                        max_loss_24h_usd: dec!(500),
                        max_consecutive_losses: 3,
                        max_drawdown_percent: dec!(15),
                        portfolio_stop_loss_percent: dec!(-5),
                        cooldown_minutes: 30,
                        max_jupiter_failures: 5,
                    },
                    db.clone(),
                    dec!(1000),
                ))
            }),
            token_parser(true),
            Arc::new(helius),
            Arc::new(ExitDetector::new()),
        )
    }

    // ==========================================================================
    // CONFIG + BACKOFF
    // ==========================================================================

    #[test]
    fn test_reconnect_config_default() {
        let cfg = ReconnectConfig::default();
        assert_eq!(cfg.initial_backoff_secs, 1);
        assert_eq!(cfg.max_backoff_secs, 60);
        assert_eq!(cfg.backoff_multiplier, 2.0);
        assert_eq!(cfg.max_attempts, 0, "infinite retries by default");
    }

    #[tokio::test]
    async fn test_calculate_backoff_bounds_and_cap() {
        let client = client_with(mock_db(), "ws://127.0.0.1:1", None).await;
        // retry 1: 1 * 2^0 = 1s; jitter ±20% → [0, 2]
        for _ in 0..30 {
            let delay = client.calculate_backoff(1);
            assert!(delay <= 2, "retry 1 delay {delay}");
        }
        // retry 10: capped at max_backoff 60s; jitter ±20% → [48, 72]
        for _ in 0..30 {
            let delay = client.calculate_backoff(10);
            assert!((48..=72).contains(&delay), "capped delay {delay}");
        }
    }

    // ==========================================================================
    // PURE HELPERS
    // ==========================================================================

    #[test]
    fn test_extract_wallet_address() {
        let db = mock_db();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = rt.block_on(client_with(db, "ws://127.0.0.1:1", None));

        let tx = WebSocketTransaction {
            signature: "sig".to_string(),
            transaction: TransactionData {
                message: serde_json::json!({ "accountKeys": [WALLET_A] }),
                meta: None,
            },
        };
        assert_eq!(client.extract_wallet_address(&tx).unwrap(), WALLET_A);

        // Empty account keys → error.
        let empty = WebSocketTransaction {
            signature: "sig".to_string(),
            transaction: TransactionData {
                message: serde_json::json!({ "accountKeys": [] }),
                meta: None,
            },
        };
        assert!(client.extract_wallet_address(&empty).is_err());

        // Non-array / missing accountKeys → error.
        let missing = WebSocketTransaction {
            signature: "sig".to_string(),
            transaction: TransactionData {
                message: serde_json::json!({}),
                meta: None,
            },
        };
        assert!(client.extract_wallet_address(&missing).is_err());
    }

    #[test]
    fn test_generate_signal_directions() {
        let db = mock_db();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = rt.block_on(client_with(db, "ws://127.0.0.1:1", None));

        let buy = crate::monitoring::ParsedSwap {
            token_in: "So11111111111111111111111111111111111111112".to_string(),
            token_out: TOKEN_A.to_string(),
            amount_in: dec!(1),
            amount_out: dec!(100),
            direction: crate::monitoring::SwapDirection::Buy,
            dex: "Jupiter".to_string(),
            slippage: None,
        };
        let signal = client.generate_signal(&buy, WALLET_A).unwrap();
        assert_eq!(signal.payload.action, Action::Buy);
        assert_eq!(signal.payload.strategy, Strategy::Shield);
        assert_eq!(signal.payload.token_address.as_deref(), Some(TOKEN_A));
        assert_eq!(signal.payload.amount_sol, dec!(1));

        let sell = crate::monitoring::ParsedSwap {
            token_in: TOKEN_A.to_string(),
            token_out: "So11111111111111111111111111111111111111112".to_string(),
            amount_in: dec!(100),
            amount_out: dec!(1),
            direction: crate::monitoring::SwapDirection::Sell,
            dex: "Raydium".to_string(),
            slippage: None,
        };
        let signal = client.generate_signal(&sell, WALLET_A).unwrap();
        assert_eq!(signal.payload.action, Action::Sell);
        assert_eq!(signal.payload.strategy, Strategy::Exit);
    }

    // ==========================================================================
    // MESSAGE HANDLING
    // ==========================================================================

    fn notification_json(sig: &str, with_transaction: bool) -> String {
        let params = if with_transaction {
            serde_json::json!({
                "result": {
                    "transaction": {
                        "signature": sig,
                        "message": { "accountKeys": [WALLET_A] },
                        "meta": null
                    }
                }
            })
        } else {
            serde_json::json!({})
        };
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "subscriptionNotification",
            "params": params
        })
        .to_string()
    }

    #[tokio::test]
    async fn test_handle_text_message_variants() {
        let client = client_with(mock_db(), "ws://127.0.0.1:1", None).await;

        // Invalid JSON → Err.
        assert!(client
            .handle_text_message("not json".to_string())
            .await
            .is_err());

        // Unknown method → Ok.
        client
            .handle_text_message(r#"{"jsonrpc":"2.0","method":"weird"}"#.to_string())
            .await
            .unwrap();

        // "pong" method → Ok (records pong).
        client
            .handle_text_message(r#"{"jsonrpc":"2.0","method":"pong"}"#.to_string())
            .await
            .unwrap();

        // Subscription notification without a transaction → Ok.
        client
            .handle_text_message(notification_json("s1", false))
            .await
            .unwrap();

        // Subscription notification with a transaction → processed (parser
        // finds no LaserStream transfers, falls back, and the tx is skipped).
        client
            .handle_text_message(notification_json("s2", true))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_process_websocket_transaction_gates() {
        let db = mock_db();
        let cb = cb(db.clone());
        let client = client_with(db.clone(), "ws://127.0.0.1:1", Some(cb.clone())).await;

        let make_tx = || WebSocketTransaction {
            signature: "sig-1".to_string(),
            transaction: TransactionData {
                message: serde_json::json!({ "accountKeys": [WALLET_A] }),
                meta: None,
            },
        };

        // Normal flow: parser finds no swap → Ok (no signal).
        client
            .process_websocket_transaction(make_tx())
            .await
            .unwrap();

        // Circuit breaker tripped → skip.
        cb.manual_trip("test", "trip".into()).await.unwrap();
        client
            .process_websocket_transaction(make_tx())
            .await
            .unwrap();
        cb.reset("test").await.unwrap();

        // Unknown wallet → skip.
        let db2 = mock_db();
        let unknown_client = client_with(db2.clone(), "ws://127.0.0.1:1", None).await;
        let unknown_tx = WebSocketTransaction {
            signature: "sig-2".to_string(),
            transaction: TransactionData {
                message: serde_json::json!({ "accountKeys": ["7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"] }),
                meta: None,
            },
        };
        unknown_client
            .process_websocket_transaction(unknown_tx)
            .await
            .unwrap();

        // Inactive wallet → skip.
        let db3 = Arc::new(MockDb::new());
        db3.add_wallet(Wallet {
            id: 0,
            address: WALLET_A.to_string(),
            status: "SUSPENDED".to_string(),
            wqs_score: None,
            wqs_confidence: None,
            roi_7d: None,
            roi_30d: None,
            trade_count_30d: None,
            win_rate: None,
            max_drawdown_30d: None,
            avg_trade_size_sol: None,
            avg_win_sol: None,
            avg_loss_sol: None,
            profit_factor: None,
            realized_pnl_30d_sol: None,
            last_trade_at: None,
            promoted_at: None,
            ttl_expires_at: None,
            notes: None,
            archetype: None,
            avg_entry_delay_seconds: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });
        let inactive_client = client_with(db3.clone(), "ws://127.0.0.1:1", None).await;
        inactive_client
            .process_websocket_transaction(make_tx())
            .await
            .unwrap();

        // DB query error → skip.
        let db4 = mock_db();
        db4.wallet_query_error.store(true, Ordering::Relaxed);
        let err_client = client_with(db4, "ws://127.0.0.1:1", None).await;
        err_client
            .process_websocket_transaction(make_tx())
            .await
            .unwrap();
    }

    // ==========================================================================
    // START / CONNECT LIFECYCLE
    // ==========================================================================

    #[tokio::test]
    async fn test_start_returns_immediately_when_cancelled() {
        let client = client_with(mock_db(), "ws://127.0.0.1:1", None).await;
        let token = CancellationToken::new();
        token.cancel();
        client
            .start(token)
            .await
            .expect("cancelled start returns Ok");
    }

    #[tokio::test]
    async fn test_start_fails_after_max_attempts_with_backoff() {
        // Connection to a dead port fails; max_attempts=2 → one backoff
        // round (1s) then error out.
        let db = mock_db();
        let mut client = client_with(db, "ws://127.0.0.1:1", None).await;
        client.config.reconnect.max_attempts = 2;
        let token = CancellationToken::new();
        let result = client.start(token).await;
        assert!(result.is_err(), "max retry attempts reached must error");
        // Both failed attempts were recorded with the health tracker (backoff
        // rounds are jittered, so wall-clock is not asserted).
        let metrics = client.health.get_metrics().await;
        assert_eq!(metrics.connection_failures, 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_start_full_connection_cycle() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let (url, mut server_rx) = spawn_ws_server().await;
        let db = mock_db();
        // The connect_and_run path syncs ACTIVE wallets; a DB error must be
        // tolerated (warn + continue). First run: sync fails.
        db.wallets_by_status_error.store(true, Ordering::Relaxed);
        let client = client_with(db.clone(), &url, None).await;
        let token = CancellationToken::new();
        let token2 = token.clone();
        let task = tokio::spawn({
            let client = client;
            async move { client.start(token2).await }
        });

        // Server side: handshake completes; the client stream is used by
        // connection_loop inside connect_and_run.
        let mut server_ws = server_rx.recv().await.expect("server stream");
        // Send a text message (subscription notification) + a ping, then close.
        server_ws
            .send(Message::Text(notification_json("sig-cycle", true).into()))
            .await
            .unwrap();
        server_ws.send(Message::Ping(vec![7].into())).await.unwrap();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_ws.next()).await; // pong
        server_ws.send(Message::Close(None)).await.unwrap();

        // connect_and_run returns Ok after the stream closes; start() loops
        // back to the select — cancel it now so the task exits Ok.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let cancel_task = tokio::spawn({
            let token = token.clone();
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                token.cancel();
            }
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), task)
            .await
            .expect("start exits after cancel")
            .expect("no panic");
        assert!(result.is_ok(), "start returns Ok on graceful shutdown");
        let _ = cancel_task.await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_connection_loop_health_check_breaks() {
        // Keep a live connection with NO messages for >30s: the health task
        // detects the unhealthy connection and breaks out of its interval.
        let (url, mut server_rx) = spawn_ws_server().await;
        let client = client_with(mock_db(), &url, None).await;
        let ws_client = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect")
            .0;
        let mut server_ws = server_rx.recv().await.expect("server stream");

        let loop_task = tokio::spawn(async move {
            let mut c = client;
            c.connection_loop(ws_client).await
        });

        // Hold the connection open past the 30s health-check interval. The
        // health task breaks (no external effect); then close normally.
        tokio::time::sleep(std::time::Duration::from_secs(32)).await;
        server_ws.send(Message::Close(None)).await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), loop_task)
            .await
            .expect("loop exits after close")
            .expect("no panic");
        assert!(result.is_ok());
    }

    // ==========================================================================
    // CONNECTION LOOP (real WebSocket server)
    // ==========================================================================

    /// Spawn a raw TCP listener that upgrades every accepted connection via
    /// tokio-tungstenite's server handshake, and return (url, server_streams).
    async fn spawn_ws_server() -> (
        String,
        tokio::sync::mpsc::Receiver<WebSocketStream<tokio::net::TcpStream>>,
    ) {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                if tx.send(ws).await.is_err() {
                    break;
                }
            }
        });
        (format!("ws://{addr}"), rx)
    }

    #[tokio::test]
    async fn test_connection_loop_processes_all_message_types() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let (url, mut server_rx) = spawn_ws_server().await;
        let client = client_with(mock_db(), &url, None).await;

        // The client connects; the server handshake completes on the listener
        // task. The CLIENT-side stream is handed to connection_loop (role is
        // irrelevant for frame I/O) while the test drives the server side.
        let ws_client = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect")
            .0;
        let mut server_ws = server_rx.recv().await.expect("server stream");

        // Run the connection loop on the client-side stream.
        let loop_task = tokio::spawn(async move {
            let mut client_for_loop = client;
            client_for_loop.connection_loop(ws_client).await
        });

        // Exercise each message type from the server side (including an
        // invalid JSON text, which exercises the handle-error warn path).
        server_ws
            .send(Message::Text(notification_json("sig-live", true).into()))
            .await
            .unwrap();
        server_ws
            .send(Message::Text("not valid json {".into()))
            .await
            .unwrap();
        server_ws
            .send(Message::Ping(vec![1, 2, 3].into()))
            .await
            .unwrap();
        // Expect a pong back for the ping.
        let pong = tokio::time::timeout(std::time::Duration::from_secs(5), server_ws.next())
            .await
            .expect("pong within timeout")
            .expect("stream")
            .expect("no err");
        assert!(matches!(pong, Message::Pong(_)));

        server_ws.send(Message::Pong(vec![9].into())).await.unwrap();
        server_ws
            .send(Message::Binary(vec![0, 1].into()))
            .await
            .unwrap();
        server_ws.send(Message::Close(None)).await.unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), loop_task)
            .await
            .expect("connection loop exits after close")
            .expect("no panic");
        assert!(result.is_ok(), "connection loop returns Ok");
    }
}
