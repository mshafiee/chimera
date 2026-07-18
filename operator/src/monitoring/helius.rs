//! Helius webhook integration for automatic transaction monitoring
//!
//! Handles webhook registration, receiving, and processing for ACTIVE wallets.

use crate::monitoring::rate_limiter::RateLimiter;
use crate::monitoring::rate_limiter::RequestPriority;
use crate::retry::{extract_status, retry_with_backoff};
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
    pub from_user_account: String,
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
    pub token_amount: String,
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
    #[serde(rename = "webhookURL")]
    pub webhook_url: Option<String>,
    #[serde(rename = "transactionTypes")]
    pub transaction_types: Option<Vec<String>>,
    #[serde(rename = "accountAddresses")]
    pub account_addresses: Option<Vec<String>>,
    #[serde(rename = "authHeader")]
    pub auth_header: Option<serde_json::Value>,
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
            base_url: crate::utils::helius_api_base_url(),
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
        let cache_size = self.metadata_cache.read().len() as u64;

        HeliusMetrics {
            cache_hits, // Actual cache hits since start
            cache_misses, // Actual cache misses since start
            successful_requests: 0, // Not actively tracked without additional state
            retried_requests: 0,    // Not actively tracked without additional state
            failed_requests: 0,     // Not actively tracked without additional state
        }
    }

    /// Get cache statistics for monitoring
    pub fn get_cache_stats(&self) -> (u64, u64, u64) {
        let cache_hits = self.cache_hits.load(std::sync::atomic::Ordering::Relaxed);
        let cache_misses = self.cache_misses.load(std::sync::atomic::Ordering::Relaxed);
        let cache_size = self.metadata_cache.read().len() as u64;
        (cache_hits, cache_misses, cache_size)
    }

    /// Get token creation time in hours since creation
    ///
    /// Returns None if:
    /// - API call fails
    /// - No transactions found for the mint address
    ///
    /// Uses shared metadata cache for unified storage of token metadata and age information.
    /// Age is calculated once and stored in the cache for 24 hours.
    pub async fn get_token_age_hours(&self, mint_address: &str) -> Result<Option<f64>> {
        // Check shared metadata cache first
        {
            let cache = self.metadata_cache.read();
            if let Some(metadata) = cache.get(mint_address) {
                // If we have cached age, return it (metadata cache has 24-hour TTL)
                if let Some(age) = metadata.age_hours {
                    self.cache_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::debug!(token = mint_address, age = age, "Cache hit for token age");
                    return Ok(Some(age));
                }
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
            "{}/addresses/{}/transactions?api-key={}&limit=1&order=asc",
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
                        // Return error with status so retry logic can determine if retryable
                        return Err(anyhow!("HTTP error: {}", status).context(format!(
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
                // Get timestamp from first transaction
                if let Some(first_tx) = transactions.first() {
                    if let Some(timestamp) = first_tx.get("timestamp").and_then(|t| t.as_i64()) {
                        return Ok(Some(timestamp));
                    }
                }
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
                .register_webhook(chunk, webhook_url)
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
    pub async fn register_webhook(&self, wallets: &[String], webhook_url: &str) -> Result<String> {
        let registration = WebhookRegistration {
            webhook_url: webhook_url.to_string(),
            transaction_types: vec!["SWAP".to_string()],
            account_addresses: wallets.to_vec(),
            webhook_type: "enhanced".to_string(),
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
                    let response = client
                        .post(&url)
                        .json(&registration)
                        .send()
                        .await
                        .context("Failed to send webhook registration request")?;

                    if !response.status().is_success() {
                        let status = response.status().as_u16();
                        let error_text = response.text().await.unwrap_or_default();
                        return Err(anyhow!("HTTP error: {}", status)
                            .context(format!("Webhook registration failed: {}", error_text)));
                    }

                    let webhook_response: WebhookResponse = response
                        .json()
                        .await
                        .context("Failed to parse webhook response")?;

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

                    if !response.status().is_success() {
                        let status = response.status().as_u16();
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
}
