//! Helius webhook integration for automatic transaction monitoring
//!
//! Handles webhook registration, receiving, and processing for ACTIVE wallets.

use crate::monitoring::rate_limiter::RateLimiter;
use crate::monitoring::rate_limiter::RequestPriority;
use chimera_core::retry::{extract_status, retry_with_backoff, HttpStatusError};
use anyhow::{anyhow, Context, Result};
use parking_lot::RwLock;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{sleep, Duration};

/// Helius API client
pub struct HeliusClient {
    api_key: String,
    client: Client,
    base_url: String,
    /// Shared metadata cache (from TokenMetadataFetcher)
    metadata_cache: Arc<RwLock<HashMap<String, crate::token::TokenMetadata>>>,
    /// Cache TTL in seconds (default: 24 hours)
    #[allow(dead_code)] // Retained for future cache-pruning logic
    cache_ttl: u64,
    /// Performance metrics: cache hits (metadata with age available)
    cache_hits: Arc<std::sync::atomic::AtomicU64>,
    /// Performance metrics: cache misses (required Helius API call)
    cache_misses: Arc<std::sync::atomic::AtomicU64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeliusWebhookPayload {
    #[serde(rename = "accountData")]
    pub account_data: Vec<AccountData>,
    #[serde(rename = "nativeTransfers")]
    pub native_transfers: Vec<NativeTransfer>,
    pub signature: String,
    #[serde(rename = "slot")]
    pub slot: u64,
    #[serde(rename = "timestamp")]
    pub timestamp: i64,
    #[serde(rename = "transactionError")]
    pub transaction_error: Option<serde_json::Value>,
    #[serde(rename = "type")]
    pub transaction_type: String,
    /// Helius enhanced webhook `events` object. When present, `events.swap`
    /// gives explicit swapper + tokenInputs/tokenOutputs — far more reliable
    /// than inferring from tokenBalanceChanges (which fails when
    /// userAccount doesn't match for newly-created token accounts, dropping
    /// ~98% of tracked-wallet SWAP events).
    #[serde(default)]
    pub events: WebhookEvents,
}

/// Top-level events container in a Helius enhanced webhook.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebhookEvents {
    #[serde(default)]
    pub swap: Option<WebhookSwapEvent>,
}

