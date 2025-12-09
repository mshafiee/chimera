//! Helius webhook integration for automatic transaction monitoring
//!
//! Handles webhook registration, receiving, and processing for ACTIVE wallets.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::monitoring::rate_limiter::RateLimiter;
use crate::monitoring::rate_limiter::RequestPriority;
use anyhow::{Context, Result};
use reqwest::Client;
use tokio::time::{sleep, Duration};

/// Helius API client
pub struct HeliusClient {
    api_key: String,
    client: Client,
    base_url: String,
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
        })
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
    async fn register_webhook(
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
