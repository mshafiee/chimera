//! Helius webhook integration for automatic transaction monitoring
//!
//! Handles webhook registration, receiving, and processing for ACTIVE wallets.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use parking_lot::RwLock;
use crate::monitoring::rate_limiter::RateLimiter;
use crate::monitoring::rate_limiter::RequestPriority;
use anyhow::{Context, Result};
use reqwest::Client;
use tokio::time::{sleep, Duration};

/// Cache entry for token creation time
struct TokenAgeCacheEntry {
    creation_timestamp: i64,
    cached_at: SystemTime,
}

/// Helius API client
pub struct HeliusClient {
    api_key: String,
    client: Client,
    base_url: String,
    /// Cache for token creation times (mint_address -> cache entry)
    token_age_cache: Arc<RwLock<HashMap<String, TokenAgeCacheEntry>>>,
    /// Cache TTL in seconds (default: 1 hour)
    token_age_cache_ttl: u64,
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

impl HeliusClient {
    pub fn new(api_key: String) -> Result<Self> {
        Ok(Self {
            api_key,
            client: Client::new(),
            base_url: "https://api.helius.xyz/v0".to_string(),
            token_age_cache: Arc::new(RwLock::new(HashMap::new())),
            token_age_cache_ttl: 3600, // 1 hour
        })
    }

    /// Get token creation time in hours since creation
    ///
    /// Returns None if:
    /// - API call fails
    /// - No transactions found for the mint address
    /// - Token is older than cache TTL (will re-fetch)
    pub async fn get_token_age_hours(&self, mint_address: &str) -> Result<Option<f64>> {
        // Check cache first
        {
            let cache = self.token_age_cache.read();
            if let Some(entry) = cache.get(mint_address) {
                // Check if cache is still valid
                if let Ok(elapsed) = entry.cached_at.elapsed() {
                    if elapsed.as_secs() < self.token_age_cache_ttl {
                        // Calculate age in hours
                        let current_timestamp = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64;
                        let age_seconds = current_timestamp - entry.creation_timestamp;
                        let age_hours = age_seconds as f64 / 3600.0;
                        return Ok(Some(age_hours));
                    }
                }
            }
        }

        // Fetch from API
        let creation_timestamp = self.get_token_creation_time(mint_address).await?;
        
        if let Some(timestamp) = creation_timestamp {
            // Cache the result
            {
                let mut cache = self.token_age_cache.write();
                cache.insert(
                    mint_address.to_string(),
                    TokenAgeCacheEntry {
                        creation_timestamp: timestamp,
                        cached_at: SystemTime::now(),
                    },
                );
            }

            // Calculate age in hours
            let current_timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            let age_seconds = current_timestamp - timestamp;
            let age_hours = age_seconds as f64 / 3600.0;
            Ok(Some(age_hours))
        } else {
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

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch token transactions")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            tracing::warn!(
                mint = mint_address,
                error = error_text,
                "Failed to fetch token creation time"
            );
            return Ok(None);
        }

        let transactions: Vec<serde_json::Value> = response
            .json()
            .await
            .context("Failed to parse transactions response")?;

        // Get timestamp from first transaction
        if let Some(first_tx) = transactions.first() {
            if let Some(timestamp) = first_tx.get("timestamp").and_then(|t| t.as_i64()) {
                return Ok(Some(timestamp));
            }
        }

        Ok(None)
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
            rate_limiter.acquire(RequestPriority::Polling).await;

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
    pub async fn register_webhook(
        &self,
        wallets: &[String],
        webhook_url: &str,
    ) -> Result<String> {
        let registration = WebhookRegistration {
            webhook_url: webhook_url.to_string(),
            transaction_types: vec!["SWAP".to_string()],
            account_addresses: wallets.to_vec(),
            webhook_type: "enhanced".to_string(),
        };

        let url = format!("{}/webhooks?api-key={}", self.base_url, self.api_key);
        let response = self
            .client
            .post(&url)
            .json(&registration)
            .send()
            .await
            .context("Failed to send webhook registration request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Webhook registration failed: {}",
                error_text
            ));
        }

        let webhook_response: WebhookResponse = response
            .json()
            .await
            .context("Failed to parse webhook response")?;

        Ok(webhook_response.webhook_id)
    }

    /// Delete a webhook
    pub async fn delete_webhook(&self, webhook_id: &str) -> Result<()> {
        let url = format!(
            "{}/webhooks/{}?api-key={}",
            self.base_url, webhook_id, self.api_key
        );

        let response = self
            .client
            .delete(&url)
            .send()
            .await
            .context("Failed to delete webhook")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Failed to delete webhook: {}", error_text));
        }

        Ok(())
    }

    /// List all webhooks
    pub async fn list_webhooks(&self) -> Result<Vec<serde_json::Value>> {
        let url = format!("{}/webhooks?api-key={}", self.base_url, self.api_key);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to list webhooks")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Failed to list webhooks: {}", error_text));
        }

        let webhooks: Vec<serde_json::Value> = response
            .json()
            .await
            .context("Failed to parse webhooks response")?;

        Ok(webhooks)
    }
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