/// Explicit swap event from Helius enhanced data. Identifies the swapper
/// and exactly what tokens were given/received — no inference needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSwapEvent {
    #[serde(default)]
    pub swapper: Option<String>,
    #[serde(default, rename = "nativeInput", alias = "native_input")]
    pub native_input: Option<WebhookSwapNativeLeg>,
    #[serde(default, rename = "nativeOutput", alias = "native_output")]
    pub native_output: Option<WebhookSwapNativeLeg>,
    #[serde(default, rename = "tokenInputs", alias = "token_inputs")]
    pub token_inputs: Vec<WebhookSwapTokenLeg>,
    #[serde(default, rename = "tokenOutputs", alias = "token_outputs")]
    pub token_outputs: Vec<WebhookSwapTokenLeg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSwapNativeLeg {
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub amount: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSwapTokenLeg {
    #[serde(default, rename = "userAccount", alias = "user_account")]
    pub user_account: String,
    #[serde(default)]
    pub mint: String,
    #[serde(default, rename = "rawTokenAmount", alias = "raw_token_amount")]
    pub raw_token_amount: Option<RawTokenAmount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountData {
    pub account: String,
    #[serde(rename = "nativeBalanceChange")]
    pub native_balance_change: Option<i64>,
    #[serde(rename = "tokenBalanceChanges")]
    pub token_balance_changes: Option<Vec<TokenBalanceChange>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeTransfer {
    pub amount: u64,
    #[serde(rename = "fromUserAccount", alias = "from_user_account")]
    pub from_user_account: String,
    #[serde(rename = "toUserAccount", alias = "to_user_account")]
    pub to_user_account: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBalanceChange {
    pub mint: String,
    #[serde(rename = "rawTokenAmount")]
    pub raw_token_amount: RawTokenAmount,
    #[serde(rename = "tokenAccount")]
    pub token_account: String,
    #[serde(rename = "userAccount")]
    pub user_account: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawTokenAmount {
    #[serde(rename = "tokenAmount", alias = "token_amount")]
    pub token_amount: String,
    #[serde(default)]
    pub decimals: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebhookRegistration {
    #[serde(rename = "webhookURL")]
    webhook_url: String,
    #[serde(rename = "transactionTypes")]
    transaction_types: Vec<String>,
    #[serde(rename = "accountAddresses")]
    account_addresses: Vec<String>,
    #[serde(rename = "webhookType")]
    webhook_type: String,
    #[serde(rename = "authHeader", skip_serializing_if = "Option::is_none")]
    auth_header: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebhookResponse {
    #[serde(rename = "webhookID")]
    webhook_id: String,
}

/// Helius API metrics for monitoring
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct HeliusMetrics {
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub successful_requests: u64,
    pub retried_requests: u64,
    pub failed_requests: u64,
}

/// Webhook update request structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookUpdate {
    #[serde(rename = "webhookURL", skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    #[serde(rename = "transactionTypes", skip_serializing_if = "Option::is_none")]
    pub transaction_types: Option<Vec<String>>,
    #[serde(rename = "accountAddresses", skip_serializing_if = "Option::is_none")]
    pub account_addresses: Option<Vec<String>>,
    #[serde(rename = "authHeader", skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<serde_json::Value>,
    #[serde(rename = "webhookType", skip_serializing_if = "Option::is_none")]
    pub webhook_type: Option<String>,
}

/// Webhook toggle request structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookToggle {
    #[serde(rename = "isActive")]
    pub is_active: bool,
}

/// Helius webhook details from API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeliusWebhook {
    #[serde(rename = "webhookID")]
    pub webhook_id: String,
    #[serde(rename = "webhookURL")]
    pub webhook_url: String,
    #[serde(rename = "accountAddresses", default)]
    pub wallet_addresses: Vec<String>,
    #[serde(rename = "transactionTypes")]
    pub transaction_types: Vec<String>,
}

/// Webhook reconciliation result for profitability assessment
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebhookReconciliationResult {
    pub total_helius_webhooks: usize,
    pub eligible_wallets: usize,
    pub ineligible_wallets: usize,
    pub deleted_webhooks: usize,
    pub failed_deletions: usize,
    pub would_delete: Vec<(String, String)>, // (webhook_id, reason)
    pub duration_ms: u64,
    pub details: Vec<WebhookReconciliationDetail>,
}

/// Individual webhook reconciliation detail
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebhookReconciliationDetail {
    pub webhook_id: String,
    pub wallet_address: String,
    pub kept: bool,
    pub reason: String,
}

impl HeliusClient {
    pub fn new(
        api_key: String,
        metadata_cache: Arc<RwLock<HashMap<String, crate::token::TokenMetadata>>>,
    ) -> Result<Self> {
        Ok(Self {
            api_key,
            client: Client::new(),
            base_url: chimera_core::utils::helius_api_base_url(),
            metadata_cache,
            cache_ttl: 86400, // 24 hours (immutable token metadata)
            cache_hits: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            cache_misses: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    /// Get current Helius API metrics
    pub fn get_metrics(&self) -> HeliusMetrics {
        let cache_hits = self.cache_hits.load(std::sync::atomic::Ordering::Relaxed);
        let cache_misses = self.cache_misses.load(std::sync::atomic::Ordering::Relaxed);
        let _cache_size = self.metadata_cache.read().len() as u64;

        HeliusMetrics {
            cache_hits, // Actual cache hits since start
            cache_misses, // Actual cache misses since start
            successful_requests: 0, // Not actively tracked without additional state
            retried_requests: 0,    // Not actively tracked without additional state
            failed_requests: 0,     // Not actively tracked without additional state
        }
    }

    /// Verify that a transaction signature exists on-chain via Helius RPC.
    ///
    /// Returns `Ok(true)` if the transaction was found, `Ok(false)` if not
    /// found, or `Err` on RPC failure. Used by the webhook receipt handler's
    /// staged RPC signature verification (B2).
    pub async fn verify_signature_exists(&self, signature: &str) -> Result<bool> {
        let url = chimera_core::utils::helius_rpc_url(&self.api_key);
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getTransaction",
            "params": [signature, {"maxSupportedTransactionVersion": 0, "commitment": "confirmed"}]
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("RPC getTransaction request failed")?;

        if !resp.status().is_success() {
            anyhow::bail!("RPC error: HTTP {}", resp.status());
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .context("Failed to parse getTransaction response")?;

        // If `result` is null, the transaction was not found on-chain
        let found = json
            .get("result")
            .map(|r| !r.is_null())
            .unwrap_or(false);

        Ok(found)
    }

    /// Get cache statistics for monitoring
    pub fn get_cache_stats(&self) -> (u64, u64, u64) {
        let cache_hits = self.cache_hits.load(std::sync::atomic::Ordering::Relaxed);
        let cache_misses = self.cache_misses.load(std::sync::atomic::Ordering::Relaxed);
        let cache_size = self.metadata_cache.read().len() as u64;
        (cache_hits, cache_misses, cache_size)
    }

        /// Fetch a wallet's recent SWAP transactions (enhanced format).
    ///
    /// Returns the raw enhanced transaction JSON objects. The enhanced
    /// `tokenTransfers[].tokenAmount` is decimal-adjusted (human units);
    /// `nativeTransfers[].amount` is raw lamports (SOL = amount / 1e9).
    ///
    /// Paginates `limit` transactions (page size 100, max 5 pages).
    pub async fn fetch_wallet_swaps(
        &self,
        wallet_address: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let mut result = Vec::new();
        let mut before: Option<String> = None;
        let target = limit.min(500);
        let mut pages = 0;

        while result.len() < target && pages < 5 {
            let mut url = format!(
                "{}/addresses/{}/transactions?api-key={}&limit=100&type=SWAP",
                self.base_url, wallet_address, self.api_key
            );
            if let Some(b) = &before {
                url.push_str(&format!("&before={}", b));
            }

            let resp = self
                .client
                .get(&url)
                .send()
                .await
                .context("Failed to fetch wallet transactions")?;
            if !resp.status().is_success() {
                return Err(anyhow!(
                    "Helius wallet transactions returned {}",
                    resp.status()
                ));
            }
            let batch: Vec<serde_json::Value> = resp
                .json()
                .await
                .context("Failed to parse wallet transactions")?;
            tracing::debug!(
                wallet = %wallet_address,
                url = %url,
                batch_len = batch.len(),
                "fetch_wallet_swaps: page received"
            );
            if batch.is_empty() {
                break;
            }

            let before_sig = batch
                .last()
                .and_then(|t| t.get("signature"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());

            result.extend(batch);
            pages += 1;
            before = before_sig;
        }

        result.truncate(target);
        Ok(result)
    }

    /// Get token creation time in hours since creation
    ///
    /// Returns None if:
    /// - API call fails
    /// - No transactions found for the mint address
    ///
    /// Uses shared metadata cache for unified storage of token metadata and age information.
    /// Age is calculated once and stored in the cache for 24 hours.
    pub async fn get_token_age_hours(&self, mint_address: &str) -> Result<Option<f64>> {        // Check shared metadata cache first
        {
            let cache = self.metadata_cache.read();
            if let Some(metadata) = cache.get(mint_address) {
                // Recompute age from the immutable creation_timestamp instead of
                // returning the stale cached age_hours, which would otherwise be
                // frozen for the full 24-hour metadata-cache TTL.
                if let Some(creation_ts) = metadata.creation_timestamp {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .context("Failed to get current timestamp")?
                        .as_secs() as i64;
                    let age_hours = (now - creation_ts) as f64 / 3600.0;
                    self.cache_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::debug!(token = mint_address, age = age_hours, "Cache hit for token age");
                    return Ok(Some(age_hours));
                }
                // Fall through if creation_timestamp is not cached
            }
        }

        // Cache miss - fetch from Helius API
        self.cache_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tracing::debug!(token = mint_address, "Cache miss for token age, fetching from Helius API");
        let creation_timestamp = self.get_token_creation_time(mint_address).await?;

        if let Some(timestamp) = creation_timestamp {
            // Calculate age in hours
            let current_timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("Failed to get current timestamp")?
                .as_secs() as i64;
            let age_seconds = current_timestamp - timestamp;
            let age_hours = age_seconds as f64 / 3600.0;

            // Update shared metadata cache with age information
            {
                let mut cache = self.metadata_cache.write();
                // We need to get the existing metadata (if any) and update it with age info
                // If no metadata exists yet, we create a minimal entry that will be enhanced by TokenMetadataFetcher later
                let updated_metadata = if let Some(mut existing_metadata) = cache.get(mint_address).cloned() {
                    // Update existing metadata with age information
                    existing_metadata.creation_timestamp = Some(timestamp);
                    existing_metadata.age_hours = Some(age_hours);
                    existing_metadata
                } else {
                    // Create minimal metadata entry with age information
                    // TokenMetadataFetcher will enrich this with full metadata later
                    crate::token::TokenMetadata {
                        mint: mint_address.to_string(),
                        freeze_authority: None,
                        mint_authority: None,
                        decimals: 0, // Will be updated by TokenMetadataFetcher
                        supply: 0,   // Will be updated by TokenMetadataFetcher
                        is_token_2022: false,
                        has_transfer_hook: false,
                        has_permanent_delegate: false,
                        creation_timestamp: Some(timestamp),
                        age_hours: Some(age_hours),
                    }
                };

                cache.insert(mint_address.to_string(), updated_metadata);
                tracing::debug!(token = mint_address, age = age_hours, "Cached token age in shared metadata cache");
            }

            Ok(Some(age_hours))
        } else {
            tracing::debug!(token = mint_address, "No token age found (API returned None)");
            Ok(None)
        }
    }

    /// Get token creation timestamp from Helius API
    ///
    /// Returns the timestamp of the first (oldest) transaction for the mint address
    async fn get_token_creation_time(&self, mint_address: &str) -> Result<Option<i64>> {
        let url = format!(
            "{}/addresses/{}/transactions?api-key={}&limit=100&order=asc",
            self.base_url, mint_address, self.api_key
        );

        let mint = mint_address.to_string();
        let client = self.client.clone();

        // Use retry logic with Helius best practices
        let result = retry_with_backoff(
            || {
                let url = url.clone();
                let client = client.clone();
                let mint = mint.clone(); // Clone for each attempt
                async move {
                    let response = client
                        .get(&url)
                        .send()
                        .await
                        .context("Failed to fetch token transactions")?;

                    if !response.status().is_success() {
                        let status = response.status().as_u16();
                        let error_text = response.text().await.unwrap_or_default();
                        tracing::warn!(
                            mint = mint,
                            status = status,
                            error = %error_text,
                            "Failed to fetch token creation time"
                        );
                        // Return an error carrying the HTTP status so the
                        // retry layer's `extract_status` can classify it: 404/422
                        // are non-retryable (token not found/invalid) and 5xx are
                        // retryable. An opaque anyhow error has no status and would
                        // silently bypass both the retry AND the non-retryable
                        // short-circuit below.
                        return Err(anyhow::Error::new(HttpStatusError::new(status))
                            .context(format!(
                                "Failed to fetch token creation time: {}",
                                error_text
                            )));
                    }

                    let transactions: Vec<serde_json::Value> = response
                        .json()
                        .await
                        .context("Failed to parse transactions response")?;

                    Ok(transactions)
                }
            },
            5,
        )
        .await;

        match result {
            Ok(transactions) => {
                // v0 API returns transactions newest-first.
                // Find the OLDEST (minimum) timestamp as the token creation time.
                let oldest_ts = transactions
                    .iter()
                    .filter_map(|tx| tx.get("timestamp").and_then(|t| t.as_i64()))
                    .min();

                if let Some(ts) = oldest_ts {
                    tracing::debug!(
                        mint = mint_address,
                        creation_timestamp = ts,
                        tx_count = transactions.len(),
                        "Found oldest transaction timestamp for token"
                    );
                    return Ok(Some(ts));
                }
                tracing::debug!(
                    mint = mint_address,
                    tx_count = transactions.len(),
                    "No timestamps found in token transactions"
                );
                Ok(None)
            }
            Err(e) => {
                // Check if this is a non-retryable error (should not happen after retries)
                let status = extract_status(&e);
                if status == 404 || status == 422 {
                    tracing::debug!(
                        mint = mint_address,
                        status = status,
                        "Token not found or invalid (non-retryable)"
                    );
                    Ok(None)
                } else {
                    tracing::error!(
                        mint = mint_address,
                        error = %e,
                        "Failed to fetch token creation time after all retries"
                    );
                    Err(e)
                }
            }
        }
    }

    /// Register webhook for a batch of wallets
    ///
    /// # Arguments
    /// * `wallets` - Wallet addresses to monitor
    /// * `webhook_url` - URL to receive webhook callbacks
    /// * `rate_limiter` - Rate limiter to respect API limits
    /// * `batch_size` - Number of wallets per batch
    /// * `batch_delay_ms` - Delay between batches (ms)
    pub async fn register_wallets_batch(
        &self,
        wallets: Vec<String>,
        webhook_url: &str,
        auth_header: Option<&str>,
        rate_limiter: Arc<RateLimiter>,
        batch_size: usize,
        batch_delay_ms: u64,
    ) -> Result<Vec<(String, String)>> {
        let mut results = Vec::new();

        for chunk in wallets.chunks(batch_size) {
            // Rate limit before each batch
            rate_limiter
                .acquire_standard(RequestPriority::Polling)
                .await;

            let webhook_id = self
                .register_webhook(chunk, webhook_url, auth_header)
                .await
                .context("Failed to register webhook batch")?;

            // Store mapping of wallets to webhook ID
            for wallet in chunk {
                results.push((wallet.clone(), webhook_id.clone()));
            }

            // Delay between batches
            if wallets.len() > batch_size {
                sleep(Duration::from_millis(batch_delay_ms)).await;
            }
        }

        Ok(results)
    }

    /// Register a single webhook for multiple wallets
    pub async fn register_webhook(
        &self,
        wallets: &[String],
        webhook_url: &str,
        auth_header: Option<&str>,
    ) -> Result<String> {
        let registration = WebhookRegistration {
            webhook_url: webhook_url.to_string(),
            transaction_types: vec!["SWAP".to_string()],
            account_addresses: wallets.to_vec(),
            webhook_type: "enhanced".to_string(),
            auth_header: auth_header.map(|s| s.to_string()),
        };

        let url = format!("{}/webhooks?api-key={}", self.base_url, self.api_key);
        let client = self.client.clone();

        // Use retry logic with Helius best practices
        retry_with_backoff(
            || {
                let url = url.clone();
                let client = client.clone();
                let registration = registration.clone();
                async move {
                    let body = serde_json::to_string(&registration)
                        .context("Failed to serialize webhook registration")?;
                    tracing::debug!(url = %url, body = %body, "Registering webhook");

                    let response = client
                        .post(&url)
                        .json(&registration)
                        .send()
                        .await
                        .context("Failed to send webhook registration request")?;

                    tracing::debug!(status = %response.status(), "Webhook registration response status");

                    if !response.status().is_success() {
                        let status = response.status().as_u16();
                        let error_text = response.text().await.unwrap_or_default();
                        return Err(anyhow!("HTTP error: {}", status)
                            .context(format!("Webhook registration failed: {}", error_text)));
                    }

                    let response_text = response
                        .text()
                        .await
                        .context("Failed to read webhook registration response")?;
                    tracing::debug!(response = %response_text, "Webhook registration response body");

                    let webhook_response: WebhookResponse = serde_json::from_str(&response_text)
                        .context(format!("Failed to parse webhook response: {}", response_text))?;

                    Ok(webhook_response.webhook_id)
                }
            },
            5,
        )
        .await
    }

    /// Delete a webhook
    pub async fn delete_webhook(&self, webhook_id: &str) -> Result<()> {
        let url = format!(
            "{}/webhooks/{}?api-key={}",
            self.base_url, webhook_id, self.api_key
        );
        let client = self.client.clone();

        // Use retry logic with Helius best practices
        retry_with_backoff(
            || {
                let url = url.clone();
                let client = client.clone();
                async move {
                    let response = client
                        .delete(&url)
                        .send()
                        .await
                        .context("Failed to delete webhook")?;

                    let status = response.status().as_u16();
                    if status == 404 {
                        return Ok(());
                    }

                    if !response.status().is_success() {
                        let error_text = response.text().await.unwrap_or_default();
                        return Err(anyhow!("HTTP error: {}", status)
                            .context(format!("Failed to delete webhook: {}", error_text)));
                    }

                    Ok(())
                }
            },
            5,
        )
        .await
    }

    /// List all webhooks
    pub async fn list_webhooks(&self) -> Result<Vec<serde_json::Value>> {
        let url = format!("{}/webhooks?api-key={}", self.base_url, self.api_key);
        let client = self.client.clone();

        // Use retry logic with Helius best practices
        retry_with_backoff(
            || {
                let url = url.clone();
                let client = client.clone();
                async move {
                    let response = client
                        .get(&url)
                        .send()
                        .await
                        .context("Failed to list webhooks")?;

                    if !response.status().is_success() {
                        let status = response.status().as_u16();
                        let error_text = response.text().await.unwrap_or_default();
                        return Err(anyhow!("HTTP error: {}", status)
                            .context(format!("Failed to list webhooks: {}", error_text)));
                    }

                    let webhooks: Vec<serde_json::Value> = response
                        .json()
                        .await
                        .context("Failed to parse webhooks response")?;

                    Ok(webhooks)
                }
            },
            5,
        )
        .await
    }

    /// Get specific webhook by ID (GET endpoint)
    pub async fn get_webhook(&self, webhook_id: &str) -> Result<serde_json::Value> {
        let url = format!(
            "{}/webhooks/{}?api-key={}",
            self.base_url, webhook_id, self.api_key
        );
        let client = self.client.clone();

        retry_with_backoff(
            || {
                let url = url.clone();
                let client = client.clone();
                async move {
                    let response = client
                        .get(&url)
                        .send()
                        .await
                        .context("Failed to get webhook")?;

                    if !response.status().is_success() {
                        let status = response.status().as_u16();
                        let error_text = response.text().await.unwrap_or_default();
                        return Err(anyhow!("HTTP error: {}", status)
                            .context(format!("Failed to get webhook: {}", error_text)));
                    }

                    let webhook: serde_json::Value = response
                        .json()
                        .await
                        .context("Failed to parse webhook response")?;

                    Ok(webhook)
                }
            },
            5,
        )
        .await
    }

    /// Get specific webhook by ID with typed return
    pub async fn get_webhook_typed(&self, webhook_id: &str) -> Result<HeliusWebhook> {
        let url = format!(
            "{}/webhooks/{}?api-key={}",
            self.base_url, webhook_id, self.api_key
        );
        let client = self.client.clone();

        retry_with_backoff(
            || {
                let url = url.clone();
                let client = client.clone();
                async move {
                    let response = client
                        .get(&url)
                        .send()
                        .await
                        .context("Failed to get webhook")?;

                    if !response.status().is_success() {
                        let status = response.status().as_u16();
                        let error_text = response.text().await.unwrap_or_default();
                        return Err(anyhow!("HTTP error: {}", status)
                            .context(format!("Failed to get webhook: {}", error_text)));
                    }

                    let webhook: HeliusWebhook = response
                        .json()
                        .await
                        .context("Failed to parse webhook response")?;

                    Ok(webhook)
                }
            },
            5,
        )
        .await
    }

    /// List all webhooks with typed return
    pub async fn list_webhooks_typed(&self) -> Result<Vec<HeliusWebhook>> {
        let url = format!("{}/webhooks?api-key={}", self.base_url, self.api_key);
        let client = self.client.clone();

        retry_with_backoff(
            || {
                let url = url.clone();
                let client = client.clone();
                async move {
                    let response = client
                        .get(&url)
                        .send()
                        .await
                        .context("Failed to list webhooks")?;

                    if !response.status().is_success() {
                        let status = response.status().as_u16();
                        let error_text = response.text().await.unwrap_or_default();
                        return Err(anyhow!("HTTP error: {}", status)
                            .context(format!("Failed to list webhooks: {}", error_text)));
                    }

                    let webhooks: Vec<HeliusWebhook> = response
                        .json()
                        .await
                        .context("Failed to parse webhooks response")?;

                    Ok(webhooks)
                }
            },
            5,
        )
        .await
    }

    /// Update an existing webhook configuration (PUT endpoint)
    ///
    /// Use this to update webhook URL without losing the webhook ID,
    /// or to modify transaction types and monitored addresses.
    pub async fn update_webhook(&self, webhook_id: &str, update: WebhookUpdate) -> Result<()> {
        let url = format!(
            "{}/webhooks/{}?api-key={}",
            self.base_url, webhook_id, self.api_key
        );
        let client = self.client.clone();

        retry_with_backoff(
            || {
                let url = url.clone();
                let client = client.clone();
                let update = update.clone();
                async move {
                    let response = client
                        .put(&url)
                        .json(&update)
                        .send()
                        .await
                        .context("Failed to update webhook")?;

                    if !response.status().is_success() {
                        let status = response.status().as_u16();
                        let error_text = response.text().await.unwrap_or_default();
                        return Err(anyhow!("HTTP error: {}", status)
                            .context(format!("Webhook update failed: {}", error_text)));
                    }

                    Ok(())
                }
            },
            5,
        )
        .await
    }

    /// Toggle webhook enabled/disabled without deletion (PATCH endpoint)
    ///
    /// Use this to temporarily suspend webhook delivery without
    /// losing the webhook configuration.
    pub async fn toggle_webhook(&self, webhook_id: &str, enabled: bool) -> Result<()> {
        let url = format!(
            "{}/webhooks/{}/toggle?api-key={}",
            self.base_url, webhook_id, self.api_key
        );
        let client = self.client.clone();
        let toggle = WebhookToggle { is_active: enabled };

        retry_with_backoff(
            || {
                let url = url.clone();
                let client = client.clone();
                let toggle = toggle.clone();
                async move {
                    let response = client
                        .patch(&url)
                        .json(&toggle)
                        .send()
                        .await
                        .context("Failed to toggle webhook")?;

                    if !response.status().is_success() {
                        let status = response.status().as_u16();
                        let error_text = response.text().await.unwrap_or_default();
                        return Err(anyhow!("HTTP error: {}", status)
                            .context(format!("Webhook toggle failed: {}", error_text)));
                    }

                    Ok(())
                }
            },
            5,
        )
        .await
    }

    /// Bulk update webhook URLs for multiple webhooks
    pub async fn bulk_update_webhook_urls(
        &self,
        updates: Vec<(String, String)>, // (webhook_id, new_url)
        rate_limiter: Arc<crate::monitoring::rate_limiter::RateLimiter>,
    ) -> Result<Vec<(String, Result<()>)>> {
        let mut results = Vec::new();

        for (webhook_id, new_url) in updates {
            rate_limiter
                .acquire_standard(crate::monitoring::rate_limiter::RequestPriority::Polling)
                .await;

            let result = self
                .update_webhook(
                    &webhook_id,
                    WebhookUpdate {
                        webhook_url: Some(new_url.clone()),
                        transaction_types: None,
                        account_addresses: None,
                        auth_header: None,
                        webhook_type: None,
                    },
                )
                .await;

            results.push((webhook_id, result));
        }

        Ok(results)
    }
}

/// Validate webhook URL reachability with a health check request.
///
/// This function sends a lightweight GET request to the webhook URL to verify
/// it is reachable and responding. This is useful for startup validation to
/// fail-fast if the webhook endpoint is misconfigured.
///
/// # Arguments
/// * `webhook_url` - The webhook URL to validate
///
/// # Returns
/// * `Ok(())` if the URL is reachable
/// * `Err(e)` if the URL is unreachable or returns an error
///
/// # Note
/// This is a lightweight check that doesn't require authentication. For endpoints
/// that require authentication, consider using the actual webhook endpoint handler
/// for validation instead.
pub async fn validate_webhook_reachability(webhook_url: &str) -> Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("Failed to build HTTP client for webhook validation")?;

    // Send a lightweight GET request to the webhook endpoint
    // Most webhook endpoints return 404 or 405 for GET requests, which
    // indicates the server is reachable even if the endpoint doesn't support GET
    let response = client
        .get(webhook_url)
        .send()
        .await
        .context("Failed to reach webhook URL")?;

    // Any response (including 4xx) indicates the URL is reachable
    tracing::info!("Webhook URL reachable, status: {}", response.status());

    // If we get any response, the URL is reachable
    // We don't require a specific status code since webhook endpoints may
    // return 404 or 405 for GET requests
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::thread;

    /// Serialize env-var-dependent tests (HELIUS_API_BASE_URL /
    /// HELIUS_RPC_BASE_URL are process-global).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Tiny synchronous HTTP/1.1 mock server for Helius API/RPC endpoints.
    /// The handler receives (method, path) and returns (status, body).
    struct MockServer {
        url: String,
        stop: Arc<AtomicBool>,
    }

    impl MockServer {
        fn spawn<H>(handler: H) -> Self
        where
            H: Fn(&str, &str) -> (u16, String) + Send + Sync + 'static,
        {
            let handler = Arc::new(handler);
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
            let addr = listener.local_addr().expect("local addr");
            let stop = Arc::new(AtomicBool::new(false));
            let stop_clone = Arc::clone(&stop);

            thread::spawn(move || {
                listener.set_nonblocking(true).ok();
                while !stop_clone.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            // Accepted sockets inherit the listener's non-blocking
                            // flag on some platforms; force blocking so the handler's
                            // read blocks for the request instead of returning
                            // WouldBlock immediately and dropping the connection.
                            let _ = stream.set_nonblocking(false);
                            let handler = Arc::clone(&handler);
                            thread::spawn(move || handle_conn(stream, handler));
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self {
                url: format!("http://{addr}"),
                stop,
            }
        }

        #[allow(clippy::await_holding_lock)]
        async fn with_env<F, Fut, T>(&self, key: &str, f: F) -> T
        where
            F: FnOnce() -> Fut,
            Fut: std::future::Future<Output = T>,
        {
            // Poison-tolerant: a test assertion failure inside the closure
            // must not take down every later env-dependent test.
            let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var(key, &self.url);
            let result = f().await;
            std::env::remove_var(key);
            result
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    fn handle_conn<H>(mut stream: TcpStream, handler: Arc<H>)
    where
        H: Fn(&str, &str) -> (u16, String) + Send + Sync + 'static,
    {
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(60)));
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        // Loop over keep-alive requests on the same connection. A real server
        // keeps the socket open for reused connections; reqwest's connection
        // pool reuses the socket across paginated requests, so closing after a
        // single request races with a pooled reuse and drops the connection
        // mid-response under parallel test load.
        loop {
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() || request_line.trim().is_empty() {
                return;
            }
            let parts: Vec<&str> = request_line.split_whitespace().collect();
            if parts.len() < 2 {
                return;
            }
            let method = parts[0].to_string();
            // Strip query string: handlers match on the path only.
            let path = parts[1].split('?').next().unwrap_or("").to_string();

            // Read headers
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line == "\r\n" || line == "\n" {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
            // Drain body
            if content_length > 0 {
                let mut buf = vec![0u8; content_length];
                let _ = reader.read_exact(&mut buf);
            }

            let (status, body) = handler(&method, &path);
            let reason = if status == 200 { "OK" } else { "Error" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
                body.len()
            );
            if stream.write_all(response.as_bytes()).is_err() || stream.flush().is_err() {
                return;
            }
        }
    }

    fn test_client_with_cache(
        cache: Arc<RwLock<HashMap<String, crate::token::TokenMetadata>>>,
    ) -> HeliusClient {
        HeliusClient::new("test-key".to_string(), cache).expect("client")
    }

    fn test_client() -> HeliusClient {
        test_client_with_cache(Arc::new(RwLock::new(HashMap::new())))
    }

    /// Mint account transaction list (with a `timestamp` field).
    fn tx_json(ts: i64, signature: &str) -> serde_json::Value {
        serde_json::json!({"timestamp": ts, "signature": signature})
    }

    // =============================================================================
    // Payload serde
    // =============================================================================

    #[test]
    fn test_webhook_payload_deserialize() {
        let json = r#"
        {
            "accountData": [],
            "nativeTransfers": [],
            "signature": "test123",
            "slot": 12345,
            "timestamp": 1234567890,
            "type": "SWAP"
        }
        "#;

        let payload: Result<HeliusWebhookPayload, _> = serde_json::from_str(json);
        assert!(payload.is_ok());
    }

    #[test]
    fn test_webhook_payload_full_roundtrip() {
        let json = r#"{
            "accountData": [{
                "account": "acct1",
                "nativeBalanceChange": -1000000000,
                "tokenBalanceChanges": [{
                    "mint": "mint1",
                    "rawTokenAmount": {"tokenAmount": "1000", "decimals": 9},
                    "tokenAccount": "ta1",
                    "userAccount": "u1"
                }]
            }],
            "nativeTransfers": [{
                "amount": 500000000,
                "fromUserAccount": "f1",
                "toUserAccount": "t1"
            }],
            "signature": "sig1",
            "slot": 10,
            "timestamp": 20,
            "transactionError": {"error": "fail"},
            "type": "SWAP",
            "events": {
                "swap": {
                    "swapper": "swapper1",
                    "nativeInput": {"account": "a", "amount": "100"},
                    "native_output": {"account": "b", "amount": "200"},
                    "tokenInputs": [{"userAccount": "u", "mint": "m", "rawTokenAmount": {"tokenAmount": "1"}}],
                    "token_outputs": [{"user_account": "u2", "mint": "m2", "raw_token_amount": {"tokenAmount": "2"}}]
                }
            }
        }"#;
        let payload: HeliusWebhookPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.transaction_type, "SWAP");
        assert!(payload.transaction_error.is_some());
        let swap = payload.events.swap.clone().expect("swap event");
        assert_eq!(swap.swapper.as_deref(), Some("swapper1"));
        assert_eq!(swap.native_input.as_ref().unwrap().amount, "100");
        assert_eq!(swap.native_output.as_ref().unwrap().amount, "200");
        assert_eq!(swap.token_inputs.len(), 1);
        assert_eq!(swap.token_outputs[0].user_account, "u2");
        assert_eq!(
            swap.token_outputs[0].raw_token_amount.as_ref().unwrap().token_amount,
            "2"
        );

        // Round-trips back out
        let re = serde_json::to_string(&payload).unwrap();
        assert!(re.contains("\"nativeInput\""));
        assert!(re.contains("\"type\":\"SWAP\""));
    }

    #[test]
    fn test_webhook_events_default_empty() {
        let payload: HeliusWebhookPayload = serde_json::from_str(
            r#"{"accountData":[],"nativeTransfers":[],"signature":"s","slot":1,"timestamp":1,"type":"SWAP"}"#,
        )
        .unwrap();
        assert!(payload.events.swap.is_none());
        let events = WebhookEvents::default();
        assert!(events.swap.is_none());
    }

    #[test]
    fn test_webhook_update_serialization_skips_none() {
        let update = WebhookUpdate {
            webhook_url: None,
            transaction_types: None,
            account_addresses: None,
            auth_header: None,
            webhook_type: None,
        };
        let json = serde_json::to_string(&update).unwrap();
        assert_eq!(json, "{}");

        let update = WebhookUpdate {
            webhook_url: Some("https://x/webhook".to_string()),
            transaction_types: Some(vec!["SWAP".to_string()]),
            account_addresses: Some(vec!["w1".to_string()]),
            auth_header: Some(serde_json::Value::String("secret".to_string())),
            webhook_type: Some("enhanced".to_string()),
        };
        let json = serde_json::to_string(&update).unwrap();
        assert!(json.contains("\"webhookURL\":\"https://x/webhook\""));
        assert!(json.contains("\"authHeader\":\"secret\""));
        assert!(json.contains("\"webhookType\":\"enhanced\""));
    }

    #[test]
    fn test_webhook_toggle_serialization() {
        let toggle = WebhookToggle { is_active: true };
        assert_eq!(serde_json::to_string(&toggle).unwrap(), "{\"isActive\":true}");
    }

    #[test]
    fn test_helius_webhook_serde() {
        let webhook: HeliusWebhook = serde_json::from_str(
            r#"{
                "webhookID": "wh-1",
                "webhookURL": "https://x/webhook",
                "accountAddresses": ["w1", "w2"],
                "transactionTypes": ["SWAP"]
            }"#,
        )
        .unwrap();
        assert_eq!(webhook.webhook_id, "wh-1");
        assert_eq!(webhook.wallet_addresses, vec!["w1", "w2"]);
        assert_eq!(webhook.transaction_types, vec!["SWAP"]);

        // accountAddresses defaults to empty
        let webhook: HeliusWebhook = serde_json::from_str(
            r#"{"webhookID":"wh-2","webhookURL":"https://x","transactionTypes":[]}"#,
        )
        .unwrap();
        assert!(webhook.wallet_addresses.is_empty());
    }

    #[test]
    fn test_webhook_registration_serialization() {
        let reg = WebhookRegistration {
            webhook_url: "https://x/webhook".to_string(),
            transaction_types: vec!["SWAP".to_string()],
            account_addresses: vec!["w1".to_string()],
            webhook_type: "enhanced".to_string(),
            auth_header: Some("auth".to_string()),
        };
        let json = serde_json::to_string(&reg).unwrap();
        assert!(json.contains("\"authHeader\":\"auth\""));
        assert!(json.contains("\"webhookType\":\"enhanced\""));

        let reg = WebhookRegistration {
            auth_header: None,
            ..reg
        };
        let json = serde_json::to_string(&reg).unwrap();
        assert!(!json.contains("authHeader"), "None auth_header must be skipped");
    }

    #[test]
    fn test_webhook_response_serde() {
        let resp: WebhookResponse = serde_json::from_str(r#"{"webhookID":"wh-9"}"#).unwrap();
        assert_eq!(resp.webhook_id, "wh-9");
    }

    #[test]
    fn test_metrics_and_cache_stats() {
        let client = test_client();
        let metrics = client.get_metrics();
        assert_eq!(metrics.cache_hits, 0);
        assert_eq!(metrics.cache_misses, 0);
        assert_eq!(metrics.successful_requests, 0);
        assert_eq!(metrics.retried_requests, 0);
        assert_eq!(metrics.failed_requests, 0);

        let (hits, misses, size) = client.get_cache_stats();
        assert_eq!(hits, 0);
        assert_eq!(misses, 0);
        assert_eq!(size, 0);
    }

    // =============================================================================
    // verify_signature_exists (RPC)
    // =============================================================================

    #[tokio::test]
    async fn test_verify_signature_exists_found() {
        let mock = MockServer::spawn(|method, path| {
            assert_eq!(method, "POST");
            assert_eq!(path, "/");
            (
                200,
                r#"{"jsonrpc":"2.0","id":1,"result":{"slot":1}}"#.to_string(),
            )
        });
        mock.with_env("HELIUS_RPC_BASE_URL", || async {
            let client = test_client();
            assert!(client.verify_signature_exists("abc123").await.unwrap());
        }).await;
    }

    #[tokio::test]
    async fn test_verify_signature_exists_not_found() {
        let mock = MockServer::spawn(|_, _| {
            (200, r#"{"jsonrpc":"2.0","id":1,"result":null}"#.to_string())
        });
        mock.with_env("HELIUS_RPC_BASE_URL", || async {
            let client = test_client();
            assert!(!client.verify_signature_exists("abc123").await.unwrap());
        }).await;
    }

    #[tokio::test]
    async fn test_verify_signature_exists_http_error() {
        let mock = MockServer::spawn(|_, _| (404, "nope".to_string()));
        mock.with_env("HELIUS_RPC_BASE_URL", || async {
            let client = test_client();
            assert!(client.verify_signature_exists("abc123").await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn test_verify_signature_exists_bad_json() {
        let mock = MockServer::spawn(|_, _| (200, "not-json".to_string()));
        mock.with_env("HELIUS_RPC_BASE_URL", || async {
            let client = test_client();
            assert!(client.verify_signature_exists("abc123").await.is_err());
        }).await;
    }

    // =============================================================================
    // fetch_wallet_swaps
    // =============================================================================

    #[tokio::test]
    async fn test_fetch_wallet_swaps_single_page() {
        let mock = MockServer::spawn(|_, path| {
            assert!(path.starts_with("/addresses/wallet-1/transactions"));
            (
                200,
                serde_json::json!([
                    {"signature": "sig1"},
                    {"signature": "sig2"}
                ])
                .to_string(),
            )
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            // target 2: one page of 2 items satisfies the target
            let txs = client.fetch_wallet_swaps("wallet-1", 2).await.unwrap();
            assert_eq!(txs.len(), 2);
        }).await;
    }

    #[tokio::test]
    async fn test_fetch_wallet_swaps_paginates_and_truncates() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);
        let mock = MockServer::spawn(move |_, path| {
            calls_clone.fetch_add(1, Ordering::Relaxed);
            assert!(path.starts_with("/addresses/wallet-1/transactions"));
            let page: Vec<serde_json::Value> = (0..100)
                .map(|i| serde_json::json!({"signature": format!("sig-{}-{i}", path.contains("before"))}))
                .collect();
            (200, serde_json::json!(page).to_string())
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            // target 150 -> two 100-item pages, truncated to 150
            let txs = client.fetch_wallet_swaps("wallet-1", 150).await.unwrap();
            assert_eq!(txs.len(), 150);
            assert!(calls.load(Ordering::Relaxed) >= 2, "must paginate");
            assert!(calls.load(Ordering::Relaxed) <= 3, "loop must stop once target reached");
        }).await;
    }

    #[tokio::test]
    async fn test_fetch_wallet_swaps_stops_on_empty_batch() {
        let mock = MockServer::spawn(|_, _| (200, "[]".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            let txs = client.fetch_wallet_swaps("wallet-1", 500).await.unwrap();
            assert!(txs.is_empty());
        }).await;
    }

    #[tokio::test]
    async fn test_fetch_wallet_swaps_http_error() {
        let mock = MockServer::spawn(|_, _| (500, "boom".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            assert!(client.fetch_wallet_swaps("wallet-1", 10).await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn test_fetch_wallet_swaps_bad_json() {
        let mock = MockServer::spawn(|_, _| (200, "not-json".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            assert!(client.fetch_wallet_swaps("wallet-1", 10).await.is_err());
        }).await;
    }

    // =============================================================================
    // get_token_age_hours / get_token_creation_time
    // =============================================================================

    #[tokio::test]
    async fn test_token_age_cache_hit() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let cache = Arc::new(RwLock::new(HashMap::from([(
            "mint1".to_string(),
            crate::token::TokenMetadata {
                mint: "mint1".to_string(),
                freeze_authority: None,
                mint_authority: None,
                decimals: 6,
                supply: 1000,
                is_token_2022: false,
                has_transfer_hook: false,
                has_permanent_delegate: false,
                creation_timestamp: Some(now - 3600), // 1 hour ago
                age_hours: None,
            },
        )])));
        let client = test_client_with_cache(cache);
        let age = client.get_token_age_hours("mint1").await.unwrap().unwrap();
        assert!(age > 0.9 && age < 1.1, "age ~1h, got {age}");

        let metrics = client.get_metrics();
        assert_eq!(metrics.cache_hits, 1);
        let (hits, _, size) = client.get_cache_stats();
        assert_eq!(hits, 1);
        assert_eq!(size, 1);
    }

    #[tokio::test]
    async fn test_token_age_cache_hit_without_timestamp_falls_through() {
        let cache = Arc::new(RwLock::new(HashMap::from([(
            "mint1".to_string(),
            crate::token::TokenMetadata {
                mint: "mint1".to_string(),
                freeze_authority: None,
                mint_authority: None,
                decimals: 6,
                supply: 1000,
                is_token_2022: false,
                has_transfer_hook: false,
                has_permanent_delegate: false,
                creation_timestamp: None,
                age_hours: None,
            },
        )])));
        let mock = MockServer::spawn(|_, path| {
            assert!(path.starts_with("/addresses/mint1/transactions"));
            (
                200,
                serde_json::json!([tx_json(1_700_000_000, "s1"), tx_json(1_700_000_100, "s2")])
                    .to_string(),
            )
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client_with_cache(cache);
            let age = client.get_token_age_hours("mint1").await.unwrap();
            assert!(age.is_some());
            // Cache updated with age info
            let cache = client.metadata_cache.read();
            let meta = cache.get("mint1").unwrap();
            assert!(meta.age_hours.is_some());
            assert!(meta.creation_timestamp.is_some());
            assert_eq!(client.get_metrics().cache_misses, 1);
        }).await;
    }

    #[tokio::test]
    async fn test_token_age_cache_miss_fetches_oldest() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let now_clone = now;
        let mock = MockServer::spawn(move |_, path| {
            // The mock strips the query string, so assert on the path only.
            assert!(path.starts_with("/addresses/mint1/transactions"));
            (
                200,
                serde_json::json!([
                    tx_json(now_clone - 7200, "newer"),   // 2h ago
                    tx_json(now_clone - 10800, "oldest"), // 3h ago
                ])
                .to_string(),
            )
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            let age = client.get_token_age_hours("mint1").await.unwrap().unwrap();
            assert!(age > 2.9 && age < 3.1, "oldest tx wins (3h), got {age}");
            let metrics = client.get_metrics();
            assert_eq!(metrics.cache_misses, 1);
        }).await;
    }

    #[tokio::test]
    async fn test_token_age_no_timestamps() {
        let mock = MockServer::spawn(|_, _| (200, "[]".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            assert!(client.get_token_age_hours("mint1").await.unwrap().is_none());
        }).await;
    }

    #[tokio::test]
    async fn test_token_age_404_non_retryable() {
        let mock = MockServer::spawn(|_, _| (404, "not found".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            assert!(client.get_token_age_hours("mint1").await.unwrap().is_none());
        }).await;
    }

    #[tokio::test]
    async fn test_token_age_unparseable_timestamps() {
        let mock = MockServer::spawn(|_, _| {
            (
                200,
                serde_json::json!([{"signature": "s1", "timestamp": "not-a-number"}]).to_string(),
            )
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            assert!(client.get_token_age_hours("mint1").await.unwrap().is_none());
        }).await;
    }

    #[tokio::test]
    async fn test_token_age_500_retries_then_fails() {
        // 5xx is retryable: exercise the backoff-retry loop and the
        // after-all-retries error path (slow by design: ~15-20s).
        let mock = MockServer::spawn(|_, _| (500, "server error".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            assert!(client.get_token_age_hours("mint1").await.is_err());
        }).await;
    }

    // =============================================================================
    // register_webhook / register_wallets_batch
    // =============================================================================

    #[tokio::test]
    async fn test_register_webhook_success() {
        let mock = MockServer::spawn(|method, path| {
            assert_eq!(method, "POST");
            assert_eq!(path, "/webhooks");
            (200, r#"{"webhookID":"wh-new-1"}"#.to_string())
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            let id = client
                .register_webhook(&["w1".to_string(), "w2".to_string()], "https://x/webhook", Some("auth"))
                .await
                .unwrap();
            assert_eq!(id, "wh-new-1");
        }).await;
    }

    #[tokio::test]
    async fn test_register_webhook_http_error() {
        let mock = MockServer::spawn(|_, _| (400, "bad request".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            let err = client
                .register_webhook(&["w1".to_string()], "https://x/webhook", None)
                .await
                .unwrap_err();
            assert!(err.to_string().contains("Webhook registration failed"));
        }).await;
    }

    #[tokio::test]
    async fn test_register_webhook_unparseable_response() {
        let mock = MockServer::spawn(|_, _| (200, "not-json".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            assert!(client
                .register_webhook(&["w1".to_string()], "https://x/webhook", None)
                .await
                .is_err());
        }).await;
    }

    #[tokio::test]
    async fn test_register_wallets_batch_chunking() {
        let register_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = Arc::clone(&register_calls);
        let mock = MockServer::spawn(move |method, path| {
            assert_eq!(method, "POST");
            assert_eq!(path, "/webhooks");
            calls.fetch_add(1, Ordering::Relaxed);
            (200, format!(r#"{{"webhookID":"wh-batch-{}"}}"#, calls.load(Ordering::Relaxed)))
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            let limiter = Arc::new(RateLimiter::new(100, 1));
            let results = client
                .register_wallets_batch(
                    vec!["w1".to_string(), "w2".to_string(), "w3".to_string()],
                    "https://x/webhook",
                    None,
                    limiter,
                    2,
                    1,
                )
                .await
                .unwrap();
            assert_eq!(results.len(), 3);
            assert_eq!(register_calls.load(Ordering::Relaxed), 2, "3 wallets / batch 2 = 2 calls");
            assert_eq!(results[0].1, results[1].1, "same webhook within batch");
        }).await;
    }

    #[tokio::test]
    async fn test_register_wallets_batch_error() {
        let mock = MockServer::spawn(|_, _| (400, "fail".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            let limiter = Arc::new(RateLimiter::new(100, 1));
            assert!(client
                .register_wallets_batch(
                    vec!["w1".to_string()],
                    "https://x/webhook",
                    None,
                    limiter,
                    2,
                    1,
                )
                .await
                .is_err());
        }).await;
    }

    // =============================================================================
    // delete_webhook
    // =============================================================================

    #[tokio::test]
    async fn test_delete_webhook_success_and_404() {
        let mock = MockServer::spawn(|method, path| {
            assert_eq!(method, "DELETE");
            assert_eq!(path, "/webhooks/wh-1");
            (200, "{}".to_string())
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            client.delete_webhook("wh-1").await.unwrap();
        }).await;

        let mock = MockServer::spawn(|_, _| (404, "gone".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            client.delete_webhook("wh-1").await.unwrap(); // 404 treated as success
        }).await;
    }

    #[tokio::test]
    async fn test_delete_webhook_error() {
        let mock = MockServer::spawn(|_, _| (400, "bad".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            assert!(client.delete_webhook("wh-1").await.is_err());
        }).await;
    }

    // =============================================================================
    // list / get webhooks
    // =============================================================================

    #[tokio::test]
    async fn test_list_webhooks() {
        let mock = MockServer::spawn(|method, path| {
            assert_eq!(method, "GET");
            assert_eq!(path, "/webhooks");
            (
                200,
                serde_json::json!([{"webhookID": "wh-1"}, {"webhookID": "wh-2"}]).to_string(),
            )
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            let webhooks = client.list_webhooks().await.unwrap();
            assert_eq!(webhooks.len(), 2);
            assert_eq!(webhooks[0]["webhookID"], "wh-1");
        }).await;
    }

    #[tokio::test]
    async fn test_list_webhooks_error() {
        let mock = MockServer::spawn(|_, _| (400, "bad".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            assert!(client.list_webhooks().await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn test_get_webhook() {
        let mock = MockServer::spawn(|_, path| {
            assert_eq!(path, "/webhooks/wh-1");
            (
                200,
                serde_json::json!({"webhookID": "wh-1", "active": true, "webhookURL": "https://x"}).to_string(),
            )
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            let wh = client.get_webhook("wh-1").await.unwrap();
            assert_eq!(wh["active"], true);
        }).await;
    }

    #[tokio::test]
    async fn test_get_webhook_typed() {
        let mock = MockServer::spawn(|_, path| {
            assert_eq!(path, "/webhooks/wh-1");
            (
                200,
                serde_json::json!({
                    "webhookID": "wh-1",
                    "webhookURL": "https://x/webhook",
                    "accountAddresses": ["w1", "w2"],
                    "transactionTypes": ["SWAP"]
                })
                .to_string(),
            )
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            let wh = client.get_webhook_typed("wh-1").await.unwrap();
            assert_eq!(wh.webhook_id, "wh-1");
            assert_eq!(wh.wallet_addresses.len(), 2);
        }).await;
    }

    #[tokio::test]
    async fn test_list_webhooks_typed() {
        let mock = MockServer::spawn(|_, _| {
            (
                200,
                serde_json::json!([{
                    "webhookID": "wh-1",
                    "webhookURL": "https://x",
                    "accountAddresses": ["w1"],
                    "transactionTypes": ["SWAP"]
                }])
                .to_string(),
            )
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            let webhooks = client.list_webhooks_typed().await.unwrap();
            assert_eq!(webhooks.len(), 1);
            assert_eq!(webhooks[0].webhook_id, "wh-1");
        }).await;
    }

    #[tokio::test]
    async fn test_list_webhooks_typed_error() {
        let mock = MockServer::spawn(|_, _| (400, "bad".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            assert!(client.list_webhooks_typed().await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn test_get_webhook_typed_error() {
        let mock = MockServer::spawn(|_, _| (404, "missing".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            assert!(client.get_webhook_typed("wh-x").await.is_err());
        }).await;
    }

    // =============================================================================
    // update / toggle / bulk update
    // =============================================================================

    #[tokio::test]
    async fn test_update_webhook_success_and_error() {
        let mock = MockServer::spawn(|method, path| {
            assert_eq!(method, "PUT");
            assert_eq!(path, "/webhooks/wh-1");
            (200, "{}".to_string())
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            client
                .update_webhook(
                    "wh-1",
                    WebhookUpdate {
                        webhook_url: Some("https://new".to_string()),
                        transaction_types: None,
                        account_addresses: None,
                        auth_header: None,
                        webhook_type: None,
                    },
                )
                .await
                .unwrap();
        }).await;

        let mock = MockServer::spawn(|_, _| (400, "bad".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            let err = client
                .update_webhook(
                    "wh-1",
                    WebhookUpdate {
                        webhook_url: Some("https://new".to_string()),
                        transaction_types: None,
                        account_addresses: None,
                        auth_header: None,
                        webhook_type: None,
                    },
                )
                .await
                .unwrap_err();
            assert!(err.to_string().contains("Webhook update failed"));
        }).await;
    }

    #[tokio::test]
    async fn test_toggle_webhook_success_and_error() {
        let mock = MockServer::spawn(|method, path| {
            assert_eq!(method, "PATCH");
            assert_eq!(path, "/webhooks/wh-1/toggle");
            (200, "{}".to_string())
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            client.toggle_webhook("wh-1", true).await.unwrap();
        }).await;

        let mock = MockServer::spawn(|_, _| (400, "bad".to_string()));
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            assert!(client.toggle_webhook("wh-1", false).await.is_err());
        }).await;
    }

    #[tokio::test]
    async fn test_bulk_update_webhook_urls() {
        let mock = MockServer::spawn(|method, _path| {
            assert_eq!(method, "PUT");
            (200, "{}".to_string())
        });
        mock.with_env("HELIUS_API_BASE_URL", || async {
            let client = test_client();
            let limiter = Arc::new(RateLimiter::new(100, 1));
            let results = client
                .bulk_update_webhook_urls(
                    vec![
                        ("wh-1".to_string(), "https://a".to_string()),
                        ("wh-2".to_string(), "https://b".to_string()),
                    ],
                    limiter,
                )
                .await
                .unwrap();
            assert_eq!(results.len(), 2);
            assert!(results[0].1.is_ok());
            assert!(results[1].1.is_ok());
        }).await;
    }

    // =============================================================================
    // validate_webhook_reachability
    // =============================================================================

    #[tokio::test]
    async fn test_validate_webhook_reachability_ok() {
        let mock = MockServer::spawn(|_, _| (404, "no route here".to_string()));
        let client = reqwest::Client::new();
        let resp = client.get(&mock.url).send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 404);
        // validate_webhook_reachability: any status means reachable
        validate_webhook_reachability(&mock.url).await.unwrap();
    }

    #[tokio::test]
    async fn test_validate_webhook_reachability_unreachable() {
        // Point at a port with no listener -> connect error
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener); // closed port
        let url = format!("http://{addr}");
        assert!(validate_webhook_reachability(&url).await.is_err());
    }

    #[test]
    fn test_webhook_metrics_default() {
        let metrics = HeliusMetrics::default();
        assert_eq!(metrics.cache_hits, 0);
        assert_eq!(metrics.cache_misses, 0);
        assert_eq!(metrics.successful_requests, 0);
        assert_eq!(metrics.retried_requests, 0);
        assert_eq!(metrics.failed_requests, 0);
        let json = serde_json::to_string(&metrics).unwrap();
        assert!(json.contains("cache_hits"));
    }
}
